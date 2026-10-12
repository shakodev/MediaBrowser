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
    Url::parse(&format!("https://www.youtube.com/playlist?list={id}")).map_err(|_| invalid())
}

#[derive(Deserialize)]
struct FlatTrack {
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
fn append(playlist: &mut Playlist, line: &[u8]) -> Result<(), String> {
    let flat: FlatTrack =
        serde_json::from_slice(line).map_err(|_| "Invalid flat playlist metadata.")?;
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
async fn read_metadata(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Playlist, String> {
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
        append(&mut playlist, &line)?;
    }
    Ok(playlist)
}

impl YtDlpResolver {
    pub async fn playlist(&self, url: &Url) -> Result<Playlist, String> {
        let url = canonical_playlist_url(url.as_str())?;
        // Project a dictionary before encoding: individual missing %(field)j values
        // print the bare NA placeholder, which is not valid JSON.
        let template = "%(.{id,title,uploader,duration,availability,playlist_title})j";
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
                "--",
                url.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| {
            "Cannot launch yt-dlp. Install it on PATH or set MEDIABROWSER_YTDLP.".to_owned()
        })?;
        let mut reader =
            BufReader::new(child.stdout.take().ok_or("Missing playlist output pipe.")?);
        let playlist = read_metadata(&mut reader).await?;
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
            let mut list = read_metadata(&mut BufReader::new(json.as_bytes())).await.unwrap();
            let track = list.track(0).unwrap();
            assert_eq!(track.metadata.title.chars().count(), 200);
            assert_eq!(track.metadata.duration_seconds, None);
            assert_eq!(track.metadata.uploader, None);
            assert!(!track.unavailable);
            let huge = vec![b'x'; 16385];
            for bad in [huge.as_slice(), b"{broken}\n", b"{}"] {
                assert!(read_metadata(&mut BufReader::new(bad)).await.is_err());
            }
            assert_eq!(read_metadata(&mut BufReader::new(b"".as_slice())).await.unwrap().len, 0);
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
        append(&mut list, br#"{"id":"BaW_jenozKc","title":"[Private video]","duration":"NA","playlist_title":"Test"}"#).unwrap();
        append(
            &mut list,
            br#"{"id":"aqz-KE-bpKQ","title":"Playable","duration":15}"#,
        )
        .unwrap();
        assert_eq!(list.len, 2);
        assert!(list.track(0).unwrap().unavailable);
        assert_eq!(list.track(1).unwrap().metadata.duration_seconds, Some(15.0));
    }
}
