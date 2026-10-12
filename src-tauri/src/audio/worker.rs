use super::{
    decoder::AudioDecoder,
    http::{HttpSource, TransferStats},
    output::{Backend, CpalBackend, Frame, OutputStream, Renderer, Span, Timeline},
    resample::Resampler,
    Operation, Request,
};
use crate::playback_state::{PlaybackSnapshot, PlaybackStatus as Status};
use crate::queue::{prefetch::Prefetch, Queue};
use crate::resolver::{ResolvedStream, StreamResolver, YtDlpResolver};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use url::Url;

struct Active {
    // Drop the stream first, before releasing its queues/decoder.
    _stream: Option<Box<dyn OutputStream>>,
    decoder: AudioDecoder,
    resampler: Resampler,
    pcm: Producer<Frame>,
    timing: Consumer<Span>,
    clock: Timeline,
    failed: Arc<AtomicBool>,
    consumed: Arc<AtomicU64>,
    produced: u64,
    pending: Vec<Frame>,
    cursor: usize,
    eof: bool,
}

impl Active {
    fn open(
        decoder: AudioDecoder,
        position: f64,
        volume: Arc<AtomicU32>,
        backend: &mut dyn Backend,
    ) -> Result<Self, String> {
        let spec = backend.spec()?;
        let capacity = (spec.rate as usize / 4).max(1); // 250 ms, independent of file length.
        let (pcm, consumer) = RingBuffer::new(capacity);
        let (timing, receive) = RingBuffer::new(256);
        let failed = Arc::new(AtomicBool::new(false));
        let consumed = Arc::new(AtomicU64::new(0));
        let renderer = Renderer {
            pcm: consumer,
            timing,
            volume,
            failed: failed.clone(),
            consumed: consumed.clone(),
            rate: spec.rate,
            channels: spec.channels,
        };
        let mut active = Self {
            _stream: None,
            decoder,
            resampler: Resampler::new(spec.rate),
            pcm,
            timing: receive,
            clock: Timeline::new(position),
            failed,
            consumed,
            produced: 0,
            pending: Vec::new(),
            cursor: 0,
            eof: false,
        };
        while active.pcm.slots() > 0 && !active.eof {
            active.fill()?;
        }
        active._stream = Some(backend.start(renderer)?);
        Ok(active)
    }

    fn fill(&mut self) -> Result<(), String> {
        if self.cursor == self.pending.len() && !self.eof && self.pcm.slots() > 0 {
            match self.decoder.next_block()? {
                Some(block) => {
                    self.pending = self.resampler.convert(block)?;
                    self.cursor = 0;
                }
                None => {
                    self.pending = self.resampler.finish();
                    self.cursor = 0;
                    self.eof = true;
                }
            }
        }
        while self.cursor < self.pending.len() && self.pcm.slots() > 0 {
            self.pcm
                .push(self.pending[self.cursor])
                .map_err(|_| "PCM buffer unexpectedly full.")?;
            self.cursor += 1;
            self.produced += 1;
        }
        Ok(())
    }
    fn tick(&mut self) -> Result<bool, String> {
        if self.failed.load(Ordering::Acquire) {
            return Err("Audio output failed or its timing queue overflowed. Check the output device and press Play to retry.".into());
        }
        // Consumption is published after the callback's timestamp span. This acquire
        // prevents marking EOF between the last PCM pop and publication of its span.
        let empty = self.consumed.load(Ordering::Acquire) == self.produced;
        self.clock.update(&mut self.timing, Instant::now())?;
        let ended = self.eof && self.cursor == self.pending.len() && empty && self.clock.drained();
        if !ended {
            for _ in 0..8 {
                if self.pcm.slots() == 0 || (self.eof && self.cursor == self.pending.len()) {
                    break;
                }
                self.fill()?;
            }
        }
        Ok(ended)
    }
}

struct YoutubeTrack {
    url: Url,
    stream: ResolvedStream,
    stats: Arc<TransferStats>,
}

