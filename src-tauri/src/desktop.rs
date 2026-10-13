//! Native tray owns application lifetime. No WebView is needed for playback.
use crate::{
    audio::AudioEngine,
    browser::BrowserState,
    playback_state::{
        persistence::{SavedPlayback, Source},
        PlaybackMode, PlaybackSnapshot, PlaybackStatus,
    },
    queue::QueueAction,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};
use tokio_util::sync::CancellationToken;

pub(crate) struct Desktop {
    quitting: AtomicBool,
    ready: AtomicBool,
    stop: CancellationToken,
    checkpoint: tokio::sync::Mutex<()>,
    path: PathBuf,
    status: MenuItem<tauri::Wry>,
    toggle: MenuItem<tauri::Wry>,
    next: MenuItem<tauri::Wry>,
    previous: MenuItem<tauri::Wry>,
    actions: tokio::sync::mpsc::Sender<Action>,
}

#[derive(Clone, Copy)]
pub(crate) enum Action {
    Toggle,
    Play,
    Pause,
    Next,
    Previous,
    Volume(f32),
    Reopen,
    Saver,
    Quit,
}

pub(crate) fn is_quitting(app: &AppHandle) -> bool {
    app.try_state::<Desktop>()
        .is_some_and(|s| s.quitting.load(Ordering::Acquire))
}
pub(crate) fn exit_ready(app: &AppHandle) -> bool {
    app.try_state::<Desktop>()
        .is_some_and(|s| s.ready.load(Ordering::Acquire))
}

pub(crate) fn data_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    #[cfg(feature = "memory-benchmark")]
    if let Some(path) = std::env::var_os("MEDIABROWSER_BENCH_DIR") {
        return Ok(PathBuf::from(path).join("state"));
    }
    app.path().app_data_dir()
}

pub(crate) fn open_controller(app: &AppHandle) -> Result<(), String> {
    if is_quitting(app) {
        return Err("MediaBrowser is quitting.".into());
    }
    if let Some(window) = app.get_webview_window("controller") {
        window.show().map_err(|e| e.to_string())?;
        return window.set_focus().map_err(|e| e.to_string());
    }
    tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])
        .map_err(|e| e.to_string())?
        .on_navigation(crate::browser::is_local_url)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn destroy_ui(app: &AppHandle) -> Result<(), String> {
    let mut errors = Vec::new();
    // destroy bypasses CloseRequested vetoes and releases each child WebView2.
    for label in ["browser", "controller"] {
        if let Some(window) = app.get_window(label) {
            if let Err(error) = window.destroy() {
                errors.push(error.to_string());
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Playback continues, but a window could not be released: {}",
            errors.join("; ")
        ))
    }
}

pub(crate) fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let (actions, mut pending_actions) = tokio::sync::mpsc::channel(16);
    let status = MenuItem::with_id(app, "status", "MediaBrowser · Ready", false, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Play / Pause", false, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", false, None::<&str>)?;
    let previous = MenuItem::with_id(app, "previous", "Previous", false, None::<&str>)?;
    let down = MenuItem::with_id(app, "volume-down", "Volume −10%", true, None::<&str>)?;
    let up = MenuItem::with_id(app, "volume-up", "Volume +10%", true, None::<&str>)?;
    let reopen = MenuItem::with_id(app, "reopen", "Reopen browser", true, None::<&str>)?;
    let saver = MenuItem::with_id(
        app,
        "saver",
        "RAM Saver (close all windows)",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &status, &toggle, &next, &previous, &down, &up, &separator, &reopen, &saver, &quit,
        ],
    )?;
    app.manage(Desktop {
        quitting: AtomicBool::new(false),
        ready: AtomicBool::new(false),
        stop: CancellationToken::new(),
        checkpoint: tokio::sync::Mutex::new(()),
        path: data_dir(app.handle())?.join("playback-state.json"),
        status,
        toggle,
        next,
        previous,
        actions,
    });
    TrayIconBuilder::with_id("player")
        .icon(
            app.default_window_icon()
                .ok_or("Missing tray icon")?
                .clone(),
        )
        .tooltip("MediaBrowser · playback continues in the tray")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let action = match event.id.as_ref() {
                "toggle" => Action::Toggle,
                "next" => Action::Next,
                "previous" => Action::Previous,
                "volume-down" => Action::Volume(-0.1),
                "volume-up" => Action::Volume(0.1),
                "reopen" => Action::Reopen,
                "saver" => Action::Saver,
                "quit" => Action::Quit,
                _ => return,
            };
            dispatch(app, action);
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                dispatch(tray.app_handle(), Action::Reopen);
            }
        })
        .build(app)?;
    let saved_saver = SavedPlayback::load(&app.state::<Desktop>().path)
        .ok()
        .flatten()
        .is_some_and(|s| s.mode == PlaybackMode::RamSaver);
    // A saved tray-only session must not create WebView2 just to tear it down.
    if !saved_saver {
        open_controller(app.handle())?;
    }
    // A plain native HWND hosts SMTC; it contains no WebView or browser runtime.
    match crate::media_keys::setup(app.handle()) {
        Ok(controls) => {
            app.manage(controls);
        }
        Err(error) => report(
            app.handle(),
            &format!("Windows media keys unavailable: {error}. Tray controls still work."),
        ),
    }
    let handle = app.handle().clone();
    let command_handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let stop = command_handle.state::<Desktop>().stop.clone();
        loop {
            let action = tokio::select! {
                biased;
                _ = stop.cancelled() => break,
                action = pending_actions.recv() => match action { Some(action) => action, None => break },
            };
            if let Err(error) = perform(&command_handle, action).await {
                report(&command_handle, &error);
            }
        }
    });
    tauri::async_runtime::spawn(async move {
        if let Err(error) = restore_checkpoint(&handle).await {
            report(&handle, &error);
            let _ = open_controller(&handle);
        }
        let stop = handle.state::<Desktop>().stop.clone();
        let mut tick = 0u32;
        loop {
            tokio::select! { biased; _ = stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
            // Do not cancel a checkpoint halfway through its blocking atomic
            // write; Quit waits for the same checkpoint mutex before final save.
            refresh(&handle, tick.is_multiple_of(10)).await;
            tick = tick.wrapping_add(1);
        }
    });
    Ok(())
}

