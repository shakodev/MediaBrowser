use super::super::output::OutputSpec;
use super::*;
use crate::queue::QueueAction;
use std::{cell::RefCell, path::Path, rc::Rc};

fn fixture(extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/sfx.{extension}"))
}

struct FakeBackend(Rc<RefCell<Option<Renderer>>>);
struct FakeStream(Rc<RefCell<Option<Renderer>>>);
impl OutputStream for FakeStream {}
impl Drop for FakeStream {
    fn drop(&mut self) {
        self.0.borrow_mut().take();
    }
}
impl Backend for FakeBackend {
    fn spec(&mut self) -> Result<OutputSpec, String> {
        Ok(OutputSpec {
            rate: 48000,
            channels: 2,
        })
    }
    fn start(&mut self, renderer: Renderer) -> Result<Box<dyn OutputStream>, String> {
        *self.0.borrow_mut() = Some(renderer);
        Ok(Box::new(FakeStream(self.0.clone())))
    }
}
fn worker() -> (Worker, Rc<RefCell<Option<Renderer>>>) {
    let renderer = Rc::new(RefCell::new(None));
    (
        Worker::new(Box::new(FakeBackend(renderer.clone()))),
        renderer,
    )
}
fn pump(worker: &mut Worker, renderer: &Rc<RefCell<Option<Renderer>>>) -> Vec<f32> {
    let mut samples = vec![0.0f32; 960 * 2];
    renderer
        .borrow_mut()
        .as_mut()
        .unwrap()
        .render(&mut samples, Instant::now() - Duration::from_millis(21));
    worker.update();
    samples
}

#[test]
fn decodes_all_local_formats_and_seeks_forward_and_backward() {
    for extension in ["mp3", "aac", "m4a"] {
        let path = fixture(extension);
        let mut baseline = AudioDecoder::open(&path).unwrap();
        let mut all = Vec::new();
        let mut first_time = None;
        let mut rate = 0;
        let mut channels = 0;
        while let Some(block) = baseline.next_block().unwrap() {
            first_time.get_or_insert(block.start_seconds);
            rate = block.rate;
            channels = block.channels;
            all.extend(block.samples);
        }
        let duration = all.len() as f64 / channels as f64 / rate as f64;
        assert!(duration > 0.08, "{extension}: {duration}");
        assert!(all.iter().any(|x: &f32| x.abs() > 0.001));
        let mut decoder = AudioDecoder::open(&path).unwrap();
        for target in [duration * 0.7, duration * 0.15, duration * 0.4, 0.0] {
            decoder.seek(target).unwrap();
            let block = decoder.next_block().unwrap().unwrap();
            assert!(
                block.start_seconds + 1e-8 >= target,
                "{extension}: {} < {target}",
                block.start_seconds
            );
            assert!(block.start_seconds - target <= 1.0 / rate as f64 + 1e-8);
            let offset = ((block.start_seconds - first_time.unwrap()) * rate as f64).round()
                as usize
                * channels;
            for (a, b) in block.samples.iter().zip(&all[offset..]).take(128) {
                assert!(
                    (a - b).abs() < 0.003,
                    "{extension}: seek sample differs: {a} vs {b}"
                );
            }
        }
    }
}