struct Worker {
    queue: Option<Queue>,
    queue_running: bool,
    page_start: u64,
    prefetch: Option<Prefetch>,
    snapshot: PlaybackSnapshot,
    path: Option<PathBuf>,
    youtube: Option<YoutubeTrack>,
    resolver: Arc<dyn StreamResolver>,
    cancel: CancellationToken,
    volume: Arc<AtomicU32>,
    active: Option<Active>,
    backend: Box<dyn Backend>,
}

impl Worker {
    fn new(backend: Box<dyn Backend>) -> Self {
        Self {
            queue: None,
            queue_running: false,
            page_start: 0,
            prefetch: None,
            snapshot: PlaybackSnapshot::default(),
            path: None,
            youtube: None,
            resolver: Arc::new(YtDlpResolver::default()),
            cancel: CancellationToken::new(),
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            active: None,
            backend,
        }
    }
    fn update(&mut self) {
        if let Some(track) = &self.youtube {
            self.snapshot.transfer = Some(track.stats.snapshot());
        }
        if let Some(active) = &mut self.active {
            if active.decoder.is_cancelled() {
                let _ = active.clock.update(&mut active.timing, Instant::now());
                self.snapshot.position_seconds = active.clock.position;
                return;
            }
            let result = active.tick();
            if active.decoder.is_cancelled() {
                let _ = active.clock.update(&mut active.timing, Instant::now());
                self.snapshot.position_seconds = active.clock.position;
                return;
            }
            self.snapshot.position_seconds = active.clock.position;
            match result {
                Ok(true) => {
                    self.snapshot.status = Status::Ended;
                    // Raw ADTS and some MP3 files do not advertise duration.
                    self.snapshot.duration_seconds = Some(self.snapshot.position_seconds);
                    self.active = None;
                }
                Err(error) => self.fail(error),
                _ => {}
            }
        }
        if self.queue_running
            && !self.cancel.is_cancelled()
            && matches!(self.snapshot.status, Status::Ended | Status::Error)
        {
            let failed = self.snapshot.status == Status::Error;
            let next = if let Some(queue) = &mut self.queue {
                if failed {
                    queue.failed()
                } else {
                    queue.consecutive_failures = 0;
                    Ok(queue.order.next(true, false))
                }
            } else {
                Ok(None)
            };
            match next {
                Ok(Some(index)) => {
                    if let Err(error) = self.play_queue(index) {
                        self.queue_running = false;
                        self.fail(error);
                    }
                }
                Ok(None) => {
                    self.queue_running = false;
                    self.prefetch = None;
                }
                Err(error) => {
                    self.queue_running = false;
                    self.prefetch = None;
                    self.fail(error);
                }
            }
        }
    }
    fn fail(&mut self, error: String) {
        self.active = None;
        self.snapshot.status = Status::Error;
        self.snapshot.error = Some(error);
    }
    fn start(&mut self, position: f64) -> Result<(), String> {
        let mut decoder = self.open_decoder()?;
        if position > 0.0 {
            decoder.seek(position)?;
        }
        self.start_decoder(decoder, position)
    }
    fn open_decoder(&self) -> Result<AudioDecoder, String> {
        if let Some(track) = &self.youtube {
            let source = HttpSource::open(
                track.url.clone(),
                track.stream.clone(),
                self.resolver.clone(),
                self.cancel.clone(),
                track.stats.clone(),
            )?;
            AudioDecoder::from_source(
                Box::new(source),
                &track.stream.extension,
                Some(self.cancel.clone()),
            )
        } else {
            AudioDecoder::open(
                self.path
                    .as_ref()
                    .ok_or("Load an audio file or YouTube video first.")?,
            )
        }
    }
    fn start_decoder(&mut self, decoder: AudioDecoder, position: f64) -> Result<(), String> {
        let active = Active::open(
            decoder,
            position,
            self.volume.clone(),
            self.backend.as_mut(),
        )?;
        self.active = Some(active);
        self.snapshot.status = Status::Playing;
        self.snapshot.error = None;
        Ok(())
    }
    fn resolve(&self, url: &Url) -> Result<ResolvedStream, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Cannot start resolver runtime.")?;
        runtime.block_on(async {
            tokio::select! { biased; _ = self.cancel.cancelled() => Err("Audio request cancelled.".into()), result = self.resolver.resolve(url) => result }
        })
    }
    fn load_youtube(&mut self, url: Url, cached: Option<ResolvedStream>) -> Result<(), String> {
        let stream = match cached {
            Some(stream) => stream,
            None => self.resolve(&url)?,
        };
        let stats = Arc::new(TransferStats::default());
        let source = HttpSource::open(
            url.clone(),
            stream.clone(),
            self.resolver.clone(),
            self.cancel.clone(),
            stats.clone(),
        )?;
        let mut decoder = AudioDecoder::from_source(
            Box::new(source),
            &stream.extension,
            Some(self.cancel.clone()),
        )?;
        decoder.next_block()?.ok_or("Audio stream is empty.")?;
        self.active = None;
        self.path = None;
        self.snapshot.path = Some(url.to_string());
        self.snapshot.metadata = Some(stream.metadata.clone());
        self.snapshot.duration_seconds = decoder.duration.or(stream.metadata.duration_seconds);
        self.snapshot.transfer = Some(stats.snapshot());
        self.youtube = Some(YoutubeTrack { url, stream, stats });
        self.snapshot.position_seconds = 0.0;
        self.snapshot.status = Status::Paused;
        self.snapshot.error = None;
        Ok(())
    }
    fn command(&mut self, operation: Operation) -> Result<PlaybackSnapshot, String> {
        self.update();
        match operation {
            Operation::Load(path) => {
                // Validate before replacing a currently playing track.
                let mut decoder = AudioDecoder::open(&path)?;
                decoder.next_block()?.ok_or("Audio file is empty.")?;
                self.clear_queue();
                self.active = None;
                self.snapshot.path = Some(path.to_string_lossy().into_owned());
                self.path = Some(path);
                self.youtube = None;
                self.snapshot.metadata = None;
                self.snapshot.transfer = None;
                self.snapshot.position_seconds = 0.0;
                self.snapshot.duration_seconds = decoder.duration;
                self.snapshot.status = Status::Paused;
                self.snapshot.error = None;
            }
            Operation::LoadYoutube(url) => {
                self.clear_queue();
                self.load_youtube(url, None)?;
            }
            Operation::Queue(action) => self.queue_command(action)?,
            Operation::PrepareHandoff(state) => self.prepare_handoff(state)?,
            Operation::StartHandoff(state, observed) => {
                state.validate()?;
                if self.snapshot.metadata.as_ref().map(|m| &m.id) != Some(&state.video_id) {
                    return Err("Prepared audio does not match the YouTube video.".into());
                }
                self.active = None;
                self.queue_running = false;
                self.prefetch = None;
                self.volume.store(0.0f32.to_bits(), Ordering::Relaxed);
                self.snapshot.volume = 0.0;
                let mut decoder = self.open_decoder()?;
                // Account for decoder initialization while the browser keeps playing.
                let position = state.position_seconds
                    + if state.paused {
                        0.0
                    } else {
                        observed.elapsed().as_secs_f64()
                    };
                if self
                    .snapshot
                    .duration_seconds
                    .is_some_and(|d| position >= d)
                {
                    return Err(
                        "The video ended while preparing RAM Saver. Retry on the next track."
                            .into(),
                    );
                }
                decoder.seek(position)?;
                self.start_decoder(decoder, position)?;
                self.snapshot.position_seconds = position;
            }
            Operation::CommitHandoff { volume, paused } => {
                if self.snapshot.status != Status::Playing
                    || !volume.is_finite()
                    || !(0.0..=1.0).contains(&volume)
                {
                    return Err("Audio stopped before the handoff could complete.".into());
                }
                if paused {
                    self.active = None;
                    self.snapshot.status = Status::Paused;
                }
                self.volume.store(volume.to_bits(), Ordering::Relaxed);
                self.snapshot.volume = volume;
                self.queue_running = !paused && self.queue.is_some();
                self.schedule_prefetch()?;
            }
            Operation::Play => {
                if self.path.is_none() && self.youtube.is_none() {
                    return Err("Load a local audio file first.".into());
                }
                if self.snapshot.status != Status::Playing {
                    let position = if self.snapshot.status == Status::Ended {
                        0.0
                    } else {
                        self.snapshot.position_seconds
                    };
                    if let Err(error) = self.start(position) {
                        self.fail(error.clone());
                        return Err(error);
                    }
                    self.snapshot.position_seconds = position;
                }
                self.queue_running = self.queue.is_some();
                self.schedule_prefetch()?;
            }
            Operation::Pause => {
                self.queue_running = false;
                self.prefetch = None;
                if self.snapshot.status == Status::Playing {
                    self.active = None;
                    self.snapshot.status = Status::Paused;
                }
            }
            Operation::Stop => {
                self.queue_running = false;
                self.prefetch = None;
                self.active = None;
                self.snapshot.position_seconds = 0.0;
                self.snapshot.status = Status::Stopped;
                self.snapshot.error = None;
            }
            Operation::Seek(seconds) => {
                self.prefetch = None;
                if !seconds.is_finite() || !(0.0..=1e9).contains(&seconds) {
                    return Err(
                        "Seek position must be a finite, non-negative number of seconds.".into(),
                    );
                }

                let position = self
                    .snapshot
                    .duration_seconds
                    .map_or(seconds, |d| seconds.min(d));
                // Failed seeks preserve the previous transport. Reopen/reset/discard
                // all PCM on successful seeks; stale samples can never reach a new stream.
                let mut decoder = self.open_decoder()?;
                decoder.seek(position)?;
                let playing = self.snapshot.status == Status::Playing;
                self.active = None;
                self.snapshot.position_seconds = position;
                self.snapshot.error = None;
                if self
                    .snapshot
                    .duration_seconds
                    .is_some_and(|d| position >= d)
                {
                    self.snapshot.status = Status::Ended;
                } else if playing {
                    if let Err(error) = self.start_decoder(decoder, position) {
                        self.fail(error.clone());
                        return Err(error);
                    }
                } else {
                    self.snapshot.status = Status::Paused;
                }
            }
            Operation::Volume(volume) => {
                if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
                    return Err("Volume must be between 0 and 1.".into());
                }
                self.volume.store(volume.to_bits(), Ordering::Relaxed);
                self.snapshot.volume = volume;
            }
            Operation::Snapshot => {}
        }
        self.refresh_queue_snapshot()?;
        Ok(self.snapshot.clone())
    }
}

pub fn run(receiver: mpsc::Receiver<Request>) {
    let mut worker = Worker::new(Box::<CpalBackend>::default());
    loop {
        let request = if worker.active.is_some() || worker.queue_running {
            match receiver.recv_timeout(Duration::from_millis(4)) {
                Ok(request) => Some(request),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(request) => Some(request),
                Err(_) => break,
            }
        };
        if let Some(request) = request {
            worker.update();
            if request.cancel.is_cancelled() {
                let _ = request.reply.send(Err(
                    "Audio request superseded by a newer transport command.".into(),
                ));
                continue;
            }
            worker.cancel = request.cancel;
            // Volume and snapshots retain the active token. Transport commands use a fresh one.

            let result = worker.command(request.operation);
            if result.is_err()
                && worker
                    .active
                    .as_ref()
                    .is_some_and(|a| a.decoder.is_cancelled())
            {
                worker
                    .fail("Playback interrupted; press Play to resume the previous track.".into());
            }
            let _ = request.reply.send(result);
        } else {
            worker.update();
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[path = "queue.rs"]
mod queue_playback;
