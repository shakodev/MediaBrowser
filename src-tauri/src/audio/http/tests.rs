use super::*;
use crate::resolver::{ResolveFuture, VideoMetadata};
use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    sync::atomic::AtomicBool,
    thread,
};

pub(crate) struct Server {
    pub(crate) url: Url,
    done: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl Server {
    pub(crate) fn new(data: Vec<u8>, mode: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/audio", listener.local_addr().unwrap())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let stop = done.clone();
        let join = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut socket, _)) => serve(&mut socket, &data, mode),
                    Err(_) => thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        Self {
            url,
            done,
            join: Some(join),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        self.join.take().unwrap().join().unwrap();
    }
}
fn serve(socket: &mut TcpStream, data: &[u8], mode: &str) {
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut request = Vec::new();
    let mut byte = [0];
    while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
        if socket.read(&mut byte).unwrap_or(0) == 0 {
            return;
        }
        request.push(byte[0]);
    }
    let request = String::from_utf8_lossy(&request).to_lowercase();
    if mode == "stall" {
        thread::sleep(Duration::from_millis(300));
    }
    if request.contains("/expired") || mode == "expired" {
        let _ = socket
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if mode == "ignored" {
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9000000\r\nConnection: close\r\n\r\n");
        return;
    }
    let range = request
        .lines()
        .find_map(|l| l.strip_prefix("range: bytes="))
        .unwrap();
    let (start, end) = range.split_once('-').unwrap();
    let start: usize = start.parse().unwrap();
    let end = end.parse::<usize>().unwrap().min(data.len() - 1);
    let declared_start = if mode == "wrong" { start + 1 } else { start };
    let headers = format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {declared_start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len(), end - start + 1);
    let _ = socket.write_all(headers.as_bytes());
    let payload = if mode == "truncated" {
        &data[start..start + 1]
    } else {
        &data[start..=end]
    };
    let _ = socket.write_all(payload);
}
struct Resolver {
    pub(crate) url: Url,
    calls: AtomicU64,
    changed: bool,
}
impl StreamResolver for Resolver {
    fn resolve<'a>(&'a self, _: &'a Url) -> ResolveFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut result = stream(self.url.clone());
            if self.changed {
                result.format_id = "different".into();
            }
            Ok(result)
        })
    }
}
fn stream(url: Url) -> ResolvedStream {
    ResolvedStream {
        media_url: url,
        headers: Default::default(),
        format_id: "140".into(),
        extension: "m4a".into(),
        metadata: VideoMetadata {
            id: "BaW_jenozKc".into(),
            title: "test".into(),
            uploader: None,
            duration_seconds: None,
        },
    }
}
fn open(
    server: &Server,
    token: CancellationToken,
    stats: Arc<TransferStats>,
) -> Result<HttpSource, String> {
    HttpSource::open(
        Url::parse("https://youtu.be/BaW_jenozKc").unwrap(),
        stream(server.url.clone()),
        Arc::new(Resolver {
            url: server.url.clone(),
            calls: AtomicU64::new(0),
            changed: false,
        }),
        token,
        stats,
    )
}
#[test]
fn bounded_ranges_support_forward_backward_and_end_seeks_without_full_download() {
    let data: Vec<u8> = (0..1024 * 1024).map(|n| (n % 251) as u8).collect();
    let server = Server::new(data.clone(), "ok");
    let stats = Arc::new(TransferStats::default());
    let mut source = open(&server, CancellationToken::new(), stats.clone()).unwrap();
    for offset in [0, 400000, 4, 900000] {
        source.seek(SeekFrom::Start(offset)).unwrap();
        let mut bytes = [0; 123];
        source.read_exact(&mut bytes).unwrap();
        assert_eq!(
            &bytes,
            &data[offset as usize..offset as usize + bytes.len()]
        );
        assert!(source.buffer.len() <= RANGE_BYTES);
    }
    source.seek(SeekFrom::End(0)).unwrap();
    assert_eq!(source.read(&mut [0; 1]).unwrap(), 0);
    assert!(source.seek(SeekFrom::End(1)).is_err());
    assert!(stats.snapshot().bytes_received < data.len() as u64);
    assert_eq!(stats.snapshot().total_bytes, data.len() as u64);
}
#[test]
fn rejects_ignored_wrong_and_truncated_ranges() {
    for mode in ["ignored", "wrong", "truncated"] {
        let server = Server::new(vec![7; RANGE_BYTES * 2], mode);
        assert!(
            open(&server, CancellationToken::new(), Arc::default()).is_err(),
            "{mode}"
        );
    }
    for value in [
        "bytes 2-1/10",
        "bytes 0-10/10",
        "bytes 0-1/*",
        "items 0-1/10",
    ] {
        assert!(parse_content_range(value).is_none());
    }
}
#[test]
fn refreshes_expired_url_at_same_offset_and_rejects_format_change() {
    let server = Server::new(vec![9; RANGE_BYTES * 4], "ok");
    for changed in [false, true] {
        let resolver = Arc::new(Resolver {
            url: server.url.clone(),
            calls: AtomicU64::new(0),
            changed,
        });
        let stats = Arc::new(TransferStats::default());
        let mut source = open(&server, CancellationToken::new(), stats.clone()).unwrap();
        source.resolver = resolver.clone();
        source.stream.media_url.set_path("/expired");
        source
            .seek(SeekFrom::Start(RANGE_BYTES as u64 * 2))
            .unwrap();
        let result = source.read(&mut [0; 10]);
        assert_eq!(result.is_ok(), !changed);
        assert_eq!(resolver.calls.load(Ordering::Relaxed), 1);
        if !changed {
            assert_eq!(stats.snapshot().url_refreshes, 1);
        }
    }
}

