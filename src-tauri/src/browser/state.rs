use std::{
    fs,
    path::PathBuf,
    sync::{atomic::AtomicBool, Mutex},
};

use serde::{Deserialize, Serialize};

use super::url_policy::{is_browser_url, is_youtube_page, HOME};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub mode: crate::playback_state::PlaybackMode,
    pub switching: bool,
    pub current_url: String,
    pub last_youtube_url: String,
    pub title: String,
    pub is_open: bool,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub error: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct SavedPage {
    last_youtube_url: String,
}

pub struct BrowserState {
    pub transition: std::sync::Arc<tokio::sync::RwLock<()>>,
    pub snapshot: Mutex<BrowserSnapshot>,
    pub opening: AtomicBool,
    path: PathBuf,
}

impl BrowserState {
    pub fn load(path: PathBuf) -> Self {
        let restored = fs::read(&path)
            .ok()
            .filter(|bytes| bytes.len() <= 16384)
            .and_then(|bytes| serde_json::from_slice::<SavedPage>(&bytes).ok())
            .and_then(|saved| url::Url::parse(&saved.last_youtube_url).ok())
            .filter(is_youtube_page)
            .map(|url| url.to_string())
            .unwrap_or_else(|| HOME.to_owned());
        Self {
            transition: std::sync::Arc::new(tokio::sync::RwLock::new(())),
            snapshot: Mutex::new(BrowserSnapshot {
                mode: crate::playback_state::PlaybackMode::Browser,
                switching: false,
                current_url: restored.clone(),
                last_youtube_url: restored,
                title: "YouTube".into(),
                is_open: false,
                loading: false,
                can_go_back: false,
                can_go_forward: false,
                error: None,
            }),
            opening: AtomicBool::new(false),
            path,
        }
    }

    pub fn get(&self) -> BrowserSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn update(&self, update: impl FnOnce(&mut BrowserSnapshot)) -> BrowserSnapshot {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        update(&mut snapshot);
        snapshot.clone()
    }

    pub fn observe_url(&self, url: &url::Url) -> BrowserSnapshot {
        if !is_browser_url(url) {
            return self.get();
        }
        self.update(|state| {
            state.current_url = url.to_string();
            if is_youtube_page(url) && state.last_youtube_url != url.as_str() {
                state.last_youtube_url = url.to_string();
                if let Err(error) = self.save(&state.last_youtube_url) {
                    state.error = Some(format!("Could not remember this page: {error}"));
                }
            }
        })
    }

    fn save(&self, url: &str) -> Result<(), Box<dyn std::error::Error>> {
        let parent = self.path.parent().ok_or("Missing settings directory")?;
        fs::create_dir_all(parent)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec(&SavedPage {
                last_youtube_url: url.into(),
            })?,
        )?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_last_committed_page_and_recovers_from_corrupt_settings() {
        let folder = std::env::temp_dir().join(format!(
            "media-browser-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = folder.join("browser-state.json");
        let state = BrowserState::load(path.clone());
        assert_eq!(state.get().current_url, HOME);
        for url in [
            "https://www.youtube.com/watch?v=first",
            "https://www.youtube.com/watch?v=second",
        ] {
            state.observe_url(&url.parse().unwrap());
            assert!(state.get().error.is_none());
        }
        state.observe_url(&"https://consent.youtube.com/m".parse().unwrap());
        assert_eq!(
            BrowserState::load(path.clone()).get().last_youtube_url,
            "https://www.youtube.com/watch?v=second"
        );
        fs::write(&path, b"broken settings").unwrap();
        assert_eq!(
            BrowserState::load(path.clone()).get().last_youtube_url,
            HOME
        );
        fs::write(&path, br#"{"last_youtube_url":"file:///C:/private"}"#).unwrap();
        assert_eq!(BrowserState::load(path).get().last_youtube_url, HOME);
        fs::remove_dir_all(folder).unwrap();
    }
}
