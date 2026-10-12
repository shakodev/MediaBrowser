//! Local-file and HTTP range playback. Blocking work and CPAL stream ownership stay on one worker.
pub(crate) mod commands;
mod decoder;
mod http;
mod output;
mod resample;
mod worker;

pub use http::TransferSnapshot;
use std::{
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub use crate::playback_state::PlaybackSnapshot;

pub(crate) enum Operation {
    Load(PathBuf),
    LoadYoutube(url::Url),
    Play,
    Pause,
    Stop,
    Seek(f64),
    Volume(f32),
    Snapshot,
}

pub(crate) struct Request {
    operation: Operation,
    cancel: CancellationToken,
    reply: oneshot::Sender<Result<PlaybackSnapshot, String>>,
}

/// Cloneable async facade; dropping the last handle shuts down the worker/device.
/// A full queue returns a busy error rather than blocking an async runtime.
#[derive(Clone)]
pub struct AudioEngine {
    sender: mpsc::SyncSender<Request>,
    control: Arc<Control>,
}

#[derive(Default)]
struct Control {
    cancel: Mutex<CancellationToken>,
}
impl Drop for Control {
    fn drop(&mut self) {
        if let Ok(token) = self.cancel.lock() {
            token.cancel();
        }
    }
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(16);
        std::thread::Builder::new()
            .name("audio-decoder".into())
            .spawn(move || worker::run(receiver))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            control: Arc::default(),
        })
    }

    async fn request(&self, operation: Operation) -> Result<PlaybackSnapshot, String> {
        let (reply, receive) = oneshot::channel();
        {
            let mut current = self
                .control
                .cancel
                .lock()
                .map_err(|_| "Audio control lock failed.")?;
            let disruptive = matches!(
                operation,
                Operation::Load(_)
                    | Operation::LoadYoutube(_)
                    | Operation::Pause
                    | Operation::Stop
                    | Operation::Seek(_)
            );
            let cancel = if disruptive {
                CancellationToken::new()
            } else {
                current.clone()
            };
            self.sender
                .try_send(Request {
                    operation,
                    reply,
                    cancel: cancel.clone(),
                })
                .map_err(|e| match e {
                    mpsc::TrySendError::Full(_) => {
                        "Audio engine is busy; retry the command.".to_owned()
                    }
                    mpsc::TrySendError::Disconnected(_) => "Audio worker has shut down.".to_owned(),
                })?;
            if disruptive {
                current.cancel();
                *current = cancel;
            }
        }
        receive
            .await
            .map_err(|_| "Audio worker has shut down.".to_owned())?
    }

    /// Validate and load a local file paused, without opening an output device.
    pub async fn load(&self, path: impl Into<PathBuf>) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Load(path.into())).await
    }
    /// Resolve and validate a public video; stream URLs remain on the Rust side.
    pub async fn load_youtube(&self, input: &str) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::LoadYoutube(
            crate::resolver::canonical_video_url(input)?,
        ))
        .await
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
        if !seconds.is_finite() || !(0.0..=1e9).contains(&seconds) {
            return Err("Seek position must be a finite, non-negative number of seconds.".into());
        }
        self.request(Operation::Seek(seconds)).await
    }
    pub async fn set_volume(&self, volume: f32) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Volume(volume)).await
    }
    pub async fn snapshot(&self) -> Result<PlaybackSnapshot, String> {
        self.request(Operation::Snapshot).await
    }
}