#[test]
fn transport_pause_seek_stop_and_replay_discard_old_pcm() {
    for extension in ["mp3", "aac", "m4a"] {
        let (mut worker, renderer) = worker();
        assert!(worker.command(Operation::Play).is_err());
        assert_eq!(worker.snapshot.status, Status::Stopped);
        assert_eq!(
            worker
                .command(Operation::Load(fixture(extension)))
                .unwrap()
                .status,
            Status::Paused
        );
        assert!(renderer.borrow().is_none());
        worker.command(Operation::Volume(0.4)).unwrap();
        assert_eq!(
            worker.command(Operation::Play).unwrap().status,
            Status::Playing
        );
        assert!(pump(&mut worker, &renderer)
            .iter()
            .any(|x| x.abs() > 0.0001));
        let paused = worker.command(Operation::Pause).unwrap();
        assert_eq!(paused.status, Status::Paused);
        assert!(paused.position_seconds > 0.0);
        assert!(renderer.borrow().is_none());
        assert_eq!(
            worker
                .command(Operation::Snapshot)
                .unwrap()
                .position_seconds,
            paused.position_seconds
        );
        assert_eq!(
            worker
                .command(Operation::Seek(0.04))
                .unwrap()
                .position_seconds,
            0.04
        );
        assert!(renderer.borrow().is_none());
        worker.command(Operation::Play).unwrap();
        let expected_first = renderer
            .borrow()
            .as_ref()
            .unwrap()
            .pcm
            .peek()
            .unwrap()
            .end_seconds;
        assert!((expected_first - 0.04).abs() < 0.0001);
        pump(&mut worker, &renderer);
        let seek = worker.command(Operation::Seek(0.01)).unwrap();
        assert_eq!(seek.status, Status::Playing);
        let first_after_seek = renderer
            .borrow()
            .as_ref()
            .unwrap()
            .pcm
            .peek()
            .unwrap()
            .end_seconds;
        assert!((first_after_seek - 0.01).abs() < 0.0001);
        assert_eq!(seek.volume, 0.4);
        let stopped = worker.command(Operation::Stop).unwrap();
        assert_eq!(stopped.status, Status::Stopped);
        assert_eq!(stopped.position_seconds, 0.0);
        assert!(renderer.borrow().is_none());
        worker.command(Operation::Play).unwrap();
        assert!(
            renderer
                .borrow()
                .as_ref()
                .unwrap()
                .pcm
                .peek()
                .unwrap()
                .end_seconds
                < 0.001
        );
    }
}

#[test]
fn eof_waits_for_device_playback_and_replay_starts_at_zero() {
    let (mut worker, renderer) = worker();
    worker.command(Operation::Load(fixture("mp3"))).unwrap();
    worker.command(Operation::Play).unwrap();
    let mut output = vec![0.0f32; 48000];
    renderer
        .borrow_mut()
        .as_mut()
        .unwrap()
        .render(&mut output, Instant::now() + Duration::from_millis(100));
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Playing);
    assert_eq!(worker.snapshot.position_seconds, 0.0);
    // Advance the device timeline deterministically, without sleeping.
    let active = worker.active.as_mut().unwrap();
    active
        .clock
        .update(&mut active.timing, Instant::now() + Duration::from_secs(2))
        .unwrap();
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Ended);
    assert!(worker.snapshot.position_seconds > 0.08);
    assert!(renderer.borrow().is_none());
    assert_eq!(
        worker.command(Operation::Play).unwrap().position_seconds,
        0.0
    );
}

#[test]
fn invalid_commands_preserve_transport_and_output_failure_is_recoverable() {
    let (mut worker, renderer) = worker();
    worker.command(Operation::Load(fixture("m4a"))).unwrap();
    worker.command(Operation::Play).unwrap();
    for invalid in [
        Operation::Seek(-1.0),
        Operation::Seek(f64::NAN),
        Operation::Volume(1.1),
        Operation::Volume(f32::INFINITY),
        Operation::Load(fixture("missing")),
    ] {
        assert!(worker.command(invalid).is_err());
        assert_eq!(worker.snapshot.status, Status::Playing);
    }
    renderer
        .borrow()
        .as_ref()
        .unwrap()
        .failed
        .store(true, Ordering::Release);
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Error);
    assert!(worker.snapshot.error.is_some());
    assert!(renderer.borrow().is_none());
    assert_eq!(
        worker.command(Operation::Play).unwrap().status,
        Status::Playing
    );
    let end = worker.snapshot.duration_seconds.unwrap();
    let seek = worker.command(Operation::Seek(end + 50.0)).unwrap();
    assert_eq!(seek.status, Status::Ended);
    assert_eq!(seek.position_seconds, end);
}

