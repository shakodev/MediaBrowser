//! Mode transitions are serialized with navigation and local audio commands.
use super::{media, native, publish, BrowserState};
use crate::{
    audio::AudioEngine,
    playback_state::{BrowserPlayback, PlaybackMode, PlaybackSnapshot, PlaybackStatus},
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Webview};

struct Switching(AppHandle);
impl Switching {
    fn begin(app: &AppHandle) -> Self {
        app.state::<BrowserState>().update(|s| {
            s.switching = true;
            s.error = None;
        });
        publish(app);
        Self(app.clone())
    }
    fn finish(&self, result: &Result<(), String>) {
        if let Err(error) = result {
            self.0
                .state::<BrowserState>()
                .update(|s| s.error = Some(error.clone()));
        }
    }
}
impl Drop for Switching {
    fn drop(&mut self) {
        self.0
            .state::<BrowserState>()
            .update(|s| s.switching = false);
        publish(&self.0);
    }
}

#[tauri::command]
pub async fn ram_saver(webview: Webview, app: AppHandle) -> Result<(), String> {
    super::authorize(&webview)?;
    enter_ram_saver(&app).await
}

pub(crate) async fn enter_ram_saver(app: &AppHandle) -> Result<(), String> {
    if crate::desktop::is_quitting(app) {
        return Err("MediaBrowser is quitting.".into());
    }
    let state = app.state::<BrowserState>();
    let _lock = state
        .transition
        .try_write()
        .map_err(|_| "A playback operation is already in progress.")?;
    if state.get().mode == PlaybackMode::RamSaver {
        return crate::desktop::destroy_ui(app);
    }
    let switching = Switching::begin(app);
    let result = enter(app).await;
    switching.finish(&result);
    result
}

async fn enter(app: &AppHandle) -> Result<(), String> {
    let view = app
        .get_webview("youtube")
        .ok_or("Open YouTube and choose a public video first.")?;
    let initial = media::capture(&view).await?;
    let engine = app.state::<AudioEngine>();
    let mut browser_paused = false;
    let mut latest = initial.clone();
    let result = async {
        engine.prepare_handoff(initial.clone()).await?;
        // Resolution/playlist extraction can take time. Use a fresh timestamp,
        // and abort if SPA navigation selected a different track or occurrence.
        latest = media::capture(&view).await?;
        if !initial.same_track(&latest) {
            return Err("YouTube changed tracks while preparing RAM Saver. Retry.".into());
        }
        let started = engine.start_handoff(latest.clone()).await?;
        confirm_engine(&engine, &started).await?;
        let current = media::capture(&view).await?;
        let audio = engine.snapshot().await?;
        if !latest.same_track(&current)
            || latest.paused != current.paused
            || (!current.paused && (audio.position_seconds - current.position_seconds).abs() > 2.0)
            || (current.paused && (latest.position_seconds - current.position_seconds).abs() > 0.25)
        {
            return Err("YouTube playback changed during handoff. Retry RAM Saver.".into());
        }
        latest = current;
        browser_paused = true;
        media::pause(&view, &latest).await?;
        engine.commit_handoff(latest.volume, latest.paused).await?;
        if latest.paused {
            engine.seek(latest.position_seconds).await?;
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = engine.pause().await;
        let _ = engine.set_volume(latest.volume).await;
        if browser_paused {
            let _ = media::resume(&view, &latest).await;
        }
        return result;
    }
    // The handoff is committed. A window-release failure must never pause the
    // only audible source or try to resume a view that was already destroyed.
    app.state::<BrowserState>()
        .update(|s| s.mode = PlaybackMode::RamSaver);
    if let Err(error) = crate::desktop::checkpoint_audio(app, &engine.snapshot().await?).await {
        crate::desktop::report(
            app,
            &format!("Playback continues, but saving failed: {error}"),
        );
    }
    drop(view);
    crate::desktop::destroy_ui(app)
}

async fn confirm_engine(engine: &AudioEngine, baseline: &PlaybackSnapshot) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        tokio::time::sleep(Duration::from_millis(60)).await;
        let state = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            engine.snapshot(),
        )
        .await
        .map_err(|_| "Audio output confirmation timed out. Browser playback is preserved.")??;
        if engine_progress(baseline, &state)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("Audio output did not advance. Browser playback is preserved; check the output device.".into());
        }
    }
}

