//! All COM access runs on the WebView2 UI thread. Native events observe full and
//! same-document (YouTube SPA) navigation without a remote JavaScript bridge.

use tauri::{AppHandle, Manager, Webview};
use webview2_com::{
    DocumentTitleChangedEventHandler, HistoryChangedEventHandler,
    Microsoft::Web::WebView2::Win32::ICoreWebView2, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, SourceChangedEventHandler,
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
    webview.with_webview(move |platform| {
        let result = (|| -> windows::core::Result<()> {
            // SAFETY: Tauri executes with_webview on the owning UI thread.
            unsafe {
                let core = platform.controller().CoreWebView2()?;
                let mut token = 0;
                let handle = app.clone();
                core.add_SourceChanged(&SourceChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(core) = sender { refresh(&handle, &core)?; }
                    Ok(())
                })), &mut token)?;
                let handle = app.clone();
                core.add_HistoryChanged(&HistoryChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(core) = sender { refresh(&handle, &core)?; }
                    Ok(())
                })), &mut token)?;
                let handle = app.clone();
                core.add_DocumentTitleChanged(&DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(core) = sender { refresh(&handle, &core)?; }
                    Ok(())
                })), &mut token)?;
                let handle = app.clone();
                core.add_NavigationStarting(&NavigationStartingEventHandler::create(Box::new(move |_, _| {
                    handle.state::<BrowserState>().update(|s| { s.loading = true; s.error = None; });
                    publish(&handle);
                    Ok(())
                })), &mut token)?;
                let handle = app.clone();
                core.add_NavigationCompleted(&NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                    let mut success = BOOL::default();
                    if let Some(args) = args { args.IsSuccess(&mut success)?; }
                    handle.state::<BrowserState>().update(|s| {
                        s.loading = false;
                        if !success.as_bool() && s.error.is_none() {
                            s.error = Some("The page could not be loaded. Check your connection and reload.".into());
                        }
                    });
                    if let Some(core) = sender { refresh(&handle, &core)?; }
                    publish(&handle);
                    Ok(())
                })), &mut token)?;
                refresh(&app, &core)?;
            }
            Ok(())
        })();
        let _ = sender.send(result.map_err(|e| e.to_string()));
    }).map_err(|e| e.to_string())?;
    receiver.await.map_err(|e| e.to_string())?
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
