fn main() {
    // Application commands must opt in to ACL permissions, just like plugin commands.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "browser_snapshot",
            "open_browser",
            "navigate_browser",
            "browser_action",
            "audio_load",
            "audio_load_youtube",
            "audio_play",
            "audio_pause",
            "audio_stop",
            "audio_seek",
            "audio_volume",
            "audio_snapshot",
            "audio_queue",
        ]),
    ))
    .expect("failed to build Tauri application metadata");
}
