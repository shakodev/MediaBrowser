pub mod audio;
mod browser;
pub mod playback_state;
pub mod resolver;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let state_path = app.path().app_data_dir()?.join("browser-state.json");
            app.manage(browser::BrowserState::load(state_path));
            app.manage(audio::AudioEngine::new()?);
            tauri::WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])?
                .on_navigation(browser::is_local_url)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            browser::browser_snapshot,
            browser::open_browser,
            browser::navigate_browser,
            browser::browser_action,
            audio::commands::audio_load,
            audio::commands::audio_play,
            audio::commands::audio_pause,
            audio::commands::audio_stop,
            audio::commands::audio_seek,
            audio::commands::audio_volume,
            audio::commands::audio_snapshot,
        ])
        .on_window_event(|window, event| {
            if window.label() == "controller" && matches!(event, tauri::WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run MediaBrowser");
}