#[test]
fn persistent_forbidden_after_buffered_audio_remains_a_retryable_error() {
    let server = Server::new(vec![9; RANGE_BYTES * 3], "ok");
    let stats = Arc::new(TransferStats::default());
    let mut source = open(&server, CancellationToken::new(), stats.clone()).unwrap();
    let mut expired = server.url.clone();
    expired.set_path("/expired");
    source.stream.media_url = expired.clone();
    source.resolver = Arc::new(Resolver {
        url: expired,
        calls: AtomicU64::new(0),
        changed: false,
    });
    // Initial bytes can play even when later requests and re-resolution fail.
    source.read_exact(&mut vec![0; RANGE_BYTES]).unwrap();
    for _ in 0..2 {
        let error = source.read(&mut [0; 1]).unwrap_err();
        assert!(error.to_string().contains("HTTP 403"));
        assert!(error.to_string().contains("update yt-dlp"));
        assert_eq!(source.position, RANGE_BYTES as u64);
    }
    assert_eq!(stats.snapshot().url_refreshes, 2);
}
#[test]
fn cancelling_interrupts_pending_headers_and_future_reads() {
    let server = Server::new(vec![0; RANGE_BYTES], "stall");
    let token = CancellationToken::new();
    let cancel = token.clone();
    let join = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        cancel.cancel();
    });
    let start = std::time::Instant::now();
    assert!(open(&server, token, Arc::default())
        .err()
        .unwrap()
        .contains("cancelled"));
    assert!(start.elapsed() < Duration::from_millis(250));
    join.join().unwrap();
    let server = Server::new(vec![0; RANGE_BYTES], "ok");
    let token = CancellationToken::new();
    let mut source = open(&server, token.clone(), Arc::default()).unwrap();
    token.cancel();
    assert!(source.read(&mut [0; 1]).is_err());
}
#[test]
fn remote_m4a_decodes_and_seeks_through_the_same_decoder() {
    let data = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sfx.m4a"),
    )
    .unwrap();
    let server = Server::new(data, "ok");
    let source = open(&server, CancellationToken::new(), Arc::default()).unwrap();
    let mut decoder =
        crate::audio::decoder::AudioDecoder::from_source(Box::new(source), "m4a", None).unwrap();
    for target in [0.0, 0.1, 0.04] {
        decoder.seek(target).unwrap();
        let block = decoder.next_block().unwrap().unwrap();
        assert!(block.samples.iter().any(|v| v.abs() > 0.001));
        assert!(block.start_seconds >= target - 1e-8);
    }
}

#[test]
fn progressive_http_mp4_initialization_does_not_scan_media_body() {
    let mut data = fast_start_fixture();
    // Many small valid atoms mimic fragmented MP4 skip behavior: eager probing
    // would read each body because it fits inside Symphonia's skip threshold.
    let padding = 64 * 1024;
    for _ in 0..64 {
        data.extend_from_slice(&(padding as u32).to_be_bytes());
        data.extend_from_slice(b"free");
        data.resize(data.len() + padding - 8, 0);
    }
    let server = Server::new(data, "ok");
    let stats = Arc::new(TransferStats::default());
    let token = CancellationToken::new();
    let source = open(&server, token.clone(), stats.clone()).unwrap();
    let mut decoder =
        crate::audio::decoder::AudioDecoder::from_source(Box::new(source), "m4a", Some(token))
            .unwrap();
    decoder.next_block().unwrap().unwrap();
    decoder.seek(0.1).unwrap();
    decoder.next_block().unwrap().unwrap();
    assert!(stats.snapshot().bytes_received < RANGE_BYTES as u64 * 3);
}

