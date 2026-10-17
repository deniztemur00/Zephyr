use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{OutputCallbackInfo, SampleFormat, Stream, StreamConfig};

use super::AudioError;

pub const BUFFER_SECONDS: usize = 2;
const PUSH_WAIT: Duration = Duration::from_millis(50);

pub struct SampleQueue {
    samples: Mutex<VecDeque<f32>>,
    space: Condvar,
    capacity: usize,
}

impl SampleQueue {
    fn new(capacity: usize) -> Self {
        Self {
            samples: Mutex::new(VecDeque::with_capacity(capacity)),
            space: Condvar::new(),
            capacity,
        }
    }

    pub fn push(&self, mut input: &[f32], should_stop: impl Fn() -> bool) -> bool {
        let mut samples = self.samples.lock().unwrap_or_else(PoisonError::into_inner);
        while !input.is_empty() {
            if should_stop() {
                return false;
            }
            let room = self.capacity - samples.len();
            if room == 0 {
                samples = self
                    .space
                    .wait_timeout(samples, PUSH_WAIT)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            let (now, later) = input.split_at(room.min(input.len()));
            samples.extend(now);
            input = later;
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    pub fn clear(&self) {
        self.samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.space.notify_all();
    }

    fn pop_into(&self, out: &mut [f32]) -> usize {
        let Ok(mut samples) = self.samples.try_lock() else {
            return 0;
        };
        let count = out.len().min(samples.len());
        for (slot, sample) in out.iter_mut().zip(samples.drain(..count)) {
            *slot = sample;
        }
        drop(samples);
        self.space.notify_one();
        count
    }
}

pub struct Shared {
    pub queue: SampleQueue,
    pub paused: AtomicBool,
    pub frames_played: AtomicU64,
    volume_bits: AtomicU32,
}

impl Shared {
    pub fn set_volume(&self, volume: f32) {
        self.volume_bits
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }
}

pub struct Output {
    _stream: Stream,
    pub rate: u32,
    pub channels: usize,
}

pub fn start(volume: f32) -> Result<(Output, std::sync::Arc<Shared>), AudioError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;
    let supported = device.default_output_config()?;
    if supported.sample_format() != SampleFormat::F32 {
        // shortcut: only f32 shared-mode output; add i16/i32 conversion if a device needs it.
        return Err(AudioError::UnsupportedFormat(supported.sample_format()));
    }
    let config: StreamConfig = supported.into();
    let channels = usize::from(config.channels);
    let capacity = config.sample_rate as usize * channels * BUFFER_SECONDS;
    let shared = std::sync::Arc::new(Shared {
        queue: SampleQueue::new(capacity),
        paused: AtomicBool::new(false),
        frames_played: AtomicU64::new(0),
        volume_bits: AtomicU32::new(0),
    });
    shared.set_volume(volume);
    let callback_shared = shared.clone();
    let stream = device.build_output_stream(
        config,
        move |out: &mut [f32], _: &OutputCallbackInfo| fill(out, &callback_shared, channels),
        |_err: cpal::Error| {},
        None,
    )?;
    stream.play()?;
    let output = Output {
        _stream: stream,
        rate: config.sample_rate,
        channels,
    };
    Ok((output, shared))
}

fn fill(out: &mut [f32], shared: &Shared, channels: usize) {
    let written = if shared.paused.load(Ordering::Relaxed) {
        0
    } else {
        shared.queue.pop_into(out)
    };
    let volume = shared.volume();
    out[..written]
        .iter_mut()
        .for_each(|sample| *sample *= volume);
    out[written..].fill(0.0);
    shared
        .frames_played
        .fetch_add((written / channels) as u64, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_fills_up_to_capacity_then_stops_when_cancelled() {
        let queue = SampleQueue::new(4);
        let cancelled = AtomicBool::new(false);
        assert!(queue.push(&[1.0, 2.0, 3.0], || cancelled.load(Ordering::Acquire)));
        cancelled.store(true, Ordering::Release);
        assert!(!queue.push(&[4.0, 5.0], || cancelled.load(Ordering::Acquire)));
        let mut out = [0.0; 8];
        assert_eq!(queue.pop_into(&mut out), 3);
        assert_eq!(out[..3], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn pop_never_returns_more_than_buffered() {
        let queue = SampleQueue::new(8);
        assert!(queue.push(&[0.5; 2], || false));
        let mut out = [9.0; 4];
        assert_eq!(queue.pop_into(&mut out), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn clear_empties_the_queue() {
        let queue = SampleQueue::new(8);
        assert!(queue.push(&[0.5; 6], || false));
        queue.clear();
        assert!(queue.is_empty());
    }
}
