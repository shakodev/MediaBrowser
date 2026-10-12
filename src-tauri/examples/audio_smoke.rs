//! Opt-in hardware verification: plays local fixtures and checks WASAPI loopback.
//! Captured audio stays in a bounded in-memory queue; only statistics are printed.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use media_browser_lib::{audio::AudioEngine, playback_state::PlaybackStatus};
use rtrb::{Consumer, RingBuffer};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(windows) {
        return Err("This smoke check requires Windows WASAPI loopback.".into());
    }
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No default output device")?;
    let config = device.default_output_config()?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err("Smoke capture requires an f32 output device.".into());
    }
    println!(
        "Output: {} · {} Hz · {} channels",
        device.name()?,
        config.sample_rate().0,
        config.channels()
    );
    let (mut send, mut receive) =
        RingBuffer::new(config.sample_rate().0 as usize * config.channels() as usize * 2);
    let failed = Arc::new(AtomicBool::new(false));
    let error = failed.clone();
    let overflow = failed.clone();
    // CPAL opens an output endpoint as an input stream in WASAPI loopback mode.
    let capture = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _| {
            for &sample in data {
                if send.push(sample).is_err() {
                    overflow.store(true, Ordering::Release);
                    break;
                }
            }
        },
        move |_| {
            error.store(true, Ordering::Release);
        },
        None,
    )?;
    capture.play()?;
    let engine = AudioEngine::new()?;
    tauri::async_runtime::block_on(async {
        engine.set_volume(0.25).await?;
        let baseline = collect(&mut receive, Duration::from_millis(200));
        println!("Baseline loopback peak: {:.6}", baseline.peak);
        for extension in ["mp3", "aac", "m4a"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/sfx.{extension}"));
            engine.load(path).await?;
            while receive.pop().is_ok() {}
            engine.play().await?;
            let start = Instant::now();
            let mut stats = Stats::default();
            let mut advanced = false;
            loop {
                stats.merge(collect(&mut receive, Duration::from_millis(10)));
                let state = engine.snapshot().await?;
                advanced |= state.position_seconds > 0.0;
                if state.status == PlaybackStatus::Error {
                    return Err(state.error.unwrap_or_default());
                }
                if state.status == PlaybackStatus::Ended {
                    stats.merge(collect(&mut receive, Duration::from_millis(100)));
                    println!(
                        "{extension}: ended at {:.6}s; {} captured samples; peak {:.6}; RMS {:.6}",
                        state.position_seconds,
                        stats.count,
                        stats.peak,
                        stats.rms()
                    );
                    break;
                }
                if start.elapsed() > Duration::from_secs(5) {
                    return Err("Playback did not finish within 5 seconds.".into());
                }
            }
            if !advanced || stats.count == 0 || stats.peak < 0.001 {
                return Err(format!(
                    "{extension}: playback produced no measurable output."
                ));
            }
            engine.seek(0.04).await?;
            engine.play().await?;
            collect(&mut receive, Duration::from_millis(20));
            let paused = engine.pause().await?;
            let position = paused.position_seconds;
            collect(&mut receive, Duration::from_millis(30));
            if engine.snapshot().await?.position_seconds != position {
                return Err("Paused playback position moved.".into());
            }
            engine.stop().await?;
            if engine.snapshot().await?.position_seconds != 0.0 {
                return Err("Stop did not reset position.".into());
            }
        }
        if failed.load(Ordering::Acquire) {
            return Err("Loopback capture failed or overflowed.".into());
        }
        Ok::<(), String>(())
    })?;
    println!("PASS: native output, media position, pause, seek and stop checks.");
    Ok(())
}

#[derive(Default)]
struct Stats {
    count: u64,
    peak: f32,
    squared: f64,
}
impl Stats {
    fn add(&mut self, sample: f32) {
        self.count += 1;
        self.peak = self.peak.max(sample.abs());
        self.squared += (sample as f64).powi(2);
    }
    fn merge(&mut self, other: Self) {
        self.count += other.count;
        self.peak = self.peak.max(other.peak);
        self.squared += other.squared;
    }
    fn rms(&self) -> f64 {
        (self.squared / self.count.max(1) as f64).sqrt()
    }
}
fn collect(receive: &mut Consumer<f32>, duration: Duration) -> Stats {
    let until = Instant::now() + duration;
    let mut stats = Stats::default();
    while Instant::now() < until {
        while let Ok(sample) = receive.pop() {
            stats.add(sample);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    stats
}