pub(crate) fn fast_start_fixture() -> Vec<u8> {
    let input = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sfx.m4a"),
    )
    .unwrap();
    let mut atoms = Vec::new();
    let mut position = 0;
    while position < input.len() {
        let length = u32::from_be_bytes(input[position..position + 4].try_into().unwrap()) as usize;
        assert!(length >= 8);
        atoms.push((position, position + length));
        position += length;
    }
    let &(start, end) = atoms
        .iter()
        .find(|&&(a, _)| &input[a + 4..a + 8] == b"moov")
        .unwrap();
    let mut moov = input[start..end].to_vec();
    // Relocating tail moov immediately after ftyp shifts each media chunk forward.
    let shift = moov.len() as u32;
    if start
        > atoms
            .iter()
            .find(|&&(a, _)| &input[a + 4..a + 8] == b"mdat")
            .unwrap()
            .0
    {
        let mut position = 4;
        while position + 12 <= moov.len() {
            if &moov[position..position + 4] == b"stco" {
                let count =
                    u32::from_be_bytes(moov[position + 8..position + 12].try_into().unwrap())
                        as usize;
                for entry in 0..count {
                    let offset = position + 12 + entry * 4;
                    let old = u32::from_be_bytes(moov[offset..offset + 4].try_into().unwrap());
                    moov[offset..offset + 4].copy_from_slice(&(old + shift).to_be_bytes());
                }
            }
            position += 1;
        }
    }
    let mut output = Vec::new();
    for (a, b) in atoms {
        if &input[a + 4..a + 8] == b"moov" {
            continue;
        }
        output.extend_from_slice(&input[a..b]);
        if &input[a + 4..a + 8] == b"ftyp" {
            output.extend_from_slice(&moov);
        }
    }
    output
}

#[test]
#[ignore = "requires yt-dlp, Deno and public YouTube network access"]
fn live_youtube_decodes_past_two_minutes_without_audio_output() {
    let url = Url::parse(
        &std::env::var("MEDIABROWSER_TEST_URL")
            .unwrap_or_else(|_| "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into()),
    )
    .unwrap();
    let resolver: Arc<dyn StreamResolver> = Arc::new(crate::resolver::YtDlpResolver::default());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let stream = runtime.block_on(resolver.resolve(&url)).unwrap();
    let extension = stream.extension.clone();
    let token = CancellationToken::new();
    let stats = Arc::new(TransferStats::default());
    let source = HttpSource::open(url, stream, resolver, token.clone(), stats.clone()).unwrap();
    let mut decoder =
        crate::audio::decoder::AudioDecoder::from_source(Box::new(source), &extension, Some(token))
            .unwrap();
    let mut position = 0.0;
    while position < 120.0 {
        let block = decoder
            .next_block()
            .unwrap_or_else(|error| panic!("at {position:.3}s: {error}"))
            .unwrap_or_else(|| panic!("early EOF at {position:.3}s"));
        position = block.start_seconds
            + block.samples.len() as f64 / block.channels as f64 / block.rate as f64;
    }
    println!(
        "Decoded through {position:.3}s; {} ranges",
        stats.snapshot().range_requests
    );
}

#[test]
#[ignore = "requires yt-dlp, Deno, public YouTube network access and an audio device"]
fn live_youtube_muted_playback_past_two_minutes() {
    let url = std::env::var("MEDIABROWSER_TEST_URL")
        .unwrap_or_else(|_| "https://www.youtube.com/watch?v=aqz-KE-bpKQ".into());
    tauri::async_runtime::block_on(async {
        let engine = crate::audio::AudioEngine::new().unwrap();
        engine.set_volume(0.0).await.unwrap();
        let parsed = Url::parse(&url).unwrap();
        let playlist_id = parsed
            .query_pairs()
            .find(|(key, _)| key == "list")
            .map(|(_, value)| value.into_owned());
        if playlist_id.is_some() {
            let video = crate::resolver::canonical_video_url(&url).unwrap();
            let state = crate::playback_state::BrowserPlayback {
                url: url.clone(),
                video_id: video
                    .query_pairs()
                    .find(|(key, _)| key == "v")
                    .unwrap()
                    .1
                    .into_owned(),
                playlist_id,
                playlist_index: None,
                position_seconds: 0.0,
                volume: 0.0,
                paused: false,
            };
            engine.prepare_handoff(state.clone()).await.unwrap();
            engine.start_handoff(state).await.unwrap();
            engine.commit_handoff(0.0, false).await.unwrap();
        } else {
            engine.load_youtube(&url).await.unwrap();
            engine.play().await.unwrap();
        }
        let initial = engine.snapshot().await.unwrap();
        let started = std::time::Instant::now();
        let mut reported = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let snapshot = engine.snapshot().await.unwrap();
            assert_eq!(
                snapshot.metadata.as_ref().map(|m| &m.id),
                initial.metadata.as_ref().map(|m| &m.id),
                "playlist skipped the current song"
            );
            if let Some(queue) = &snapshot.queue {
                assert_eq!(queue.skipped, 0);
            }
            assert_eq!(
                snapshot.status,
                crate::playback_state::PlaybackStatus::Playing,
                "at {:.3}s: {:?}",
                snapshot.position_seconds,
                snapshot.error
            );
            let seconds = snapshot.position_seconds as u64;
            if seconds / 15 > reported {
                reported = seconds / 15;
                println!("Playing at {:.3}s", snapshot.position_seconds);
            }
            if snapshot.position_seconds >= 120.0 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(180),
                "playback stalled"
            );
        }
        engine.stop().await.unwrap();
    });
}
