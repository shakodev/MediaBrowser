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
- Closing either window destroys its views and leaves the native tray available. Use tray **Quit** to terminate the app.
- Local MP3, AAC (ADTS), and AAC-LC M4A playback through a custom Rust engine. Paste a file path into **Local playback**, load it, then use Play/Pause/Stop, volume, the position slider, or a seek time in seconds. Quoted Windows paths are accepted.
- YouTube playlist and Mix/radio playback with automatic advancement, previous/next, track selection, shuffle, repeat off/all/one, and a paged track list with current-track metadata.
- RAM Saver buttons in the controller and browser toolbar hand the current public video or playlist track to the Rust engine. Browser Mode restores the current audio track and position.
- RAM Saver destroys every WebView window. A native Windows tray provides play/pause, next/previous, volume, browser reopening and clean Quit; Windows media keys use SMTC.
- Playback checkpoints restore RAM Saver paused without starting WebView2. See [tray lifecycle, persistence, benchmark instructions and limitations](docs/ram-saver.md).

## RAM Saver handoff

Open a public YouTube video, then press **RAM Saver** in either window. Rust requests a small, on-demand playback snapshot: video ID, public `list` ID, one-based `index`, current media timestamp, effective volume (zero when muted), and paused state. Native WebView2 navigation events continue to track SPA changes. The snapshot checks the current URL against the loaded player identity; ads, live streams, loading players and stale SPA state fail safely. The remote webview has no Tauri permissions or application message bridge. The integration does not read cookies or expose filesystem access or audio stream URLs. This integration reads only the current player and HTML media element, never YouTube's internal autoplay queue.

For a playlist, yt-dlp extracts public flat metadata into the existing disk-backed queue. The captured index and video ID select the exact occurrence; original playlist indexes are retained when provided by yt-dlp. If hidden entries shifted extraction order, a unique video ID can recover the track. Mix/radio (`RD`) URLs load a snapshot of up to 50 recommendations around the current watch video. Mix handoff locates the current video by ID because regenerated recommendations can change indexes. Missing videos or ambiguous duplicates produce an error. The initial transfer never plays a different track or skips an unsuccessful current track. Watch Later and liked-video queues are rejected; single videos remain single tracks.

Resolution runs while browser playback continues. A fresh capture then rechecks identity and timestamp. The Rust engine seeks and starts muted, with queue advancement suspended. The transfer requires its device-timed playback clock to advance before pausing YouTube, applying volume and closing the remote WebView2 view and browser window. Paused browser playback stays paused after this output check. Navigation, seeks or playback changes during preparation, resolver/network/decoder/device failures, or a failed confirmation leave Browser Mode active and show an error. A failed commit pauses Rust and resumes browser media if the transfer paused it.

Choose **Reopen browser** from the tray (or double-click it) to restore the current Rust track, public playlist, original index, position, volume and paused state. This uses the current queue entry, including tracks reached with Next/Previous or automatic advancement. The new WebView2 is muted at the native level during restoration, including any ads. Rust continues playing until YouTube confirms the target video, seek and playback progress; only then is Rust paused and the browser unmuted. On failure the new browser is closed and Rust remains available. A stopped or paused Rust track restores paused. Replacing audio with a local file restores the remembered browser page instead.

Mix restoration keeps the radio ID and current video, omitting the snapshot's index so YouTube can regenerate its recommendations. Mix checkpoints likewise rebuild recommendations around the saved current track on restart; the exact recommendation order is not preserved.

MVP limitations: only publicly resolvable, non-live videos and playlists supported by the existing AAC/M4A engine; no authenticated extraction, internal autoplay queue, YouTube shuffle/repeat synchronization, or seamless/gapless switching. YouTube DOM/player changes, consent, ads, autoplay restrictions, changed playlists and slow seeks can prevent a transfer; the current source remains available for retry. Position checks allow up to two seconds of startup drift. Checkpoints retain track/playlist context and shuffle/repeat preferences, but playlist contents and shuffle order are rebuilt on restart.

Automated checks cover capture and SPA races, ads/unsupported pages, playback-context validation, duplicate/shifted playlist indexes, muted startup and timestamped PCM output, output confirmation, restoration state and local-only ACLs. The release benchmark verifies live Windows/YouTube round-trip handoff, zero retained WebViews, native playback progress and process-tree exit.

