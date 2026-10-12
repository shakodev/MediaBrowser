use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SizedSample,
};
use rtrb::{Consumer, Producer};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub samples: [f32; 2],
    pub end_seconds: f64,
}

#[derive(Clone, Copy)]
pub struct Span {
    pub start: Instant,
    pub end: Instant,
    pub media_start: f64,
    pub media_end: f64,
}

/// Callback owns only preallocated queues and atomics: no locks, IO, decode, logging,
/// allocations or waits. Underruns produce silence without consuming media time.
pub struct Renderer {
    pub pcm: Consumer<Frame>,
    pub timing: Producer<Span>,
    pub volume: Arc<AtomicU32>,
    pub failed: Arc<AtomicBool>,
    pub consumed: Arc<AtomicU64>,
    pub rate: u32,
    pub channels: usize,
}

impl Renderer {
    pub fn render<T: Sample + FromSample<f32>>(&mut self, output: &mut [T], playback: Instant) {
        output.fill(T::EQUILIBRIUM);
        let gain = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let mut first = None;
        let mut last = 0.0;
        let mut count = 0;
        for frame in output.chunks_exact_mut(self.channels) {
            let Ok(pcm) = self.pcm.pop() else {
                break;
            };
            first.get_or_insert(pcm.end_seconds - 1.0 / self.rate as f64);
            last = pcm.end_seconds;
            if self.channels == 1 {
                frame[0] = T::from_sample(
                    ((pcm.samples[0] + pcm.samples[1]) * 0.5 * gain).clamp(-1.0, 1.0),
                );
            } else {
                frame[0] = T::from_sample((pcm.samples[0] * gain).clamp(-1.0, 1.0));
                frame[1] = T::from_sample((pcm.samples[1] * gain).clamp(-1.0, 1.0));
            }
            count += 1;
        }
        if let Some(media_start) = first {
            let span = Span {
                start: playback,
                end: playback + Duration::from_secs_f64(count as f64 / self.rate as f64),
                media_start,
                media_end: last,
            };
            if self.timing.push(span).is_err() {
                self.failed.store(true, Ordering::Release);
            }
            // Publish consumption only after its timeline span is visible to the worker.
            self.consumed.fetch_add(count, Ordering::Release);
        }
    }
}

pub struct Timeline {
    pending: VecDeque<Span>,
    pub position: f64,
}
impl Timeline {
    pub fn new(position: f64) -> Self {
        Self {
            pending: VecDeque::with_capacity(256),
            position,
        }
    }
    pub fn update(&mut self, timing: &mut Consumer<Span>, now: Instant) -> Result<(), String> {
        while let Ok(span) = timing.pop() {
            if self.pending.len() == 256 {
                return Err("Audio timing queue overflowed.".into());
            }
            self.pending.push_back(span);
        }
        while let Some(span) = self.pending.front() {
            if now >= span.end {
                self.position = span.media_end;
                self.pending.pop_front();
            } else {
                if now >= span.start {
                    let fraction = now.duration_since(span.start).as_secs_f64()
                        / span.end.duration_since(span.start).as_secs_f64();
                    self.position =
                        span.media_start + (span.media_end - span.media_start) * fraction;
                }
                break;
            }
        }
        Ok(())
    }
    pub fn drained(&self) -> bool {
        self.pending.is_empty()
    }
}

#[derive(Clone, Copy)]
pub struct OutputSpec {
    pub rate: u32,
    pub channels: usize,
}
pub trait OutputStream {}
impl OutputStream for cpal::Stream {}

pub trait Backend {
    fn spec(&mut self) -> Result<OutputSpec, String>;
    fn start(&mut self, renderer: Renderer) -> Result<Box<dyn OutputStream>, String>;
}

#[derive(Default)]
pub struct CpalBackend {
    device: Option<(cpal::Device, cpal::SupportedStreamConfig)>,
}
impl Backend for CpalBackend {
    fn spec(&mut self) -> Result<OutputSpec, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("No audio output device is available.")?;
        let config = device
            .default_output_config()
            .map_err(|e| format!("Cannot configure audio output: {e}"))?;
        let spec = OutputSpec {
            rate: config.sample_rate().0,
            channels: config.channels() as usize,
        };
        if spec.rate == 0 || spec.rate > 384000 || !(1..=32).contains(&spec.channels) {
            return Err("Unsupported output device configuration.".into());
        }
        self.device = Some((device, config));
        Ok(spec)
    }
    fn start(&mut self, renderer: Renderer) -> Result<Box<dyn OutputStream>, String> {
        let (device, config) = self.device.take().ok_or("Output was not configured.")?;
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config.into(), renderer),
            cpal::SampleFormat::I16 => build::<i16>(&device, &config.into(), renderer),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config.into(), renderer),
            format => return Err(format!("Unsupported audio device sample format: {format}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("Cannot start audio output: {e}"))?;
        Ok(Box::new(stream))
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut renderer: Renderer,
) -> Result<cpal::Stream, String> {
    let failed = renderer.failed.clone();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let stamp = info.timestamp();
                let delay = stamp
                    .playback
                    .duration_since(&stamp.callback)
                    .unwrap_or_default();
                renderer.render(data, Instant::now() + delay);
            },
            move |_| {
                failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| format!("Cannot open audio output: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;
    #[test]
    fn callback_handles_volume_underrun_and_device_latency() {
        let (mut producer, pcm) = RingBuffer::new(2);
        let (timing, mut receive) = RingBuffer::new(4);
        let mut render = Renderer {
            pcm,
            timing,
            volume: Arc::new(AtomicU32::new(0.5f32.to_bits())),
            failed: Arc::new(AtomicBool::new(false)),
            consumed: Arc::new(AtomicU64::new(0)),
            rate: 100,
            channels: 2,
        };
        producer
            .push(Frame {
                samples: [0.8, -0.8],
                end_seconds: 1.01,
            })
            .unwrap();
        producer
            .push(Frame {
                samples: [0.4, -0.4],
                end_seconds: 1.02,
            })
            .unwrap();
        let now = Instant::now();
        let mut data = [9.0f32; 8];
        render.render(&mut data, now + Duration::from_millis(30));
        assert_eq!(data, [0.4, -0.4, 0.2, -0.2, 0.0, 0.0, 0.0, 0.0]);
        let mut clock = Timeline::new(1.0);
        clock.update(&mut receive, now).unwrap();
        assert_eq!(clock.position, 1.0);
        clock
            .update(&mut receive, now + Duration::from_millis(40))
            .unwrap();
        assert!((clock.position - 1.01).abs() < 1e-6);
        clock
            .update(&mut receive, now + Duration::from_secs(1))
            .unwrap();
        assert_eq!(clock.position, 1.02);
        render.render(&mut data, now + Duration::from_secs(1));
        assert_eq!(data, [0.0; 8]);
        clock
            .update(&mut receive, now + Duration::from_secs(2))
            .unwrap();
        assert_eq!(clock.position, 1.02);
    }
}
