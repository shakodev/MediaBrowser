<p align="center">
  <img src="src-tauri/icons/source.svg" alt="" width="80" height="80">
</p>

# MediaBrowser

**Browse YouTube. Keep the music. Free the RAM.**

MediaBrowser is a lightweight YouTube music player for Windows. Find something to listen to in the built-in browser, then switch to **RAM Saver** to continue listening through a custom Rust audio engine. Every WebView closes, leaving a small native player and system tray controls.

Built with Rust, Tauri v2, and WebView2, with a vanilla TypeScript frontend. Audio playback uses CPAL, Symphonia, Reqwest, and Tokio; yt-dlp resolves streams and playlist metadata. No Electron, MPV, FFmpeg, or external audio player is required.

[Getting started](#getting-started) · [Usage](#usage) · [Memory usage](#memory-usage) · [Development](#development) · [Known limitations](#known-limitations)

> Early development, Windows only. Build from source using the instructions below; installer packaging is currently disabled.

## Features

- **Browser Mode** — browse and search YouTube with an address bar, back/forward navigation, and a remembered last page.
- **RAM Saver** — transfer the current track, playback position, volume, and paused state to native audio playback. Return to Browser Mode when you need it.
- **Native playback controls** — play/pause, seek, volume, and previous/next without a web renderer. Close the player to keep listening from the tray.
- **Playlists and Mixes** — automatic advancement, track selection, shuffle, repeat, and a paged track list. Mixes load a snapshot of up to 50 tracks.
- **Windows media keys** — control native playback through Windows System Media Transport Controls (SMTC).
- **Session recovery** — saved RAM Saver sessions reopen paused in the native player, without starting WebView2.
- **Local audio** — play MP3 and AAC-LC files in ADTS (`.aac`) or M4A containers using the same audio engine.

## Getting started

### Prerequisites

To build and run MediaBrowser on Windows, install:

| Dependency | Requirement |
| --- | --- |
| [Node.js](https://nodejs.org/) | 24 or newer, with npm |
| [Rust](https://rustup.rs/) | 1.97 or newer, using the MSVC toolchain; Cargo on `PATH` |
| [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) | **Desktop development with C++**, including a Windows SDK |
| [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) | Required for Browser Mode |
| [yt-dlp](https://github.com/yt-dlp/yt-dlp#installation) | A current official Windows executable on `PATH` |
| [Deno](https://deno.com/) | 2.3 or newer on `PATH`, for YouTube challenge resolution |

yt-dlp and Deno are needed for YouTube audio playback and RAM Saver; local files do not need them. MediaBrowser does not install or update these dependencies. Use the official yt-dlp executable with bundled EJS scripts; see its [runtime setup guide](https://github.com/yt-dlp/yt-dlp/wiki/EJS). The [Tauri Windows prerequisites](https://v2.tauri.app/start/prerequisites/#windows) cover the native build setup.

### Run from source

```powershell
git clone https://github.com/shakodev/MediaBrowser.git
cd MediaBrowser
npm ci
npm run tauri dev
```

If yt-dlp or Deno is outside `PATH`, set its absolute executable path in the same PowerShell session before launching:

```powershell
$env:MEDIABROWSER_YTDLP = 'C:\Tools\yt-dlp.exe'
$env:MEDIABROWSER_DENO = 'C:\Tools\deno.exe'
npm run tauri dev
```

### Build a release executable

```powershell
npm run tauri build -- --no-bundle
.\src-tauri\target\release\media-browser.exe
```

The executable still requires WebView2 for browsing and yt-dlp/Deno for YouTube audio. `npm run dev` on its own starts only the frontend; use `npm run tauri dev` for the working desktop app.

## Usage

### Browse, then switch to RAM Saver

1. Open YouTube from the controller and play a public, non-live video or playlist track.
2. Click **RAM Saver** in the controller or browser toolbar.
3. Once native audio output is confirmed, the browser and controller close and the native player opens. Playback retains the current position, volume, and paused state.
4. Use **Browser** in the native player or **Reopen browser** in the tray to return to YouTube at the current track and position.

If a transfer fails, the current playback source remains available and the app reports an error. Switching modes can take a moment while the stream resolves and seeks.

Closing a window keeps the app in the tray. Double-click the tray icon to reopen the native player in RAM Saver. Choose **Quit** from the tray menu to stop playback and exit.

### Load a link or local file directly

The controller's **Audio playback** section also accepts:

- **A video URL:** paste a public YouTube watch, Shorts, or `youtu.be` URL, click **Load YouTube**, then **Play**.
- **A playlist or Mix:** paste its URL and click **Play playlist**. For a Mix, use the watch URL containing `list=RD…` to retain the seed video. Reload it for fresh recommendations when the finite queue ends.
- **A local file:** paste the full path, click **Load file**, then **Play**. Quoted Windows paths are accepted.

Loading audio directly keeps the controller open. Use the browser's RAM Saver handoff when you want to transfer browser playback and release all WebViews.

## Memory usage

RAM Saver destroys the browser and controller WebViews. The remaining process handles native controls, the tray, and audio playback. Audio is streamed on demand with bounded buffers; playlist metadata is stored in a temporary disk-backed queue.

Recorded release measurements on Windows 11:

| Run | Mode | Median private memory across the app's process tree |
| --- | --- | ---: |
| 12 Oct 2026, YouTube playback, earlier tray-only build | Browser Mode | 630.67 MiB |
| Same run | RAM Saver, first handoff | 7.94 MiB |
| 13 Oct 2026, local MP3 playback, native controls build | RAM Saver, controls open after restart | 6.035 MiB |

These are short measurements on one machine, not a fixed RAM requirement. The native-controls run used a local file; it is not a live YouTube comparison. Stream resolution temporarily adds yt-dlp/Deno processes. The [benchmark notes](docs/ram-saver.md#reproducing-the-benchmark) include the method, recorded results, and reproduction commands.

## Known limitations

- Native YouTube playback supports publicly resolvable, non-live AAC/M4A streams. Private, age-restricted, authenticated, DRM, HLS/DASH-manifest, and Opus/WebM-only streams are unsupported.
- Browser sign-in does not grant native playback access to restricted content. Watch Later and liked-video queues are unsupported; embedded Google sign-in may also be restricted.
- YouTube changes, ads, consent screens, and autoplay restrictions can prevent mode switching. Handoffs are not gapless, and position checks allow up to two seconds of startup drift.
- Mixes are finite snapshots, with no continuous radio replenishment or signed-in personalization. Playlist contents and shuffle order are rebuilt on restart.
- Local playback supports mono/stereo MP3 and AAC-LC. HE-AAC, ALAC, DRM, and multichannel source audio are unsupported. Audio device changes require a manual retry.

If a YouTube track fails to load or stops after about a minute, update the yt-dlp executable and Deno used by the app, then restart. Check `MEDIABROWSER_YTDLP` and `MEDIABROWSER_DENO` if you have multiple installations. More detail is in the [streaming notes](docs/technical-notes.md#youtube-audio-streams).

## Development

The frontend provides trusted local controls. Rust owns browser lifecycle, stream resolution, queues, persistence, and audio playback. Remote YouTube content has no Tauri permissions or filesystem access. Signed stream URLs and playback headers stay in Rust and are not saved in checkpoints.

| Path | Purpose |
| --- | --- |
| [`src/ui/`](src/ui/) | Controller, browser toolbar, and audio UI |
| [`src/api/`](src/api/) | Typed frontend command wrappers |
| [`src-tauri/src/browser/`](src-tauri/src/browser/) | WebView2 integration, navigation policy, and mode handoff |
| [`src-tauri/src/audio/`](src-tauri/src/audio/) | Streaming, decoding, resampling, and CPAL output |
| [`src-tauri/src/resolver/`](src-tauri/src/resolver/) | Bounded yt-dlp subprocesses and playlist extraction |
| [`src-tauri/src/queue/`](src-tauri/src/queue/) | Disk-backed playlists, traversal, and next-track resolution |
| [`src-tauri/src/playback_state/`](src-tauri/src/playback_state/) | Validated playback checkpoints and recovery |
| [`src-tauri/src/native_player.rs`](src-tauri/src/native_player.rs) | Native Win32 player controls |
| [`scripts/`](scripts/) | Process-tree checks and memory benchmarks |

### Checks

Run from the repository root after `npm ci`:

```powershell
npm run build
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Default tests use local fixtures and deterministic playback backends. Hardware and live YouTube checks are opt-in; see the [technical notes](docs/technical-notes.md#checks) and [streaming checks](docs/technical-notes.md#youtube-audio-streams).

### Contributing

Bug reports and focused pull requests are welcome. [Open an issue](https://github.com/shakodev/MediaBrowser/issues) with reproduction steps, your Windows version, dependency versions, and whether the problem occurs in Browser Mode or RAM Saver. For larger changes, open an issue first to discuss the approach.

Keep changes modular and mindful of memory use, and run the relevant checks above before submitting a pull request.

## Documentation

- [RAM Saver controls, session recovery, and benchmarks](docs/ram-saver.md)
- [Audio engine, handoff, security, and verification history](docs/technical-notes.md)
- [Audio fixture sources and license](src-tauri/tests/fixtures/README.md)

MediaBrowser is an independent project and is not affiliated with YouTube or Google.
