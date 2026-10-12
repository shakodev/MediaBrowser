# Tray-only RAM Saver

RAM Saver transfers the current public YouTube video to the existing Rust audio
engine, confirms that audio reaches the output device, pauses YouTube, then
destroys both the browser window (including its toolbar) and controller window.
It never hides a WebView to simulate a memory saving. The Tauri event loop stays
alive with a native system tray and a plain, invisible HWND for Windows SMTC.
That HWND has no WebView2 instance.

The tray provides Play/Pause, Next, Previous, Volume ±10%, Reopen browser, RAM
Saver, and Quit. Next/Previous are enabled for public playlist queues. Native
transport is disabled while the YouTube browser owns playback. Windows selects
which media session receives hardware media keys; MediaBrowser does not globally
grab those keys from other applications. The session exposes title, artist and
playback status. If SMTC initialization fails, the tray remains usable and shows
the error.

Closing windows leaves the tray available. Explicit Quit cancels active IO and
resolver work, joins the audio worker (dropping CPAL, the decoder, prefetch and
temporary playlist records), writes a final checkpoint, disables SMTC and exits
Tauri. Resolver processes belong to kill-on-close Windows jobs, which also contain
their Deno descendants. Tauri's last-window exit request is prevented; explicit
exit is allowed only after cleanup completes.

## Persistence and recovery

`playback-state.json` lives in Tauri's application data directory next to the
existing `browser-state.json`. It contains public video/playlist identity and
occurrence, position, volume, mode, shuffle and repeat preferences, or a local
file path and position. Signed stream URLs, request headers and cookies are never
saved. Records are versioned, validated, capped at 16 KiB before parsing and
atomically replaced after flushing. Corrupt records show an error and leave the
controller available.

Periodic checkpoints run about every ten seconds while responsive, and after
native transport changes, during handoff and at Quit. Transitions or slow IO can
delay them; a crash loses changes since the last successful checkpoint. RAM Saver
restarts without creating any WebViews, resolves streams afresh and restores
**paused**; use tray Play to resume. A failed network/file restore opens the
controller for recovery. Browser Mode restores the saved public watch URL and
timestamp when reopened. Playlist contents are fetched again, so removed videos
or changed/ambiguous occurrences can prevent restoration. Shuffle/repeat settings
survive restart; the randomized traversal order itself is regenerated.

## Reproducing the benchmark

### Measured on 12 October 2026

Release build on Windows 11 build 26200, Ryzen 5 2600X (12 logical processors),
WebView2 154.0.4258.62, Rust 1.97.1, yt-dlp 2026.08.19 and Deno 2.9.7. The public
Big Buck Bunny video played through actual browser/native output. Each measured
phase lasted 15 seconds after eight seconds of settling.

| Mode | Median total private memory | Steady process count |
| --- | ---: | ---: |
| Browser Mode | 630.67 MiB | 11 |
| Controller only, Rust audio playing | 139.09 MiB | 7 |
| RAM Saver, first handoff | 7.94 MiB | 1 |
| RAM Saver, controller closed again | 7.86 MiB | 1 |
| Browser reopened | 637.16 MiB | 11 |
| RAM Saver, subsequent handoff | 8.57 MiB | 1 |
| Fresh tray-only restart, audio resumed | 6.45 MiB | 1 |

The main bottleneck was the controller keeping WebView2 alive: destroying it
removed about **131 MiB** within the final run. Capping the orchestration runtime
at two workers reduced first-handoff private memory from **9.46 MiB** in the
initial run to **7.94 MiB**, without changing audio buffers. Starting saved RAM
Saver sessions directly in the tray avoids loading WebView2 at all. Its **6.45 MiB**
versus roughly **8 MiB** after browsing also shows some retained parent-process
cost after browser use; individual allocator/DLL costs were not heap-profiled.
This is a short benchmark on one video and machine, not a universal memory bound
or a long-duration leak test.

Both runs finished with no surviving descendants. The main run also destroyed
the native media host, confirmed **zero windows and zero WebViews** while audio
continued, then used graceful Quit. Restart restored **169.146 seconds and 25%
volume, paused**, with no WebViews created. Native volume and pause/resume, Windows
media-session Play/Pause, browser restoration and repeated handoff passed.

The [recorded summaries and lifecycle evidence](benchmarks/2026-10-12.json) preserve
all sampled peaks. One final sample labelled `ram-saver-1` captured the following
controller-opening transition (23.91 MiB and four newly created WebView2 children).
It was not a retained browser tree; the probe now marks that transition before
creating the controller. Other RAM phases had only the native process. Early
preliminary runs using the old PID-only ancestry logic are excluded from these
final results. Raw CSVs remain in the ignored `.verification/memory-verified/`
and `.verification/memory-resume-verified/` directories.

Validation: **47 Rust tests**, **4 frontend tests**, production frontend build,
normal release build, Rust formatting, Clippy with warnings denied, and the
PowerShell process-identity/orphan/PID-reuse regression checks passed.

### Run it yourself

Use a release build with the opt-in probe; normal builds contain no benchmark
driver or external control interface. Install the existing yt-dlp/Deno resolver
prerequisites first (or set `MEDIABROWSER_YTDLP` and `MEDIABROWSER_DENO`).

```powershell
npm run build
cargo build --manifest-path src-tauri/Cargo.toml --release --features memory-benchmark
./scripts/Test-ProcessTree.ps1
./scripts/Measure-Memory.ps1 -OutputDirectory .verification/memory-run
./scripts/Measure-Memory.ps1 -OutputDirectory .verification/memory-resume `
  -ResumeCheckpoint .verification/memory-run/state/playback-state.json
```

The probe uses the public Big Buck Bunny video by default. It tests real browser
and native audio playback, handoff, all-WebView destruction, a controller-only
comparison, browser reconstruction, repeated handoff, volume, pause/resume and
Windows media-session commands. The resume run checks paused restoration before
playing. It uses isolated checkpoint files, while WebView2 retains the normal
browser profile. Consent screens, ads or YouTube/network errors fail the run
explicitly. A failed measurement is not reported as a successful comparison.

The PowerShell sampler sums `Win32_Process.PrivatePageCount` in bytes over the
root **and recursively discovered descendants**. This is private committed memory,
not working set, shared pages or the executable size. PID creation times protect
against reuse, discovered descendants remain tracked after parent exit, and
unrelated WebView2 processes are excluded. Samples include resolver and Deno
processes when present. Sub-second processes can fall between samples. Eight
seconds of settling are recorded separately, followed by 20-second steady phases
(configurable). `samples.csv`, `processes.csv`, `summary.json`, `lifecycle.jsonl`
and `run.json` retain the measurements, assertions and exit result. A ten-minute
timeout reports failure and does not kill an arbitrary process tree.

## Remaining costs and limits

The tray-only process still needs native Tauri/Windows/COM state, Tokio, CPAL and
the output device, Symphonia's decoder/container metadata, TLS/HTTP state, stacks,
and allocator overhead. Audio retains a 128 KiB HTTP range and 250 ms PCM ring
(about 188 KiB at 48 kHz stereo including per-frame timestamps), plus bounded
packet/resampling/timing buffers. Container metadata varies by track. Playlist
records remain disk-backed; only one resolver result is prefetched. Resolution
temporarily adds yt-dlp/Deno memory and should be distinguished from steady
playback. No working-set trimming, forced garbage collection, browser-process
killing or reduced audio safety buffers are used.

Memory and media-key behavior depend on Windows, the audio device, WebView2,
YouTube content and which session Windows selects. Physical key presses and
physical listening require hardware verification; the probe verifies Windows
session commands and the engine's device-consumption clock.