fn engine_progress(baseline: &PlaybackSnapshot, state: &PlaybackSnapshot) -> Result<bool, String> {
    if state.status != PlaybackStatus::Playing
        || state.path != baseline.path
        || state.queue.as_ref().and_then(|q| q.current)
            != baseline.queue.as_ref().and_then(|q| q.current)
    {
        return Err(state.error.clone().unwrap_or_else(|| {
            "Audio could not start at the requested track. Browser playback is preserved.".into()
        }));
    }
    // This clock advances only when PCM reaches the device, including latency.
    Ok(state.position_seconds > baseline.position_seconds + 0.02)
}

pub(crate) fn restore_target(audio: &PlaybackSnapshot) -> Result<BrowserPlayback, String> {
    let metadata = audio
        .metadata
        .as_ref()
        .ok_or("Load a public YouTube video to restore Browser Mode.")?;
    let playlist_id = audio
        .queue
        .as_ref()
        .and_then(|q| q.source_url.as_ref())
        .and_then(|url| url::Url::parse(url).ok())
        .and_then(|url| {
            url.query_pairs()
                .find(|(k, _)| k == "list")
                .map(|(_, v)| v.into_owned())
        });
    // A Mix's disk snapshot index does not describe YouTube's regenerated queue.
    // Reopen around the current video instead of sending a stale recommendation index.
    let playlist_index = if playlist_id
        .as_deref()
        .is_some_and(|id| id.starts_with("RD"))
    {
        None
    } else {
        audio.queue.as_ref().and_then(|q| q.source_index)
    };
    let mut target = BrowserPlayback {
        url: String::new(),
        video_id: metadata.id.clone(),
        playlist_id,
        playlist_index,
        position_seconds: audio.position_seconds,
        volume: audio.volume,
        paused: audio.status != PlaybackStatus::Playing,
    };
    target.url = target.watch_url().to_string();
    target.validate()?;
    Ok(target)
}

pub(crate) async fn return_to_browser(app: &AppHandle) -> Result<(), String> {
    if app.state::<BrowserState>().get().mode != PlaybackMode::RamSaver {
        return super::open(app, None).await;
    }
    let switching = Switching::begin(app);
    let result = restore(app).await;
    switching.finish(&result);
    result
}

