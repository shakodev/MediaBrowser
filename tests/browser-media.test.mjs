import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../src-tauri/src/browser/media.js", import.meta.url), "utf8");
function page() {
  const media = { currentTime: 32.375, volume: 0.42, muted: false, paused: false, ended: false, readyState: 4, duration: 180 };
  const location = { href: "https://www.youtube.com/watch?v=aqz-KE-bpKQ&list=PL_test&index=8" };
  let id = "aqz-KE-bpKQ";
  let ad = false;
  const player = { querySelector: () => media, getVideoData: () => ({ video_id: id }), classList: { contains: () => ad } };
  const context = vm.createContext({ URL, location, document: { querySelector: () => player } });
  const capture = vm.runInContext(`(${source})`, context);
  return { capture, media, location, loaded: value => { id = value; }, ad: value => { ad = value; } };
}

test("captures public URL context and current HTML media position and volume", () => {
  const p = page();
  assert.deepEqual(JSON.parse(JSON.stringify(p.capture())), {
    url: p.location.href, videoId: "aqz-KE-bpKQ", playlistId: "PL_test", playlistIndex: 8,
    positionSeconds: 32.375, volume: 0.42, paused: false,
  });
  p.media.muted = true;
  p.media.paused = true;
  assert.equal(p.capture().volume, 0);
  assert.equal(p.capture().paused, true);
});

test("reads fresh SPA context and rejects URL/player disagreement during navigation", () => {
  const p = page();
  p.location.href = "https://www.youtube.com/watch?v=BaW_jenozKc&list=PL_test&index=9";
  assert.equal(p.capture(), null);
  p.loaded("BaW_jenozKc");
  p.media.currentTime = 1.5;
  assert.equal(p.capture().videoId, "BaW_jenozKc");
  assert.equal(p.capture().playlistIndex, 9);
  assert.equal(p.capture().positionSeconds, 1.5);
});

test("rejects ads, live, loading, consent and non-video pages", () => {
  const p = page();
  p.ad(true); assert.equal(p.capture(), null); p.ad(false);
  p.media.duration = Infinity; assert.equal(p.capture(), null); p.media.duration = 180;
  // An ended track still has a valid restoration position.
  p.media.ended = true; p.media.paused = true; assert.equal(p.capture().paused, true); p.media.ended = false;
  p.media.readyState = 1; assert.equal(p.capture(), null); p.media.readyState = 4;
  for (const url of ["https://consent.youtube.com/m", "https://youtube.com.evil/watch?v=aqz-KE-bpKQ", "https://www.youtube.com/results?search_query=music", "https://www.youtube.com/playlist?list=PL_test"]) {
    p.location.href = url;
    assert.equal(p.capture(), null);
  }
});

test("single video and shorts do not invent an internal autoplay queue", () => {
  const p = page();
  for (const url of ["https://www.youtube.com/watch?v=aqz-KE-bpKQ", "https://www.youtube.com/shorts/aqz-KE-bpKQ"]) {
    p.location.href = url;
    assert.equal(p.capture().playlistId, null);
    assert.equal(p.capture().playlistIndex, null);
  }
});
