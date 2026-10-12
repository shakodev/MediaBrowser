import { audio, type PlaybackSnapshot, type Repeat } from "../api/audio";
import { errorMessage } from "../api/browser";

export function mountAudio(root: HTMLElement): void {
  root.innerHTML = `
    <div class="local-audio-heading"><strong>Audio playback</strong><span id="audio-status" role="status">Load a public YouTube video or local audio file</span></div>
    <form id="youtube-load" class="local-file-form"><label class="sr-only" for="youtube-url">Public YouTube video URL</label><input id="youtube-url" type="url" placeholder="https://www.youtube.com/watch?v=…" autocomplete="off" spellcheck="false" required><button type="submit" class="primary">Load YouTube</button></form>
    <form id="playlist-load" class="local-file-form"><label class="sr-only" for="playlist-url">YouTube playlist or Mix URL</label><input id="playlist-url" type="url" placeholder="YouTube playlist or Mix URL" autocomplete="off" spellcheck="false" required><button type="submit" class="primary">Play playlist</button></form>
    <form id="audio-load" class="local-file-form"><label class="sr-only" for="audio-path">Local audio file path</label><input id="audio-path" placeholder="C:\\Music\\track.m4a" autocomplete="off" spellcheck="false" required><button type="submit" class="primary">Load file</button></form>
    <div class="local-transport"><button id="audio-play" class="primary" disabled>Play</button><button id="audio-pause" disabled>Pause</button><button id="audio-stop" disabled>Stop</button><output id="audio-time">0:00 / —</output><label class="volume-control">Volume <input id="audio-volume" type="range" min="0" max="1" step="0.01" value="1"></label></div>
    <div class="local-seek"><label class="sr-only" for="audio-progress">Playback position</label><input id="audio-progress" type="range" min="0" max="1" step="0.01" value="0" disabled><form id="audio-seek"><label class="sr-only" for="audio-seconds">Seek to seconds</label><input id="audio-seconds" type="number" min="0" step="0.01" placeholder="Seconds" aria-label="Seek to seconds" required><button disabled>Seek</button></form></div>
    <p id="audio-metadata" class="track-metadata"></p>
    <section id="playlist-panel" hidden aria-label="Playlist queue">
      <div class="queue-heading"><strong id="playlist-title"></strong><span id="playlist-current"></span></div>
      <div class="queue-controls"><button id="queue-previous">Previous track</button><button id="queue-next">Next track</button><label><input id="queue-shuffle" type="checkbox"> Shuffle</label><label>Repeat <select id="queue-repeat"><option value="off">Off</option><option value="all">All</option><option value="one">One</option></select></label></div>
      <p id="queue-notice" role="status"></p>
      <ol id="queue-tracks" class="queue-tracks"></ol>
      <div class="queue-pagination"><button id="queue-page-back">Previous page</button><output id="queue-page-label"></output><button id="queue-page-next">Next page</button><button id="queue-show-current">Show current track</button></div>
    </section>
    <p id="audio-error" class="error-message" role="status" aria-live="polite"></p>`;
  const path = root.querySelector<HTMLInputElement>("#audio-path")!;
  const progress = root.querySelector<HTMLInputElement>("#audio-progress")!;
  const volume = root.querySelector<HTMLInputElement>("#audio-volume")!;
  const error = root.querySelector<HTMLElement>("#audio-error")!;
  let busy = false;
  let revision = 0;
  let disposed = false;
  let dragging = false;
  let state: PlaybackSnapshot | undefined;
  let trackSignature = "";
  let timer: ReturnType<typeof setTimeout>;

  function render(next: PlaybackSnapshot): void {
    state = next;
    root.querySelector("#audio-status")!.textContent = next.path
      ? `${next.status} · ${(next.metadata?.title ?? next.path.split(/[\\/]/).pop())}`
      : "Load a public YouTube video or local audio file";
    root.querySelector("#audio-time")!.textContent = `${time(next.positionSeconds)} / ${next.durationSeconds === null ? "—" : time(next.durationSeconds)}`;
    root.querySelector("#audio-metadata")!.textContent = next.metadata
      ? [next.metadata.uploader, next.metadata.durationSeconds === null ? null : time(next.metadata.durationSeconds)].filter(Boolean).join(" · ") : "";
    renderQueue(next);
    progress.disabled = busy || next.durationSeconds === null || !next.path;
    progress.max = String(next.durationSeconds ?? 1);
    if (!dragging) progress.value = String(next.positionSeconds);
    if (document.activeElement !== volume) volume.value = String(next.volume);
    for (const button of root.querySelectorAll<HTMLButtonElement>("button")) {
      if (button.closest("#playlist-panel")) continue;
      button.disabled = busy || (button.closest("#audio-load, #youtube-load, #playlist-load") === null && !next.path);
    }
    root.querySelector<HTMLButtonElement>("#audio-pause")!.disabled = busy || next.status !== "playing";
    root.querySelector<HTMLButtonElement>("#audio-play")!.disabled = busy || !next.path || next.status === "playing";
    root.querySelector<HTMLButtonElement>("#audio-stop")!.disabled = !next.path && !busy;
    if (next.error) error.textContent = next.error;
  }
  function renderQueue(next: PlaybackSnapshot): void {
    const panel = root.querySelector<HTMLElement>("#playlist-panel")!;
    const queue = next.queue;
    const page = next.queuePage;
    panel.hidden = !queue || !page;
    if (!queue || !page) { trackSignature = ""; root.querySelector("#queue-tracks")!.replaceChildren(); return; }
    root.querySelector("#playlist-title")!.textContent = `${queue.title} · ${queue.total} tracks`;
    root.querySelector("#playlist-current")!.textContent = queue.current === null ? "" : `Current: ${queue.current + 1}`;
    const mix = queue.sourceUrl && new URL(queue.sourceUrl).searchParams.get("list")?.startsWith("RD");
    root.querySelector("#queue-notice")!.textContent = [queue.notice, mix ? "Mix snapshot · Reload the Mix for fresh recommendations." : null].filter(Boolean).join(" ");
    const shuffle = root.querySelector<HTMLInputElement>("#queue-shuffle")!;
    const repeat = root.querySelector<HTMLSelectElement>("#queue-repeat")!;
    shuffle.checked = queue.shuffle;
    repeat.value = queue.repeat;
    shuffle.disabled = repeat.disabled = busy;
    const signature = JSON.stringify([page, queue.current, next.status]);
    if (signature !== trackSignature) {
      trackSignature = signature;
      const list = root.querySelector<HTMLOListElement>("#queue-tracks")!;
      list.start = page.start + 1;
      list.replaceChildren(...page.tracks.map((track, offset) => {
        const item = document.createElement("li");
        const button = document.createElement("button");
        const index = page.start + offset;
        button.textContent = `${track.metadata.title}${track.metadata.uploader ? ` · ${track.metadata.uploader}` : ""}${track.unavailable ? " · unavailable" : track.failed ? " · failed (retry)" : ""}`;
        if (index === queue.current) { button.setAttribute("aria-current", "true"); button.classList.add("current-track"); }
        button.addEventListener("click", () => void perform(() => audio.queue({ kind: "select", index })));
        item.append(button);
        return item;
      }));
    }
    for (const button of panel.querySelectorAll<HTMLButtonElement>("button")) button.disabled = busy;
    root.querySelector<HTMLButtonElement>("#queue-page-back")!.disabled = busy || page.start === 0;
    root.querySelector<HTMLButtonElement>("#queue-page-next")!.disabled = busy || page.start + page.tracks.length >= queue.total;
    root.querySelector("#queue-page-label")!.textContent = `${page.start + 1}–${page.start + page.tracks.length} of ${queue.total}`;
  }
  async function perform(action: () => Promise<PlaybackSnapshot>, interrupt = false): Promise<void> {
    if (busy && !interrupt) return;
    busy = true;
    const operationRevision = ++revision;
    error.textContent = "";
    if (state) render(state);
    root.querySelector<HTMLButtonElement>("#audio-stop")!.disabled = false;
    try {
      const next = await action();
      if (operationRevision !== revision) return;
      busy = false;
      render(next);
    } catch (cause) {
      if (operationRevision !== revision) return;
      busy = false;
      if (state) render(state);
      error.textContent = errorMessage(cause);
    }
  }
  root.querySelector("#youtube-load")!.addEventListener("submit", (event) => {
    event.preventDefault();
    void perform(() => audio.loadYoutube(root.querySelector<HTMLInputElement>("#youtube-url")!.value.trim()));
  });
  root.querySelector("#playlist-load")!.addEventListener("submit", (event) => {
    event.preventDefault();
    void perform(() => audio.queue({ kind: "load", url: root.querySelector<HTMLInputElement>("#playlist-url")!.value.trim() }));
  });
  root.querySelector("#queue-previous")!.addEventListener("click", () => void perform(() => audio.queue({ kind: "previous" })));
  root.querySelector("#queue-next")!.addEventListener("click", () => void perform(() => audio.queue({ kind: "next" })));
  const configureQueue = () => {
    const shuffle = root.querySelector<HTMLInputElement>("#queue-shuffle")!.checked;
    const repeat = root.querySelector<HTMLSelectElement>("#queue-repeat")!.value as Repeat;
    void perform(() => audio.queue({ kind: "configure", shuffle, repeat }));
  };
  root.querySelector("#queue-shuffle")!.addEventListener("change", configureQueue);
  root.querySelector("#queue-repeat")!.addEventListener("change", configureQueue);
  root.querySelector("#queue-page-back")!.addEventListener("click", () => void perform(() => audio.queue({ kind: "page", start: Math.max(0, (state?.queuePage?.start ?? 0) - 40) })));
  root.querySelector("#queue-page-next")!.addEventListener("click", () => void perform(() => audio.queue({ kind: "page", start: (state?.queuePage?.start ?? 0) + 40 })));
  root.querySelector("#queue-show-current")!.addEventListener("click", () => void perform(() => audio.queue({ kind: "page", start: Math.floor((state?.queue?.current ?? 0) / 40) * 40 })));
  root.querySelector("#audio-load")!.addEventListener("submit", (event) => {
    event.preventDefault();
    void perform(() => audio.load(path.value.trim().replace(/^"|"$/g, "")));
  });
  root.querySelector("#audio-play")!.addEventListener("click", () => void perform(audio.play));
  root.querySelector("#audio-pause")!.addEventListener("click", () => void perform(audio.pause));
  root.querySelector("#audio-stop")!.addEventListener("click", () => void perform(audio.stop, true));
  volume.addEventListener("change", () => void perform(() => audio.volume(Number(volume.value))));
  progress.addEventListener("pointerdown", () => { dragging = true; });
  progress.addEventListener("pointerup", () => { dragging = false; });
  progress.addEventListener("pointercancel", () => { dragging = false; });
  progress.addEventListener("change", () => void perform(() => audio.seek(Number(progress.value))));
  root.querySelector("#audio-seek")!.addEventListener("submit", (event) => {
    event.preventDefault();
    void perform(() => audio.seek(Number(root.querySelector<HTMLInputElement>("#audio-seconds")!.value)));
  });
  async function poll(): Promise<void> {
    const current = revision;
    try {
      if (!busy && !document.hidden) {
        const next = await audio.snapshot();
        if (current === revision) render(next);
      }
    } catch {
      error.textContent = "Launch the desktop app to use audio playback.";
    }
    if (!disposed) timer = setTimeout(() => void poll(), state?.status === "playing" ? 250 : 1000);
  }
  window.addEventListener("pagehide", () => { disposed = true; clearTimeout(timer); }, { once: true });
  void poll();
}

function time(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}
