//! Shared browser and local audio playback vocabulary.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackMode {
    #[default]
    Browser,
    RamSaver,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackStatus {
    #[default]
    Unavailable,
    Loading,
    Playing,
    Paused,
    Stopped,
    Ended,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSnapshot {
    pub status: PlaybackStatus,
    pub path: Option<String>,
    pub metadata: Option<crate::resolver::VideoMetadata>,
    pub transfer: Option<crate::audio::TransferSnapshot>,
    pub position_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub volume: f32,
    pub error: Option<String>,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            path: None,
            metadata: None,
            transfer: None,
            position_seconds: 0.0,
            duration_seconds: None,
            volume: 1.0,
            error: None,
        }
    }
}
