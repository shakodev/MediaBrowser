import {
  errorMessage,
  navigate,
  openBrowser,
  ramSaver,
  watchBrowser,
} from "../api/browser";
import { icon } from "./icons";
import { mountAudio } from "./audio";

export function mountController(root: HTMLDivElement): void {
  root.innerHTML = `
    <div class="shell">
      <aside class="sidebar">
        <div class="brand"><span class="brand-mark">${icon("wave")}</span><span>MediaBrowser<small>LESS WEIGHT. MORE MUSIC.</small></span></div>
        <span class="nav-label">WORKSPACE</span>
        <button class="nav-item selected" id="browser-mode" aria-current="page">${icon("browser")} Browser Mode</button>
        <button class="nav-item" id="ram-saver" disabled title="Continue this YouTube video with audio only">${icon("leaf")} RAM Saver</button>
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
        <footer class="local-player" id="local-player" aria-label="Local audio playback"></footer>
      </div>
    </div>`;

  mountAudio(root.querySelector<HTMLElement>("#local-player")!);

  const open = root.querySelector<HTMLButtonElement>("#open-browser")!;
  const resume = root.querySelector<HTMLButtonElement>("#resume")!;
  const input = root.querySelector<HTMLInputElement>("#search")!;
  const submit = root.querySelector<HTMLButtonElement>(".search-submit")!;
  const error = root.querySelector<HTMLParagraphElement>("#error")!;
  const saver = root.querySelector<HTMLButtonElement>("#ram-saver")!;
  const browserMode = root.querySelector<HTMLButtonElement>("#browser-mode")!;
  let busy = false;
  let switching = false;
  let isOpen = false;
  let mode: "browser" | "ramSaver" = "browser";
  function controls(): void {
    open.disabled = resume.disabled = submit.disabled = browserMode.disabled = busy || switching;
    saver.disabled = busy || switching || !isOpen || mode === "ramSaver";
  }
  async function perform(action: () => Promise<void>): Promise<void> {
    if (busy) return;
    busy = true;
    controls();
    error.textContent = "";
    try {
      await action();
    } catch (cause) {
      error.textContent = errorMessage(cause);
    } finally {
      busy = false;
      controls();
    }
  }
  open.addEventListener("click", () => void perform(openBrowser));
  resume.addEventListener("click", () => void perform(openBrowser));
  browserMode.addEventListener("click", () => void perform(openBrowser));
  saver.addEventListener("click", () => void perform(ramSaver));
  root
    .querySelector<HTMLFormElement>("#search-form")!
    .addEventListener("submit", (event) => {
      event.preventDefault();
      void perform(() => navigate(input.value));
    });
  void watchBrowser((state) => {
    switching = state.switching;
    isOpen = state.isOpen;
    mode = state.mode;
    controls();
    saver.classList.toggle("selected", mode === "ramSaver");
    browserMode.classList.toggle("selected", mode === "browser");
    for (const [button, selected] of [[saver, mode === "ramSaver"], [browserMode, mode === "browser"]] as const) {
      if (selected) button.setAttribute("aria-current", "page"); else button.removeAttribute("aria-current");
    }
    root.querySelector("h1")!.textContent = mode === "ramSaver" ? "RAM Saver Mode" : "Browser Mode";
    root.querySelector(".pill")!.textContent = mode === "ramSaver" ? "Rust audio" : "WebView2";
    root.querySelector("#page-title")!.textContent = state.title || "YouTube";
    root.querySelector("#current-url")!.textContent = state.currentUrl;
    root.querySelector("#current-url")!.setAttribute("title", state.currentUrl);
    root.querySelector("#session-status")!.textContent = switching ? "Switching playback…" : mode === "ramSaver" ? "Audio only · browser released" : state.loading
      ? "Loading page…"
      : state.isOpen
        ? "Browser window open"
        : "Last visited page";
    open.querySelector("span")!.textContent = mode === "ramSaver" ? "Return to Browser Mode" : state.isOpen
      ? "Return to YouTube"
      : "Open YouTube";
    error.textContent = state.error ?? "";
  }).catch(() => {
    error.textContent =
      "Launch the desktop app with npm run tauri dev to connect to YouTube.";
  });
}
