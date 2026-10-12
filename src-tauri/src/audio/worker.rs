use super::{
    decoder::LocalDecoder,
    output::{Backend, CpalBackend, Frame, OutputStream, Renderer, Span, Timeline},
    resample::Resampler,
    Operation, Request,
};
use crate::playback_state::{PlaybackSnapshot, PlaybackStatus as Status};
use rtrb::{Consumer, Producer, RingBuffer};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

struct Active {
    // Drop the stream first, before releasing its queues/decoder.
    _stream: Option<Box<dyn OutputStream>>,
    decoder: LocalDecoder,
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
        decoder: LocalDecoder,
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

struct Worker {
    snapshot: PlaybackSnapshot,
    path: Option<PathBuf>,
    volume: Arc<AtomicU32>,
    active: Option<Active>,
    backend: Box<dyn Backend>,
}

impl Worker {
    fn new(backend: Box<dyn Backend>) -> Self {
        Self {
            snapshot: PlaybackSnapshot::default(),
            path: None,
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            active: None,
            backend,
        }
    }
    fn update(&mut self) {
        if let Some(active) = &mut self.active {
            let result = active.tick();
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
    }
    fn fail(&mut self, error: String) {
        self.active = None;
        self.snapshot.status = Status::Error;
        self.snapshot.error = Some(error);
    }
    fn start(&mut self, position: f64) -> Result<(), String> {
        let path = self.path.as_ref().ok_or("Load a local audio file first.")?;
        let mut decoder = LocalDecoder::open(path)?;
        if position > 0.0 {
            decoder.seek(position)?;
        }
        self.start_decoder(decoder, position)
    }
    fn start_decoder(&mut self, decoder: LocalDecoder, position: f64) -> Result<(), String> {
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
    fn command(&mut self, operation: Operation) -> Result<PlaybackSnapshot, String> {
        self.update();
        match operation {
            Operation::Load(path) => {
                // Validate before replacing a currently playing track.
                let mut decoder = LocalDecoder::open(&path)?;
                decoder.next_block()?.ok_or("Audio file is empty.")?;
                self.active = None;
                self.snapshot.path = Some(path.to_string_lossy().into_owned());
                self.path = Some(path);
                self.snapshot.position_seconds = 0.0;
                self.snapshot.duration_seconds = decoder.duration;
                self.snapshot.status = Status::Paused;
                self.snapshot.error = None;
            }
            Operation::Play => {
                if self.path.is_none() {
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
            }
            Operation::Pause => {
                if self.snapshot.status == Status::Playing {
                    self.active = None;
                    self.snapshot.status = Status::Paused;
                }
            }
            Operation::Stop => {
                self.active = None;
                self.snapshot.position_seconds = 0.0;
                self.snapshot.status = Status::Stopped;
                self.snapshot.error = None;
            }
            Operation::Seek(seconds) => {
                if !seconds.is_finite() || !(0.0..=1e9).contains(&seconds) {
                    return Err(
                        "Seek position must be a finite, non-negative number of seconds.".into(),
                    );
                }
                let path = self.path.as_ref().ok_or("Load a local audio file first.")?;
                let position = self
                    .snapshot
                    .duration_seconds
                    .map_or(seconds, |d| seconds.min(d));
                // Failed seeks preserve the previous transport. Reopen/reset/discard
                // all PCM on successful seeks; stale samples can never reach a new stream.
                let mut decoder = LocalDecoder::open(path)?;
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
        Ok(self.snapshot.clone())
    }
}

pub fn run(receiver: mpsc::Receiver<Request>) {
    let mut worker = Worker::new(Box::<CpalBackend>::default());
    loop {
        let request = if worker.active.is_some() {
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
            let _ = request.reply.send(worker.command(request.operation));
        } else {
            worker.update();
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
