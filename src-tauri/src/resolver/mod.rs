//! yt-dlp resolves public videos only. It never transfers or transforms media.
pub mod playlist;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap, future::Future, path::PathBuf, pin::Pin, process::Stdio, time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMetadata {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_seconds: Option<f64>,
}

// Deliberately no Debug or Serialize: signed URLs and request headers are secrets.
#[derive(Clone)]
pub struct ResolvedStream {
    pub media_url: Url,
    pub headers: HeaderMap,
    pub format_id: String,
    pub extension: String,
    pub metadata: VideoMetadata,
}
pub type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ResolvedStream, String>> + Send + 'a>>;
pub trait StreamResolver: Send + Sync {
    fn resolve<'a>(&'a self, youtube_url: &'a Url) -> ResolveFuture<'a>;
}

#[derive(Clone)]
pub struct YtDlpResolver {
    pub executable: PathBuf,
    pub deno: PathBuf,
}
impl Default for YtDlpResolver {
    fn default() -> Self {
        Self {
            executable: std::env::var_os("MEDIABROWSER_YTDLP")
                .unwrap_or_else(|| "yt-dlp".into())
                .into(),
            deno: std::env::var_os("MEDIABROWSER_DENO")
                .unwrap_or_else(|| "deno".into())
                .into(),
        }
    }
}
impl StreamResolver for YtDlpResolver {
    fn resolve<'a>(&'a self, youtube_url: &'a Url) -> ResolveFuture<'a> {
        Box::pin(async move {
            let url = canonical_video_url(youtube_url.as_str())?;
            let version =
                subprocess(&self.executable, &["--version".into()], "yt-dlp", 4096).await?;
            if version.is_empty() {
                return Err(
                    "yt-dlp returned no version; reinstall the official executable.".into(),
                );
            }
            let deno = subprocess(&self.deno, &["--version".into()], "Deno", 4096).await?;
            let version = String::from_utf8_lossy(&deno);
            let supported = version
                .split_whitespace()
                .nth(1)
                .and_then(|s| {
                    let mut parts = s.split('.');
                    Some((
                        parts.next()?.parse::<u32>().ok()?,
                        parts.next()?.parse::<u32>().ok()?,
                    ))
                })
                .is_some_and(|(major, minor)| major > 2 || (major == 2 && minor >= 3));
            if !supported {
                return Err(
                    "Deno 2.3 or newer is required; update Deno and restart the app.".into(),
                );
            }
            let args = vec![
                "--ignore-config".into(), "--no-playlist".into(), "--skip-download".into(),
                "--dump-single-json".into(), "--no-warnings".into(), "--no-progress".into(),
                "--no-js-runtimes".into(), "--js-runtimes".into(), format!("deno:{}", self.deno.display()),
                "--socket-timeout".into(), "15".into(), "--retries".into(), "1".into(),
                "--format".into(), "bestaudio[ext=m4a][acodec^=mp4a.40.2][protocol=https]/bestaudio[ext=m4a][acodec^=mp4a.40.2][protocol=http]/bestaudio[ext=aac][protocol=https]".into(),
                "--".into(), url.to_string(),
            ];
            parse(&subprocess(&self.executable, &args, "yt-dlp", 4 * 1024 * 1024).await?)
        })
    }
}

pub fn canonical_video_url(input: &str) -> Result<Url, String> {
    let invalid =
        || "Enter a public HTTPS YouTube video URL (watch, shorts, or youtu.be).".to_owned();
    let url = Url::parse(input).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(invalid());
    }
    let id = match url.host_str().unwrap_or("") {
        "youtu.be" => url.path().strip_prefix('/').map(str::to_owned),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            if url.path() == "/watch" {
                url.query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.into_owned())
            } else {
                url.path().strip_prefix("/shorts/").map(str::to_owned)
            }
        }
        _ => None,
    }
    .ok_or_else(invalid)?;
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid());
    }
    Url::parse(&format!("https://www.youtube.com/watch?v={id}")).map_err(|_| invalid())
}

async fn subprocess(
    path: &PathBuf,
    args: &[String],
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound { format!("Missing {name}. Install it on PATH or set MEDIABROWSER_{} to its executable path, then restart the app.", if name == "Deno" { "DENO" } else { "YTDLP" }) }
        else { format!("Cannot launch {name}; check its executable path and permissions.") }
    })?;
    let stdout = child.stdout.take().ok_or("Missing resolver output pipe.")?;
    let stderr = child.stderr.take().ok_or("Missing resolver error pipe.")?;
    let read = async {
        let output = async {
            let mut data = Vec::new();
            stdout.take(limit + 1).read_to_end(&mut data).await?;
            Ok::<_, std::io::Error>(data)
        };
        let errors = async {
            let mut data = Vec::new();
            stderr.take(65537).read_to_end(&mut data).await?;
            Ok::<_, std::io::Error>(data)
        };
        let (data, errors) = tokio::try_join!(output, errors)?;
        if data.len() as u64 > limit || errors.len() > 65536 {
            return Err(std::io::Error::other("Resolver output limit exceeded"));
        }
        Ok::<_, std::io::Error>((data, errors, child.wait().await?))
    };
    let result = tokio::time::timeout(Duration::from_secs(60), read).await;
    match result {
        Ok(Ok((data, _, status))) if status.success() => Ok(data),
        Ok(Ok((_, errors, _))) => {
            // Classify only; never forward raw stderr (which can contain signed URLs).
            let message = String::from_utf8_lossy(&errors).to_lowercase();
            if message.contains("format") && message.contains("available") {
                Err("No compatible AAC/M4A HTTP audio stream is available for this video. Update yt-dlp or choose another public video.".into())
            } else if message.contains("challenge")
                || message.contains("javascript")
                || message.contains("deno")
            {
                Err("YouTube challenge resolution failed. Update yt-dlp and Deno; use the official yt-dlp executable with bundled EJS scripts.".into())
            } else {
                Err(format!("{name} failed. The video may be private, restricted, unavailable, or blocked by YouTube. Update yt-dlp and Deno and try another public video."))
            }
        }
        other => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(if other.is_err() {
                format!("{name} timed out after 60 seconds.")
            } else {
                format!("{name} output exceeded its limit or could not be read.")
            })
        }
    }
}

