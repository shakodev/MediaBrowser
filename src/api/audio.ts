import { invoke } from "@tauri-apps/api/core";

export interface PlaybackSnapshot {
  status: "loading" | "stopped" | "paused" | "playing" | "ended" | "error";
  queue: { sourceUrl: string | null; sourceIndex: number | null; title: string; total: number; current: number | null; shuffle: boolean; repeat: Repeat; skipped: number; notice: string | null } | null;
  queuePage: { start: number; tracks: { playlistIndex: number | null; metadata: NonNullable<PlaybackSnapshot["metadata"]>; unavailable: boolean; failed: boolean }[] } | null;
  path: string | null;
  metadata: { id: string; title: string; uploader: string | null; durationSeconds: number | null } | null;
  transfer: { bytesReceived: number; totalBytes: number; rangeRequests: number; urlRefreshes: number; bufferLimitBytes: number } | null;
  positionSeconds: number;
  durationSeconds: number | null;
  volume: number;
  error: string | null;
}

export type Repeat = "off" | "all" | "one";
export type QueueAction =
  | { kind: "load"; url: string }
  | { kind: "next" | "previous" }
  | { kind: "select"; index: number }
  | { kind: "configure"; shuffle: boolean; repeat: Repeat }
  | { kind: "page"; start: number };

export const audio = {
  queue: (action: QueueAction) => invoke<PlaybackSnapshot>("audio_queue", { action }),
  loadYoutube: (url: string) => invoke<PlaybackSnapshot>("audio_load_youtube", { url }),
  load: (path: string) => invoke<PlaybackSnapshot>("audio_load", { path }),
  play: () => invoke<PlaybackSnapshot>("audio_play"),
  pause: () => invoke<PlaybackSnapshot>("audio_pause"),
  stop: () => invoke<PlaybackSnapshot>("audio_stop"),
  seek: (seconds: number) => invoke<PlaybackSnapshot>("audio_seek", { seconds }),
  volume: (volume: number) => invoke<PlaybackSnapshot>("audio_volume", { volume }),
  snapshot: () => invoke<PlaybackSnapshot>("audio_snapshot"),
};
