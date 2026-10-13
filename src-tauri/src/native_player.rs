//! Small Win32 controls on a Tauri Window, with no WebView, renderer or timer.
//! All HWNDs and subclass data stay on the existing UI thread. Desktop's one
//! second snapshot tick supplies progress; gestures use its bounded action queue.
use crate::{
    browser::BrowserState,
    desktop::{self, Action},
    playback_state::{PlaybackMode, PlaybackSnapshot, PlaybackStatus},
};
use std::cell::RefCell;
use tauri::{AppHandle, Manager};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{
            CreateFontW, DeleteObject, GetSysColor, CLIP_DEFAULT_PRECIS, COLOR_BTNFACE,
            DEFAULT_CHARSET, DEFAULT_QUALITY, HFONT, OUT_DEFAULT_PRECIS,
        },
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{EnableWindow, GetKeyState, SetFocus, VK_SHIFT, VK_TAB},
            Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
};

const LABEL: &str = "ram-saver";
const SUBCLASS: usize = 1;
const SEEK_STEPS: i32 = 10_000;
// commctrl.h defines TBM_GETPOS as WM_USER (omitted by windows metadata).
const TBM_GETPOS: u32 = WM_USER;
// winuser.h static-control style: display metadata ampersands literally.
const SS_NOPREFIX: u32 = 0x80;
const PREVIOUS: usize = 101;
const TOGGLE: usize = 102;
const NEXT: usize = 103;
const BROWSER: usize = 104;

#[derive(Clone, Default)]
struct Model {
    title: String,
    detail: String,
    path: Option<String>,
    occurrence: Option<u64>,
    duration: Option<f64>,
    position: f64,
    volume: f32,
    playing: bool,
    queue: bool,
    active: bool,
}
impl Model {
    fn from_snapshot(app: &AppHandle, snapshot: &PlaybackSnapshot) -> Self {
        let browser = app.state::<BrowserState>().get();
        Self {
            title: snapshot
                .metadata
                .as_ref()
                .map(|m| m.title.clone())
                .or_else(|| {
                    snapshot
                        .path
                        .as_ref()
                        .and_then(|p| std::path::Path::new(p).file_name())
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "MediaBrowser".into()),
            detail: browser
                .error
                .or_else(|| snapshot.error.clone())
                .unwrap_or_else(|| {
                    let artist = snapshot
                        .metadata
                        .as_ref()
                        .and_then(|m| m.uploader.as_deref())
                        .unwrap_or("RAM Saver");
                    format!("{artist} · {:?}", snapshot.status)
                }),
            path: snapshot.path.clone(),
            occurrence: snapshot.queue.as_ref().and_then(|q| q.current),
            duration: valid_duration(snapshot.duration_seconds),
            position: snapshot.position_seconds,
            volume: snapshot.volume,
            playing: snapshot.status == PlaybackStatus::Playing,
            queue: snapshot.queue.as_ref().is_some_and(|q| q.total > 0),
            active: browser.mode == PlaybackMode::RamSaver && !browser.switching,
        }
    }
}

fn valid_duration(value: Option<f64>) -> Option<f64> {
    value.filter(|d| d.is_finite() && *d > 0.0 && *d <= 1e9)
}
fn time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The window is created on the event-loop thread; errors leave tray playback usable.
pub(crate) fn show(app: &AppHandle) -> Result<(), String> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if desktop::is_quitting(&handle)
            || handle.state::<BrowserState>().get().mode != PlaybackMode::RamSaver
        {
            return;
        }
        if let Err(error) = create(&handle) {
            desktop::report(
                &handle,
                &format!("RAM Saver window unavailable: {error}. Tray controls still work."),
            );
        }
    })
    .map_err(|e| e.to_string())
}

