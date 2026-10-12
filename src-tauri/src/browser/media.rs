//! A one-way, on-demand WebView2 evaluation. Remote content receives no IPC grants.
use crate::playback_state::BrowserPlayback;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Webview;

const READ: &str = include_str!("media.js");

async fn evaluate(view: &Webview, body: String) -> Result<serde_json::Value, String> {
    if !view
        .url()
        .is_ok_and(|url| super::url_policy::is_youtube_page(&url))
    {
        return Err("Open a public YouTube video first.".into());
    }
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        + 2500;
    let script = format!("(() => {{ try {{ if (Date.now() > {deadline}) return null; const read = ({READ}); {body} }} catch (_) {{ return null; }} }})()");
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    view.eval_with_callback(script, move |result| {
        let _ = send.try_send(result);
    })
    .map_err(|_| "Cannot inspect YouTube playback.")?;
    let json = tokio::time::timeout(Duration::from_secs(3), receive.recv())
        .await
        .map_err(|_| "YouTube playback inspection timed out. Retry when the page responds.")?
        .ok_or("YouTube browser closed during the transition.")?;
    if json.len() > 16384 {
        return Err("YouTube returned oversized playback state.".into());
    }
    serde_json::from_str(&json).map_err(|_| "Invalid YouTube playback response.".into())
}

pub async fn capture(view: &Webview) -> Result<BrowserPlayback, String> {
    let value = evaluate(view, "return read();".into()).await?;
    let state: BrowserPlayback = serde_json::from_value(value)
        .map_err(|_| "Wait for the video to load or the ad to finish, then retry. RAM Saver needs a public, non-live YouTube video.".to_owned())?;
    state.validate()?;
    Ok(state)
}

/// Identity checks and side effects happen in the same JavaScript task.
pub async fn apply(view: &Webview, expected: &BrowserPlayback, action: &str) -> Result<(), String> {
    expected.validate()?;
    let target = serde_json::to_string(expected).map_err(|_| "Invalid restore state.")?;
    let body = format!("const expected = {target}; const current = read(); if (!current || current.videoId !== expected.videoId || current.playlistId !== expected.playlistId || current.playlistIndex !== expected.playlistIndex) return false; const video = document.querySelector('#movie_player video'); {action} return true;");
    if evaluate(view, body).await? != serde_json::Value::Bool(true) {
        return Err("YouTube playback changed or is not ready. Retry the mode switch.".into());
    }
    Ok(())
}

pub async fn pause(view: &Webview, expected: &BrowserPlayback) -> Result<(), String> {
    apply(view, expected, "if (Math.abs(video.currentTime - expected.positionSeconds) > 3 || video.paused !== expected.paused) return false; video.dataset.mediaBrowserPaused = expected.paused ? 'already-paused' : 'handoff'; video.pause();").await
}

pub async fn restore(view: &Webview, target: &BrowserPlayback) -> Result<(), String> {
    apply(view, target, "video.volume = expected.volume; video.muted = false; video.currentTime = expected.positionSeconds; if (expected.paused) video.pause(); else video.play().catch(() => {});").await
}

pub async fn resume(view: &Webview, target: &BrowserPlayback) -> Result<(), String> {
    apply(
        view,
        target,
        "if (!expected.paused && video.dataset.mediaBrowserPaused === 'handoff') video.play().catch(() => {}); delete video.dataset.mediaBrowserPaused;",
    )
    .await
}
