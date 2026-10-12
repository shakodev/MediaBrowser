use super::{decoder::Block, output::Frame};

/// Streaming interpolation; integer rate ratio prevents per-packet timing drift.
pub struct Resampler {
    input_rate: u32,
    output_rate: u32,
    index: u64,
    next: u64,
    previous: Option<([f32; 2], f64)>,
}

impl Resampler {
    pub fn new(output_rate: u32) -> Self {
        Self {
            input_rate: 0,
            output_rate,
            index: 0,
            next: 0,
            previous: None,
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
                result.push(Frame {
                    samples: [mix(0), mix(1)],
                    end_seconds: previous_time
                        + (time - previous_time) * fraction
                        + 1.0 / self.output_rate as f64,
                });
                self.next += block.rate as u64;
            }
            self.previous = Some((stereo, time));
            self.index += 1;
        }
        Ok(result)
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
        assert!((frames.len() as i64 - 48000).abs() <= 2);
        assert!((frames.last().unwrap().end_seconds - 1.0).abs() < 1.0 / 44100.0);
        assert!(frames.iter().all(|f| f.samples == [0.25, 0.25]));
    }
}