fn create(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_window(LABEL) {
        window.show().map_err(|e| e.to_string())?;
        return window.set_focus().map_err(|e| e.to_string());
    }
    // Match standard controls, including the user's system contrast colors.
    let background = unsafe { GetSysColor(COLOR_BTNFACE) };
    let window = tauri::Window::builder(app, LABEL)
        .title("RAM Saver · MediaBrowser")
        .inner_size(460.0, 264.0)
        .resizable(false)
        .maximizable(false)
        .theme(Some(tauri::Theme::Light))
        .background_color(tauri::window::Color(
            background as u8,
            (background >> 8) as u8,
            (background >> 16) as u8,
            255,
        ))
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    let result = window.hwnd().map_err(|e| e.to_string()).and_then(|hwnd| {
        // SAFETY: creation/subclassing runs on the HWND's owning UI thread.
        unsafe { Player::install(hwnd, app.clone()).map_err(|e| e.to_string()) }
    });
    if let Err(error) = result {
        let _ = window.destroy();
        return Err(error);
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())
}

pub(crate) fn close(app: &AppHandle) {
    if let Some(window) = app.get_window(LABEL) {
        let _ = window.destroy();
    }
}

pub(crate) fn update(app: &AppHandle, snapshot: &PlaybackSnapshot) {
    if app.get_window(LABEL).is_none() {
        return;
    }
    let model = Model::from_snapshot(app, snapshot);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_window(LABEL) else {
            return;
        };
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        // SAFETY: the registered subclass owns this Box until WM_NCDESTROY,
        // and lookup/use are synchronous on the same thread as destruction.
        unsafe {
            let mut data = 0;
            if GetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, Some(&mut data)).as_bool() {
                let cell = &*(data as *const RefCell<Player>);
                if let Ok(mut player) = cell.try_borrow_mut() {
                    player.refresh(model);
                }
            }
        }
    });
}

struct Player {
    app: AppHandle,
    hwnd: HWND,
    title: HWND,
    detail: HWND,
    seek_label: HWND,
    seek: HWND,
    elapsed: HWND,
    previous: HWND,
    toggle: HWND,
    next: HWND,
    browser: HWND,
    volume_label: HWND,
    volume: HWND,
    font: HFONT,
    model: Model,
    seeking: bool,
    changing_volume: bool,
}

