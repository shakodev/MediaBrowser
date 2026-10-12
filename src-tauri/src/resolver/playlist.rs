//! Stream compact flat metadata into a temporary disk index, one capped line at a time.
use super::YtDlpResolver;
use crate::queue::{Playlist, Track};
use serde::Deserialize;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use url::Url;

// Radio recommendations are open-ended; take a finite snapshot for the disk queue.
pub const MIX_TRACK_LIMIT: u64 = 50;

pub fn is_mix_url(url: &Url) -> bool {
    url.query_pairs()
        .any(|(k, v)| k == "list" && v.starts_with("RD"))
}

pub fn canonical_playlist_url(input: &str) -> Result<Url, String> {
    let invalid = || "Enter a public HTTPS YouTube playlist URL with a list parameter.".to_owned();
    let url = Url::parse(input).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "youtube.com"
                    | "www.youtube.com"
                    | "m.youtube.com"
                    | "music.youtube.com"
                    | "youtu.be"
            )
        )
    {
        return Err(invalid());
    }
    let id = url
        .query_pairs()
        .find(|(k, _)| k == "list")
        .map(|(_, v)| v.into_owned())
        .ok_or_else(invalid)?;
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid());
    }
    if let Some(seed) = id.strip_prefix("RD") {
        // Mixes live on watch pages. A playlist URL loses the seed YouTube needs.
        let mut watch = if url.path() == "/playlist" && !url.query_pairs().any(|(k, _)| k == "v") {
            // Only plain RD + video ID encodes its seed. Other RD variants need v=.
            super::canonical_video_url(&format!("https://www.youtube.com/watch?v={seed}"))
        } else {
            super::canonical_video_url(input)
        }
        .map_err(|_| {
            "Open the Mix in YouTube and use its watch URL with a valid video ID.".to_owned()
        })?;
        watch
            .query_pairs_mut()
            .append_pair("list", &id)
            .append_pair("start_radio", "1");
        return Ok(watch);
    }
    Url::parse(&format!("https://www.youtube.com/playlist?list={id}")).map_err(|_| invalid())
}

#[derive(Deserialize)]
struct FlatTrack {
    playlist_id: Option<String>,
    playlist_index: Option<u64>,
    id: Option<String>,
    title: Option<String>,
    uploader: Option<String>,
    duration: Option<serde_json::Value>,
    availability: Option<String>,
    playlist_title: Option<String>,
}
fn bounded(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}
fn append(playlist: &mut Playlist, line: &[u8], mix_id: Option<&str>) -> Result<(), String> {
    let flat: FlatTrack =
        serde_json::from_slice(line).map_err(|_| "Invalid flat playlist metadata.")?;
    if let Some(id) = mix_id {
        if flat.playlist_id.as_deref() != Some(id) {
            return Err(
                "YouTube did not return the requested Mix. Reload it in YouTube and retry.".into(),
            );
        }
        if playlist.len >= MIX_TRACK_LIMIT {
            return Err("Mix metadata exceeds its track limit.".into());
        }
    }
    if playlist.len == 0 {
        if let Some(title) = flat.playlist_title {
            playlist.title = bounded(&title, 200);
        }
    }
    let id = flat.id.unwrap_or_default();
    let valid_id = id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    let title = flat.title.unwrap_or_else(|| "Unavailable video".into());
    let unavailable = !valid_id
        || matches!(
            flat.availability.as_deref(),
            Some("private" | "premium_only" | "subscriber_only" | "needs_auth")
        )
        || matches!(title.as_str(), "[Private video]" | "[Deleted video]");
    playlist.push(&Track {
        playlist_index: flat.playlist_index.filter(|i| *i > 0),
        metadata: super::VideoMetadata {
            id: if valid_id { id } else { String::new() },
            title: bounded(&title, 200),
            uploader: flat.uploader.map(|s| bounded(&s, 120)),
            duration_seconds: flat
                .duration
                .and_then(|v| v.as_f64())
                .filter(|d| d.is_finite() && *d > 0.0),
        },
        unavailable,
        failed: false,
    })
}
async fn read_metadata(
    reader: &mut (impl AsyncBufRead + Unpin),
    mix_id: Option<&str>,
) -> Result<Playlist, String> {
    let mut playlist = Playlist::new()?;
    loop {
        // take limits read_until allocation, including maliciously long lines.
        let mut line = Vec::new();
        let count = tokio::time::timeout(
            Duration::from_secs(60),
            (&mut *reader).take(16385).read_until(b'\n', &mut line),
        )
        .await
        .map_err(|_| "Playlist extraction stalled for 60 seconds.")?
        .map_err(|_| "Cannot read playlist metadata.")?;
        if count == 0 {
            break;
        }
        if count > 16384 || !line.ends_with(b"\n") {
            return Err("Playlist metadata line exceeds its limit or is incomplete.".into());
        }
        append(&mut playlist, &line, mix_id)?;
    }
    Ok(playlist)
}

