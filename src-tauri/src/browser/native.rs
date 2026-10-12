//! All COM access runs on the WebView2 UI thread. Native events observe full and
//! same-document (YouTube SPA) navigation without a remote JavaScript bridge.

use std::{cell::Cell, rc::Rc};

use tauri::{AppHandle, Manager, Webview};
use webview2_com::{
    DocumentTitleChangedEventHandler, HistoryChangedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
    },
    NavigationCompletedEventHandler, NavigationStartingEventHandler, SourceChangedEventHandler,
};
use windows::core::{BOOL, PWSTR};

use super::{publish, BrowserAction, BrowserState};

fn refresh(app: &AppHandle, core: &ICoreWebView2) -> windows::core::Result<()> {
    let mut source = PWSTR::null();
    let mut title = PWSTR::null();
    let mut back = BOOL::default();
    let mut forward = BOOL::default();
    // SAFETY: these interfaces and their event callbacks remain on the UI thread.
    unsafe {
        core.Source(&mut source)?;
        let source = webview2_com::take_pwstr(source);
        core.DocumentTitle(&mut title)?;
        let title = webview2_com::take_pwstr(title);
        core.CanGoBack(&mut back)?;
        core.CanGoForward(&mut forward)?;
        let state = app.state::<BrowserState>();
        if let Ok(url) = url::Url::parse(&source) {
            state.observe_url(&url);
        }
        state.update(|snapshot| {
            snapshot.title = title;
            snapshot.can_go_back = back.as_bool();
            snapshot.can_go_forward = forward.as_bool();
        });
    }
    publish(app);
    Ok(())
}

pub async fn attach(webview: &Webview, app: AppHandle) -> Result<(), String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |platform| {
            // SAFETY: Tauri executes with_webview on the owning UI thread.
            let result = unsafe { platform.controller().CoreWebView2() }
                .and_then(|core| register_events(&app, &core));
            let _ = sender.send(result.map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
    receiver.await.map_err(|e| e.to_string())?
}

fn register_events(app: &AppHandle, core: &ICoreWebView2) -> windows::core::Result<()> {
    let mut token = 0;
    // Rc is created here and captured only by callbacks on the owning UI thread.
    let navigation_id = Rc::new(Cell::new(0_u64));
    // WebView2 owns these handlers until its view is destroyed. No COM handle is
    // captured by a handler, avoiding a COM object/handler reference cycle.
    unsafe {
        let handle = app.clone();
        core.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(core) = sender {
                    refresh(&handle, &core)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let handle = app.clone();
        core.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(core) = sender {
                    refresh(&handle, &core)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let handle = app.clone();
        core.add_DocumentTitleChanged(
            &DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                if let Some(core) = sender {
                    refresh(&handle, &core)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let handle = app.clone();
        let current_id = navigation_id.clone();
        core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = webview2_com::take_pwstr(uri);
                    // Preserve the URL policy's error when Wry cancels a blocked URL.
                    if !url::Url::parse(&uri)
                        .is_ok_and(|url| super::url_policy::is_browser_url(&url))
                    {
                        return Ok(());
                    }
                    let mut id = 0;
                    args.NavigationId(&mut id)?;
                    current_id.set(id);
                }
                handle.state::<BrowserState>().update(|s| {
                    s.loading = true;
                    s.error = None;
                });
                publish(&handle);
                Ok(())
            })),
            &mut token,
        )?;
        let handle = app.clone();
        core.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                let mut success = BOOL::default();
                let mut status = Default::default();
                if let Some(args) = args {
                    let mut id = 0;
                    args.NavigationId(&mut id)?;
                    if id != navigation_id.get() {
                        return Ok(());
                    }
                    args.IsSuccess(&mut success)?;
                    args.WebErrorStatus(&mut status)?;
                }
                handle.state::<BrowserState>().update(|s| {
                    s.loading = false;
                    if !success.as_bool()
                        && status != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED
                        && s.error.is_none()
                    {
                        s.error = Some(
                            "The page could not be loaded. Check your connection and reload."
                                .into(),
                        );
                    }
                });
                if let Some(core) = sender {
                    refresh(&handle, &core)?;
                } else {
                    publish(&handle);
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    refresh(app, core)
}

pub async fn control(webview: &Webview, action: BrowserAction) -> Result<(), String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |platform| {
            // SAFETY: dispatched to the WebView2 owning UI thread by Tauri.
            let result = unsafe {
                platform
                    .controller()
                    .CoreWebView2()
                    .and_then(|core| match action {
                        BrowserAction::Back => core.GoBack(),
                        BrowserAction::Forward => core.GoForward(),
                        BrowserAction::Reload => core.Reload(),
                    })
            };
            let _ = sender.send(result.map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
    receiver.await.map_err(|e| e.to_string())?
}
