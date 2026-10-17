pub mod bench;
mod convert;
mod decode;
mod fragments;
mod http_source;
mod native;
mod output;
mod resolve;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use thiserror::Error;

pub use self::convert::RESAMPLE_CHUNK_FRAMES;
use self::decode::Job;
pub use self::output::BUFFER_SECONDS;
use self::output::{Output, Shared};
use self::resolve::StreamInfo;
pub use self::resolve::{Resolver, Source, StreamFormat};

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no audio output device")]
    NoOutputDevice,
    #[error("output device uses unsupported sample format {0:?}")]
    UnsupportedFormat(cpal::SampleFormat),
    #[error("audio output: {0}")]
    Output(#[from] cpal::Error),
    #[error(transparent)]
    Resolve(#[from] resolve::ResolveError),
    #[error("stream: {0}")]
    Io(#[from] io::Error),
    #[error("decode: {0}")]
    Decode(#[from] symphonia::core::errors::Error),
    #[error("resample: {0}")]
    Resample(String),
    #[error("stream has no playable audio track")]
    NoTrack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Idle,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub position: Duration,
    pub duration: Option<Duration>,
}

enum Phase {
    Loading,
    Decoding,
    Done,
    Failed(String),
}

pub struct TrackSlot {
    job: u64,
    phase: Phase,
    duration: Option<Duration>,
    details: Option<Details>,
}

// The current track's stream and how it is converted for the output device.
#[derive(Debug, Clone, PartialEq)]
pub struct Details {
    pub format: StreamFormat,
    pub sample_rate: u32,
    pub channels: usize,
    pub output_rate: u32,
    pub output_channels: usize,
}

type Prefetched = Arc<Mutex<Option<(String, StreamInfo)>>>;

pub struct Player {
    output: Output,
    shared: Arc<Shared>,
    slot: Arc<Mutex<TrackSlot>>,
    cancelled: Arc<AtomicBool>,
    seek_request: Arc<Mutex<Option<Duration>>>,
    resolver: Arc<Resolver>,
    prefetched: Prefetched,
    prefetching: Arc<AtomicBool>,
}

impl Player {
    pub fn new(ytdlp: PathBuf, volume: f32) -> Result<Self, AudioError> {
        let (output, shared) = output::start(volume)?;
        let resolver = Arc::new(Resolver::new(ytdlp));
        let warm = resolver.clone();
        thread::spawn(move || warm.warm_up());
        Ok(Self {
            output,
            shared,
            slot: Arc::new(Mutex::new(TrackSlot {
                job: 0,
                phase: Phase::Done,
                duration: None,
                details: None,
            })),
            cancelled: Arc::new(AtomicBool::new(false)),
            seek_request: Arc::new(Mutex::new(None)),
            resolver,
            prefetched: Arc::new(Mutex::new(None)),
            prefetching: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn play(&mut self, video_id: &str) {
        self.cancelled.store(true, Ordering::Release);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.seek_request = Arc::new(Mutex::new(None));
        self.shared.queue.clear();
        self.shared.frames_played.store(0, Ordering::Relaxed);
        self.shared.paused.store(false, Ordering::Relaxed);
        let id = {
            let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
            *slot = TrackSlot {
                job: slot.job + 1,
                phase: Phase::Loading,
                duration: None,
                details: None,
            };
            slot.job
        };
        let job = Job {
            id,
            video_id: video_id.to_owned(),
            resolver: self.resolver.clone(),
            prefetched: self.take_prefetched(video_id),
            shared: self.shared.clone(),
            slot: self.slot.clone(),
            cancelled: self.cancelled.clone(),
            seek_request: self.seek_request.clone(),
            device_rate: self.output.rate,
            device_channels: self.output.channels,
        };
        thread::spawn(move || job.run());
    }

    pub fn seek(&self, position: Duration) {
        *self
            .seek_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(position);
        let frames = position.as_secs_f64() * f64::from(self.output.rate);
        self.shared
            .frames_played
            .store(frames as u64, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.set_volume(volume);
    }

    pub fn stop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.shared.queue.clear();
        self.shared.frames_played.store(0, Ordering::Relaxed);
        *self
            .prefetched
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        *slot = TrackSlot {
            job: slot.job + 1,
            phase: Phase::Done,
            duration: None,
            details: None,
        };
    }

    pub fn toggle_pause(&self) {
        self.shared.paused.fetch_xor(true, Ordering::Relaxed);
    }

    pub fn prefetch(&self, video_id: &str) {
        if self.prefetching.swap(true, Ordering::AcqRel) {
            return;
        }
        let (resolver, video_id) = (self.resolver.clone(), video_id.to_owned());
        let (prefetched, prefetching) = (self.prefetched.clone(), self.prefetching.clone());
        thread::spawn(move || {
            if let Ok(stream) = resolver.resolve(&video_id) {
                *prefetched.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some((video_id, stream));
            }
            prefetching.store(false, Ordering::Release);
        });
    }

    pub fn status(&self) -> Status {
        let slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let state = match &slot.phase {
            _ if slot.job == 0 => State::Idle,
            Phase::Loading => State::Loading,
            Phase::Failed(error) => State::Failed(error.clone()),
            Phase::Done if self.shared.queue.is_empty() => State::Ended,
            _ if self.shared.paused.load(Ordering::Relaxed) => State::Paused,
            Phase::Decoding | Phase::Done => State::Playing,
        };
        let frames = self.shared.frames_played.load(Ordering::Relaxed);
        Status {
            state,
            position: Duration::from_secs_f64(frames as f64 / f64::from(self.output.rate)),
            duration: slot.duration,
        }
    }

    pub fn details(&self) -> Option<Details> {
        let slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.details.clone()
    }

    pub fn output_format(&self) -> (u32, usize) {
        (self.output.rate, self.output.channels)
    }

    fn take_prefetched(&self, video_id: &str) -> Option<StreamInfo> {
        let mut prefetched = self
            .prefetched
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match prefetched.take() {
            Some((id, stream)) if id == video_id => Some(stream),
            other => {
                *prefetched = other;
                None
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