async fn restore_checkpoint(app: &AppHandle) -> Result<(), String> {
    #[cfg(feature = "memory-benchmark")]
    if std::env::var_os("MEDIABROWSER_BENCH_DIR").is_some()
        && std::env::var_os("MEDIABROWSER_BENCH_RESUME").is_none()
    {
        return Ok(());
    }
    let browser = app.state::<BrowserState>();
    let _guard = browser.transition.write().await;
    let saved = SavedPlayback::load(&app.state::<Desktop>().path)?;
    if let Some(saved) = saved {
        if saved.mode == PlaybackMode::RamSaver {
            saved.restore(&app.state::<AudioEngine>()).await?;
            browser.update(|s| s.mode = PlaybackMode::RamSaver);
            destroy_ui(app)?;
        } else if let Source::Youtube { playback } = saved.source {
            // Browser resumes at a timestamp when explicitly reopened.
            browser.observe_url(&playback.watch_url());
        }
    }
    Ok(())
}

pub(crate) fn report(app: &AppHandle, error: &str) {
    app.state::<BrowserState>()
        .update(|s| s.error = Some(error.to_owned()));
    crate::browser::publish(app);
    let text: String = error.chars().take(160).collect();
    let _ = app
        .state::<Desktop>()
        .status
        .set_text(format!("Error: {text}"));
    if let Some(tray) = app.tray_by_id("player") {
        let _ = tray.set_tooltip(Some(&format!("MediaBrowser: {text}")));
    }
}

pub(crate) fn dispatch(app: &AppHandle, action: Action) {
    if matches!(action, Action::Quit) {
        quit(app);
        return;
    }
    if is_quitting(app) {
        return;
    }
    // Rapid media-key events are ordered and bounded. They must not disappear
    // because the previous key is still saving its checkpoint or updating SMTC.
    if app.state::<Desktop>().actions.try_send(action).is_err() {
        report(app, "Playback controls are busy; retry shortly.");
    }
}

pub(crate) async fn perform(app: &AppHandle, action: Action) -> Result<(), String> {
    match action {
        Action::Reopen => return crate::browser::reopen(app).await,
        Action::Saver => return crate::browser::handoff::enter_ram_saver(app).await,
        Action::Quit => return Ok(()),
        _ => {}
    }
    let browser = app.state::<BrowserState>();
    let _guard = browser.transition.write().await;
    if is_quitting(app) {
        return Ok(());
    }
    if browser.get().mode != PlaybackMode::RamSaver && app.get_webview("youtube").is_some() {
        return Err("Use YouTube's controls in Browser Mode, or switch to RAM Saver.".into());
    }
    let engine = app.state::<AudioEngine>();
    let snapshot = engine.snapshot().await?;
    let snapshot = match action {
        Action::Toggle if snapshot.status == PlaybackStatus::Playing => engine.pause().await?,
        Action::Toggle | Action::Play => engine.play().await?,
        Action::Pause => engine.pause().await?,
        Action::Next => engine.queue(QueueAction::Next).await?,
        Action::Previous => engine.queue(QueueAction::Previous).await?,
        Action::Volume(delta) => {
            engine
                .set_volume((snapshot.volume + delta).clamp(0.0, 1.0))
                .await?
        }
        _ => unreachable!(),
    };
    browser.update(|s| s.error = None);
    update_controls(app, &snapshot);
    checkpoint_audio(app, &snapshot).await?;
    Ok(())
}

