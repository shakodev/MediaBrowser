# MediaBrowser

A Windows YouTube music browser built with Rust, Tauri v2, WebView2, and a small vanilla TypeScript/Vite frontend.

## Run

Install Node.js 24+, Rust 1.97+ with the MSVC toolchain, the Visual Studio C++ build tools / Windows SDK, and the Microsoft Edge WebView2 Runtime. Ensure Cargo is on `PATH`.

```powershell
npm ci
npm run tauri dev
```

For a standalone Windows executable:

```powershell
npm run tauri build -- --no-bundle
```

The executable is `src-tauri/target/release/media-browser.exe`. Installer packaging is deliberately disabled for this stage. `npm run dev` alone shows the controller UI but cannot control a native browser.

## Current stage

- Dark controller with YouTube search, URL entry, and a current-page summary.
- Separate Windows browser with address/search bar, back, forward, reload, and home controls.
- Native WebView2 `SourceChanged`, `HistoryChanged`, and title events track both document navigation and YouTube SPA changes. No injected YouTube tracking script or polling loop is required.
- Last committed YouTube URL is atomically saved under `%APPDATA%/com.mediabrowser.desktop/browser-state.json` and restored when the browser is next opened. Consent and sign-in URLs never replace the saved YouTube URL.
- Closing the browser releases its views; reopening restores the saved page. Closing the controller exits the application.

## Structure and security

`src/ui` contains the controller and trusted toolbar; `src/api` is their typed command boundary. Rust browser code is split into window lifecycle/commands, native WebView2 integration, state storage, and URL policy under `src-tauri/src/browser`.

The browser window has two sibling webviews: a bundled `browser-toolbar` and remote `youtube` content. Only `controller` and `browser-toolbar` have a local Tauri capability. There are no window-wide grants or remote-origin permissions. All application commands are registered with the Tauri ACL manifest and also verify the caller's webview label and local URL. State events explicitly target local webviews. Local pages use a restrictive CSP and cannot navigate to remote content.

Top-level remote navigation is limited to HTTPS YouTube hosts and the exact Google sign-in/consent hosts listed in `url_policy.rs`. Other destinations, popups, and downloads are blocked. Embedded Google sign-in may be restricted by Google; popup-based authentication is not implemented.

`audio`, `resolver`, and `playback_state` expose interfaces/types only. RAM Saver and native transport controls are visibly unavailable. The future audio implementation will use CPAL, Symphonia, Reqwest, and Tokio; yt-dlp will resolve streams only. No external player or decoder process is included.

## Checks

```powershell
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Tests cover URL/search parsing, navigation boundaries, saved-page replacement/restoration, corrupt settings recovery, and the capability boundary. For live verification, open YouTube, search in both the toolbar and YouTube itself, follow a result, use back/forward/reload/home, then close and reopen the browser and app to confirm restoration. Check that the toolbar and controller reflect SPA URL changes.

Verified on Windows on 12 October 2026: frontend production build, Rust formatting, all five tests, and Clippy with warnings denied passed. Live checks passed for toolbar search, YouTube's own SPA search and result navigation, synchronized controller/toolbar URLs, back/forward/reload/home, rejection of `file:` addresses, browser close/reopen restoration, and controller exit closing both windows. The 3.1 MiB standalone release executable also launched and restored the saved YouTube search page using bundled local controls.

This stage does not implement audio-only playback, stream resolution, a queue, media keys, or installer distribution. Playback uses YouTube's own controls in Browser Mode. WebView2 and YouTube determine browser-process memory use; no RAM reduction target is claimed yet.
