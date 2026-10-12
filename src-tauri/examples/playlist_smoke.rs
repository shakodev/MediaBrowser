//! Opt-in metadata-only verification; never resolves or downloads audio.
use media_browser_lib::resolver::{playlist::canonical_playlist_url, YtDlpResolver};
fn main() -> Result<(), String> {
    let input = std::env::args()
        .nth(1)
        .ok_or("Pass a public YouTube playlist URL.")?;
    let url = canonical_playlist_url(&input)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let mut playlist = runtime.block_on(YtDlpResolver::default().playlist(&url))?;
    let first_page_len = playlist.page(0)?.len();
    let last_title = playlist.track(playlist.len - 1)?.metadata.title;
    println!(
        "{}: {} tracks; first page {} tracks; last title: {}",
        playlist.title, playlist.len, first_page_len, last_title
    );
    Ok(())
}
