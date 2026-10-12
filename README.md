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
- Local MP3, AAC (ADTS), and AAC-LC M4A playback through a custom Rust engine. Paste a file path into **Local playback**, load it, then use Play/Pause/Stop, volume, the position slider, or a seek time in seconds. Quoted Windows paths are accepted.

## Structure and security

`src/ui` contains the controller and trusted toolbar; `src/api` is their typed command boundary. Rust browser code is split into window lifecycle/commands, native WebView2 integration, state storage, and URL policy under `src-tauri/src/browser`.

The browser window has two sibling webviews: a bundled `browser-toolbar` and remote `youtube` content. Only `controller` and `browser-toolbar` have a local Tauri capability. There are no window-wide grants or remote-origin permissions. All application commands are registered with the Tauri ACL manifest and also verify the caller's webview label and local URL. State events explicitly target local webviews. Local pages use a restrictive CSP and cannot navigate to remote content.

Top-level remote navigation is limited to HTTPS YouTube hosts and the exact Google sign-in/consent hosts listed in `url_policy.rs`. Other destinations, popups, and downloads are blocked. Embedded Google sign-in may be restricted by Google; popup-based authentication is not implemented.

Audio commands have a separate controller-only capability and verify the caller's label and local origin. Remote YouTube content and the browser toolbar cannot open local audio files or control playback.

## Local audio engine

`src-tauri/src/audio` separates the async API, dedicated decoding worker, Symphonia decoder, streaming sample-rate conversion, CPAL output callback, and Tauri commands. One worker owns all file reads, decoder state, and device lifecycle. The CPAL callback only consumes a preallocated, lock-free SPSC PCM ring and publishes playback timing through a second bounded ring. It performs no decoding, IO, allocation, locking, logging, or waiting. An empty ring produces silence and does not advance media time.

The PCM ring holds 250 ms of stereo `f32` frames with source timestamps (about 188 KiB at 48 kHz). Only a bounded decoded packet and pending conversion block are retained; the engine does not decode or cache the whole song. Mono files are duplicated to stereo; stereo is averaged for mono devices. On multichannel output devices, only the first two channels are used.

Rust callers can clone `AudioEngine` and await `load(path)`, `play()`, `pause()`, `stop()`, `seek(seconds)`, `set_volume(0.0..=1.0)`, and `snapshot()`. Tokio oneshot replies bridge a bounded 16-command queue to the worker. A full queue returns a busy error immediately; dropping the last handle closes the worker and device. An accepted command continues even if its awaiting future is dropped. Tauri exposes the corresponding `audio_*` commands; `src/api/audio.ts` provides typed wrappers.

- Loading validates the file and leaves playback paused at zero without opening a device. Invalid loads preserve the existing track.
- Pause freezes the audible position and releases the output stream. Resume reopens the decoder at that position; Stop resets to zero and retains the loaded path. Play after natural end restarts at zero.
- Seek accepts finite non-negative seconds, clamps to a known duration, resets decoder history with preroll, trims to the requested sample, and replaces all queued PCM. Seeking while paused remains paused; seeking while playing continues playback. Invalid requests preserve transport state.
- Volume applies in the callback without restarting playback. Position comes from the source sample timestamps scheduled against CPAL's predicted device playback time, including device latency; it does not derive from time since pressing Play. Natural end waits for queued samples to reach the device.
- Decode/output failures release the device and expose an error snapshot. Play retries using the current default device. There is no automatic device switching.

This stage supports mono/stereo MP3 and AAC-LC in ADTS or ISO/MP4 containers. HE-AAC, ALAC, DRM, and multichannel source audio are unsupported. AAC encoder priming/padding and MP4 edit-list handling depend on Symphonia. Duration can be unknown or estimated for raw AAC or MP3 without timing metadata; the final decoded duration is available at EOF. The numeric seek control works when duration is unknown. Linear resampling maintains continuous timing across packets but is not a band-limited quality resampler. Position accuracy depends on the device driver's timestamps; seeks/pause can have short device restart latency.

`resolver` remains an interface only. RAM Saver, YouTube stream resolution, queues, and media keys are outside this stage. Browser Mode still uses YouTube's own playback controls independently; avoid playing both sources simultaneously. There is no MPV, FFmpeg, Electron, or external audio player.

## Checks

```powershell
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Tests cover URL/search parsing, navigation boundaries, saved-page replacement/restoration, corrupt settings recovery, and the capability boundary. Audio tests use small licensed MP3/AAC/M4A fixtures and a deterministic output backend to check decoded samples after forward/backward seeks, state transitions, pause position, discarded PCM, volume, underrun silence, device latency, EOF draining/replay, invalid inputs, failure recovery, and queue backpressure.

An opt-in Windows hardware check plays the three fixtures at 25% volume and measures WASAPI loopback from the default output endpoint. It keeps captured audio only in a bounded in-memory queue and prints signal statistics:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example audio_smoke
```

Verified for the audio stage on 12 October 2026: frontend build, Rust formatting, all 15 tests, and Clippy with warnings denied passed. The hardware check passed on **Headphones (TE-C), 48 kHz stereo**: silent baseline; nonzero MP3/AAC/M4A output peaks of approximately 0.188/0.213/0.213; final positions of 0.250000/0.325079/0.325079 seconds; pause, seek and stop checks passed. This verifies delivery to the Windows output endpoint. Physical headphone listening was not confirmed.

For live browser verification, open YouTube, search in both the toolbar and YouTube itself, follow a result, use back/forward/reload/home, then close and reopen the browser and app to confirm restoration. Check that the toolbar and controller reflect SPA URL changes.

The preceding Browser Mode stage was verified on Windows on 12 October 2026: live checks passed for toolbar search, YouTube's own SPA search and result navigation, synchronized controller/toolbar URLs, back/forward/reload/home, rejection of `file:` addresses, browser close/reopen restoration, and controller exit closing both windows. That stage's 3.1 MiB standalone release executable also launched and restored the saved YouTube search page using bundled local controls.

WebView2 and YouTube determine browser-process memory use; no RAM reduction target is claimed yet. Installer distribution remains outside this stage.
