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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{ResolveFuture, VideoMetadata};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Resolver {
        active: AtomicUsize,
        peak: AtomicUsize,
        pending: bool,
    }
    struct Guard<'a>(&'a AtomicUsize);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl StreamResolver for Resolver {
        fn resolve<'a>(&'a self, _: &'a Url) -> ResolveFuture<'a> {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                let _guard = Guard(&self.active);
                self.peak.fetch_max(active, Ordering::SeqCst);
                if self.pending {
                    std::future::pending::<()>().await;
                }
                Ok(ResolvedStream {
                    media_url: Url::parse("https://example.com/audio").unwrap(),
                    headers: Default::default(),
                    format_id: "140".into(),
                    extension: "m4a".into(),
                    metadata: VideoMetadata {
                        id: "BaW_jenozKc".into(),
                        title: "Prefetched".into(),
                        uploader: None,
                        duration_seconds: None,
                    },
                })
            })
        }
    }
    #[test]
    fn single_result_is_consumed_once_and_stale_results_are_discarded() {
        let resolver = Arc::new(Resolver {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            pending: false,
        });
        let token = CancellationToken::new();
        let url = Url::parse("https://youtu.be/BaW_jenozKc").unwrap();
        let mut prefetch = Prefetch::start(1, url.clone(), resolver.clone(), &token).unwrap();
        prefetch.thread.take().unwrap().join().unwrap();
        assert_eq!(prefetch.take_ready().unwrap().metadata.title, "Prefetched");
        assert!(prefetch.take_ready().is_none());
        let mut prefetch = Prefetch::start(2, url, resolver.clone(), &token).unwrap();
        prefetch.thread.take().unwrap().join().unwrap();
        prefetch.started = Instant::now() - Duration::from_secs(61);
        assert!(prefetch.take_ready().is_none());
        assert_eq!(resolver.peak.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn cancellation_reaps_pending_resolvers_before_replacement() {
        let resolver = Arc::new(Resolver {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            pending: true,
        });
        let token = CancellationToken::new();
        let url = Url::parse("https://youtu.be/BaW_jenozKc").unwrap();
        for _ in 0..10 {
            let prefetch = Prefetch::start(1, url.clone(), resolver.clone(), &token).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while resolver.active.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            drop(prefetch);
            assert_eq!(resolver.active.load(Ordering::SeqCst), 0);
        }
        assert_eq!(resolver.peak.load(Ordering::SeqCst), 1);
        let prefetch = Prefetch::start(1, url, resolver.clone(), &token).unwrap();
        token.cancel();
        drop(prefetch);
        assert_eq!(resolver.active.load(Ordering::SeqCst), 0);
    }
}
