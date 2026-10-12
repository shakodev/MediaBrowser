import {
  errorMessage,
  navigate,
  openBrowser,
  watchBrowser,
} from "../api/browser";
import { icon } from "./icons";

export function mountController(root: HTMLDivElement): void {
  root.innerHTML = `
    <div class="shell">
      <aside class="sidebar">
        <div class="brand"><span class="brand-mark">${icon("wave")}</span><span>MediaBrowser<small>LESS WEIGHT. MORE MUSIC.</small></span></div>
        <span class="nav-label">WORKSPACE</span>
        <div class="nav-item selected" aria-current="page">${icon("browser")} Browser Mode <span class="active-dot"></span></div>
        <button class="nav-item" disabled title="Audio-only playback is planned for a later stage">${icon("leaf")} RAM Saver <span class="soon">SOON</span></button>
        <div class="sidebar-bottom"><span class="system-dot"></span> Built for Windows<small>Rust · Tauri · WebView2</small></div>
      </aside>
      <div class="workspace">
        <header class="page-header"><div><span class="eyebrow">YOUR LISTENING SPACE</span><h1>Browser Mode</h1></div><span class="pill"><span class="system-dot"></span> WebView2</span></header>
        <main>
          <section class="hero">
            <div class="hero-copy"><span class="eyebrow accent">A LITTLE LESS, A LITTLE LIGHTER</span><h2>Your music.<br>Room to breathe.</h2><p>Find a track, a mix, or your next favorite.<br>YouTube opens in its own browsing window.</p><button class="primary" id="open-browser">${icon("external")} <span>Open YouTube</span></button></div>
            <div class="record-art" aria-hidden="true"><div class="orbit orbit-two"></div><div class="orbit"></div><div class="record"><div class="record-center">${icon("wave")}</div></div><span class="art-caption">MADE FOR THE MUSIC</span></div>
          </section>
          <form class="search-form" id="search-form"><label class="sr-only" for="search">Search YouTube or paste a YouTube link</label>${icon("search")}<input id="search" type="text" placeholder="Search YouTube or paste a link" autocomplete="off" spellcheck="false" maxlength="8192"><button class="search-submit" type="submit" aria-label="Search YouTube">${icon("arrow")}</button></form>
          <section class="session-section"><div class="section-heading"><h3>Browsing session</h3><span id="session-status">Ready when you are</span></div><div class="session-card"><span class="session-icon">${icon("browser")}</span><div class="session-details"><strong id="page-title">YouTube</strong><span id="current-url">https://www.youtube.com/</span></div><button class="icon-button" id="resume" aria-label="Open last visited page" title="Open last visited page">${icon("external")}</button></div><p class="session-note">Your last YouTube page is remembered automatically.</p></section>
          <p class="error-message" id="error" role="status" aria-live="polite"></p>
        </main>
        <footer class="player"><div class="player-track"><span class="track-art">${icon("music")}</span><div><strong>Playback lives in YouTube</strong><span>Native audio controls are coming later</span></div></div><div class="transport" aria-label="Native playback is not implemented"><button disabled aria-label="Previous track">${icon("previous")}</button><button class="play-button" disabled aria-label="Native playback unavailable">${icon("play")}</button><button disabled aria-label="Next track">${icon("next")}</button></div><span class="browser-badge">BROWSER MODE</span></footer>
      </div>
    </div>`;

  const open = root.querySelector<HTMLButtonElement>("#open-browser")!;
  const resume = root.querySelector<HTMLButtonElement>("#resume")!;
  const input = root.querySelector<HTMLInputElement>("#search")!;
  const submit = root.querySelector<HTMLButtonElement>(".search-submit")!;
  const error = root.querySelector<HTMLParagraphElement>("#error")!;
  let busy = false;
  async function perform(action: () => Promise<void>): Promise<void> {
    if (busy) return;
    busy = true;
    open.disabled = resume.disabled = submit.disabled = true;
    error.textContent = "";
    try {
      await action();
    } catch (cause) {
      error.textContent = errorMessage(cause);
    } finally {
      busy = false;
      open.disabled = resume.disabled = submit.disabled = false;
    }
  }
  open.addEventListener("click", () => void perform(openBrowser));
  resume.addEventListener("click", () => void perform(openBrowser));
  root
    .querySelector<HTMLFormElement>("#search-form")!
    .addEventListener("submit", (event) => {
      event.preventDefault();
      void perform(() => navigate(input.value));
    });
  void watchBrowser((state) => {
    root.querySelector("#page-title")!.textContent = state.title || "YouTube";
    root.querySelector("#current-url")!.textContent = state.currentUrl;
    root.querySelector("#current-url")!.setAttribute("title", state.currentUrl);
    root.querySelector("#session-status")!.textContent = state.loading
      ? "Loading page…"
      : state.isOpen
        ? "Browser window open"
        : "Last visited page";
    open.querySelector("span")!.textContent = state.isOpen
      ? "Return to YouTube"
      : "Open YouTube";
    error.textContent = state.error ?? "";
  }).catch(() => {
    error.textContent =
      "Launch the desktop app with npm run tauri dev to connect to YouTube.";
  });
}
