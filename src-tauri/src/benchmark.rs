//! Opt-in release-build lifecycle probe. Drives the same production handoffs;
//! private memory is sampled externally, including WebView2/resolver children.
use crate::{
    audio::AudioEngine,
    browser::{self, media, BrowserState},
    desktop,
    playback_state::PlaybackStatus,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

fn directory() -> Option<PathBuf> {
    std::env::var_os("MEDIABROWSER_BENCH_DIR").map(PathBuf::from)
}
fn phase(app: &AppHandle, name: &str, detail: serde_json::Value) -> Result<(), String> {
    let dir = directory().ok_or("Missing benchmark directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let value = serde_json::json!({"phase": name, "pid": std::process::id(), "webviews": app.webviews().keys().collect::<Vec<_>>(), "windows": app.windows().keys().collect::<Vec<_>>(), "detail": detail});
    std::fs::write(dir.join("phase.tmp"), serde_json::to_vec(&value).unwrap())
        .map_err(|e| e.to_string())?;
    std::fs::rename(dir.join("phase.tmp"), dir.join("phase.json")).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("lifecycle.jsonl"))
        .map_err(|e| e.to_string())?;
    writeln!(log, "{value}").map_err(|e| e.to_string())
}
pub(crate) fn start(app: &AppHandle) {
    if directory().is_none() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = run(&app).await;
        let (name, detail) = match result {
            Ok(()) => ("complete", serde_json::json!({"success":true})),
            Err(error) => ("failed", serde_json::json!({"error":error})),
        };
        let _ = phase(&app, name, detail);
        desktop::quit(&app);
    });
}
async fn sample(app: &AppHandle, name: &str, native_audio: bool) -> Result<(), String> {
    // Allow asynchronous Tauri destruction/process exits to settle. The sampler
    // still records these transition samples separately from steady state.
    phase(app, "settling", serde_json::json!({"next":name}))?;
    tokio::time::sleep(Duration::from_secs(8)).await;
    if name.starts_with("ram-saver") && !app.webviews().is_empty() {
        return Err("RAM Saver retained a WebView".into());
    }
    let engine = app.state::<AudioEngine>();
    let before = engine.snapshot().await?;
    let browser_before = if name.starts_with("browser") {
        Some(media::capture(&app.get_webview("youtube").ok_or("Missing YouTube view")?).await?)
    } else {
        None
    };
    phase(
        app,
        name,
        serde_json::json!({"audio":before,"browser":browser_before}),
    )?;
    let seconds = std::env::var("MEDIABROWSER_BENCH_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(20)
        .clamp(5, 120);
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let after = engine.snapshot().await?;
    if let Some(before) = browser_before {
        let after =
            media::capture(&app.get_webview("youtube").ok_or("Missing YouTube view")?).await?;
        if after.paused || after.position_seconds - before.position_seconds < seconds as f64 * 0.8 {
            return Err(format!("Browser playback did not progress in {name}"));
        }
    }
    if native_audio
        && (after.status != PlaybackStatus::Playing
            || after.position_seconds - before.position_seconds < seconds as f64 * 0.8)
    {
        return Err(format!(
            "Audio failed to progress in {name}: {:?}, {} -> {}",
            after.status, before.position_seconds, after.position_seconds
        ));
    }
    Ok(())
}
async fn run(app: &AppHandle) -> Result<(), String> {
    phase(app, "starting", serde_json::Value::Null)?;
    if app.try_state::<crate::media_keys::MediaKeys>().is_none() {
        return Err("Windows media session was not initialized".into());
    }
    if std::env::var_os("MEDIABROWSER_BENCH_RESUME").is_some() {
        return resume(app).await;
    }
    let url = std::env::var("MEDIABROWSER_BENCH_URL")
        .unwrap_or_else(|_| "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into());
    crate::resolver::canonical_video_url(&url)?;
    app.state::<BrowserState>()
        .observe_url(&url.parse().map_err(|_| "Invalid benchmark URL")?);
    browser::reopen(app).await?;
    let view = app.get_webview("youtube").ok_or("Missing YouTube view")?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut target = loop {
        if let Ok(state) = media::capture(&view).await {
            break state;
        }
        if Instant::now() >= deadline {
            return Err("YouTube never became ready (network, ad or consent). No Browser/RAM comparison was made.".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    target.paused = false;
    target.volume = 0.15;
    target.position_seconds = 10.0;
    media::restore(&view, &target).await?;
    drop(view);
    sample(app, "browser", false).await?;
    phase(app, "handoff", serde_json::Value::Null)?;
    browser::handoff::enter_ram_saver(app).await?;
    sample(app, "ram-saver-1", true).await?;
    // Isolate the old controller-only design's retained browser-process cost.
    phase(app, "opening-controller", serde_json::Value::Null)?;
    desktop::open_controller(app)?;
    sample(app, "controller-only", true).await?;
    phase(app, "closing-controller", serde_json::Value::Null)?;
    desktop::destroy_ui(app)?;
    sample(app, "ram-saver-2", true).await?;
    phase(app, "transport-checks", serde_json::Value::Null)?;
    verify_media_session(app).await?;
    desktop::perform(app, desktop::Action::Pause).await?;
    let paused = app.state::<AudioEngine>().snapshot().await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let still = app.state::<AudioEngine>().snapshot().await?;
    if still.status != PlaybackStatus::Paused
        || (still.position_seconds - paused.position_seconds).abs() > 0.03
    {
        return Err("Tray pause failed".into());
    }
    desktop::perform(app, desktop::Action::Volume(0.1)).await?;
    desktop::perform(app, desktop::Action::Play).await?;
    phase(app, "reopening", serde_json::Value::Null)?;
    browser::reopen(app).await?;
    sample(app, "browser-reopened", false).await?;
    phase(app, "handoff-2", serde_json::Value::Null)?;
    browser::handoff::enter_ram_saver(app).await?;
    sample(app, "ram-saver-3", true).await?;
    // Exercise Tauri's actual last-window path as well as its normal SMTC host
    // arrangement. Media keys were verified before intentionally removing host.
    phase(app, "last-window-check", serde_json::Value::Null)?;
    app.state::<crate::media_keys::MediaKeys>().close();
    app.get_window("media-session")
        .ok_or("Missing native media host")?
        .destroy()
        .map_err(|e| e.to_string())?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    if !app.windows().is_empty()
        || app.state::<AudioEngine>().snapshot().await?.status != PlaybackStatus::Playing
    {
        return Err("Last-window close did not preserve headless playback".into());
    }
    phase(app, "last-window-verified", serde_json::Value::Null)?;
    Ok(())
}

async fn resume(app: &AppHandle) -> Result<(), String> {
    use crate::playback_state::persistence::{SavedPlayback, Source};
    let saved = SavedPlayback::load(
        &desktop::data_dir(app)
            .map_err(|e| e.to_string())?
            .join("playback-state.json"),
    )?
    .ok_or("Missing checkpoint")?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let browser = app.state::<BrowserState>().get();
        if browser.mode == crate::playback_state::PlaybackMode::RamSaver {
            break;
        }
        if let Some(error) = browser.error {
            return Err(error);
        }
        if Instant::now() >= deadline {
            return Err("Checkpoint restore timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let state = app.state::<AudioEngine>().snapshot().await?;
    if state.status != PlaybackStatus::Paused || !app.webviews().is_empty() {
        return Err("Checkpoint did not restore paused with no WebViews".into());
    }
    let (position, volume) = match saved.source {
        Source::Youtube { playback } => {
            if state.metadata.as_ref().map(|m| m.id.as_str()) != Some(playback.video_id.as_str()) {
                return Err("Restored a different video".into());
            }
            (playback.position_seconds, playback.volume)
        }
        Source::Local {
            position_seconds,
            volume,
            ..
        } => (position_seconds, volume),
    };
    if (position - state.position_seconds).abs() > 0.25 || (volume - state.volume).abs() > 0.01 {
        return Err("Checkpoint position or volume was not restored".into());
    }
    phase(
        app,
        "restored-checkpoint",
        serde_json::to_value(&state).unwrap(),
    )?;
    desktop::perform(app, desktop::Action::Play).await?;
    sample(app, "ram-saver-restart", true).await?;
    verify_media_session(app).await?;
    Ok(())
}

async fn verify_media_session(app: &AppHandle) -> Result<(), String> {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as WindowsStatus,
    };
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|e| e.to_string())?
        .await
        .map_err(|e| e.to_string())?;
    let found = {
        let sessions = manager.GetSessions().map_err(|e| e.to_string())?;
        let mut found = None;
        for index in 0..sessions.Size().map_err(|e| e.to_string())? {
            let session = sessions.GetAt(index).map_err(|e| e.to_string())?;
            let id = session
                .SourceAppUserModelId()
                .map_err(|e| e.to_string())?
                .to_string();
            if id.to_lowercase().contains("media-browser")
                || id.to_lowercase().contains("mediabrowser")
            {
                found = Some((session, id));
                break;
            }
        }
        found
    };
    let (session, id) =
        found.ok_or("MediaBrowser did not register a discoverable Windows media session")?;
    for playing in [false, true] {
        let operation = if playing {
            session.TryPlayAsync()
        } else {
            session.TryPauseAsync()
        }
        .map_err(|e| e.to_string())?;
        let accepted = tokio::time::timeout(Duration::from_secs(6), async { operation.await })
            .await
            .map_err(|_| "Windows media control request timed out")?
            .map_err(|e| e.to_string())?;
        if !accepted {
            return Err("Windows rejected the media control request".into());
        }
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let status = app.state::<AudioEngine>().snapshot().await?.status;
            let windows_status = session
                .GetPlaybackInfo()
                .and_then(|info| info.PlaybackStatus())
                .map_err(|e| e.to_string())?;
            if status
                == if playing {
                    PlaybackStatus::Playing
                } else {
                    PlaybackStatus::Paused
                }
                && windows_status
                    == if playing {
                        WindowsStatus::Playing
                    } else {
                        WindowsStatus::Paused
                    }
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!("Windows media control did not reach the audio worker: requested playing={playing}, engine={status:?}, Windows={windows_status:?}, error={:?}", app.state::<BrowserState>().get().error));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    phase(
        app,
        "media-session-verified",
        serde_json::json!({"session":id,"pause":true,"play":true}),
    )
}
