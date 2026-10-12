//! Shared browser and local audio playback vocabulary.

use serde::{Deserialize, Serialize};
pub mod persistence;

/// Public page identity and HTML media state; never stream URLs or cookies.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserPlayback {
    pub url: String,
    pub video_id: String,
    pub playlist_id: Option<String>,
    /// YouTube URL indexes are one based.
    pub playlist_index: Option<u64>,
    pub position_seconds: f64,
    pub volume: f32,
    pub paused: bool,
}

impl BrowserPlayback {
    pub fn validate(&self) -> Result<(), String> {
        let video = crate::resolver::canonical_video_url(&self.url)?;
        if video
            .query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())
            != Some(self.video_id.clone())
            || !self.position_seconds.is_finite()
            || !(0.0..=1e9).contains(&self.position_seconds)
            || !self.volume.is_finite()
            || !(0.0..=1.0).contains(&self.volume)
            || self.playlist_index == Some(0)
        {
            return Err(
                "YouTube returned invalid playback state. Reload the video and retry.".into(),
            );
        }
        let url = url::Url::parse(&self.url).map_err(|_| "Invalid page URL.")?;
        let list = url
            .query_pairs()
            .find(|(k, _)| k == "list")
            .map(|(_, v)| v.into_owned());
        let index = url
            .query_pairs()
            .find(|(k, _)| k == "index")
            .map(|(_, v)| v.parse::<u64>())
            .transpose()
            .map_err(|_| "Invalid playlist index.")?;
        if list != self.playlist_id || index != self.playlist_index {
            return Err("YouTube's playlist context changed. Retry RAM Saver.".into());
        }
        if let Some(id) = &self.playlist_id {
            crate::resolver::playlist::canonical_playlist_url(&self.url)?;
            if id.starts_with("RD") || matches!(id.as_str(), "WL" | "LL") {
                return Err("RAM Saver supports public playlists, not Mix, radio, Watch Later or liked-video queues.".into());
            }
        }
        Ok(())
    }
    pub fn same_track(&self, other: &Self) -> bool {
        self.video_id == other.video_id
            && self.playlist_id == other.playlist_id
            && self.playlist_index == other.playlist_index
    }
    pub fn watch_url(&self) -> url::Url {
        let mut url = url::Url::parse("https://www.youtube.com/watch").unwrap();
        let mut query = url.query_pairs_mut();
        query.append_pair("v", &self.video_id);
        if let Some(list) = &self.playlist_id {
            query.append_pair("list", list);
            if let Some(index) = self.playlist_index {
                query.append_pair("index", &index.to_string());
            }
        }
        query.append_pair("t", &format!("{:.3}s", self.position_seconds));
        drop(query);
        url
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
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
    pub queue: Option<crate::queue::QueueSnapshot>,
    pub queue_page: Option<crate::queue::QueuePage>,
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
            queue: None,
            queue_page: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> BrowserPlayback {
        BrowserPlayback {
            url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ&list=PL_test&index=3".into(),
            video_id: "aqz-KE-bpKQ".into(),
            playlist_id: Some("PL_test".into()),
            playlist_index: Some(3),
            position_seconds: 12.5,
            volume: 0.6,
            paused: false,
        }
    }
    #[test]
    fn playback_context_validates_url_identity_indexes_and_numeric_bounds() {
        let good = state();
        good.validate().unwrap();
        for bad in [
            BrowserPlayback {
                url: "https://example.com/watch?v=aqz-KE-bpKQ".into(),
                ..good.clone()
            },
            BrowserPlayback {
                video_id: "BaW_jenozKc".into(),
                ..good.clone()
            },
            BrowserPlayback {
                playlist_id: None,
                ..good.clone()
            },
            BrowserPlayback {
                playlist_index: Some(2),
                ..good.clone()
            },
            BrowserPlayback {
                position_seconds: f64::NAN,
                ..good.clone()
            },
            BrowserPlayback {
                position_seconds: -1.0,
                ..good.clone()
            },
            BrowserPlayback {
                volume: f32::INFINITY,
                ..good.clone()
            },
            BrowserPlayback {
                volume: 1.1,
                ..good.clone()
            },
        ] {
            assert!(bad.validate().is_err());
        }
        for list in ["RDaqz-KE-bpKQ", "WL", "LL"] {
            let mut bad = good.clone();
            bad.playlist_id = Some(list.into());
            bad.url = bad.watch_url().to_string();
            assert!(bad.validate().is_err());
        }
        let moved = BrowserPlayback {
            playlist_index: Some(4),
            ..good.clone()
        };
        assert!(!good.same_track(&moved));
        assert!(good.same_track(&BrowserPlayback {
            position_seconds: 90.0,
            ..good.clone()
        }));
    }
}
