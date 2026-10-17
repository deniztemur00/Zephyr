use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

use super::convert::Converter;
use super::fragments::FragmentIndex;
use super::http_source::{HttpSource, Refresh};
use super::output::Shared;
use super::resolve::{Resolver, StreamFormat, StreamInfo};
use super::{AudioError, Details, Phase, TrackSlot};

const DRAIN_POLL: Duration = Duration::from_millis(50);

pub struct Job {
    pub id: u64,
    pub video_id: String,
    pub resolver: Arc<Resolver>,
    pub prefetched: Option<StreamInfo>,
    pub shared: Arc<Shared>,
    pub slot: Arc<Mutex<TrackSlot>>,
    pub cancelled: Arc<AtomicBool>,
    pub seek_request: Arc<Mutex<Option<Duration>>>,
    pub device_rate: u32,
    pub device_channels: usize,
}

impl Job {
    pub fn run(self) {
        let result = self.decode();
        let phase = match result {
            Ok(()) => Phase::Done,
            Err(err) => Phase::Failed(err.to_string()),
        };
        self.update(|slot| slot.phase = phase);
    }

    fn decode(&self) -> Result<(), AudioError> {
        let stream = match &self.prefetched {
            Some(stream) => stream.clone(),
            None => self.resolver.resolve(&self.video_id)?,
        };
        if self.is_cancelled() {
            return Ok(());
        }
        let refresh: Refresh = {
            let (resolver, video_id) = (self.resolver.clone(), self.video_id.clone());
            Arc::new(move || resolver.resolve(&video_id).ok())
        };
        let (format, origin) = Origin::open(&stream, Some(refresh))?;
        self.play_track(format, &origin, &stream.format)
    }

    fn play_track(
        &self,
        mut format: Box<dyn FormatReader>,
        origin: &Origin,
        stream_format: &StreamFormat,
    ) -> Result<(), AudioError> {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(AudioError::NoTrack)?;
        let track_id = track.id;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or(AudioError::NoTrack)?;
        let rate = params.sample_rate.ok_or(AudioError::NoTrack)?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .ok_or(AudioError::NoTrack)?;
        let duration = track
            .num_frames
            .map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(rate)));
        let time_base = track.time_base;
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())?;
        let mut converter = Converter::new(rate, channels, self.device_rate, self.device_channels)?;
        self.update(|slot| {
            slot.phase = Phase::Decoding;
            slot.duration = duration;
            slot.details = Some(Details {
                format: stream_format.clone(),
                sample_rate: rate,
                channels,
                output_rate: self.device_rate,
                output_channels: self.device_channels,
            });
        });

        let mut decoded: Vec<f32> = Vec::new();
        let stop = || self.is_cancelled() || self.seek_pending();
        let mut sink = |samples: &[f32]| self.shared.queue.push(samples, stop);
        loop {
            if self.is_cancelled() {
                return Ok(());
            }
            if let Some(target) = self.take_seek() {
                self.shared.queue.clear();
                let landed =
                    seek_to(&mut format, origin, track_id, time_base, target).unwrap_or(target);
                let frames = landed.as_secs_f64() * f64::from(self.device_rate);
                self.shared
                    .frames_played
                    .store(frames as u64, Ordering::Relaxed);
                decoder.reset();
                converter.reset();
                self.update(|slot| slot.phase = Phase::Decoding);
            }
            match format.next_packet()? {
                Some(packet) if packet.track_id == track_id => {
                    match decoder.decode(&packet) {
                        Ok(buffer) => buffer.copy_to_vec_interleaved(&mut decoded),
                        Err(SymphoniaError::DecodeError(_)) => continue,
                        Err(err) => return Err(err.into()),
                    }
                    converter.feed(&decoded, &mut sink)?;
                }
                Some(_) => {}
                None => {
                    converter.flush(&mut sink)?;
                    self.update(|slot| slot.phase = Phase::Done);
                    if !self.wait_for_drain_or_seek() {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn wait_for_drain_or_seek(&self) -> bool {
        while !self.is_cancelled() {
            if self.seek_pending() {
                return true;
            }
            if self.shared.queue.is_empty() {
                return false;
            }
            thread::sleep(DRAIN_POLL);
        }
        false
    }

    fn seek_pending(&self) -> bool {
        self.seek_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn take_seek(&self) -> Option<Duration> {
        self.seek_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn update(&self, change: impl FnOnce(&mut TrackSlot)) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.job == self.id && !self.is_cancelled() {
            change(&mut slot);
        }
    }
}

pub struct Origin {
    template: HttpSource,
    index: Option<FragmentIndex>,
}

impl Origin {
    pub fn open(
        stream: &StreamInfo,
        refresh: Option<Refresh>,
    ) -> Result<(Box<dyn FormatReader>, Self), AudioError> {
        let source = HttpSource::open(stream, refresh)?;
        let origin = Self {
            index: FragmentIndex::parse(source.prefix()),
            template: source.reopen(),
        };
        Ok((open_format(Box::new(source))?, origin))
    }
}

fn open_format(source: Box<dyn MediaSource>) -> Result<Box<dyn FormatReader>, AudioError> {
    let stream = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    hint.with_extension("m4a");
    Ok(symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?)
}

pub fn seek_to(
    format: &mut Box<dyn FormatReader>,
    origin: &Origin,
    track_id: u32,
    time_base: Option<TimeBase>,
    target: Duration,
) -> Option<Duration> {
    seek_via_index(format, origin, target)
        .or_else(|| seek_within(format.as_mut(), track_id, time_base, target))
}

fn seek_via_index(
    format: &mut Box<dyn FormatReader>,
    origin: &Origin,
    target: Duration,
) -> Option<Duration> {
    let index = origin.index.as_ref()?;
    let fragment = index.locate(target.as_secs_f64());
    let spliced = index.splice(origin.template.reopen(), fragment);
    let mut reader = open_format(Box::new(spliced)).ok()?;
    let track = reader.default_track(TrackType::Audio)?;
    let (track_id, time_base) = (track.id, track.time_base);
    let fragment_start = Duration::from_secs_f64(fragment.start_secs);
    let within = seek_within(
        reader.as_mut(),
        track_id,
        time_base,
        target.saturating_sub(fragment_start),
    )
    .unwrap_or_default();
    *format = reader;
    Some(fragment_start + within)
}

fn seek_within(
    format: &mut dyn FormatReader,
    track_id: u32,
    time_base: Option<TimeBase>,
    target: Duration,
) -> Option<Duration> {
    let time = Time::try_from_secs_f64(target.as_secs_f64())?;
    let seeked = format
        .seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(track_id),
            },
        )
        .ok()?;
    let landed = time_base?.calc_time(seeked.actual_ts)?;
    Some(Duration::from_secs_f64(landed.as_secs_f64().max(0.0)))
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len())
    }
}
