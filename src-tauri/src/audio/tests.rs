use super::super::output::OutputSpec;
use super::*;
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
        let mut baseline = LocalDecoder::open(&path).unwrap();
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
        let mut decoder = LocalDecoder::open(&path).unwrap();
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
                reply,
            })
            .unwrap();
        let busy = super::super::AudioEngine { sender };
        assert!(busy.snapshot().await.unwrap_err().contains("busy"));
    });
}
