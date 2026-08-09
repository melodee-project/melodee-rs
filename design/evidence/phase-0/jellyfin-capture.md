# Phase 0 Evidence — Jellyfin Compatibility Capture

**Issue:** FIRST-10-ISSUES.md Issue 7 — Capture Jellyfin compatibility critical path
**Requirements:** PRD-API-JF-001 through PRD-API-JF-006
**Baseline:** legacy commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (authoritative SHA);
server version `2.2.0`; DB migration head `20260714155936_AddDeleteSourceResidueAfterIngestSetting`
**Recorded:** 2026-08-08
**Golden evidence:** `compatibility/jellyfin/golden/` (8 directories, 100+ captures, each with
`provenance.json`). **Inventory:** `compatibility/jellyfin/contracts/route-inventory.json`
(60 routes, **100% scenario coverage**). Jellyfin is a distinct surface — a music-focused
subset, not a complete Jellyfin server.

## 1. Routing behavior (PRD-API-JF-002)

Captured routing truth for `JellyfinRoutingMiddleware`:

- **Rewritten regardless of headers (allowlist):** `/System/Info/Public`, `/System/Ping`,
  `/Users/AuthenticateByName`, `/Users/Public`.
- **Marker-triggered rewrite:** `Authorization`/`X-Emby-Authorization` starting `MediaBrowser`;
  `X-MediaBrowser-Token`; `X-Emby-Token`; `api_key` query under a Jellyfin path prefix.
- **Root `/`:** rewritten only with a marker (→ `/api/jf`, 200 empty, HEAD supported);
  without markers it is the Blazor app (captured: HTML login page, volatile prerender state
  masked).
- **No-marker API paths fall through:** `/System/Info` and `/Users/Me` without markers do NOT
  reach the Jellyfin controllers (Blazor page / login redirect) — the middleware detection is
  part of the contract and cannot be redesigned without compatibility consequences.
- `/api/jf/*` explicit form works without rewrite; when `jellyfin.enabled=false` the middleware
  hard-404s explicit `/api/jf/*` but lets detected root-form requests fall through.
- **Unsupported scope:** no controllers exist for video/TV/LiveTV/admin; detected requests get
  ASP.NET 404 (empty body). Non-music `includeItemTypes` are silently dropped (empty result,
  not an error).

## 2. Authentication (PRD-API-JF-003)

- **Issue:** `POST /Users/AuthenticateByName {Username, Pw}` → `{User, SessionInfo,
  AccessToken, ServerId}`. **The DTO reads ONLY `Pw`** — a body with `Password` and no `Pw`
  → 400. AccessToken = 64-char lowercase hex (masked; structure-validated); ServerId =
  MD5(Jwt:Issuer) — deterministic in the fixture env; SessionInfo.Id ephemeral (never stored).
- **Token model:** stateful; 32 random bytes hex; stored as HMAC-SHA256(pepper, salt+token)
  with a cleartext 8-char prefix for lookup; expiry `jellyfin.token.expiresAfterHours`
  (default 168h); max 10 active per user (oldest soft-revoked).
- **Transports (all captured working):** Authorization MediaBrowser Token, X-Emby-Authorization,
  X-MediaBrowser-Token, X-Emby-Token, `api_key` query.
- **Failures:** 400 (missing fields), 401 (wrong password == unknown user envelope),
  401 for missing/garbage/expired/revoked tokens (indistinguishable).
- **Logout:** `POST /Sessions/Logout` → 204, revokes the presented token; revoked token → 401
  immediately. Idempotent (204 even with a garbage token). **Unlike the native JWT surface,
  Jellyfin tokens die at logout.**

## 3. Browsing, artists, genres

- Envelope: `ItemsResult {Items, TotalRecordCount, StartIndex}` (PascalCase); `JellyfinBaseItem`
  with `Type` MusicArtist/MusicAlbum/Audio/Playlist/MusicGenre/UserView; **null fields are
  serialized as null** (no null-stripping); dates `yyyy-MM-ddTHH:mm:ss.fffffffZ`; IDs are
  ApiKey GUIDs in N-format (no dashes, no prefix); genre IDs are deterministic MD5 GUIDs.
- UserViews: one per unlocked Storage library (CollectionType music); no libraries → synthetic
  "Music" view with a per-request random GUID (quirk noted).
- Items dispatch: includeItemTypes (artist/album/song listings) or parentId probe
  (artist→albums, album→songs); `sortBy/sortOrder/recursive/fields/userId` **accepted but
  ignored**. Global song listing sorts by SongNumber ONLY (no tiebreak → nondeterministic;
  masked, documented).
- Unknown item id → **404 JellyfinProblemDetails** (clean — contrast: native 500, OpenSubsonic
  ok+error/302 classes).
- Artists/AlbumArtists (same handler), artist detail, genre-overlap similar; collection and
  item ETags (quoted MD5, deterministic at fixed seed) with If-None-Match → 304.
- Genres/MusicGenres: near-identical handlers with slightly different GUID fallback behavior
  (recorded); Unicode genre `環境音楽` present.
- `Items/{id}/Similar` with an **artist** id returns empty (only song/album probed) — quirk.
- Users/{id}/Items[/{}] are **307 RedirectPreserveMethod** to `/Items` — relies on the
  middleware re-detecting the follow-up request via auth headers.
- Items/Filters: genres + years; Items/InstantMix and Songs/InstantMix random (masked).

