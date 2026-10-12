//! Small, versioned checkpoints. Never persist resolved URLs, headers or cookies.
use super::{BrowserPlayback, PlaybackMode, PlaybackSnapshot};
use crate::{
    audio::AudioEngine,
    queue::{QueueAction, Repeat},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Source {
    Youtube {
        playback: BrowserPlayback,
    },
    Local {
        path: PathBuf,
        position_seconds: f64,
        volume: f32,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedPlayback {
    version: u8,
    pub mode: PlaybackMode,
    pub source: Source,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl SavedPlayback {
    pub fn browser(playback: BrowserPlayback) -> Self {
        Self {
            version: 1,
            mode: PlaybackMode::Browser,
            source: Source::Youtube { playback },
            shuffle: false,
            repeat: Repeat::Off,
        }
    }

    pub fn audio(snapshot: &PlaybackSnapshot) -> Result<Option<Self>, String> {
        let source = if snapshot.metadata.is_some() {
            Source::Youtube {
                playback: crate::browser::handoff::restore_target(snapshot)?,
            }
        } else if let Some(path) = &snapshot.path {
            Source::Local {
                path: path.into(),
                position_seconds: snapshot.position_seconds,
                volume: snapshot.volume,
            }
        } else {
            return Ok(None);
        };
        let saved = Self {
            version: 1,
            mode: PlaybackMode::RamSaver,
            source,
            shuffle: snapshot.queue.as_ref().is_some_and(|q| q.shuffle),
            repeat: snapshot.queue.as_ref().map_or(Repeat::Off, |q| q.repeat),
        };
        saved.validate()?;
        Ok(Some(saved))
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported playback checkpoint version.".into());
        }
        match &self.source {
            Source::Youtube { playback } => playback.validate(),
            Source::Local {
                path,
                position_seconds,
                volume,
            } => {
                if !path.is_absolute()
                    || !position_seconds.is_finite()
                    || !(0.0..=1e9).contains(position_seconds)
                    || !volume.is_finite()
                    || !(0.0..=1.0).contains(volume)
                {
                    Err("Invalid local playback checkpoint.".into())
                } else {
                    Ok(())
                }
            }
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Cannot read playback checkpoint: {e}")),
        };
        let mut bytes = Vec::new();
        file.take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err("Playback checkpoint is too large.".into());
        }
        let saved: Self =
            serde_json::from_slice(&bytes).map_err(|_| "Playback checkpoint is corrupt.")?;
        saved.validate()?;
        Ok(Some(saved))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err("Playback checkpoint is too large.".into());
        }
        fs::create_dir_all(path.parent().ok_or("Missing checkpoint directory.")?)
            .map_err(|e| e.to_string())?;
        let temporary = path.with_extension("json.tmp");
        // Flush before atomically replacing the previous good checkpoint.
        use std::io::Write;
        let mut file = fs::File::create(&temporary).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(temporary, path).map_err(|e| e.to_string())
    }

    /// Fresh resolution on restart; saved signed URLs would already be expiring.
    /// Resume paused so startup never unexpectedly produces sound.
    pub async fn restore(&self, engine: &AudioEngine) -> Result<(), String> {
        self.validate()?;
        let (position, volume) = match &self.source {
            Source::Youtube { playback } => {
                engine.prepare_handoff(playback.clone()).await?;
                if playback.playlist_id.is_some() {
                    engine
                        .queue(QueueAction::Configure {
                            shuffle: self.shuffle,
                            repeat: self.repeat,
                        })
                        .await?;
                }
                (playback.position_seconds, playback.volume)
            }
            Source::Local {
                path,
                position_seconds,
                volume,
            } => {
                engine.load(path).await?;
                (*position_seconds, *volume)
            }
        };
        engine.set_volume(volume).await?;
        engine.seek(position).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_is_bounded_validated_atomic_and_contains_no_stream_secrets() {
        let folder = std::env::temp_dir().join(format!("media-checkpoint-{}", std::process::id()));
        let path = folder.join("state.json");
        let mut saved = SavedPlayback::browser(BrowserPlayback {
            url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into(),
            video_id: "aqz-KE-bpKQ".into(),
            playlist_id: None,
            playlist_index: None,
            position_seconds: 35.5,
            volume: 0.4,
            paused: false,
        });
        saved.save(&path).unwrap();
        saved.mode = PlaybackMode::RamSaver;
        saved.save(&path).unwrap();
        assert_eq!(
            SavedPlayback::load(&path).unwrap().unwrap().mode,
            PlaybackMode::RamSaver
        );
        let bytes = fs::read_to_string(&path).unwrap();
        assert!(!bytes.contains("googlevideo"));
        saved.version = 2;
        assert!(saved.save(&path).is_err());
        assert!(SavedPlayback::load(&path).unwrap().is_some());
        fs::write(&path, b"{}").unwrap();
        assert!(SavedPlayback::load(&path).is_err());
        fs::write(&path, vec![b' '; LIMIT as usize + 1]).unwrap();
        assert!(SavedPlayback::load(&path).is_err());
        fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn local_resume_restores_position_and_volume_paused_and_shutdown_joins() {
        tauri::async_runtime::block_on(async {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sfx.mp3");
            let saved = SavedPlayback {
                version: 1,
                mode: PlaybackMode::RamSaver,
                source: Source::Local {
                    path,
                    position_seconds: 0.1,
                    volume: 0.35,
                },
                shuffle: false,
                repeat: Repeat::Off,
            };
            let engine = AudioEngine::new().unwrap();
            saved.restore(&engine).await.unwrap();
            let state = engine.snapshot().await.unwrap();
            assert_eq!(state.status, super::super::PlaybackStatus::Paused);
            assert_eq!(state.position_seconds, 0.1);
            assert_eq!(state.volume, 0.35);
            assert!(SavedPlayback::audio(&state).unwrap().is_some());
            let clone = engine.clone();
            engine.shutdown().await.unwrap();
            assert!(clone.play().await.is_err());
            assert!(clone.shutdown().await.is_err());
        });
    }
}