fn update_controls(app: &AppHandle, snapshot: &PlaybackSnapshot) {
    let state = app.state::<Desktop>();
    let active = app.state::<BrowserState>().get().mode == PlaybackMode::RamSaver
        || app.get_webview("youtube").is_none();
    let loaded = snapshot.path.is_some();
    let playing = snapshot.status == PlaybackStatus::Playing;
    if let Some(keys) = app.try_state::<crate::media_keys::MediaKeys>() {
        if let Err(error) = keys.update(snapshot, active && loaded) {
            report(
                app,
                &format!("Windows media session update failed: {error}"),
            );
        }
    }
    let _ = state
        .toggle
        .set_text(if playing { "Pause" } else { "Play" });
    let _ = state.toggle.set_enabled(active && loaded);
    let queue = active && snapshot.queue.as_ref().is_some_and(|q| q.total > 0);
    let _ = state.next.set_enabled(queue);
    let _ = state.previous.set_enabled(queue);
    // Leave errors visible until an action succeeds (including in headless mode).
    if app.state::<BrowserState>().get().error.is_none() {
        let title = snapshot
            .metadata
            .as_ref()
            .map_or("MediaBrowser", |m| m.title.as_str());
        let _ = state.status.set_text(format!(
            "{} · {:?} · {:.0}%",
            title.chars().take(60).collect::<String>(),
            snapshot.status,
            snapshot.volume * 100.0
        ));
    }
}

pub(crate) async fn checkpoint_audio(
    app: &AppHandle,
    snapshot: &PlaybackSnapshot,
) -> Result<(), String> {
    if let Some(saved) = SavedPlayback::audio(snapshot)? {
        save(app, saved).await?;
    }
    Ok(())
}
async fn save(app: &AppHandle, saved: SavedPlayback) -> Result<(), String> {
    let state = app.state::<Desktop>();
    let _guard = state.checkpoint.lock().await;
    let path = state.path.clone();
    tauri::async_runtime::spawn_blocking(move || saved.save(&path))
        .await
        .map_err(|e| e.to_string())?
}
async fn refresh(app: &AppHandle, checkpoint: bool) {
    let browser = app.state::<BrowserState>();
    let Ok(_guard) = browser.transition.try_read() else {
        return;
    };
    if let Ok(snapshot) = app.state::<AudioEngine>().snapshot().await {
        if let Some(error) = snapshot.error.as_deref() {
            if browser.get().error.as_deref() != Some(error) {
                report(app, error);
            }
        }
        update_controls(app, &snapshot);
        if checkpoint {
            let result = if browser.get().mode == PlaybackMode::RamSaver
                || app.get_webview("youtube").is_none()
            {
                checkpoint_audio(app, &snapshot).await
            } else if let Some(view) = app.get_webview("youtube") {
                match crate::browser::media::capture(&view).await {
                    Ok(playback) => save(app, SavedPlayback::browser(playback)).await,
                    Err(_) => Ok(()), // Home, consent and ads have no restorable track.
                }
            } else {
                Ok(())
            };
            if let Err(error) = result {
                report(app, &error);
            }
        }
    }
}

pub(crate) fn quit(app: &AppHandle) {
    let state = app.state::<Desktop>();
    if state.quitting.swap(true, Ordering::AcqRel) {
        return;
    }
    state.stop.cancel();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Cancel first: waiting for the transition lock first could leave Quit
        // stuck behind yt-dlp or an HTTP response that needs cancellation.
        let final_audio = app.state::<AudioEngine>().shutdown().await;
        let browser = app.state::<BrowserState>();
        let _guard = browser.transition.write().await;
        if browser.get().mode == PlaybackMode::RamSaver || app.get_webview("youtube").is_none() {
            if let Ok(snapshot) = final_audio {
                if let Err(error) = checkpoint_audio(&app, &snapshot).await {
                    eprintln!("{error}");
                }
            }
        } else if let Some(view) = app.get_webview("youtube") {
            if let Ok(playback) = crate::browser::media::capture(&view).await {
                if let Err(error) = save(&app, SavedPlayback::browser(playback)).await {
                    eprintln!("{error}");
                }
            }
        }
        app.state::<Desktop>().ready.store(true, Ordering::Release);
        if let Some(keys) = app.try_state::<crate::media_keys::MediaKeys>() {
            keys.close();
        }
        app.exit(0);
    });
}