#[test]
fn asynchronous_facade_loads_without_a_device_and_bounds_its_command_queue() {
    tauri::async_runtime::block_on(async {
        let engine = super::super::AudioEngine::new().unwrap();
        assert_eq!(
            engine.load(fixture("mp3")).await.unwrap().status,
            Status::Paused
        );
        let clone = engine.clone();
        assert_eq!(clone.seek(0.02).await.unwrap().position_seconds, 0.02);
        assert_eq!(engine.stop().await.unwrap().position_seconds, 0.0);
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (reply, _) = tokio::sync::oneshot::channel();
        sender
            .try_send(Request {
                operation: Operation::Snapshot,
                cancel: CancellationToken::new(),
                reply,
            })
            .unwrap();
        let busy = super::super::AudioEngine {
            sender,
            control: Arc::default(),
        };
        assert!(busy.snapshot().await.unwrap_err().contains("busy"));
    });
}

#[test]
fn youtube_transport_uses_remote_source_and_preserves_seek_and_pause() {
    use crate::resolver::{ResolveFuture, ResolvedStream, VideoMetadata};
    struct Resolver(url::Url);
    impl StreamResolver for Resolver {
        fn resolve<'a>(&'a self, _: &'a url::Url) -> ResolveFuture<'a> {
            Box::pin(async move {
                Ok(ResolvedStream {
                    media_url: self.0.clone(),
                    headers: Default::default(),
                    format_id: "140".into(),
                    extension: "m4a".into(),
                    metadata: VideoMetadata {
                        id: "BaW_jenozKc".into(),
                        title: "remote test".into(),
                        uploader: None,
                        duration_seconds: Some(0.3),
                    },
                })
            })
        }
    }
    let server =
        super::super::http::tests::Server::new(std::fs::read(fixture("m4a")).unwrap(), "ok");
    let (mut worker, renderer) = worker();
    worker.resolver = Arc::new(Resolver(server.url.clone()));
    let loaded = worker
        .command(Operation::LoadYoutube(
            url::Url::parse("https://youtu.be/BaW_jenozKc").unwrap(),
        ))
        .unwrap();
    assert_eq!(loaded.metadata.unwrap().title, "remote test");
    assert!(loaded.path.unwrap().starts_with("https://youtu.be/"));
    worker.command(Operation::Play).unwrap();
    assert!(pump(&mut worker, &renderer).iter().any(|v| v.abs() > 0.001));
    // Model a transport command cancelling an in-flight source before execution.
    worker.cancel.cancel();
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Playing);
    worker.cancel = CancellationToken::new();
    worker.command(Operation::Seek(0.04)).unwrap();
    assert_eq!(worker.snapshot.status, Status::Playing);
    assert!(pump(&mut worker, &renderer).iter().any(|v| v.abs() > 0.001));
    worker.command(Operation::Pause).unwrap();
    let paused = worker.snapshot.position_seconds;
    worker.update();
    assert_eq!(worker.snapshot.position_seconds, paused);
    worker.command(Operation::Play).unwrap();
    worker.command(Operation::Stop).unwrap();
    assert_eq!(worker.snapshot.position_seconds, 0.0);
    // Handoff preparation must not start output or lose the captured position.
    let target = crate::playback_state::BrowserPlayback {
        url: "https://www.youtube.com/watch?v=BaW_jenozKc".into(),
        video_id: "BaW_jenozKc".into(),
        playlist_id: None,
        playlist_index: None,
        position_seconds: 0.04,
        volume: 0.37,
        paused: true,
    };
    worker
        .command(Operation::PrepareHandoff(target.clone()))
        .unwrap();
    assert!(renderer.borrow().is_none());
    worker
        .command(Operation::StartHandoff(target.clone(), Instant::now()))
        .unwrap();
    assert_eq!(worker.snapshot.position_seconds, 0.04);
    assert!(!worker.queue_running);
    assert!(pump(&mut worker, &renderer)
        .iter()
        .all(|sample| *sample == 0.0));
    assert!(worker.snapshot.position_seconds > 0.04);
    worker
        .command(Operation::CommitHandoff {
            volume: 0.37,
            paused: true,
        })
        .unwrap();
    assert_eq!(worker.snapshot.status, Status::Paused);
    assert_eq!(worker.snapshot.volume, 0.37);
    assert!(renderer.borrow().is_none());
    // A mismatched prepared track is rejected before starting any output.
    let wrong = crate::playback_state::BrowserPlayback {
        url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into(),
        video_id: "aqz-KE-bpKQ".into(),
        ..target
    };
    assert!(worker
        .command(Operation::StartHandoff(wrong, Instant::now()))
        .is_err());
    assert!(renderer.borrow().is_none());
}