## Playlists

Paste a public YouTube playlist URL (or a watch URL containing `list=`) into the playlist field and press **Play playlist**. The app extracts flat metadata first, then starts the first playable track. Select any track from the list; use the page controls to browse long playlists or **Show current track** to locate playback. Pause and Stop suspend automatic advancement. Loading a single video or local file replaces the queue.

YouTube Mix/radio links work in the same field, for example `https://www.youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY&start_radio=1`. Use the watch URL to retain the seed video; plain `playlist?list=RD<video-id>` links also work. Mixes load up to 50 tracks into a finite queue, with a 120-second extraction deadline. Playback stops at the end unless repeat is enabled. Reload the Mix for fresh recommendations; continuous radio replenishment and signed-in personalization are not implemented. Regular playlists keep their full extraction behavior.

`resolver/playlist.rs` runs yt-dlp with `--flat-playlist --lazy-playlist --skip-download` and a compact JSON projection, reading one line at a time with a 16 KiB cap and a 60-second inactivity timeout. Missing optional fields and unavailable entries are retained. The Rust queue stores metadata and failure flags in a temporary file using fixed 4 KiB records, with no growing in-memory index. The file uses Windows delete-on-close and is released when replacing the queue or exiting. Disk usage grows with track count; free disk space is required. Loading waits for metadata extraction to complete before playback starts.

Rust and the UI retain at most one page of 40 tracks per snapshot. Shuffle computes an affine permutation with a coprime stride instead of allocating a playlist-sized array; every entry is visited once per pass, and Previous retraces that order. This offers fewer possible orders than a full Fisher–Yates shuffle. Changing shuffle preserves the current track. Repeat One applies to automatic completion; manual Next still advances.

Signed audio URLs remain in Rust. Only the current track and one cancellable next-track resolution are retained; prefetch downloads no audio. Prefetched results expire after 60 seconds and are resolved again on demand. Queue changes, pause, stop, seek, replacement and shutdown cancel or discard speculative work. Existing HTTP URL refresh handles rejected/expired playback URLs.

Unavailable entries and resolution/decoding failures are marked and skipped in the requested direction. Cancellation does not mark tracks failed. Repeat cannot create an endless all-unavailable loop: recovery stops after one unsuccessful pass and leaves the queue available for manual retry. YouTube may hide some unavailable entries entirely; only metadata returned by yt-dlp can appear. Playback retains the single-video engine's public, non-live AAC/M4A restrictions. Transitions are not gapless. RAM Saver uses this same queue and checkpoints its public source, current occurrence and playback preferences.

