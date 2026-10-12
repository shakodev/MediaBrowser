//! Windows SMTC routes media keys to the selected media session, without global
//! hotkey grabs. Its native host survives destruction of every WebView window.
use crate::{
    desktop::{self, Action},
    playback_state::{PlaybackSnapshot, PlaybackStatus},
};
use std::sync::Mutex;
use tauri::AppHandle;
use windows::{
    core::{factory, HSTRING},
    Foundation::TypedEventHandler,
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    },
    Win32::System::WinRT::ISystemMediaTransportControlsInterop,
};

pub(crate) struct MediaKeys {
    controls: SystemMediaTransportControls,
    token: i64,
    last: Mutex<Option<SessionState>>,
}

#[derive(PartialEq)]
struct SessionState {
    enabled: bool,
    status: PlaybackStatus,
    queue: bool,
    title: String,
    artist: String,
}

pub(crate) fn setup(app: &AppHandle) -> Result<MediaKeys, String> {
    let host = tauri::Window::builder(app, "media-session")
        .title("MediaBrowser media session")
        .visible(false)
        .skip_taskbar(true)
        .build()
        .map_err(|e| e.to_string())?;
    let result = (|| -> windows::core::Result<MediaKeys> {
        let interop: ISystemMediaTransportControlsInterop =
            factory::<SystemMediaTransportControls, _>()?;
        // SAFETY: valid native HWND created above on the Tauri UI thread; this
        // host remains owned by the app until after the session is disabled.
        let controls: SystemMediaTransportControls = unsafe {
            interop.GetForWindow(
                host.hwnd()
                    .map_err(|_| windows::core::Error::from_thread())?,
            )?
        };
        controls.SetIsEnabled(false)?;
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        let app = app.clone();
        let token = controls.ButtonPressed(&TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                if let Some(action) = action(args.Button()?) {
                    desktop::dispatch(&app, action);
                }
            }
            Ok(())
        }))?;
        Ok(MediaKeys {
            controls,
            token,
            last: Mutex::new(None),
        })
    })();
    if result.is_err() {
        let _ = host.destroy();
    }
    result.map_err(|e| e.to_string())
}

fn action(button: SystemMediaTransportControlsButton) -> Option<Action> {
    match button {
        SystemMediaTransportControlsButton::Play => Some(Action::Play),
        SystemMediaTransportControlsButton::Pause => Some(Action::Pause),
        SystemMediaTransportControlsButton::Next => Some(Action::Next),
        SystemMediaTransportControlsButton::Previous => Some(Action::Previous),
        _ => None,
    }
}

impl MediaKeys {
    pub fn update(&self, snapshot: &PlaybackSnapshot, enabled: bool) -> Result<(), String> {
        self.update_inner(snapshot, enabled)
            .map_err(|e| e.to_string())
    }
    fn update_inner(
        &self,
        snapshot: &PlaybackSnapshot,
        enabled: bool,
    ) -> windows::core::Result<()> {
        let title = snapshot
            .metadata
            .as_ref()
            .map(|m| m.title.clone())
            .unwrap_or_else(|| "MediaBrowser".into());
        let queue = snapshot.queue.as_ref().is_some_and(|q| q.total > 0);
        let artist = snapshot
            .metadata
            .as_ref()
            .and_then(|m| m.uploader.as_deref())
            .unwrap_or("");
        let key = SessionState {
            enabled,
            status: snapshot.status,
            queue,
            title: title.clone(),
            artist: artist.into(),
        };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() == Some(&key) {
            return Ok(());
        }
        self.controls.SetIsEnabled(enabled)?;
        self.controls.SetPlaybackStatus(match snapshot.status {
            PlaybackStatus::Playing => MediaPlaybackStatus::Playing,
            PlaybackStatus::Paused => MediaPlaybackStatus::Paused,
            PlaybackStatus::Loading => MediaPlaybackStatus::Changing,
            _ => MediaPlaybackStatus::Stopped,
        })?;
        self.controls.SetIsNextEnabled(queue)?;
        self.controls.SetIsPreviousEnabled(queue)?;
        let display = self.controls.DisplayUpdater()?;
        display.SetType(MediaPlaybackType::Music)?;
        let properties = display.MusicProperties()?;
        properties.SetTitle(&HSTRING::from(title))?;
        properties.SetArtist(&HSTRING::from(
            snapshot
                .metadata
                .as_ref()
                .and_then(|m| m.uploader.as_deref())
                .unwrap_or(""),
        ))?;
        display.Update()?;
        *last = Some(key);
        Ok(())
    }
    pub fn close(&self) {
        let _ = self.controls.SetIsEnabled(false);
        let _ = self.controls.RemoveButtonPressed(self.token);
    }
}
impl Drop for MediaKeys {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_buttons_map_to_explicit_transport_actions() {
        assert!(matches!(
            action(SystemMediaTransportControlsButton::Play),
            Some(Action::Play)
        ));
        assert!(matches!(
            action(SystemMediaTransportControlsButton::Pause),
            Some(Action::Pause)
        ));
        assert!(matches!(
            action(SystemMediaTransportControlsButton::Next),
            Some(Action::Next)
        ));
        assert!(matches!(
            action(SystemMediaTransportControlsButton::Previous),
            Some(Action::Previous)
        ));
        assert!(action(SystemMediaTransportControlsButton::FastForward).is_none());
    }
}
