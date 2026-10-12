import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export interface BrowserState {
  mode: "browser" | "ramSaver";
  switching: boolean;
  currentUrl: string;
  lastYoutubeUrl: string;
  title: string;
  isOpen: boolean;
  loading: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  error: string | null;
}

export const openBrowser = () => invoke<void>("open_browser");
export const ramSaver = () => invoke<void>("ram_saver");
export const navigate = (input: string) =>
  invoke<void>("navigate_browser", { input });
export const browserAction = (action: "back" | "forward" | "reload") =>
  invoke<void>("browser_action", { action });

export async function watchBrowser(
  update: (state: BrowserState) => void,
): Promise<void> {
  const unlisten = await getCurrentWebview().listen<BrowserState>(
    "browser-state",
    ({ payload }) => update(payload),
  );
  window.addEventListener("pagehide", unlisten, { once: true });
  update(await invoke<BrowserState>("browser_snapshot"));
}

export function errorMessage(error: unknown): string {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "Something went wrong. Please try again.";
}