Metadata-only live verification (no audio resolution or playback):

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example playlist_smoke -- 'https://www.youtube.com/playlist?list=YOUR_PLAYLIST_ID'
cargo run --manifest-path src-tauri/Cargo.toml --example playlist_smoke -- 'https://www.youtube.com/watch?v=3s93p9xrCIY&list=RD3s93p9xrCIY&start_radio=1'
```

Verified on 12 October 2026 with yt-dlp 2026.08.19: the public **Data Analysis with Dr Mike Pound** playlist produced 11 indexed tracks, with successful first-page and final-record reads. Tests cover sequential/shuffled traversal, repeat, manual selection, backward failure recovery, automatic advancement, unavailable resolution and runtime failures, retry, cancellation, prefetch expiry/reaping, metadata limits and 10,000-track bounded paging. Live playlist audio transitions have not been manually listened to.

Mix metadata verified on 12 October 2026 using the example above through the Rust resolver: **Mix - Chiwa - Nightlife** returned exactly 50 tracks, with 40 on the first page and a successful final-record read. Regression tests cover seed preservation, RD variants, missing/mismatched Mix metadata, extraction bounds, browser capture, radio restoration and checkpoint context. Mix audio handoff and transitions have not been manually listened to.

## Structure and security

`src/ui` contains the controller and trusted toolbar; `src/api` is their typed command boundary. Rust browser code is split into window lifecycle/commands, native WebView2 integration, state storage, and URL policy under `src-tauri/src/browser`.

The browser window has two sibling webviews: a bundled `browser-toolbar` and remote `youtube` content. Only `controller` and `browser-toolbar` have a local Tauri capability. There are no window-wide grants or remote-origin permissions. All application commands are registered with the Tauri ACL manifest and also verify the caller's webview label and local URL. State events explicitly target local webviews. Local pages use a restrictive CSP and cannot navigate to remote content.

Top-level remote navigation is limited to HTTPS YouTube hosts and the exact Google sign-in/consent hosts listed in `url_policy.rs`. Other destinations, popups, and downloads are blocked. Embedded Google sign-in may be restricted by Google; popup-based authentication is not implemented.

Local-file and YouTube audio commands have a separate controller-only capability and verify the caller's label and local origin. Remote YouTube content has no application permissions. The trusted browser toolbar can request the narrow RAM Saver handoff; direct audio and local-file commands remain controller-only.

## Custom audio engine

`src-tauri/src/audio` separates the async API, dedicated decoding worker, Symphonia decoder, streaming sample-rate conversion, CPAL output callback, and Tauri commands. One worker owns all file reads, decoder state, and device lifecycle. The CPAL callback only consumes a preallocated, lock-free SPSC PCM ring and publishes playback timing through a second bounded ring. It performs no decoding, IO, allocation, locking, logging, or waiting. An empty ring produces silence and does not advance media time.

The PCM ring holds 250 ms of stereo `f32` frames with source timestamps (about 188 KiB at 48 kHz). Only a bounded decoded packet and pending conversion block are retained; the engine does not decode or cache the whole song. Mono files are duplicated to stereo; stereo is averaged for mono devices. On multichannel output devices, only the first two channels are used.

Rust callers can clone `AudioEngine` and await `load(path)`, `play()`, `pause()`, `stop()`, `seek(seconds)`, `set_volume(0.0..=1.0)`, and `snapshot()`. Tokio oneshot replies bridge a bounded 16-command queue to the worker. A full queue returns a busy error immediately; dropping the last handle closes the worker and device. An accepted command continues if its awaiting future is dropped. A newer load, pause, stop, or seek cancels earlier network/resolver work immediately; queued requests that share the cancelled token report that they were superseded. Queue rejection does not cancel playback. Tauri exposes the corresponding `audio_*` commands; `src/api/audio.ts` provides typed wrappers.

- Loading validates the file and leaves playback paused at zero without opening a device. Invalid loads preserve the existing track.
- Pause freezes the audible position and releases the output stream. Resume reopens the decoder at that position; Stop resets to zero and retains the loaded path. Play after natural end restarts at zero.
- Seek accepts finite non-negative seconds, clamps to a known duration, resets decoder history with preroll, trims to the requested sample, and replaces all queued PCM. Seeking while paused remains paused; seeking while playing continues playback. Invalid requests preserve transport state.
- Volume applies in the callback without restarting playback. Position comes from the source sample timestamps scheduled against CPAL's predicted device playback time, including device latency; it does not derive from time since pressing Play. Natural end waits for queued samples to reach the device.
- Decode/output failures release the device and expose an error snapshot. Play retries using the current default device. There is no automatic device switching.

This stage supports mono/stereo MP3 and AAC-LC in ADTS or ISO/MP4 containers. HE-AAC, ALAC, DRM, and multichannel source audio are unsupported. AAC encoder priming/padding and MP4 edit-list handling depend on Symphonia. Duration can be unknown or estimated for raw AAC or MP3 without timing metadata; the final decoded duration is available at EOF. The numeric seek control works when duration is unknown. Linear resampling maintains continuous timing across packets but is not a band-limited quality resampler. Position accuracy depends on the device driver's timestamps; seeks/pause can have short device restart latency.

RAM Saver coordinates browser and engine playback as described above. Windows SMTC routes media keys to the native engine while in RAM Saver. Direct audio-load controls remain available independently; use RAM Saver when transferring browser playback. There is no MPV, FFmpeg, Electron, or external audio player.

## YouTube audio streams

Install a current official **yt-dlp executable** and **Deno 2.3+**, place both on `PATH`, and restart the app. Alternatively set `MEDIABROWSER_YTDLP` and `MEDIABROWSER_DENO` to absolute executable paths before launching. Missing or unlaunchable dependencies produce actionable errors. The official yt-dlp executable bundles its EJS challenge scripts; Python installations need the matching `yt-dlp[default]` dependencies. See the [official runtime/EJS setup guide](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

Paste a public YouTube watch, shorts, or youtu.be video URL into **Audio playback → Load YouTube**, then press Play. The title and transport use the same controller as local files. Stop stays available during loading so extraction or network IO can be cancelled. Use RAM Saver to transfer and close Browser Mode; loading a URL directly is independent.

`resolver` runs yt-dlp with `Command` argument arrays, no shell, ignored user configuration, no playlist, and no media download or postprocessing. It prefers AAC-LC M4A direct HTTPS audio, then HTTP M4A or HTTPS AAC. Typed JSON must describe audio-only, non-live, unencrypted compatible media. JSON output is limited to 4 MiB, stderr to 64 KiB, and each subprocess to 60 seconds; cancellation kills the resolver. Only classified errors escape stderr. Signed stream URLs and playback headers stay in Rust, have no Debug/Serialize implementation, and are never logged, persisted, or sent to the frontend. The snapshot includes the canonical public video URL, metadata, and aggregate transfer counters.

`audio/http.rs` is a seekable Symphonia media source using Reqwest and a current-thread Tokio runtime on the decoding worker. It retains one 128 KiB range, validates exact HTTP 206 / Content-Range and stable resource length, rejects ignored ranges or redirects, and retries interrupted transfers at most twice. Reads happen on demand; there is no disk cache or whole-song prefetch. The existing 250 ms PCM ring and 64 KiB Symphonia read buffer remain bounded. Container indexes/metadata and subprocess runtime memory are additional; no total process RAM target is claimed.

Fast-start M4A uses progressive Symphonia initialization, then enables byte-range seeking, avoiding its eager scan of every MP4 fragment. Tail-metadata MP4 uses normal seekable probing. Symphonia still discovers fragmented MP4 seek indexes incrementally, so distant seeks can require reading intervening fragment headers and portions of media. Seeking/resuming reopens the stream at the audible position with decoder preroll. Empty buffers produce silence without moving media time. HTTP 401/403/410 triggers one re-resolution per range request and retries the same byte offset only if video ID, format, and length match; repeated failures or changed formats expose a safe retry/reload error. Reopening an expired stored URL can therefore resolve again.

Private, sign-in/age/region restricted, live, DRM, HLS/DASH manifests, Opus/WebM-only videos, and streams requiring authentication or PO tokens are unsupported. YouTube can reject resolved URLs; update yt-dlp/Deno or try another public video. Dependencies are detected, not installed or updated by the app.

An opt-in Windows check resolves a public video, plays a few seconds through the custom engine at 25% volume, measures WASAPI loopback, seeks, checks pause/resume and stop, and asserts transferred audio bytes are less than the resource length:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example youtube_smoke
# Optional public video URL as the first argument; choose a long enough video.
```

