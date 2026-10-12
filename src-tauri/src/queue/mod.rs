//! Fixed-size disk records and a constant-space traversal; no playlist-sized RAM index.
pub(crate) mod prefetch;
use crate::resolver::VideoMetadata;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

const RECORD_BYTES: usize = 4096;
pub const PAGE_SIZE: usize = 40;
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub metadata: VideoMetadata,
    pub unavailable: bool,
    pub failed: bool,
}

pub struct Playlist {
    file: Option<File>,
    path: PathBuf,
    pub title: String,
    pub len: u64,
}
impl Playlist {
    pub fn new() -> Result<Self, String> {
        for _ in 0..100 {
            let path = std::env::temp_dir().join(format!(
                "mediabrowser-queue-{}-{}.tmp",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(0x04000000); // FILE_FLAG_DELETE_ON_CLOSE, including crashes.
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file: Some(file),
                        path,
                        title: "YouTube playlist".into(),
                        len: 0,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("Cannot create the temporary playlist index.".into()),
            }
        }
        Err("Cannot create a unique playlist index.".into())
    }
    pub fn push(&mut self, track: &Track) -> Result<(), String> {
        self.write(self.len, track)?;
        self.len += 1;
        Ok(())
    }
    fn write(&mut self, index: u64, track: &Track) -> Result<(), String> {
        let data = serde_json::to_vec(track).map_err(|_| "Invalid playlist metadata.")?;
        if data.len() > RECORD_BYTES - 4 {
            return Err("Playlist metadata record is too large.".into());
        }
        let mut record = [0u8; RECORD_BYTES];
        record[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
        record[4..4 + data.len()].copy_from_slice(&data);
        let offset = index
            .checked_mul(RECORD_BYTES as u64)
            .ok_or("Playlist index overflow.")?;
        let file = self.file.as_mut().ok_or("Playlist closed.")?;
        file.seek(SeekFrom::Start(offset))
            .and_then(|_| file.write_all(&record))
            .map_err(|_| "Cannot write playlist index (check free disk space).".to_owned())
    }
    pub fn track(&mut self, index: u64) -> Result<Track, String> {
        if index >= self.len {
            return Err("Track is outside the playlist.".into());
        }
        let file = self.file.as_mut().ok_or("Playlist closed.")?;
        let mut record = [0u8; RECORD_BYTES];
        file.seek(SeekFrom::Start(index * RECORD_BYTES as u64))
            .and_then(|_| file.read_exact(&mut record))
            .map_err(|_| "Cannot read playlist index.".to_owned())?;
        let size = u32::from_le_bytes(record[..4].try_into().unwrap()) as usize;
        if size > RECORD_BYTES - 4 {
            return Err("Corrupt playlist index.".into());
        }
        serde_json::from_slice(&record[4..4 + size])
            .map_err(|_| "Corrupt playlist metadata.".into())
    }
    pub fn mark_failed(&mut self, index: u64, failed: bool) -> Result<(), String> {
        let mut track = self.track(index)?;
        track.failed = failed;
        self.write(index, &track)
    }
    pub fn page(&mut self, start: u64) -> Result<Vec<Track>, String> {
        (start.min(self.len)..self.len.min(start.saturating_add(PAGE_SIZE as u64)))
            .map(|i| self.track(i))
            .collect()
    }
}
impl Drop for Playlist {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

/// An affine permutation visits every track once without an O(n) shuffle array.
/// Previous retraces that order. Manual selection moves the cursor within it.
pub struct Order {
    len: u64,
    position: u64,
    stride: u64,
    offset: u64,
    pub shuffle: bool,
    pub repeat: Repeat,
}
impl Order {
    pub fn new(len: u64) -> Self {
        Self {
            len,
            position: 0,
            stride: 1,
            offset: 0,
            shuffle: false,
            repeat: Repeat::Off,
        }
    }
    fn index(&self, position: u64) -> u64 {
        ((position as u128 * self.stride as u128 + self.offset as u128) % self.len as u128) as u64
    }
    pub fn current(&self) -> Option<u64> {
        (self.len > 0).then(|| self.index(self.position))
    }
    pub fn select(&mut self, index: u64) -> Result<(), String> {
        if index >= self.len {
            return Err("Track is outside the playlist.".into());
        }
        // Multiplicative inverse of stride modulo len (Euclid, constant space).
        let (mut a, mut b, mut x, mut y) = (self.stride as i128, self.len as i128, 1i128, 0i128);
        while b != 0 {
            let q = a / b;
            (a, b) = (b, a - q * b);
            (x, y) = (y, x - q * y);
        }
        let delta = (index as i128 - self.offset as i128).rem_euclid(self.len as i128);
        self.position =
            ((delta as u128 * x.rem_euclid(self.len as i128) as u128) % self.len as u128) as u64;
        Ok(())
    }
    pub fn configure(&mut self, shuffle: bool, repeat: Repeat, seed: u64) {
        let current = self.current().unwrap_or(0);
        if shuffle != self.shuffle && self.len > 0 {
            self.stride = if shuffle && self.len > 2 {
                let mut stride = 2 + seed % (self.len - 2);
                while gcd(stride, self.len) != 1 {
                    stride = if stride + 1 < self.len { stride + 1 } else { 1 };
                }
                stride
            } else {
                1
            };
            self.offset = if shuffle { current } else { 0 };
            self.select(current).expect("current index is valid");
        }
        self.shuffle = shuffle;
        self.repeat = repeat;
    }
    pub fn next(&mut self, automatic: bool, failed: bool) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        if automatic && !failed && self.repeat == Repeat::One {
            return self.current();
        }
        if self.position + 1 == self.len {
            if self.repeat == Repeat::Off {
                return None;
            }
            self.position = 0;
        } else {
            self.position += 1;
        }
        self.current()
    }
    pub fn previous(&mut self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        if self.position == 0 {
            if self.repeat == Repeat::Off {
                return None;
            }
            self.position = self.len - 1;
        } else {
            self.position -= 1;
        }
        self.current()
    }
    pub fn peek_next(&self) -> Option<u64> {
        if self.len == 0 {
            return None;
        }
        if self.repeat == Repeat::One {
            return self.current();
        }
        if self.position + 1 < self.len {
            Some(self.index(self.position + 1))
        } else if self.repeat == Repeat::All {
            Some(self.index(0))
        } else {
            None
        }
    }
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    pub title: String,
    pub total: u64,
    pub current: Option<u64>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub skipped: u64,
    pub notice: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuePage {
    pub start: u64,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueueAction {
    Load { url: String },
    Next,
    Previous,
    Select { index: u64 },
    Configure { shuffle: bool, repeat: Repeat },
    Page { start: u64 },
}
pub struct Queue {
    pub playlist: Playlist,
    pub order: Order,
    pub skipped: u64,
    pub notice: Option<String>,
    // Stops all-unavailable repeat playlists after at most one full sweep.
    pub consecutive_failures: u64,
}
impl Queue {
    pub fn new(playlist: Playlist) -> Self {
        Self {
            order: Order::new(playlist.len),
            playlist,
            skipped: 0,
            notice: None,
            consecutive_failures: 0,
        }
    }
    pub fn failed(&mut self) -> Result<Option<u64>, String> {
        if let Some(index) = self.order.current() {
            self.playlist.mark_failed(index, true)?;
        }
        self.skipped += 1;
        self.consecutive_failures += 1;
        self.notice = Some(format!("Skipped {} unavailable track(s).", self.skipped));
        if self.consecutive_failures >= self.playlist.len {
            return Ok(None);
        }
        Ok(self.order.next(false, true))
    }
    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            title: self.playlist.title.clone(),
            total: self.playlist.len,
            current: self.order.current(),
            shuffle: self.order.shuffle,
            repeat: self.order.repeat,
            skipped: self.skipped,
            notice: self.notice.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
