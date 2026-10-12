pub(crate) mod handoff;
pub(crate) mod media;
mod native;
mod state;
mod url_policy;

use std::sync::atomic::Ordering;

use serde::Deserialize;
use tauri::{
    webview::{NewWindowResponse, WebviewBuilder},
    AppHandle, Emitter, EventTarget, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl,
    Window,
};

use state::BrowserSnapshot;
pub use state::BrowserState;
pub use url_policy::is_local_url;
use url_policy::{address_or_search, is_browser_url};

const TOOLBAR_HEIGHT: f64 = 100.0;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserAction {
    Back,
    Forward,
    Reload,
}

pub fn publish(app: &AppHandle) {
    let snapshot = app.state::<BrowserState>().get();
    // Never broadcast browsing state to the remote child webview.
    for label in ["controller", "browser-toolbar"] {
        let _ = app.emit_to(EventTarget::webview(label), "browser-state", &snapshot);
    }
}

fn authorize(view: &Webview) -> Result<(), String> {
    if matches!(view.label(), "controller" | "browser-toolbar")
        && view.url().is_ok_and(|url| is_local_url(&url))
    {
        Ok(())
    } else {
        Err("Browser commands are available only to local controls.".into())
    }
}

#[tauri::command]
pub async fn browser_snapshot(webview: Webview, app: AppHandle) -> Result<BrowserSnapshot, String> {
    authorize(&webview)?;
    Ok(app.state::<BrowserState>().get())
}

struct OpeningGuard<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for OpeningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn open_browser(webview: Webview, app: AppHandle) -> Result<(), String> {
    authorize(&webview)?;
    reopen(&app).await
}

pub(crate) async fn reopen(app: &AppHandle) -> Result<(), String> {
    if crate::desktop::is_quitting(app) {
        return Err("MediaBrowser is quitting.".into());
    }
    let state = app.state::<BrowserState>();
    let _guard = state
        .transition
        .try_write()
        .map_err(|_| "A playback operation is already in progress.")?;
    crate::desktop::open_controller(app)?;
    handoff::return_to_browser(app).await
}

async fn open(app: &AppHandle, destination: Option<url::Url>) -> Result<(), String> {
    let state = app.state::<BrowserState>();
    if state
        .opening
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("The browser is already opening.".into());
    }
    let _guard = OpeningGuard(&state.opening);
    if let Some(window) = app.get_window("browser") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let window = Window::builder(app, "browser")
        .title("YouTube · MediaBrowser")
        .inner_size(1120., 800.)
        .min_inner_size(720., 480.)
        .theme(Some(tauri::Theme::Dark))
        .build()
        .map_err(|e| e.to_string())?;
    let result = build_views(app, &window, destination).await;
    if let Err(error) = result {
        let _ = window.close();
        return Err(error);
    }
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
            if let Some(window) = handle.get_window("browser") {
                resize(&handle, &window);
            }
        }
        tauri::WindowEvent::Destroyed => {
            handle.state::<BrowserState>().update(|s| {
                s.is_open = false;
                s.loading = false;
                s.can_go_back = false;
                s.can_go_forward = false;
            });
            publish(&handle);
        }
        _ => {}
    });
    state.update(|s| {
        s.is_open = true;
    });
    resize(app, &window);
    publish(app);
    Ok(())
}

async fn build_views(
    app: &AppHandle,
    window: &Window,
    destination: Option<url::Url>,
) -> Result<(), String> {
    window
        .add_child(
            WebviewBuilder::new(
                "browser-toolbar",
                WebviewUrl::App("index.html?view=browser".into()),
            )
            .on_navigation(is_local_url),
            LogicalPosition::new(0., 0.),
            LogicalSize::new(1120., TOOLBAR_HEIGHT),
        )
        .map_err(|e| e.to_string())?;
    let handle = app.clone();
    let remote = window
        .add_child(
            WebviewBuilder::new(
                "youtube",
                WebviewUrl::External("about:blank".parse().unwrap()),
            )
            .on_navigation(move |url| {
                let allowed = url.as_str() == "about:blank" || is_browser_url(url);
                if !allowed {
                    handle.state::<BrowserState>().update(|s| {
                        s.loading = false;
                        s.error = Some(
                            "This browser only opens YouTube and its sign-in or consent pages."
                                .into(),
                        );
                    });
                    publish(&handle);
                }
                allowed
            })
            .on_new_window(|_, _| NewWindowResponse::Deny)
            .on_download(|_, _| false),
            LogicalPosition::new(0., TOOLBAR_HEIGHT),
            LogicalSize::new(1120., 700.),
        )
        .map_err(|e| e.to_string())?;
    native::attach(&remote, app.clone()).await?;
    native::set_muted(&remote, destination.is_some()).await?;
    let url = match destination {
        Some(url) => url,
        None => address_or_search(&app.state::<BrowserState>().get().last_youtube_url)?,
    };
    remote.navigate(url).map_err(|e| e.to_string())?;
    Ok(())
}

fn resize(app: &AppHandle, window: &Window) {
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    if let Some(toolbar) = app.get_webview("browser-toolbar") {
        let _ = toolbar.set_size(LogicalSize::new(size.width, TOOLBAR_HEIGHT));
    }
    if let Some(remote) = app.get_webview("youtube") {
        let _ = remote.set_position(LogicalPosition::new(0., TOOLBAR_HEIGHT));
        let _ = remote.set_size(LogicalSize::new(
            size.width,
            (size.height - TOOLBAR_HEIGHT).max(1.),
        ));
    }
}

#[tauri::command]
pub async fn navigate_browser(
    webview: Webview,
    app: AppHandle,
    input: String,
) -> Result<(), String> {
    authorize(&webview)?;
    let url = address_or_search(&input)?;
    let state = app.state::<BrowserState>();
    let _guard = state
        .transition
        .try_write()
        .map_err(|_| "A playback operation is already in progress.")?;
    handoff::return_to_browser(&app).await?;
    app.get_webview("youtube")
        .ok_or("Open the browser first.")?
        .navigate(url)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn browser_action(
    webview: Webview,
    app: AppHandle,
    action: BrowserAction,
) -> Result<(), String> {
    authorize(&webview)?;
    let state = app.state::<BrowserState>();
    let _guard = state
        .transition
        .try_write()
        .map_err(|_| "A playback operation is already in progress.")?;
    let remote = app
        .get_webview("youtube")
        .ok_or("Open the browser first.")?;
    native::control(&remote, action).await
}
