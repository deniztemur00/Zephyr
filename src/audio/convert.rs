use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use super::AudioError;

pub const RESAMPLE_CHUNK_FRAMES: usize = 1024;

pub struct Converter {
    resampler: Option<Fft<f32>>,
    src_channels: usize,
    dst_channels: usize,
    pending: Vec<f32>,
    resampled: Vec<f32>,
    mapped: Vec<f32>,
}

impl Converter {
    pub fn new(
        src_rate: u32,
        src_channels: usize,
        dst_rate: u32,
        dst_channels: usize,
    ) -> Result<Self, AudioError> {
        let resampler = (src_rate != dst_rate)
            .then(|| {
                Fft::new(
                    src_rate as usize,
                    dst_rate as usize,
                    RESAMPLE_CHUNK_FRAMES,
                    src_channels,
                    FixedSync::Input,
                )
            })
            .transpose()
            .map_err(|err| AudioError::Resample(err.to_string()))?;
        let resampled_len = resampler
            .as_ref()
            .map_or(0, |r| r.output_frames_max() * src_channels);
        Ok(Self {
            resampler,
            src_channels,
            dst_channels,
            pending: Vec::new(),
            resampled: vec![0.0; resampled_len],
            mapped: Vec::new(),
        })
    }

    pub fn feed(
        &mut self,
        input: &[f32],
        sink: &mut impl FnMut(&[f32]) -> bool,
    ) -> Result<bool, AudioError> {
        if self.resampler.is_none() {
            return Ok(self.map_and_send(input, sink));
        }
        self.pending.extend_from_slice(input);
        while self.pending.len() >= self.chunk_samples() {
            let (consumed, produced) = self.resample(None)?;
            self.pending.drain(..consumed * self.src_channels);
            if !self.send_resampled(produced, sink) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn flush(&mut self, sink: &mut impl FnMut(&[f32]) -> bool) -> Result<bool, AudioError> {
        if self.resampler.is_none() || self.pending.is_empty() {
            return Ok(true);
        }
        let real_frames = self.pending.len() / self.src_channels;
        self.pending.resize(self.chunk_samples(), 0.0);
        let (_, produced) = self.resample(Some(real_frames))?;
        self.pending.clear();
        Ok(self.send_resampled(produced, sink))
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }

    fn chunk_samples(&self) -> usize {
        self.resampler
            .as_ref()
            .map_or(0, |r| r.input_frames_next() * self.src_channels)
    }

    fn resample(&mut self, partial_len: Option<usize>) -> Result<(usize, usize), AudioError> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok((0, 0));
        };
        let in_frames = resampler.input_frames_next();
        let out_frames = self.resampled.len() / self.src_channels;
        let input = InterleavedSlice::new(&self.pending, self.src_channels, in_frames)
            .map_err(|err| AudioError::Resample(err.to_string()))?;
        let mut output =
            InterleavedSlice::new_mut(&mut self.resampled, self.src_channels, out_frames)
                .map_err(|err| AudioError::Resample(err.to_string()))?;
        let indexing = Indexing {
            partial_len,
            ..Default::default()
        };
        resampler
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|err| AudioError::Resample(err.to_string()))
    }

    fn send_resampled(&mut self, frames: usize, sink: &mut impl FnMut(&[f32]) -> bool) -> bool {
        let resampled = std::mem::take(&mut self.resampled);
        let sent = self.map_and_send(&resampled[..frames * self.src_channels], sink);
        self.resampled = resampled;
        sent
    }

    fn map_and_send(&mut self, input: &[f32], sink: &mut impl FnMut(&[f32]) -> bool) -> bool {
        if self.src_channels == self.dst_channels {
            return sink(input);
        }
        self.mapped.clear();
        for frame in input.chunks_exact(self.src_channels) {
            self.mapped.extend((0..self.dst_channels).map(|channel| {
                if self.src_channels == 1 {
                    frame[0]
                } else {
                    frame.get(channel).copied().unwrap_or(0.0)
                }
            }));
        }
        sink(&self.mapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(converter: &mut Converter, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut sink = |samples: &[f32]| {
            out.extend_from_slice(samples);
            true
        };
        assert!(converter.feed(input, &mut sink).unwrap());
        assert!(converter.flush(&mut sink).unwrap());
        out
    }

    #[test]
    fn passes_through_when_formats_match() {
        let mut converter = Converter::new(48_000, 2, 48_000, 2).unwrap();
        assert_eq!(
            collect(&mut converter, &[0.1, 0.2, 0.3, 0.4]),
            [0.1, 0.2, 0.3, 0.4]
        );
    }

    #[test]
    fn duplicates_mono_and_pads_extra_channels() {
        let mut mono = Converter::new(48_000, 1, 48_000, 2).unwrap();
        assert_eq!(collect(&mut mono, &[0.5, 0.25]), [0.5, 0.5, 0.25, 0.25]);
        let mut stereo = Converter::new(48_000, 2, 48_000, 4).unwrap();
        assert_eq!(collect(&mut stereo, &[0.1, 0.2]), [0.1, 0.2, 0.0, 0.0]);
    }

    #[test]
    fn resamples_one_second_to_the_output_rate() {
        let mut converter = Converter::new(44_100, 2, 48_000, 2).unwrap();
        let input: Vec<f32> = (0..44_100 * 2)
            .map(|i| ((i / 2) as f32 * 0.01).sin() * 0.5)
            .collect();
        let frames = collect(&mut converter, &input).len() / 2;
        assert!((47_000..=49_500).contains(&frames), "got {frames} frames");
    }

    #[test]
    fn stops_when_sink_refuses() {
        let mut converter = Converter::new(44_100, 2, 48_000, 2).unwrap();
        let mut refuse = |_: &[f32]| false;
        assert!(!converter.feed(&vec![0.0; 8192], &mut refuse).unwrap());
    }
}