#[test]
fn stop_cancels_accepted_work_and_queue_backpressure_does_not_cancel_it() {
    tauri::async_runtime::block_on(async {
        let (sender, receiver) = mpsc::sync_channel::<Request>(1);
        let engine = super::super::AudioEngine {
            sender,
            control: Arc::default(),
        };
        let (accepted, waiting) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let first = receiver.recv().unwrap();
            accepted.send(()).unwrap();
            let stop = receiver.recv().unwrap();
            // Dispatch reserves the queue slot before publishing cancellation, so
            // wait for the submitting thread to finish that synchronous section.
            let deadline = Instant::now() + Duration::from_secs(1);
            while !first.cancel.is_cancelled() && Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert!(first.cancel.is_cancelled());
            assert!(!stop.cancel.is_cancelled());
            first
                .reply
                .send(Err("Audio request cancelled.".into()))
                .unwrap();
            stop.reply.send(Ok(PlaybackSnapshot::default())).unwrap();
        });
        let (load, stop) =
            tokio::join!(engine.load_youtube("https://youtu.be/BaW_jenozKc"), async {
                waiting.await.unwrap();
                engine.stop().await
            });
        assert!(load.unwrap_err().contains("cancelled"));
        assert_eq!(stop.unwrap().status, Status::Stopped);
        thread.join().unwrap();

        let (sender, receiver) = mpsc::sync_channel::<Request>(1);
        let engine = super::super::AudioEngine {
            sender: sender.clone(),
            control: Arc::default(),
        };
        let token = engine.control.cancel.lock().unwrap().clone();
        let (reply, _) = tokio::sync::oneshot::channel();
        sender
            .try_send(Request {
                operation: Operation::Snapshot,
                reply,
                cancel: token.clone(),
            })
            .unwrap();
        assert!(engine.stop().await.unwrap_err().contains("busy"));
        assert!(!token.is_cancelled());
        drop(receiver);
    });
}

#[test]
fn shutdown_cancels_inflight_work_even_with_a_full_command_queue() {
    tauri::async_runtime::block_on(async {
        let (sender, receiver) = mpsc::sync_channel::<Request>(1);
        let engine = super::super::AudioEngine {
            sender: sender.clone(),
            control: Arc::default(),
        };
        let (accepted, waiting) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            let request = receiver.recv().unwrap();
            accepted.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !request.cancel.is_cancelled() {
                assert!(
                    Instant::now() < deadline,
                    "shutdown did not cancel active IO"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = request.reply.send(Err("cancelled".into()));
            PlaybackSnapshot::default()
        });
        *engine.control.worker.lock().unwrap() = Some(worker);
        let (load, shutdown) =
            tokio::join!(engine.load_youtube("https://youtu.be/BaW_jenozKc"), async {
                waiting.await.unwrap();
                let (reply, _) = tokio::sync::oneshot::channel();
                sender
                    .try_send(Request {
                        operation: Operation::Snapshot,
                        cancel: CancellationToken::new(),
                        reply,
                    })
                    .unwrap();
                engine.shutdown().await
            });
        assert!(load.is_err());
        shutdown.unwrap();
        assert!(engine.snapshot().await.is_err());
    });
}

