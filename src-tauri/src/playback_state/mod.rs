//! Shared vocabulary for the later audio stage. No simulated playback state.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackMode {
    #[default]
    Browser,
    RamSaver,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackStatus {
    #[default]
    Unavailable,
    Loading,
    Playing,
    Paused,
    Stopped,
}
