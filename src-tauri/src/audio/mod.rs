//! Local-file playback. Blocking work and CPAL stream ownership stay on one worker.
pub(crate) mod commands;
mod decoder;
mod output;
mod resample;
mod worker;

use std::{path::PathBuf, sync::mpsc};
use tokio::sync::oneshot;

pub use crate::playback_state::PlaybackSnapshot;

pub(crate) enum Operation {
    Load(PathBuf),
    Play,
    Pause,
    Stop,
    Seek(f64),
    Volume(f32),
    Snapshot,
}

pub(crate) struct Request {
    operation: Operation,
    reply: oneshot::Sender<Result<PlaybackSnapshot, String>>,
}

/// Cloneable async facade; dropping the last handle shuts down the worker/device.
/// A full queue returns a busy error rather than blocking an async runtime.
#[derive(Clone)]
pub struct AudioEngine {
    sender: mpsc::SyncSender<Request>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(16);
        std::thread::Builder::new()
            .name("audio-decoder".into())
            .spawn(move || worker::run(receiver))
            .map_err(|e| e.to_string())?;
        Ok(Self { sender })
    }

    async fn request(&self, operation: Operation) -> Result<PlaybackSnapshot, String> {
        let (reply, receive) = oneshot::channel();
        self.sender
            .try_send(Request { operation, reply })
            .map_err(|e| match e {
                mpsc::TrySendError::Full(_) => {
                    "Audio engine is busy; retry the command.".to_owned()
                }
                mpsc::TrySendError::Disconnected(_) => "Audio worker has shut down.".to_owned(),
            })?;
        receive
            .await
            .map_err(|_| "Audio worker has shut down.".to_owned())?
    }

    /// Validate and load a local file paused, without opening an output device.
    pub async fn load(&self, path: impl Into<PathBuf>) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Load(path.into())).await
    }
    pub async fn play(&self) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Play).await
    }
    pub async fn pause(&self) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Pause).await
    }
    pub async fn stop(&self) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Stop).await
    }
    pub async fn seek(&self, seconds: f64) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Seek(seconds)).await
    }
    pub async fn set_volume(&self, volume: f32) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Volume(volume)).await
    }
    pub async fn snapshot(&self) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Snapshot).await
    }
}
