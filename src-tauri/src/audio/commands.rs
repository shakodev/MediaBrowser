use super::{AudioEngine, PlaybackSnapshot};
use tauri::{State, Webview};

fn authorize(view: &Webview) -> Result<(), String> {
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

#[tauri::command]
pub async fn audio_load(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    path: String,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.load(path).await
}
#[tauri::command]
pub async fn audio_play(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.play().await
}
#[tauri::command]
pub async fn audio_pause(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.pause().await
}
#[tauri::command]
pub async fn audio_stop(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.stop().await
}
#[tauri::command]
pub async fn audio_seek(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    seconds: f64,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.seek(seconds).await
}
#[tauri::command]
pub async fn audio_volume(
    webview: Webview,
    engine: State<'_, AudioEngine>,
    volume: f32,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.set_volume(volume).await
}
#[tauri::command]
pub async fn audio_snapshot(
    webview: Webview,
    engine: State<'_, AudioEngine>,
) -> Result<PlaybackSnapshot, String> {
    authorize(&webview)?;
    engine.snapshot().await
}