struct QueueResolver {
    url: Url,
    fail_mask: AtomicU64,
    calls: std::sync::Mutex<Vec<u64>>,
}
impl StreamResolver for QueueResolver {
    fn resolve<'a>(&'a self, url: &'a Url) -> crate::resolver::ResolveFuture<'a> {
        Box::pin(async move {
            let id = url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .unwrap()
                .1
                .into_owned();
            let index = id.parse::<u64>().unwrap();
            self.calls.lock().unwrap().push(index);
            if self.fail_mask.load(Ordering::Relaxed) & (1 << index) != 0 {
                return Err("Unavailable test video".into());
            }
            Ok(ResolvedStream {
                media_url: self.url.clone(),
                headers: Default::default(),
                format_id: "140".into(),
                extension: "m4a".into(),
                metadata: crate::resolver::VideoMetadata {
                    id,
                    title: format!("Resolved {index}"),
                    uploader: Some("Test artist".into()),
                    duration_seconds: Some(0.3),
                },
            })
        })
    }
}
fn test_queue(len: u64, unavailable: Option<u64>) -> Queue {
    let mut list = crate::queue::Playlist::new().unwrap();
    for index in 0..len {
        list.push(&crate::queue::Track {
            playlist_index: Some(index + 1),
            metadata: crate::resolver::VideoMetadata {
                id: format!("{index:011}"),
                title: format!("Flat {index}"),
                uploader: None,
                duration_seconds: None,
            },
            unavailable: unavailable == Some(index),
            failed: false,
        })
        .unwrap();
    }
    Queue::new(list)
}

#[test]
fn playlist_skips_resolution_and_runtime_failures_then_recovers_on_manual_retry() {
    let server =
        super::super::http::tests::Server::new(std::fs::read(fixture("m4a")).unwrap(), "ok");
    let resolver = Arc::new(QueueResolver {
        url: server.url.clone(),
        fail_mask: AtomicU64::new(0b10),
        calls: Default::default(),
    });
    let (mut worker, renderer) = worker();
    worker.resolver = resolver.clone();
    worker.queue = Some(test_queue(4, Some(0)));
    // Known deleted entries must never reach the audio resolver.
    worker.play_queue(0).unwrap();
    assert_eq!(worker.queue.as_ref().unwrap().order.current(), Some(2));
    assert_eq!(worker.queue.as_ref().unwrap().skipped, 2);
    assert_eq!(
        worker.snapshot.metadata.as_ref().unwrap().title,
        "Resolved 2"
    );
    assert!(pump(&mut worker, &renderer).iter().any(|v| v.abs() > 0.001));
    assert!(!resolver.calls.lock().unwrap().contains(&0));
    // A mid-stream decoder/device failure advances automatically without a UI command.
    worker.prefetch = None;
    worker.fail("Injected decoder failure".into());
    worker.update();
    assert_eq!(worker.queue.as_ref().unwrap().order.current(), Some(3));
    assert_eq!(worker.snapshot.status, Status::Playing);
    assert_eq!(worker.queue.as_ref().unwrap().skipped, 3);
    resolver.fail_mask.store(0, Ordering::Relaxed);
    let selected = worker
        .command(Operation::Queue(QueueAction::Select { index: 1 }))
        .unwrap();
    assert_eq!(selected.queue.unwrap().current, Some(1));
    assert_eq!(selected.metadata.unwrap().title, "Resolved 1");
    assert!(
        !worker
            .queue
            .as_mut()
            .unwrap()
            .playlist
            .track(1)
            .unwrap()
            .failed
    );
    worker.command(Operation::Pause).unwrap();
    assert!(!worker.queue_running);
    assert!(worker.prefetch.is_none());
    worker.command(Operation::Play).unwrap();
    assert!(worker.queue_running);
    worker.command(Operation::Stop).unwrap();
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Stopped);
    assert!(worker.prefetch.is_none());
}

