//! Synchronous seekable adapter for Symphonia, backed by cancellable Tokio HTTP IO.
//! One 128 KiB range is retained; no disk cache or whole-file download.
use crate::resolver::{ResolvedStream, StreamResolver};
use reqwest::{header, Client, StatusCode};
use serde::Serialize;
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use symphonia::core::io::MediaSource;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use url::Url;

pub const RANGE_BYTES: usize = 128 * 1024;
#[derive(Default)]
pub struct TransferStats {
    bytes: AtomicU64,
    total: AtomicU64,
    requests: AtomicU64,
    refreshes: AtomicU64,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSnapshot {
    pub bytes_received: u64,
    pub total_bytes: u64,
    pub range_requests: u64,
    pub url_refreshes: u64,
    pub buffer_limit_bytes: usize,
}
impl TransferStats {
    pub fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            bytes_received: self.bytes.load(Ordering::Relaxed),
            total_bytes: self.total.load(Ordering::Relaxed),
            range_requests: self.requests.load(Ordering::Relaxed),
            url_refreshes: self.refreshes.load(Ordering::Relaxed),
            buffer_limit_bytes: RANGE_BYTES,
        }
    }
}

pub struct HttpSource {
    runtime: Runtime,
    client: Client,
    resolver: Arc<dyn StreamResolver>,
    youtube_url: Url,
    stream: ResolvedStream,
    cancel: CancellationToken,
    stats: Arc<TransferStats>,
    position: u64,
    length: u64,
    buffer_start: u64,
    buffer: Vec<u8>,
}
impl HttpSource {
    pub fn open(
        youtube_url: Url,
        stream: ResolvedStream,
        resolver: Arc<dyn StreamResolver>,
        cancel: CancellationToken,
        stats: Arc<TransferStats>,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "Cannot start HTTP runtime.")?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Cannot start HTTP client.")?;
        let mut source = Self {
            runtime,
            client,
            resolver,
            youtube_url,
            stream,
            cancel,
            stats,
            position: 0,
            length: 0,
            buffer_start: 0,
            buffer: Vec::new(),
        };
        source.fetch().map_err(|e| e.to_string())?;
        Ok(source)
    }
    fn fetch(&mut self) -> io::Result<()> {
        self.buffer = Vec::new();
        let result = self.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => Err(failure("Audio request cancelled.")),
                result = fetch_range(&self.client, &self.resolver, &self.youtube_url, &mut self.stream, self.position, self.length, &self.stats) => result,
            }
        });
        let (buffer, length) = result?;
        self.length = length;
        self.buffer_start = self.position;
        self.buffer = buffer;
        Ok(())
    }
}
async fn fetch_range(
    client: &Client,
    resolver: &Arc<dyn StreamResolver>,
    youtube_url: &Url,
    stream: &mut ResolvedStream,
    start: u64,
    known_length: u64,
    stats: &TransferStats,
) -> io::Result<(Vec<u8>, u64)> {
    let end = if known_length == 0 {
        start + RANGE_BYTES as u64 - 1
    } else {
        (start + RANGE_BYTES as u64).min(known_length) - 1
    };
    let mut refreshed = false;
    for attempt in 0..3 {
        stats.requests.fetch_add(1, Ordering::Relaxed);
        let response = client
            .get(stream.media_url.clone())
            .headers(stream.headers.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::RANGE, format!("bytes={start}-{end}"))
            .send()
            .await;
        let mut response = match response {
            Ok(response) => response,
            Err(_) if attempt < 2 => continue,
            Err(_) => return Err(failure("Audio HTTP request failed or timed out. Check your connection and press Play to retry.")),
        };
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED | StatusCode::GONE
        ) && !refreshed
        {
            let fresh = resolver.resolve(youtube_url).await.map_err(failure)?;
            if fresh.format_id != stream.format_id
                || fresh.metadata.id != stream.metadata.id
                || fresh.extension != stream.extension
            {
                return Err(failure(
                    "The refreshed audio format changed; reload the video.",
                ));
            }
            *stream = fresh;
            stats.refreshes.fetch_add(1, Ordering::Relaxed);
            refreshed = true;
            continue;
        }
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(failure(format!("Audio server returned HTTP {}; a valid byte-range response is required. Reload the video or update yt-dlp.", response.status().as_u16())));
        }
        let range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|h| h.to_str().ok())
            .and_then(parse_content_range)
            .ok_or_else(|| failure("Audio server returned an invalid Content-Range."))?;
        if range.0 != start
            || range.1 != end.min(range.2.saturating_sub(1))
            || range.2 == 0
            || (known_length != 0 && known_length != range.2)
            || response.headers().contains_key(header::CONTENT_ENCODING)
        {
            return Err(failure(
                "Audio range or media length changed; reload the video.",
            ));
        }
        let expected = (range.1 - range.0 + 1) as usize;
        if expected > RANGE_BYTES {
            return Err(failure("Audio response exceeds the buffer limit."));
        }
        let mut data = Vec::with_capacity(expected);
        let mut failed = false;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    stats.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    if chunk.len() > expected - data.len() {
                        return Err(failure("Audio response exceeds its declared range."));
                    }
                    data.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if failed || data.len() != expected {
            if attempt < 2 {
                continue;
            }
            return Err(failure(
                "Audio HTTP range was interrupted or truncated. Press Play to retry.",
            ));
        }
        stats.total.store(range.2, Ordering::Relaxed);
        return Ok((data, range.2));
    }
    Err(failure("Audio URL recovery failed; reload the video."))
}
fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (span, length) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let result = (start.parse().ok()?, end.parse().ok()?, length.parse().ok()?);
    (result.0 <= result.1 && result.1 < result.2).then_some(result)
}
fn failure(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
impl Read for HttpSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(failure("Audio request cancelled."));
        }
        if output.is_empty() || self.position >= self.length {
            return Ok(0);
        }
        if self.position < self.buffer_start
            || self.position >= self.buffer_start + self.buffer.len() as u64
        {
            self.fetch()?;
        }
        let offset = (self.position - self.buffer_start) as usize;
        let count = output.len().min(self.buffer.len() - offset);
        output[..count].copy_from_slice(&self.buffer[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for HttpSource {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        let position = match offset {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.position as i128 + n as i128,
            SeekFrom::End(n) => self.length as i128 + n as i128,
        };
        if position < 0 || position > self.length as i128 {
            return Err(failure("Invalid audio byte seek."));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}
impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.length)
    }
}

#[cfg(test)]
pub(crate) mod tests;
