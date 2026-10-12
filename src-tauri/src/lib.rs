pub mod audio;
pub mod playback_state;
pub mod resolver;

pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to run MediaBrowser");
}
