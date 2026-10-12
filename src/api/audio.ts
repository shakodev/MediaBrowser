import { invoke } from "@tauri-apps/api/core";

export interface PlaybackSnapshot {
  status: "stopped" | "paused" | "playing" | "ended" | "error";
  path: string | null;
  positionSeconds: number;
  durationSeconds: number | null;
  volume: number;
  error: string | null;
}

export const audio = {
  load: (path: string) => invoke<PlaybackSnapshot>("audio_load", { path }),
  play: () => invoke<PlaybackSnapshot>("audio_play"),
  pause: () => invoke<PlaybackSnapshot>("audio_pause"),
  stop: () => invoke<PlaybackSnapshot>("audio_stop"),
  seek: (seconds: number) => invoke<PlaybackSnapshot>("audio_seek", { seconds }),
  volume: (volume: number) => invoke<PlaybackSnapshot>("audio_volume", { volume }),
  snapshot: () => invoke<PlaybackSnapshot>("audio_snapshot"),
};
