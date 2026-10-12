//! Playlist transport integration on the existing audio worker.
use super::*;
use crate::queue::{QueueAction, QueuePage};

impl Worker {
    pub(super) fn prepare_handoff(
        &mut self,
        state: crate::playback_state::BrowserPlayback,
    ) -> Result<(), String> {
        state.validate()?;
        self.active = None;
        self.clear_queue();
        if state.playlist_id.is_some() {
            let url = crate::resolver::playlist::canonical_playlist_url(&state.url)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "Cannot start playlist runtime.")?;
            let resolver = YtDlpResolver::default();
            let mut playlist = runtime.block_on(async {
                tokio::select! { biased; _ = self.cancel.cancelled() => Err("Handoff cancelled.".into()), result = resolver.playlist(&url) => result }
            })?;
            let index = playlist.locate(&state.video_id, state.playlist_index)?;
            if playlist.track(index)?.unavailable {
                return Err("The current playlist track is unavailable.".into());
            }
            let mut queue = Queue::new(playlist);
            queue.source_url = Some(url.to_string());
            queue.order.select(index)?;
            self.page_start =
                index / crate::queue::PAGE_SIZE as u64 * crate::queue::PAGE_SIZE as u64;
            self.queue = Some(queue);
        }
        // Do not use play_queue: the initial handoff must never skip the requested track.
        self.load_youtube(crate::resolver::canonical_video_url(&state.url)?, None)?;
        Ok(())
    }
    pub(super) fn clear_queue(&mut self) {
        self.prefetch = None;
        self.queue = None;
        self.queue_running = false;
        self.page_start = 0;
    }
    pub(super) fn schedule_prefetch(&mut self) -> Result<(), String> {
        self.prefetch = None;
        if !self.queue_running {
            return Ok(());
        }
        let Some(queue) = &mut self.queue else {
            return Ok(());
        };
        let Some(index) = queue.order.peek_next() else {
            return Ok(());
        };
        let track = queue.playlist.track(index)?;
        if track.unavailable {
            return Ok(());
        }
        let url = crate::resolver::canonical_video_url(&format!(
            "https://www.youtube.com/watch?v={}",
            track.metadata.id
        ))?;
        self.prefetch = Prefetch::start(index, url, self.resolver.clone(), &self.cancel);
        Ok(())
    }
    pub(super) fn play_queue(&mut self, index: u64) -> Result<(), String> {
        self.play_queue_direction(index, false)
    }
    fn play_queue_direction(&mut self, mut index: u64, backwards: bool) -> Result<(), String> {
        self.active = None;
        self.youtube = None;
        self.path = None;
        self.queue_running = false;
        loop {
            if self.cancel.is_cancelled() {
                self.prefetch = None;
                return Err("Audio request cancelled.".into());
            }
            let queue = self.queue.as_mut().ok_or("Load a playlist first.")?;
            queue.order.select(index)?;
            let track = queue.playlist.track(index)?;
            self.snapshot.path = None;
            self.snapshot.metadata = Some(track.metadata.clone());
            self.snapshot.transfer = None;
            self.snapshot.position_seconds = 0.0;
            self.snapshot.duration_seconds = track.metadata.duration_seconds;
            self.snapshot.status = Status::Loading;
            let cached = self
                .prefetch
                .as_ref()
                .filter(|p| p.index == index)
                .and_then(Prefetch::take_ready);
            self.prefetch = None;
            let result = if track.unavailable {
                Err("Video is unavailable.".into())
            } else {
                let url = crate::resolver::canonical_video_url(&format!(
                    "https://www.youtube.com/watch?v={}",
                    track.metadata.id
                ))?;
                self.load_youtube(url, cached).and_then(|_| self.start(0.0))
            };
            match result {
                Ok(()) => {
                    let queue = self.queue.as_mut().unwrap();
                    queue.playlist.mark_failed(index, false)?;
                    self.queue_running = true;
                    self.schedule_prefetch()?;
                    return Ok(());
                }
                Err(error) => {
                    if self.cancel.is_cancelled() {
                        return Err(error);
                    }
                    let queue = self.queue.as_mut().unwrap();
                    match queue.failed_in_direction(backwards)? {
                        Some(next) => index = next,
                        None => {
                            self.fail("No playable tracks remain in this playlist pass. Select a track to retry.".into());
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
    pub(super) fn queue_command(&mut self, action: QueueAction) -> Result<(), String> {
        match action {
            QueueAction::Load { url } => {
                let url = crate::resolver::playlist::canonical_playlist_url(&url)?;
                self.prefetch = None;
                self.active = None;
                self.queue_running = false;
                self.snapshot.status = if self.snapshot.path.is_some() {
                    Status::Paused
                } else {
                    Status::Stopped
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "Cannot start playlist runtime.")?;
                let resolver = YtDlpResolver::default();
                let playlist = runtime.block_on(async {
                    tokio::select! { biased; _ = self.cancel.cancelled() => Err("Playlist load cancelled.".into()), result = resolver.playlist(&url) => result }
                })?;
                self.queue = Some(Queue::new(playlist));
                self.queue.as_mut().unwrap().source_url = Some(url.to_string());
                self.page_start = 0;
                self.play_queue(0)?;
            }
            QueueAction::Page { start } => {
                let queue = self.queue.as_ref().ok_or("Load a playlist first.")?;
                if start >= queue.playlist.len {
                    return Err("Page is outside the playlist.".into());
                }
                self.page_start = start;
            }
            QueueAction::Configure { shuffle, repeat } => {
                let queue = self.queue.as_mut().ok_or("Load a playlist first.")?;
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                queue.order.configure(shuffle, repeat, seed);
                self.schedule_prefetch()?;
            }
            action => {
                let queue = self.queue.as_mut().ok_or("Load a playlist first.")?;
                let backwards = matches!(action, QueueAction::Previous);
                let index = match action {
                    QueueAction::Next => queue.order.next(false, false),
                    QueueAction::Previous => {
                        queue.order.previous().or_else(|| queue.order.current())
                    }
                    QueueAction::Select { index } => {
                        queue.order.select(index)?;
                        Some(index)
                    }
                    _ => unreachable!(),
                };
                queue.consecutive_failures = 0;
                if let Some(index) = index {
                    self.play_queue_direction(index, backwards)?;
                } else {
                    self.active = None;
                    self.prefetch = None;
                    self.queue_running = false;
                    self.snapshot.status = Status::Ended;
                }
            }
        }
        Ok(())
    }
    pub(super) fn refresh_queue_snapshot(&mut self) -> Result<(), String> {
        self.snapshot.queue = self.queue.as_ref().map(Queue::snapshot);
        if let (Some(queue), Some(snapshot)) = (&mut self.queue, &mut self.snapshot.queue) {
            if let Some(index) = queue.order.current() {
                snapshot.source_index = Some(
                    queue
                        .playlist
                        .track(index)?
                        .playlist_index
                        .unwrap_or(index + 1),
                );
            }
        }
        self.snapshot.queue_page = match &mut self.queue {
            Some(queue) => Some(QueuePage {
                start: self.page_start,
                tracks: queue.playlist.page(self.page_start)?,
            }),
            None => None,
        };
        Ok(())
    }
}
