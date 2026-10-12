// Executed only on demand by Rust. No messages, listeners, timers or Tauri access.
() => {
  const url = new URL(location.href);
  if (url.protocol !== "https:" || !["www.youtube.com", "youtube.com", "m.youtube.com", "music.youtube.com"].includes(url.hostname)) return null;
  const id = url.pathname === "/watch" ? url.searchParams.get("v") : url.pathname.match(/^\/shorts\/([\w-]{11})\/?$/)?.[1];
  const player = document.querySelector("#movie_player");
  const video = player?.querySelector("video");
  // URL and loaded media must agree during SPA transitions. Never transfer an ad.
  if (!id || id.length !== 11 || !video || player.classList.contains("ad-showing") || player.getVideoData?.()?.video_id !== id || video.readyState < 2 || !Number.isFinite(video.duration)) return null;
  return {
    url: location.href,
    videoId: id,
    playlistId: url.searchParams.get("list"),
    playlistIndex: url.searchParams.has("index") ? Number(url.searchParams.get("index")) : null,
    positionSeconds: video.currentTime,
    volume: video.muted ? 0 : video.volume,
    paused: video.paused,
  };
}