Verified on 12 October 2026 with yt-dlp **2026.08.19**, Deno **2.9.7**, public [Big Buck Bunny video](https://www.youtube.com/watch?v=aqz-KE-bpKQ), and **Headphones (TE-C), 48 kHz stereo**. The successful run transferred **1,048,576 / 10,271,496 audio bytes** across **8 range requests**, with a **128 KiB** retained range limit. Position advanced to **5.019 seconds** before seeking; playback/seek/resume loopback peaks were **0.009924 / 0.066173 / 0.071406**. Pause and stop passed. No audio file was created. This confirms output at the Windows endpoint; physical listening was not confirmed. The older installed yt-dlp 2026.07.04 produced rejected stream URLs. Portable verification dependencies are kept only in ignored `.verification/` and do not configure the app automatically.

The streaming stage also passes the frontend production build, Rust formatting, **26 Rust tests** (including controller ACL checks), and Clippy for all targets with warnings denied. Regression coverage includes bounded ranges, forward/backward byte seeking, ignored/truncated/wrong ranges, cancellation and queue backpressure, expired URL recovery and format changes, remote decoding/transport, dependency failures, JSON validation, subprocess output limits, and avoiding eager MP4 body scans.

## Checks

```powershell
npm run build
npm test
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

Measured process-tree private memory and the reproducible release benchmark are documented in [RAM Saver](docs/ram-saver.md). Installer distribution remains outside this stage.
