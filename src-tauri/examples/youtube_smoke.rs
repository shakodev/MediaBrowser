//! Opt-in public YouTube -> HTTP ranges -> Symphonia -> CPAL -> WASAPI loopback.
//! No media file is created and no signed URL is printed.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use media_browser_lib::{audio::AudioEngine, playback_state::PlaybackStatus};
use rtrb::{Consumer, RingBuffer};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(windows) {
        return Err("Windows WASAPI loopback is required.".into());
    }
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into());
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No output device.")?;
    let config = device.default_output_config()?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err("Loopback check requires f32 output.".into());
    }
    println!(
        "Output: {} · {} Hz · {} channels",
        device.name()?,
        config.sample_rate().0,
        config.channels()
    );
    let (mut send, mut receive) =
        RingBuffer::new(config.sample_rate().0 as usize * config.channels() as usize * 4);
    let failed = Arc::new(AtomicBool::new(false));
    let overflow = failed.clone();
    let error = failed.clone();
    let capture = device.build_input_stream(
        &config.into(),
        move |samples: &[f32], _| {
            for &sample in samples {
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
        println!(
            "Baseline peak: {:.6}",
            collect(&mut receive, Duration::from_millis(200))
        );
        let loaded = engine.load_youtube(&url).await?;
        let metadata = loaded.metadata.as_ref().ok_or("Missing video metadata.")?;
        println!(
            "Resolved public video {} · duration {:?} seconds",
            metadata.id, loaded.duration_seconds
        );
        while receive.pop().is_ok() {}
        engine.play().await?;
        let peak = measure(&engine, &mut receive, 5.0).await?;
        let before_seek = engine.snapshot().await?;
        if before_seek.position_seconds <= 1.0 || peak < 0.001 {
            return Err("No measurable native playback or position progress.".into());
        }
        let target = before_seek.duration_seconds.unwrap_or(60.0).min(30.0) / 2.0;
        engine.seek(target).await?;
        let seek_peak = measure(&engine, &mut receive, 2.0).await?;
        let paused = engine.pause().await?;
        collect(&mut receive, Duration::from_millis(150));
        if engine.snapshot().await?.position_seconds != paused.position_seconds {
            return Err("Paused position moved.".into());
        }
        engine.play().await?;
        let resume_peak = measure(&engine, &mut receive, 1.0).await?;
        if seek_peak < 0.001 || resume_peak < 0.001 {
            return Err("Seek/resume produced no measurable output.".into());
        }
        let final_state = engine.stop().await?;
        if final_state.position_seconds != 0.0 {
            return Err("Stop did not reset position.".into());
        }
        let transfer = final_state.transfer.ok_or("No transfer statistics.")?;
        println!("Playback peak {peak:.6}; seek peak {seek_peak:.6}; resume peak {resume_peak:.6}; position before seek {:.3}s", before_seek.position_seconds);
        println!("HTTP bytes received {} / {}; ranges {}; URL refreshes {}; retained range limit {} bytes", transfer.bytes_received, transfer.total_bytes, transfer.range_requests, transfer.url_refreshes, transfer.buffer_limit_bytes);
        if transfer.bytes_received >= transfer.total_bytes {
            return Err("This check requires fetching less than the complete audio resource; choose a longer video.".into());
        }
        if failed.load(Ordering::Acquire) {
            return Err("Loopback capture failed or overflowed.".into());
        }
        Ok::<_, String>(())
    })?;
    println!("PASS: public YouTube resolution, partial HTTP transfer, native audio output, seek, pause/resume and stop.");
    Ok(())
}
async fn measure(
    engine: &AudioEngine,
    receive: &mut Consumer<f32>,
    seconds: f64,
) -> Result<f32, String> {
    let start = Instant::now();
    let mut peak = 0.0_f32;
    while start.elapsed().as_secs_f64() < seconds {
        peak = peak.max(collect(receive, Duration::from_millis(25)));
        let state = engine.snapshot().await?;
        if state.status == PlaybackStatus::Error {
            return Err(state.error.unwrap_or_default());
        }
    }
    Ok(peak)
}
fn collect(receive: &mut Consumer<f32>, duration: Duration) -> f32 {
    let start = Instant::now();
    let mut peak = 0.0_f32;
    while start.elapsed() < duration {
        while let Ok(sample) = receive.pop() {
            peak = peak.max(sample.abs());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    peak
}
