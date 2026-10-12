import { browserAction, errorMessage, navigate, watchBrowser } from "../api/browser";
import { icon } from "./icons";

export function mountBrowser(root: HTMLDivElement): void {
  document.body.classList.add("browser-chrome");
  root.innerHTML = `<div class="browser-toolbar"><form id="address-form" class="address-row"><span class="mini-brand">${icon("wave")}</span><div class="navigation-buttons"><button class="icon-button" id="back" type="button" aria-label="Back" title="Back" disabled>${icon("back")}</button><button class="icon-button" id="forward" type="button" aria-label="Forward" title="Forward" disabled>${icon("forward")}</button><button class="icon-button" id="reload" type="button" aria-label="Reload" title="Reload">${icon("reload")}</button><button class="icon-button" id="home" type="button" aria-label="YouTube home" title="YouTube home">${icon("home")}</button></div><div class="address-field">${icon("lock")}<label class="sr-only" for="address">YouTube address or search</label><input id="address" type="text" autocomplete="off" spellcheck="false" placeholder="Search YouTube or enter a YouTube URL" maxlength="8192"><button type="submit" class="icon-button" aria-label="Go" title="Go">${icon("arrow")}</button></div></form><div class="browser-status" role="status" aria-live="polite"><span class="system-dot" id="load-dot"></span><span id="browser-message">Opening YouTube…</span><span class="isolation-label">YouTube browser</span></div></div>`;
  const address = root.querySelector<HTMLInputElement>("#address")!;
  const back = root.querySelector<HTMLButtonElement>("#back")!;
  const forward = root.querySelector<HTMLButtonElement>("#forward")!;
  const message = root.querySelector<HTMLSpanElement>("#browser-message")!;
  let currentUrl = "";
  let busy = false;

  async function perform(action: () => Promise<void>): Promise<void> {
    if (busy) return;
    busy = true;
    try { await action(); } catch (error) { message.textContent = errorMessage(error); }
    finally { busy = false; }
  }
  for (const action of ["back", "forward", "reload"] as const) {
    root.querySelector(`#${action}`)!.addEventListener("click", () => void perform(() => browserAction(action)));
  }
  root.querySelector("#home")!.addEventListener("click", () => void perform(() => navigate("https://www.youtube.com/")));
  root.querySelector<HTMLFormElement>("#address-form")!.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = address.value;
    address.blur();
    void perform(() => navigate(input));
  });
  address.addEventListener("focus", () => address.select());
  address.addEventListener("keydown", (event) => {
    if (event.key === "Escape") { address.value = currentUrl; address.blur(); }
  });
  document.addEventListener("keydown", (event) => {
    if (event.ctrlKey && event.key.toLowerCase() === "l") { event.preventDefault(); address.focus(); }
  });
  void watchBrowser((state) => {
    currentUrl = state.currentUrl;
    if (document.activeElement !== address) address.value = currentUrl;
    back.disabled = !state.canGoBack;
    forward.disabled = !state.canGoForward;
    root.querySelector("#load-dot")!.classList.toggle("loading", state.loading);
    message.textContent = state.error ?? (state.loading ? "Loading…" : state.title || "YouTube");
    message.classList.toggle("has-error", Boolean(state.error));
  }).catch((error) => { message.textContent = errorMessage(error); });
}
