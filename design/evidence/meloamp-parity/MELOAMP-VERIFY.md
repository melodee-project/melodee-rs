# MeloAmp × melodee-rs parity verification

MeloAmp is the native-API desktop client this backend must serve. This record covers
a full pass: real catalogue (50 imported Releases, 385 Tracks), seeded client data,
every navigation target driven through the running Electron application, and the
defects the pass produced.

Harness: Electron launched through Playwright (`_electron`), credentials injected into
`localStorage`, one sign-in, then per-route navigation with console-error and
`>= 400` response capture, image decode checks (`naturalWidth`) and audio-element
state checks. Nothing in MeloAmp was modified.

## Defects found and fixed in melodee-rs

| # | Symptom in the client | Cause in melodee-rs | Fix |
|---|---|---|---|
| 1 | Artist cards/detail showed broken images (9 of 20 artists) | `/images/artist_<id>/…` returned `200` with an **empty body** for a known Artist without artwork; the frozen endpoint serves the captured default image for any entity that exists | `image_entity_exists` distinguishes "unknown id" (empty `200`) from "known entity without artwork" (captured default); a withheld Release gets the default, never its own cover |
| 2 | History: "No play history yet" for an account with 546 plays | `/user/last-played` reported `meta.totalCount` but hard-coded `data: []` (literal reproduction of a frozen serialization bug) | returns the plays it counts as `{id, song, playedAt, duration}` (ADR-0188) |
| 3 | Chart cards showed blank names and "undefined tracks" | frozen `title`/`itemCount` vs the client's `name`/`trackCount` | aliases mirrored from the frozen keys (ADR-0188) |
| 4 | Equalizer view crashed (`t.map is not a function`) | frozen `{presets, meta}` vs the client's envelope reader | `data` alias alongside `presets` (ADR-0187) |
| 5 | Analytics "Top content" panel errored (`400`) | client sends plural `type=songs`; the frozen validator accepts `song`/`album`/`artist` | plural accepted as a tolerated alias; unknown values still `400` |
| 6 | Scrobble sometimes returned `500` | a client timestamp in the future violated `native_now_playing_time_check`; also the 5xx path logged nothing | timestamps are clamped to server time (the frozen server ignores client time), and every `5xx` now logs its cause |
| 7 | Dashboard errored for non-admin accounts | `/system/stats` returned `404` where the frozen capability filter returns `403` | `403` with the frozen `FORBIDDEN` body |

## Client-side incompatibilities (not changed here)

These cannot be fixed without editing MeloAmp, and the frozen C# service behaves the
same way, so they are recorded rather than "fixed" by breaking the captured contract:

- `/playback-tools` shows an unavailable/error state while the Jukebox is disabled.
  C# seeds `jukebox.enabled = false`, so `/playback-backend/status` and
  `/playback-backend/capabilities` answer `400` in both servers; the page is only
  meaningful with a Jukebox backend configured.
- Chart cards request `/api/v1/charts/{id}/image`; the frozen service emits that same
  `imageUrl` but implements no such route, and neither does melodee-rs. The client
  falls back to its own placeholder.

## Performance: artwork slowness

The "slow images" observation was the **dev stack running an unoptimised build**, not
a backend performance defect.

| Measurement (1024 px cover) | Debug build | Release build |
|---|---|---|
| Cold request | 3.46 s | 0.13 s |
| Repeat request (before derivative cache) | 3.46 s | 0.12 s |
| Repeat request (after derivative cache) | 0.013 s | 0.010 s |
| 20 concurrent cold covers (512 px) | seconds | 0.21 s total |

`original` (no decode) is 15 ms; the publication-plan lookup is 0.05 ms. Two changes
came out of the investigation: a bounded in-process derivative cache keyed by the
immutable publication hash (repeat views and ETag revalidations no longer decode),
and `MELODEE_DEV_PROFILE=release` support in the dev stack so client testing runs the
binary the product actually ships.

## Verification runs

- Nav sweep, admin account: **20/21 views clean**; the only remaining failure is
  `/playback-tools` (Jukebox disabled, frozen parity).
- Nav sweep, listener account (`meloamp`): no unexpected failures; Dashboard reports
  the capability error exactly as the frozen server does.
- Data-consumption checks after seeding (`scripts/seed_meloamp_parity_data.py`):
  **11/11** screens render seeded data (favourites, history, playlists, smart
  playlist, chart, requests, shares, analytics, equalizer, genres, dashboard).
- Playback: album play and song play both reach `readyState 4` with a real duration
  and run (scrobbles recorded; `/user/last-played` grows accordingly).
- Images: 0 broken API image URLs across every swept view.