impl Player {
    unsafe fn install(hwnd: HWND, app: AppHandle) -> windows::core::Result<()> {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        })
        .ok()?;
        let child = |class: PCWSTR, label: &str, id: usize, style: WINDOW_STYLE| {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                &HSTRING::from(label),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                1,
                1,
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )
        };
        let title = child(
            w!("STATIC"),
            "Loading playback…",
            0,
            WINDOW_STYLE(SS_NOPREFIX),
        )?;
        let detail = child(w!("STATIC"), "RAM Saver", 0, WINDOW_STYLE(SS_NOPREFIX))?;
        let seek_label = child(
            w!("STATIC"),
            "Playback position",
            0,
            WINDOW_STYLE(SS_NOPREFIX),
        )?;
        let seek = child(
            TRACKBAR_CLASSW,
            "Playback position",
            105,
            WS_TABSTOP | WINDOW_STYLE(TBS_NOTICKS),
        )?;
        let elapsed = child(w!("STATIC"), "0:00 / --:--", 0, WINDOW_STYLE(SS_NOPREFIX))?;
        let previous = child(w!("BUTTON"), "Previous", PREVIOUS, WS_TABSTOP)?;
        let toggle = child(w!("BUTTON"), "Play", TOGGLE, WS_TABSTOP)?;
        let next = child(w!("BUTTON"), "Next", NEXT, WS_TABSTOP)?;
        let browser = child(w!("BUTTON"), "Browser", BROWSER, WS_TABSTOP)?;
        let volume_label = child(w!("STATIC"), "Volume", 0, WINDOW_STYLE(SS_NOPREFIX))?;
        let volume = child(
            TRACKBAR_CLASSW,
            "Volume",
            106,
            WS_TABSTOP | WINDOW_STYLE(TBS_NOTICKS),
        )?;
        for control in [seek, previous, toggle, next, browser, volume] {
            SetWindowSubclass(control, Some(keyboard_proc), SUBCLASS, hwnd.0 as usize).ok()?;
        }
        send(seek, TBM_SETRANGEMAX, 0, SEEK_STEPS as isize);
        send(seek, TBM_SETPAGESIZE, 0, 500);
        send(volume, TBM_SETRANGEMAX, 0, 100);
        send(volume, TBM_SETPAGESIZE, 0, 10);
        let mut player = Self {
            app,
            hwnd,
            title,
            detail,
            seek_label,
            seek,
            elapsed,
            previous,
            toggle,
            next,
            browser,
            volume_label,
            volume,
            font: HFONT::default(),
            model: Model::default(),
            seeking: false,
            changing_volume: false,
        };
        player.layout();
        player.refresh(Model {
            title: "Loading playback…".into(),
            detail: "RAM Saver".into(),
            ..Default::default()
        });
        let data = Box::into_raw(Box::new(RefCell::new(player)));
        if !SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, data as usize).as_bool() {
            drop(Box::from_raw(data));
            return Err(windows::core::Error::from_thread());
        }
        Ok(())
    }

    unsafe fn layout(&mut self) {
        let scale = GetDpiForWindow(self.hwnd).max(96) as f64 / 96.0;
        let px = |n: i32| (n as f64 * scale).round() as i32;
        let font = CreateFontW(
            -px(14),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            0,
            w!("Segoe UI"),
        );
        let controls = [
            (self.title, 20, 16, 420, 25),
            (self.detail, 20, 44, 420, 40),
            (self.seek_label, 20, 84, 420, 20),
            (self.seek, 16, 104, 428, 28),
            (self.elapsed, 20, 134, 420, 22),
            (self.previous, 20, 165, 96, 34),
            (self.toggle, 128, 165, 96, 34),
            (self.next, 236, 165, 96, 34),
            (self.browser, 344, 165, 96, 34),
            (self.volume_label, 20, 227, 104, 24),
            (self.volume, 126, 223, 318, 30),
        ];
        for (hwnd, x, y, width, height) in controls {
            let _ = MoveWindow(hwnd, px(x), px(y), px(width), px(height), true);
            if !font.is_invalid() {
                send(hwnd, WM_SETFONT, font.0 as usize, 1);
            }
        }
        if !font.is_invalid() {
            if !self.font.is_invalid() {
                let _ = DeleteObject(self.font.into());
            }
            self.font = font;
        }
    }

    unsafe fn refresh(&mut self, model: Model) {
        // Freeze track identity/duration until the gesture ends. The worker
        // validates it atomically even if the queue changes before dispatch.
        if self.seeking {
            self.model.active = model.active;
        } else {
            self.model = model.clone();
            if let Some(duration) = model.duration {
                send(
                    self.seek,
                    TBM_SETLINESIZE,
                    0,
                    (5.0 / duration * SEEK_STEPS as f64).round().max(1.0) as isize,
                );
                send(
                    self.seek,
                    TBM_SETPAGESIZE,
                    0,
                    (30.0 / duration * SEEK_STEPS as f64).round().max(1.0) as isize,
                );
            }
            let pos = model.duration.map_or(0, |d| {
                ((model.position / d).clamp(0.0, 1.0) * SEEK_STEPS as f64).round() as i32
            });
            send(self.seek, TBM_SETPOS, 1, pos as isize);
            self.show_time(model.position);
        }
        set_text(self.title, &model.title);
        set_text(self.detail, &model.detail);
        set_text(self.toggle, if model.playing { "Pause" } else { "Play" });
        let loaded = model.active && model.path.is_some();
        let _ = EnableWindow(self.toggle, loaded);
        let _ = EnableWindow(self.previous, model.active && model.queue);
        let _ = EnableWindow(self.next, model.active && model.queue);
        let _ = EnableWindow(self.seek, loaded && model.duration.is_some());
        let _ = EnableWindow(self.volume, model.active);
        if !self.changing_volume {
            let value = (model.volume * 100.0).round() as i32;
            send(self.volume, TBM_SETPOS, 1, value as isize);
            set_text(self.volume_label, &format!("Volume {value}%"));
        }
    }

    unsafe fn show_time(&self, position: f64) {
        let duration = self
            .model
            .duration
            .map(time)
            .unwrap_or_else(|| "--:--".into());
        set_text(self.elapsed, &format!("{} / {duration}", time(position)));
    }

    unsafe fn scroll(&mut self, control: HWND, code: u32) {
        if !self.model.active {
            return;
        }
        let value = send(control, TBM_GETPOS, 0, 0).0 as i32;
        if control == self.seek {
            let Some(duration) = self.model.duration else {
                return;
            };
            let seconds = value as f64 / SEEK_STEPS as f64 * duration;
            if code == TB_ENDTRACK {
                if self.seeking {
                    self.seeking = false;
                    if let Some(path) = self.model.path.clone() {
                        desktop::dispatch(
                            &self.app,
                            Action::Seek {
                                seconds,
                                path,
                                occurrence: self.model.occurrence,
                            },
                        );
                    }
                }
            } else {
                self.seeking = true;
                self.show_time(seconds);
            }
        } else if control == self.volume {
            set_text(self.volume_label, &format!("Volume {value}%"));
            if code == TB_ENDTRACK {
                if self.changing_volume {
                    self.changing_volume = false;
                    desktop::dispatch(&self.app, Action::SetVolume(value as f32 / 100.0));
                }
            } else {
                self.changing_volume = true;
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // WM_NCDESTROY follows child destruction; no control still uses the font.
        if !self.font.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.font.into());
            }
        }
    }
}