impl YtDlpResolver {
    pub async fn playlist(&self, url: &Url) -> Result<Playlist, String> {
        let url = canonical_playlist_url(url.as_str())?;
        let mix_id = url
            .query_pairs()
            .find(|(k, v)| k == "list" && v.starts_with("RD"))
            .map(|(_, v)| v.into_owned());
        // Project a dictionary before encoding: individual missing %(field)j values
        // print the bare NA placeholder, which is not valid JSON.
        let template =
            "%(.{id,title,uploader,duration,availability,playlist_id,playlist_title,playlist_index})j";
        let mut command = Command::new(&self.executable);
        command
            .args([
                "--ignore-config",
                "--flat-playlist",
                "--lazy-playlist",
                "--yes-playlist",
                "--skip-download",
                "--ignore-errors",
                "--no-warnings",
                "--no-progress",
                "--no-cache-dir",
                "--encoding",
                "utf-8",
                "--socket-timeout",
                "15",
                "--retries",
                "1",
                "--print",
                template,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if mix_id.is_some() {
            command.args(["--playlist-end", &MIX_TRACK_LIMIT.to_string()]);
        }
        command.args(["--", url.as_str()]);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| {
            "Cannot launch yt-dlp. Install it on PATH or set MEDIABROWSER_YTDLP.".to_owned()
        })?;
        #[cfg(windows)]
        let _job = super::job::ResolverJob::attach(&mut child)?;
        let mut reader =
            BufReader::new(child.stdout.take().ok_or("Missing playlist output pipe.")?);
        let metadata = read_metadata(&mut reader, mix_id.as_deref());
        let playlist = if mix_id.is_some() {
            tokio::time::timeout(Duration::from_secs(120), metadata)
                .await
                .map_err(|_| {
                    "Mix extraction timed out after 120 seconds. Retry the Mix.".to_owned()
                })??
        } else {
            metadata.await?
        };
        let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .map_err(|_| "Playlist extractor did not exit.")?
            .map_err(|_| "Cannot wait for yt-dlp.".to_owned())?;
        if !status.success() {
            return Err("Playlist extraction failed. It may be private, unavailable, or blocked by YouTube.".into());
        }
        if playlist.len == 0 {
            return Err("Playlist is empty or unavailable.".into());
        }
        Ok(playlist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_reader_bounds_lines_and_handles_missing_fields_and_unicode() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            // Same dictionary shape as real yt-dlp: missing keys are omitted.
            let json = format!("{}\n", serde_json::json!({ "id": "BaW_jenozKc", "title": "🎵".repeat(1000), "playlist_title": "Public playlist" }));
            let mut list = read_metadata(&mut BufReader::new(json.as_bytes()), None).await.unwrap();
            let track = list.track(0).unwrap();
            assert_eq!(track.metadata.title.chars().count(), 200);
            assert_eq!(track.metadata.duration_seconds, None);
            assert_eq!(track.metadata.uploader, None);
            assert!(!track.unavailable);
            let huge = vec![b'x'; 16385];
            for bad in [huge.as_slice(), b"{broken}\n", b"{}"] {
                assert!(read_metadata(&mut BufReader::new(bad), None).await.is_err());
            }
            assert_eq!(read_metadata(&mut BufReader::new(b"".as_slice()), None).await.unwrap().len, 0);
        });
    }
    #[test]
    fn urls_are_canonical_and_flat_unavailable_entries_are_retained() {
        assert_eq!(
            canonical_playlist_url("https://music.youtube.com/watch?v=BaW_jenozKc&list=PL_test-1")
                .unwrap()
                .as_str(),
            "https://www.youtube.com/playlist?list=PL_test-1"
        );
        for url in [
            "https://youtube.com.evil/playlist?list=PLx",
            "https://user@youtube.com/playlist?list=PLx",
            "http://youtube.com/playlist?list=PLx",
            "https://youtube.com/playlist?list=--x%26evil",
            "https://youtube.com/watch?v=BaW_jenozKc",
        ] {
            assert!(canonical_playlist_url(url).is_err());
        }
        let mut list = Playlist::new().unwrap();
        append(&mut list, br#"{"id":"BaW_jenozKc","title":"[Private video]","duration":"NA","playlist_title":"Test"}"#, None).unwrap();
        append(
            &mut list,
            br#"{"id":"aqz-KE-bpKQ","title":"Playable","duration":15,"playlist_index":8}"#,
            None,
        )
        .unwrap();
        assert_eq!(list.len, 2);
        assert!(list.track(0).unwrap().unavailable);
        assert_eq!(list.track(1).unwrap().metadata.duration_seconds, Some(15.0));
        assert_eq!(list.track(1).unwrap().playlist_index, Some(8));
    }

    #[test]
    fn mix_urls_preserve_the_watch_seed_and_are_idempotent() {
        let expected =
            "https://www.youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY&start_radio=1";
        for input in [
            expected,
            "https://music.youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY&index=17&t=23",
            "https://youtu.be/3s93p9xrCIY?list=RD3s93p9xrCIY",
            "https://www.youtube.com/playlist?list=RD3s93p9xrCIY",
        ] {
            let url = canonical_playlist_url(input).unwrap();
            assert_eq!(url.as_str(), expected);
            assert!(is_mix_url(&url));
            assert_eq!(canonical_playlist_url(url.as_str()).unwrap(), url);
        }
        // A current track can differ from the seed encoded by the original RD ID.
        for id in ["RD3s93p9xrCIY", "RDMM", "RDAMVM3s93p9xrCIY", "RDEM_test"] {
            let url = canonical_playlist_url(&format!(
                "https://www.youtube.com/watch?v=BaW_jenozKc&list={id}"
            ))
            .unwrap();
            assert!(url.as_str().contains("v=BaW_jenozKc&list="));
            assert_eq!(canonical_playlist_url(url.as_str()).unwrap(), url);
        }
        for input in [
            "https://youtube.com/playlist?list=RDMM",
            "https://youtube.com/watch?v=bad&list=RD3s93p9xrCIY",
            "https://youtube.com.evil/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY",
            "http://youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY",
            "https://user@youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY",
            "https://youtube.com/watch?v=3s93p9xrCIY&list=RDx%26evil",
        ] {
            assert!(canonical_playlist_url(input).is_err(), "{input}");
        }
        assert!(!is_mix_url(
            &canonical_playlist_url("https://youtube.com/playlist?list=PL_test").unwrap()
        ));
    }

    #[tokio::test]
    async fn mix_metadata_requires_playlist_context_and_stays_bounded() {
        let row = format!(
            "{}\n",
            serde_json::json!({
                "id": "3s93p9xrCIY", "title": "Nightlife", "playlist_id": "RD3s93p9xrCIY",
                "playlist_title": "Mix - Nightlife", "playlist_index": 1
            })
        );
        let batch = row.repeat(MIX_TRACK_LIMIT as usize);
        let mut list = read_metadata(&mut BufReader::new(batch.as_bytes()), Some("RD3s93p9xrCIY"))
            .await
            .unwrap();
        assert_eq!(list.len, MIX_TRACK_LIMIT);
        assert_eq!(list.title, "Mix - Nightlife");
        assert_eq!(list.page(0).unwrap().len(), crate::queue::PAGE_SIZE);
        assert_eq!(list.track(0).unwrap().metadata.id, "3s93p9xrCIY");
        let too_many = row.repeat(MIX_TRACK_LIMIT as usize + 1);
        assert!(read_metadata(
            &mut BufReader::new(too_many.as_bytes()),
            Some("RD3s93p9xrCIY")
        )
        .await
        .err()
        .unwrap()
        .contains("limit"));
        for bad in [
            "{\"id\":\"3s93p9xrCIY\",\"title\":\"Single video fallback\"}\n",
            "{\"id\":\"3s93p9xrCIY\",\"playlist_id\":\"RDdifferent\"}\n",
        ] {
            assert!(
                read_metadata(&mut BufReader::new(bad.as_bytes()), Some("RD3s93p9xrCIY"))
                    .await
                    .is_err()
            );
        }
    }
}
