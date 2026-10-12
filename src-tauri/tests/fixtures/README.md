# Local audio fixtures

Small sound-effect fixtures from Chromium's `media/test/data` under its BSD license
(see `LICENSE.chromium`). `sfx.adts` is stored here as `sfx.aac`; other bytes are unchanged.

- https://raw.githubusercontent.com/chromium/chromium/main/media/test/data/sfx.mp3
- https://raw.githubusercontent.com/chromium/chromium/main/media/test/data/sfx.m4a
- https://raw.githubusercontent.com/chromium/chromium/main/media/test/data/sfx.adts

Tests decode these files directly with Symphonia. No external decoder or player is used.
