use super::{decoder::Block, output::Frame};

/// Streaming interpolation; integer rate ratio prevents per-packet timing drift.
pub struct Resampler {
    input_rate: u32,
    output_rate: u32,
    index: u64,
    next: u64,
    previous: Option<([f32; 2], f64)>,
    held: Option<Frame>,
}

impl Resampler {
    pub fn new(output_rate: u32) -> Self {
        Self {
            input_rate: 0,
            output_rate,
            index: 0,
            next: 0,
            previous: None,
            held: None,
        }
    }
    pub fn convert(&mut self, block: Block) -> Result<Vec<Frame>, String> {
        if self.input_rate != 0 && self.input_rate != block.rate {
            return Err("Audio sample rate changed within the file.".into());
        }
        self.input_rate = block.rate;
        let mut result = Vec::with_capacity(
            (block.samples.len() / block.channels) * self.output_rate as usize
                / block.rate as usize
                + 2,
        );
        for (i, sample) in block.samples.chunks_exact(block.channels).enumerate() {
            let stereo = [sample[0], sample[block.channels - 1]];
            let time = block.start_seconds + i as f64 / block.rate as f64;
            let (previous, previous_time) = self.previous.unwrap_or((stereo, time));
            while self.next <= self.index * self.output_rate as u64 {
                let fraction = if self.index == 0 {
                    1.0
                } else {
                    (self.next as f64 / self.output_rate as f64 - (self.index - 1) as f64)
                        .clamp(0.0, 1.0)
                };
                let mix = |c: usize| previous[c] + (stereo[c] - previous[c]) * fraction as f32;
                let frame = Frame {
                    samples: [mix(0), mix(1)],
                    end_seconds: previous_time
                        + (time - previous_time) * fraction
                        + 1.0 / self.output_rate as f64,
                };
                if let Some(previous) = self.held.replace(frame) {
                    result.push(previous);
                }
                self.next += block.rate as u64;
            }
            self.previous = Some((stereo, time));
            self.index += 1;
        }
        Ok(result)
    }

    /// Hold the last input sample through the final partial interpolation interval.
    /// Without this, EOF would lose up to one source sample of media time.
    pub fn finish(&mut self) -> Vec<Frame> {
        let mut result = Vec::new();
        if let Some((samples, time)) = self.previous {
            let end = time + 1.0 / self.input_rate as f64;
            while self.next < self.index * self.output_rate as u64 {
                let offset = self.next as f64 / self.output_rate as f64 - (self.index - 1) as f64;
                let frame = Frame {
                    samples,
                    end_seconds: (time
                        + offset / self.input_rate as f64
                        + 1.0 / self.output_rate as f64)
                        .min(end),
                };
                if let Some(previous) = self.held.replace(frame) {
                    result.push(previous);
                }
                self.next += self.input_rate as u64;
            }
            // One held frame lets EOF clamp a partial downsampling interval before
            // its timestamp is published to the callback.
            if let Some(mut last) = self.held.take() {
                last.end_seconds = end;
                result.push(last);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversion_preserves_timing_across_packet_boundaries() {
        let mut resampler = Resampler::new(48000);
        let mut frames = Vec::new();
        for i in 0..100 {
            frames.extend(
                resampler
                    .convert(Block {
                        samples: vec![0.25; 441],
                        channels: 1,
                        rate: 44100,
                        start_seconds: i as f64 * 0.01,
                    })
                    .unwrap(),
            );
        }
        frames.extend(resampler.finish());
        assert_eq!(frames.len(), 48000);
        assert!((frames.last().unwrap().end_seconds - 1.0).abs() < 1e-9);
        assert!(frames.iter().all(|f| f.samples == [0.25, 0.25]));
    }

    #[test]
    fn downsampling_clamps_the_final_partial_frame_to_source_end() {
        let mut resampler = Resampler::new(8000);
        let mut frames = resampler
            .convert(Block {
                samples: vec![0.5; 17],
                channels: 1,
                rate: 48000,
                start_seconds: 0.0,
            })
            .unwrap();
        frames.extend(resampler.finish());
        assert_eq!(frames.len(), 3);
        assert!((frames.last().unwrap().end_seconds - 17.0 / 48000.0).abs() < 1e-9);
        assert!(resampler.finish().is_empty());
    }
}
