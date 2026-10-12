//! One cancellable resolver only. No audio bytes or decoder are prefetched.
use crate::resolver::{ResolvedStream, StreamResolver};
use std::{
    sync::{mpsc, Arc},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub struct Prefetch {
    pub index: u64,
    started: Instant,
    cancel: CancellationToken,
    receive: mpsc::Receiver<Result<ResolvedStream, String>>,
    thread: Option<JoinHandle<()>>,
}
impl Prefetch {
    pub fn start(
        index: u64,
        url: Url,
        resolver: Arc<dyn StreamResolver>,
        parent: &CancellationToken,
    ) -> Option<Self> {
        let cancel = parent.child_token();
        let task_cancel = cancel.clone();
        let (send, receive) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new().name("next-track-resolver".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread().enable_all().build()
                .map_err(|_| "Cannot start prefetch runtime.".to_owned()).and_then(|runtime| runtime.block_on(async {
                    tokio::select! { biased; _ = task_cancel.cancelled() => Err("Prefetch cancelled.".into()), result = resolver.resolve(&url) => result }
                }));
            let _ = send.try_send(result);
        }).ok()?;
        Some(Self {
            index,
            started: Instant::now(),
            cancel,
            receive,
            thread: Some(thread),
        })
    }
    pub fn take_ready(&self) -> Option<ResolvedStream> {
        if self.cancel.is_cancelled() || self.started.elapsed() > Duration::from_secs(60) {
            return None;
        }
        // A speculative failure is retried on demand; it never skips a playable track.
        self.receive.try_recv().ok().and_then(Result::ok)
    }
}
impl Drop for Prefetch {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Reap before another task is started: never accumulate resolver threads/processes.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