async fn restore(app: &AppHandle) -> Result<(), String> {
    let engine = app.state::<AudioEngine>();
    let initial = engine.snapshot().await?;
    // Existing local-file controls can replace a RAM Saver track.
    if initial.metadata.is_none() {
        super::open(app, None).await?;
        engine.pause().await?;
        app.state::<BrowserState>()
            .update(|s| s.mode = PlaybackMode::Browser);
        return Ok(());
    }
    let expected = restore_target(&initial)?;
    super::open(app, Some(expected.watch_url())).await?;
    let view = app
        .get_webview("youtube")
        .ok_or("Browser closed during restoration.")?;
    let mut paused_engine = false;
    let result = async {
        let deadline = Instant::now() + Duration::from_secs(30);
        // Native WebView2 mute remains in effect even if YouTube autoplays an ad.
        loop {
            if crate::desktop::is_quitting(app) { return Err("MediaBrowser is quitting.".into()); }
            if app.get_webview("youtube").is_none() { return Err("Browser closed during restoration. Rust audio is preserved.".into()); }
            if let Ok(current) = media::capture(&view).await {
                if expected.same_track(&current) { break; }
            }
            if Instant::now() >= deadline { return Err("YouTube did not load the requested video (consent, ads or unavailable media). Rust audio is preserved; retry Browser Mode.".into()); }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let target = restore_target(&engine.snapshot().await?)?;
        if !expected.same_track(&target) { return Err("Audio advanced to another track while opening YouTube. Retry Browser Mode.".into()); }
        media::restore(&view, &target).await?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if crate::desktop::is_quitting(app) { return Err("MediaBrowser is quitting.".into()); }
            if let Ok(current) = media::capture(&view).await {
                if !target.same_track(&current) { return Err("YouTube changed tracks during restoration. Retry.".into()); }
                let elapsed = current.position_seconds - target.position_seconds;
                if (current.volume - target.volume).abs() < 0.02 && ((target.paused && current.paused && elapsed.abs() < 0.25)
                    || (!target.paused && !current.paused && elapsed > 0.02 && elapsed < 2.0)) {
                    let audio = restore_target(&engine.snapshot().await?)?;
                    if !target.same_track(&audio) || target.paused != audio.paused || (current.position_seconds - audio.position_seconds).abs() > 2.0 {
                        return Err("Playback moved while YouTube was starting. Rust audio is preserved; retry Browser Mode.".into());
                    }
                    break;
                }
            }
            if Instant::now() >= deadline { return Err("YouTube could not seek or start playback. Rust audio is preserved; retry Browser Mode.".into()); }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        // Source remains available for rollback until native browser unmute succeeds.
        engine.pause().await?;
        paused_engine = true;
        native::set_muted(&view, false).await?;
        app.state::<BrowserState>().update(|s| s.mode = PlaybackMode::Browser);
        Ok(())
    }.await;
    if result.is_err() {
        let _ = view.close();
        if let Some(window) = app.get_window("browser") {
            let _ = window.close();
        }
        if paused_engine && initial.status == PlaybackStatus::Playing {
            let _ = engine.play().await;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_confirmation_requires_media_progress_on_the_requested_track() {
        let baseline = PlaybackSnapshot {
            path: Some("https://www.youtube.com/watch?v=aqz-KE-bpKQ".into()),
            status: PlaybackStatus::Playing,
            position_seconds: 42.0,
            ..Default::default()
        };
        assert!(!engine_progress(&baseline, &baseline).unwrap());
        let advanced = PlaybackSnapshot {
            position_seconds: 42.1,
            ..baseline.clone()
        };
        assert!(engine_progress(&baseline, &advanced).unwrap());
        for invalid in [
            PlaybackSnapshot {
                status: PlaybackStatus::Paused,
                ..advanced.clone()
            },
            PlaybackSnapshot {
                path: Some("different track".into()),
                ..advanced.clone()
            },
            PlaybackSnapshot {
                status: PlaybackStatus::Error,
                error: Some("Device unavailable".into()),
                ..advanced.clone()
            },
        ] {
            assert!(engine_progress(&baseline, &invalid).is_err());
        }
    }
    #[test]
    fn restoration_uses_current_queue_track_position_and_volume() {
        let mut state = PlaybackSnapshot {
            metadata: Some(crate::resolver::VideoMetadata {
                id: "aqz-KE-bpKQ".into(),
                title: "Video".into(),
                uploader: None,
                duration_seconds: None,
            }),
            position_seconds: 42.375,
            volume: 0.37,
            status: PlaybackStatus::Playing,
            ..Default::default()
        };
        let mut playlist = crate::queue::Queue::new(crate::queue::Playlist::new().unwrap());
        playlist.source_url = Some("https://www.youtube.com/playlist?list=PL_public".into());
        let mut queue = playlist.snapshot();
        queue.current = Some(7);
        queue.source_index = Some(8);
        state.queue = Some(queue);
        let target = restore_target(&state).unwrap();
        assert_eq!(target.playlist_index, Some(8));
        assert_eq!(target.position_seconds, 42.375);
        assert_eq!(target.volume, 0.37);
        assert!(!target.paused);
        assert!(target.url.contains("list=PL_public&index=8&t=42.375s"));
        state.status = PlaybackStatus::Stopped;
        assert!(restore_target(&state).unwrap().paused);

        // After Next, restore the new current track in the same radio context.
        state.queue.as_mut().unwrap().source_url = Some(
            "https://www.youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY&start_radio=1".into(),
        );
        let mix = restore_target(&state).unwrap();
        assert_eq!(mix.video_id, "aqz-KE-bpKQ");
        assert_eq!(mix.playlist_id.as_deref(), Some("RD3s93p9xrCIY"));
        assert_eq!(mix.playlist_index, None);
        assert!(mix.url.contains("start_radio=1"));
        assert!(!mix.url.contains("index="));
        assert!(mix.paused);
        let saved = crate::playback_state::persistence::SavedPlayback::audio(&state)
            .unwrap()
            .unwrap();
        assert!(serde_json::to_string(&saved)
            .unwrap()
            .contains("RD3s93p9xrCIY"));
    }
}
