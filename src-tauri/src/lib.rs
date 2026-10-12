pub mod audio;
mod browser;
mod desktop;
mod media_keys;
pub mod playback_state;
pub mod queue;
pub mod resolver;

use tauri::Manager;

pub fn run() {
    // Orchestration is light IO work. Decode/CPAL and synchronous HTTP bridges
    // already own dedicated threads; a CPU-count-sized Tokio pool wastes RAM.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("app-async")
        .enable_all()
        .build()
        .expect("failed to start app runtime");
    tauri::async_runtime::set(runtime.handle().clone());
    tauri::Builder::default()
        .setup(|app| {
            let state_path = desktop::data_dir(app.handle())?.join("browser-state.json");
            app.manage(browser::BrowserState::load(state_path));
            app.manage(audio::AudioEngine::new()?);
            desktop::setup(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            browser::browser_snapshot,
            browser::open_browser,
            browser::navigate_browser,
            browser::browser_action,
            browser::handoff::ram_saver,
            audio::commands::audio_load,
            audio::commands::audio_load_youtube,
            audio::commands::audio_play,
            audio::commands::audio_pause,
            audio::commands::audio_stop,
            audio::commands::audio_seek,
            audio::commands::audio_volume,
            audio::commands::audio_snapshot,
            audio::commands::audio_queue,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build MediaBrowser")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                // Last-window closure keeps the native tray/audio event loop alive.
                // Programmatic exit also goes through graceful shutdown first.
                if !desktop::exit_ready(app) {
                    api.prevent_exit();
                    if code.is_some() {
                        desktop::quit(app);
                    }
                }
            }
        });
}