#[test]
fn playlist_eof_advances_and_repeat_one_replays_then_stops_at_end() {
    let server =
        super::super::http::tests::Server::new(std::fs::read(fixture("m4a")).unwrap(), "ok");
    let (mut worker, _renderer) = worker();
    worker.resolver = Arc::new(QueueResolver {
        url: server.url.clone(),
        fail_mask: AtomicU64::new(0),
        calls: Default::default(),
    });
    worker.queue = Some(test_queue(3, None));
    worker.play_queue(0).unwrap();
    // Model the drained EOF state; device drain timing already has its own tests.
    worker.active = None;
    worker.snapshot.status = Status::Ended;
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Playing);
    assert_eq!(worker.queue.as_ref().unwrap().order.current(), Some(1));
    worker
        .queue_command(QueueAction::Configure {
            shuffle: false,
            repeat: crate::queue::Repeat::One,
        })
        .unwrap();
    worker.active = None;
    worker.snapshot.status = Status::Ended;
    worker.update();
    assert_eq!(worker.queue.as_ref().unwrap().order.current(), Some(1));
    assert_eq!(worker.snapshot.status, Status::Playing);
    worker
        .queue_command(QueueAction::Configure {
            shuffle: false,
            repeat: crate::queue::Repeat::Off,
        })
        .unwrap();
    worker
        .queue_command(QueueAction::Select { index: 2 })
        .unwrap();
    worker.active = None;
    worker.snapshot.status = Status::Ended;
    worker.update();
    assert_eq!(worker.snapshot.status, Status::Ended);
    assert!(!worker.queue_running);
    assert!(worker.prefetch.is_none());
}

#[test]
fn all_unavailable_playlist_terminates_and_cancellation_does_not_skip_tracks() {
    let (mut worker, _) = worker();
    let resolver = Arc::new(QueueResolver {
        url: Url::parse("http://127.0.0.1:1/audio").unwrap(),
        fail_mask: AtomicU64::new(u64::MAX),
        calls: Default::default(),
    });
    worker.resolver = resolver.clone();
    worker.queue = Some(test_queue(3, None));
    worker
        .queue
        .as_mut()
        .unwrap()
        .order
        .configure(false, crate::queue::Repeat::All, 1);
    worker.play_queue(0).unwrap();
    assert_eq!(worker.snapshot.status, Status::Error);
    assert_eq!(worker.queue.as_ref().unwrap().skipped, 3);
    assert_eq!(*resolver.calls.lock().unwrap(), vec![0, 1, 2]);
    worker.update();
    assert_eq!(resolver.calls.lock().unwrap().len(), 3);
    worker.cancel.cancel();
    assert!(worker.play_queue(0).unwrap_err().contains("cancelled"));
    assert_eq!(worker.queue.as_ref().unwrap().skipped, 3);
}

#[test]
fn previous_skips_unavailable_entries_backwards_and_restarts_at_the_beginning() {
    let server =
        super::super::http::tests::Server::new(std::fs::read(fixture("m4a")).unwrap(), "ok");
    let (mut worker, _) = worker();
    worker.resolver = Arc::new(QueueResolver {
        url: server.url.clone(),
        fail_mask: AtomicU64::new(0b10),
        calls: Default::default(),
    });
    worker.queue = Some(test_queue(3, None));
    worker.play_queue(2).unwrap();
    let previous = worker
        .command(Operation::Queue(QueueAction::Previous))
        .unwrap();
    assert_eq!(previous.queue.unwrap().current, Some(0));
    assert_eq!(previous.status, Status::Playing);
    let previous = worker
        .command(Operation::Queue(QueueAction::Previous))
        .unwrap();
    assert_eq!(previous.queue.unwrap().current, Some(0));
    assert_eq!(previous.status, Status::Playing);
}
