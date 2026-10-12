use super::{AudioEngine, PlaybackSnapshot};
use tauri::{Manager, State, Webview};

fn authorize_read(view: &Webview) -> Result<(), String> {
    if view.label() == "controller"
        && view
            .url()
            .is_ok_and(|url| crate::browser::is_local_url(&url))
    {
        Ok(())
    } else {
        Err("Audio commands are available only to the local controller.".into())
    }
}

fn authorize(view: &Webview) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, String> {
    authorize_read(view)?;
    view.app_handle()
        .state::<crate::browser::BrowserState>()
        .transition
        .clone()
        .try_read_owned()
        .map_err(|_| "A mode switch or playback operation is already in progress.".into())
}

#[tauri::command]
pub async fn audio_load(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    path: String,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.load(path).await
}
#[tauri::command]
pub async fn audio_play(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.play().await
}
#[tauri::command]
pub async fn audio_pause(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.pause().await
}
#[tauri::command]
pub async fn audio_stop(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.stop().await
}
#[tauri::command]
pub async fn audio_seek(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    seconds: f64,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.seek(seconds).await
}
#[tauri::command]
pub async fn audio_volume(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    volume: f32,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.set_volume(volume).await
}
#[tauri::command]
pub async fn audio_snapshot(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    authorize_read(&webview)?;
    engine.snapshot().await
}

#[tauri::command]
pub async fn audio_load_youtube(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    url: String,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.load_youtube(&url).await
}

#[tauri::command]
pub async fn audio_queue(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    action: crate::queue::QueueAction,
) -> Result<PlaybackSnapshot, String> {
    let _guard = authorize(&webview)?;
    engine.queue(action).await
}