unsafe fn send(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> LRESULT {
    SendMessageW(hwnd, message, Some(WPARAM(wparam)), Some(LPARAM(lparam)))
}
unsafe fn set_text(hwnd: HWND, text: &str) {
    // Avoid repainting unchanged labels every second.
    let text: String = text.chars().filter(|c| *c != '\0').take(512).collect();
    let mut old = [0u16; 1024];
    let len = GetWindowTextW(hwnd, &mut old) as usize;
    if String::from_utf16_lossy(&old[..len]) != text {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(text));
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // Owned by this subclass, only accessed on the UI thread. RefCell prevents
    // aliasing if a native control sends a synchronous notification during update.
    if message == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(window_proc), SUBCLASS);
        drop(Box::from_raw(data as *mut RefCell<Player>));
        return DefSubclassProc(hwnd, message, wparam, lparam);
    }
    let cell = &*(data as *const RefCell<Player>);
    if let Ok(mut player) = cell.try_borrow_mut() {
        match message {
            WM_COMMAND if (wparam.0 >> 16) as u32 == BN_CLICKED => {
                let action = match wparam.0 & 0xffff {
                    PREVIOUS => Some(Action::Previous),
                    TOGGLE => Some(Action::Toggle),
                    NEXT => Some(Action::Next),
                    BROWSER => Some(Action::Reopen),
                    _ => None,
                };
                if let Some(action) = action {
                    desktop::dispatch(&player.app, action);
                    return LRESULT(0);
                }
            }
            WM_HSCROLL => {
                player.scroll(HWND(lparam.0 as *mut _), (wparam.0 & 0xffff) as u32);
                return LRESULT(0);
            }
            WM_SIZE => player.layout(),
            WM_SETFOCUS => {
                let _ = SetFocus(Some(player.browser));
            }
            _ => {}
        }
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    parent: usize,
) -> LRESULT {
    if message == WM_KEYDOWN && wparam.0 == VK_TAB.0 as usize {
        if let Ok(next) = GetNextDlgTabItem(
            HWND(parent as *mut _),
            Some(hwnd),
            GetKeyState(VK_SHIFT.0 as i32) < 0,
        ) {
            let _ = SetFocus(Some(next));
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(keyboard_proc), SUBCLASS);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_handles_unknown_and_invalid_durations_and_long_tracks() {
        for value in [
            None,
            Some(0.0),
            Some(-1.0),
            Some(f64::INFINITY),
            Some(f64::NAN),
        ] {
            assert_eq!(valid_duration(value), None);
        }
        assert_eq!(valid_duration(Some(7200.5)), Some(7200.5));
        assert_eq!(time(65.9), "1:05");
        assert_eq!(time(3661.0), "1:01:01");
    }
}