#[derive(Deserialize)]
struct Info {
    id: String,
    title: String,
    uploader: Option<String>,
    duration: Option<f64>,
    url: String,
    format_id: String,
    ext: String,
    acodec: String,
    vcodec: String,
    protocol: String,
    #[serde(default)]
    http_headers: HashMap<String, String>,
    #[serde(default)]
    is_live: bool,
    #[serde(default)]
    has_drm: bool,
}
fn parse(json: &[u8]) -> Result<ResolvedStream, String> {
    let info: Info = serde_json::from_slice(json).map_err(|_| {
        "yt-dlp returned invalid or incomplete stream JSON; update yt-dlp.".to_owned()
    })?;
    let media_url = Url::parse(&info.url).map_err(|_| "yt-dlp returned an invalid media URL.")?;
    if !matches!(media_url.scheme(), "http" | "https")
        || !media_url.username().is_empty()
        || media_url.password().is_some()
        || !matches!(info.protocol.as_str(), "http" | "https")
        || info.vcodec != "none"
        || info.is_live
        || info.has_drm
        || !((info.ext == "m4a" && info.acodec == "mp4a.40.2")
            || (info.ext == "aac" && matches!(info.acodec.as_str(), "aac" | "mp4a.40.2")))
    {
        return Err(
            "Only non-live, unencrypted AAC-LC audio-only HTTP streams are supported.".into(),
        );
    }
    let mut headers = HeaderMap::new();
    for (name, value) in info.http_headers {
        // Only playback headers; never accept Host, Range, encoding, auth, or cookies.
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "user-agent" | "referer" | "origin" | "accept" | "accept-language"
        ) {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| "Invalid stream header.")?,
                HeaderValue::from_str(&value).map_err(|_| "Invalid stream header.")?,
            );
        }
    }
    Ok(ResolvedStream {
        media_url,
        headers,
        format_id: info.format_id,
        extension: info.ext,
        metadata: VideoMetadata {
            id: info.id,
            title: info.title,
            uploader: info.uploader,
            duration_seconds: info.duration.filter(|d| d.is_finite() && *d > 0.0),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_missing_dependencies_and_bounds_subprocess_output() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let missing = std::env::temp_dir().join("mediabrowser-missing-dependency-98f71.exe");
            for name in ["yt-dlp", "Deno"] {
                let error = subprocess(&missing, &[], name, 4096).await.unwrap_err();
                assert!(error.contains(&format!("Missing {name}")));
                assert!(error.contains("MEDIABROWSER_"));
            }
            let exe = std::env::current_exe().unwrap();
            let error = subprocess(&exe, &["--list".into()], "test", 16)
                .await
                .unwrap_err();
            assert!(error.contains("limit"));
        });
    }
    #[test]
    fn validates_and_canonicalizes_video_urls() {
        assert_eq!(
            canonical_video_url("https://youtu.be/BaW_jenozKc?t=3&list=x")
                .unwrap()
                .as_str(),
            "https://www.youtube.com/watch?v=BaW_jenozKc"
        );
        for url in [
            "--exec=bad",
            "https://youtube.com.evil/watch?v=BaW_jenozKc",
            "https://user@youtube.com/watch?v=BaW_jenozKc",
            "https://youtube.com/playlist?list=x",
            "http://youtu.be/BaW_jenozKc",
            "https://youtu.be/BaW_jenozKc/more",
        ] {
            assert!(canonical_video_url(url).is_err());
        }
    }
    #[test]
    fn json_is_typed_and_errors_never_echo_secrets() {
        let good = br#"{"id":"BaW_jenozKc","title":"test","uploader":null,"duration":10,"url":"https://example.com/audio?sig=SECRET","format_id":"140","ext":"m4a","acodec":"mp4a.40.2","vcodec":"none","protocol":"https"}"#;
        assert!(parse(good).is_ok());
        for bad in [
            String::from_utf8_lossy(good).replace("mp4a.40.2", "opus"),
            String::from_utf8_lossy(good).replace("https://example", "file://example"),
            "SECRET".to_owned(),
        ] {
            let error = parse(bad.as_bytes()).err().unwrap();
            assert!(!error.contains("SECRET"));
        }
    }
}