## 4. Playback (PRD-STREAM-002 relevant)

- Direct play: GET/HEAD `/Audio/{id}/stream` → 200, `Accept-Ranges: bytes`, Content-Type from
  the **actual file** (never the requested container).
- **Ranges are correct single-pass** here (unlike the OpenSubsonic double-processing bug):
  start/middle/suffix 206 with correct `Content-Range`; invalid/unsatisfiable → **416**
  `JellyfinProblemDetails` with `Content-Range: bytes */{size}` (RFC-correct; OpenSubsonic
  uses 404, native uses 400 — three surfaces, three different invalid-range behaviors).
- **No transcoding:** `stream.{container}` without `static=true` → 400 "Transcoding is not
  supported. Use static=true for direct streaming."; with `static=true` (or `/universal`,
  which forces it) the requested container is **ignored** and the original file served.
  PlaybackInfo advertises `SupportsTranscoding=false`.
- Items/{id}/File and /Download: download-role gated, `Content-Disposition: attachment`,
  manual 206 support.
- `/Audio/{id}/Lyrics`: sibling `.lrc` parsed into `{Lyrics:[{Text, Start}]}`; missing file →
  `{Lyrics: []}` (never an error). Fixture Polar Veil has a seeded `.lrc` sidecar.

## 5. User state and sessions

- FavoriteItems POST/DELETE round-trips on song/album/artist (verify via
  `?enableUserData=true`); PlayedItems POST/DELETE on songs/albums (artists → 404); unknown
  item → 404.
- Sessions/Playing|Progress|Stopped → 204 always (invalid ItemId silently ignored); playback
  reporting feeds `ScrobbleService` (now-playing → scrobble on stop, unless Failed=true).
- GET /Sessions is **synthesized from the scrobble now-playing cache** (no session table):
  entries with NowPlayingItem, PlayMethod DirectPlay, CanSeek true.
- Sessions/Playing/Ping, Capabilities/Full, Capabilities → acknowledge-only 204.

## 6. Playlists

- List/detail/items work; items carry `ParentIndexNumber` = playlist position.
- **Create with Ids works on a clean database** (`CreatePlaylistAsync` is correct — proper
  SongApiKeys, 0-based PlaylistOrder; order follows DB fetch order, not request order —
  masked quirk). The 500s observed mid-investigation traced to poisoned rows left by the
  **OpenSubsonic** partial-create path, not to the Jellyfin create.
- **Add-items bug boundary (precise):** add to an EMPTY playlist → 204 (but the row gets
  `SongApiKey=Guid.Empty` — latent corruption); add to a NON-EMPTY playlist → **500**
  (`PlaylistService.AddSongsToPlaylistAsync` recomputes Duration via an un-included Song
  navigation). Same root cause family as native add-songs and OS playlist mutations.
- Move (reorder), remove (entryIds are actually song ApiKeys), update (Name/IsPublic only),
  delete (204) all round-trip on the seeded playlist; DELETE /Items/{id} deletes owned
  playlists only (songs → 404); deleted playlist → clean 404.

## 7. Images

- Entities WITH stored artwork (fixture cover.png/artist.png): 200 with sniffed content type,
  `Cache-Control: public, max-age=86400`, quoted ETag, HEAD, If-None-Match → 304, size buckets
  via maxWidth/maxHeight (other image params ignored).
- Entities WITHOUT stored artwork: **404** — cross-surface difference: the OpenSubsonic
  `/images/` route serves default images instead. Jellyfin clients must tolerate artwork 404s.

## 8. Unsupported scope evidence (PRD-API-JF-005)

`/Videos`, `/LiveTv/Channels`, `/Shows/NextUp` (with markers) → 404 empty body. Non-music
includeItemTypes → silently empty. `Items/{id}/Refresh` → no-op 204. No explicit 501/410
anywhere on this surface — absence is plain 404, and that IS the compatibility boundary.

## 9. Known-client workflows (PRD-API-JF-006)

The captured route set covers the documented music-client smoke path: discovery (HEAD / with
MediaBrowser header, /System/Info/Public) → AuthenticateByName → UserViews → Items browsing
(includeItemTypes/parentId) → detail → direct stream + seek (range) → favorites/played →
sessions progress/stopped → playlists → logout. Real-client smoke (Finamp/Feishin/Streamyfin)
remains a Phase 8 verification activity; no client compatibility is claimed beyond the
captured server behavior.

## 10. Files and tests

- Scenarios: `compatibility/scenarios/jellyfin-{discovery,auth,browse,playback,userstate,playlists,images,extra}.json` (8 files).
- Golden: `compatibility/jellyfin/golden/` (+ `provenance.json` per dir).
- Inventory: `compatibility/jellyfin/contracts/route-inventory.json` — 60/60 routes covered
  (`scripts/build_jellyfin_inventory.py --check`).
- Tests: `tests/python/test_jellyfin_capture.py` (19 tests: secret scan, routing, auth,
  playback/range evidence, playlist evidence, 100% inventory coverage).
- The manifest generator's `[controller]` token substitution was fixed (Jellyfin controllers
  use `[Route("api/jf/[controller]")]`); manifests regenerated.

## 11. Privacy review

Golden contains no access tokens (masked at capture, structure-validated), no fixture password
outside documented synthetic scenario definitions, no placeholder baseUrl, no production data.
Enforced by `GoldenSecretScanTests` in `tests/python/test_jellyfin_capture.py`.
