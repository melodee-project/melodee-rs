# Phase 9 WP-07 — Last.fm integration and external provider legacy evidence

- **Purpose:** behavioral evidence for the WP-07 real Last.fm integration (scrobble
  + auth session lifecycle) and the external-provider inventory required by
  PRD-EXT-001..004. Governing legacy decisions: legacy ADR-0003
  (settings + caching) and legacy ADR-0004 (per-user session-key lifecycle).
  Governing Rust decisions: ADR-0134 §7 (`external_provider_cache`,
  `native_lastfm_links`) and `design/ROAD-TO-FEATURE-COMPLETION.md` WP-07 scope
  (captured "unconfigured" shapes remain the disabled-state contract).
- **Date:** 2026-08-14.
- **Source basis:** legacy oracle `/home/steven/src/github/melodee` source read
  at frozen commit `4028b58dbd3302ed431e2c5b32d9dde5634db218`, plus the frozen
  golden captures already in `compatibility/melodee-api-v1/golden/native-scrobble/`.
  Ground-up rewrite rule applies: the C# code below is behavioral evidence, not
  an architecture template.
- **Capture-gap limitation (PHASE-9-PLAN §6 risk 6):** configured-state golden
  capture of the Last.fm flows was **impossible** — the plan's oracle re-capture
  requires a real, configured Last.fm API application and test account, which is
  not available in this environment. Per the risk-6 fallback, the configured-state
  behavior below is documented from legacy source as behavioral evidence and this
  capture gap is recorded here as the documented limitation. The existing
  unconfigured goldens remain the authoritative disabled-state contract.

## 1. Legacy ADR-0004 — Last.fm session key lifecycle (per-user)

Source: `design/adr/ADR-0004-last-fm-session-key-lifecycle-per-user.md`
(Status: Accepted, 2025-12-13) in the legacy repo.

- Use the **web authentication flow** (`auth.getSession`) to obtain a session key.
  Explicitly **do not** use `auth.getMobileSession` — that would require
  collecting the user's Last.fm password, which legacy forbids.
- Store the session key **per Melodee user** in the database
  (`User.LastFmSessionKey`). Treat as a secret: never log it, never expose it
  via APIs.
- Runtime behavior:
  - Global Last.fm scrobbling enabled but the user has no session key →
    **success/no-op** (optional once-per-occurrence Debug/Warn log).
  - Last.fm returns an "invalid session" error during scrobble/now-playing →
    **clear `User.LastFmSessionKey`** and require re-linking.
- There is **no refresh flow**; re-authentication is the only recovery after
  revocation.

## 2. Legacy ADR-0003 — External integrations use Settings + caching

Source: `design/adr/ADR-0003-external-integrations-use-settings-caching.md`
(Status: Accepted, 2025-12-13).

- All external API keys/secrets/tokens are stored in the **Settings** table.
- Missing or invalid credentials ⇒ the integration is **disabled**; fail
  gracefully (no unhandled exceptions) and return an empty/neutral result.
- External API calls are cached via the DI-injected `ICacheManager`.

## 3. Auth / linking flow (token → session key)

All in `src/Melodee.Blazor/Controllers/Melodee/ScrobbleController.cs`
(controller attributes at lines 21–27: JWT bearer, `UserCapability.Scrobble`
capability via `MelodeeApiAuthFilter` (`src/Melodee.Blazor/Filters/MelodeeApiAuthFilter.cs:119`,
which maps the capability to `User.IsScrobblingEnabled`), `melodee-api` rate
limiter, route base `api/v{version}/scrobble`).

1. **Auth URL** (`GetLastFmAuthUrl`, lines 156–182):
   - `callback` query param must parse as an absolute `http`/`https` URI, else a
     validation `ApiError` is returned (line 168).
   - Reads `scrobbling.lastFm.apiKey` from Settings; blank →
     `400 "Last.fm not configured"` (lines 174–178).
   - Returns `200 { "url": "https://www.last.fm/api/auth/?api_key=<key>&cb=<urlencoded callback>" }`
     (lines 180–181). The server does **not** call `auth.getToken`; the client
     visits last.fm and posts the returned token back to Melodee.
2. **Session exchange** (`ExchangeLastFmSession`, lines 187–219; private
   `GetLastFmSessionKeyAsync`, lines 245–277):
   - Request body `{ "token": "<lastfm token>" }`
     (`Controllers/Melodee/Models/LastFmSessionRequest.cs`).
   - Server-side GET against the named `HttpClient "LastFm"` (base address
     `https://ws.audioscrobbler.com`, 10 s timeout —
     `src/Melodee.Blazor/Program.cs:217-221`):

     ```text
     2.0/?method=auth.getSession&api_key=<key>&token=<token>&api_sig=<sig>&format=json
     ```

   - **`api_sig` construction** (`BuildApiSignature`, lines 284–291):
     `MD5("api_key" + apiKey + "method" + "auth.getSession" + "token" + token + secret)`,
     lowercase hex, parameters concatenated with **no separators**, secret
     appended last. MD5 is mandated by the Last.fm Web Services Authentication
     protocol and annotated `lgtm[cs/weak-crypto]`; see
     <https://www.last.fm/api/authentication>. `HashHelper.cs:9-43` carries the
     same note (MD5 kept for OpenSubsonic/Last.fm compatibility only).
   - Response parsed as `{ "session": { "key", "name" } }`
     (`Controllers/Melodee/Models/LastFmSessionResponse.cs`); only
     `session.key` is used — the Last.fm username is **discarded**.
   - Missing apiKey/secret, non-2xx, or exception → null →
     `400 "Unable to exchange Last.fm session key"`. DB save failure →
     `400 <message or "Unable to save Last.fm session key">`.
   - Success → `200 { "message": "Last.fm linked" }`; the key is persisted and
     never returned.
3. **Storage:** `UserService.SetLastFmSessionKeyAsync`
   (`src/Melodee.Common/Services/UserService.cs:824-854`) writes
   `User.LastFmSessionKey` (empty strings nullified via `Nullify()`), bumps
   `LastUpdatedAt`, clears the user cache. Column is
   `character varying(64)` nullable — **plaintext, no encryption**
   (`src/Melodee.Common/Data/Models/User.cs:94-95`; migration
   `src/Melodee.Common/Migrations/20260524160636_InitialBaseline.cs:267`).
   ADR-0004 says "treat as a secret" but at-rest storage is plaintext —
   flagged as a **deliberate-improvement candidate** for the Rust port (§8).
4. **Disconnect** (lines 224–243): nulls the session key, returns
   `200 { "message": "Last.fm disconnected" }`. Idempotent: succeeds when
   nothing is linked and when Last.fm is unconfigured (matches the captured
   golden, §7).

## 4. Scrobble / now-playing submission

Implementation: `src/Melodee.Common/Plugins/Scrobbling/LastFmScrobbler.cs`
using the **Hqub.Last.fm v2.5.1** NuGet package
(`Directory.Packages.props:29`). The library builds signed POSTs to
`ws.audioscrobbler.com/2.0/` internally (it computes `api_sig` itself from the
configured key/secret and the per-user session key set on
`client.Session.SessionKey`).

Guard behavior (both methods, lines 32–44 and 78–90):

- apiKey or sharedSecret blank → return **success** (`Data = true`) with message
  "Last.fm scrobbling not configured."
- User has no session key → Debug log, return **success** with message
  "Last.fm not linked." (ADR-0004 silent no-op).

**NowPlaying** — `client.Track.UpdateNowPlayingAsync` (lines 51–56):

| Last.fm param | Legacy value |
|---|---|
| track | `scrobble.SongTitle` |
| artist | `scrobble.SongArtist` (the track artist string) |
| trackNumber | `scrobble.SongNumber ?? 0` |
| album | `scrobble.AlbumTitle` |
| albumArtist | `scrobble.ArtistName` (the release artist) |

No duration or mbid is sent for now-playing.

**Scrobble** — `client.Track.ScrobbleAsync` with a **single-element** batch
(lines 98–112):

| Last.fm param | Legacy value |
|---|---|
| track | `scrobble.SongTitle` |
| artist | `scrobble.SongArtist` |
| timestamp | `DateTime.UtcNow` **at processing time** — the client-submitted `ScrobbleRequest.Timestamp`/`time` values are ignored |
| album | `scrobble.AlbumTitle` |
| albumArtist | `scrobble.ArtistName` |
| mbid | `scrobble.SongMusicBrainzId?.ToString()` (nullable) |
| duration | song duration in seconds (int-parsed) |
| trackNumber | `scrobble.SongNumber ?? 0` |
| chosenByUser | `!scrobble.IsRandomizedScrobble` |

Result is success iff `scrobbleResult.Accepted > 0`. **No batching** (one track
per HTTP call; the OpenSubsonic multi-id path loops sequentially in
`OpenSubsonicApiService.ScrobbleAsync`, lines 2007–2072), **no retry, no
offline queue/spool**, and **no plugin-level rate limiting** on scrobbles — the
named LastFm `HttpClient` has only a 10 s timeout, and the server endpoints sit
under the `"melodee-api"` rate limiter.

Orchestration (`src/Melodee.Common/Services/ScrobbleService.cs`): scrobblers
run ordered by `SortOrder` — `MelodeeScrobbler` (0, internal play
counts/history, always enabled; `Plugins/Scrobbling/MelodeeScrobbler.cs`) then
`LastFmScrobbler` (1, gated on `scrobbling.lastFm.Enabled`,
`ScrobbleService.cs:50-53`). Completed scrobbles use the full song duration as
seconds played (line 160). All three API surfaces funnel here: native v1
`POST /api/v1/scrobble`, OpenSubsonic `/rest/scrobble[.view]`, and Jellyfin
`Sessions/Playing{,/Stopped}`
(`src/Melodee.Blazor/Controllers/Jellyfin/SessionsController.cs:78-186`).

## 5. Failure semantics

- `Hqub.Lastfm.NotAuthenticatedException` → `ClearSessionKeyAsync`
  (`LastFmScrobbler.cs:137-148`): nulls `User.LastFmSessionKey`, bumps
  `LastUpdatedAt`, Warn log; result false. This is the ADR-0004
  clear-on-invalid-session rule.
- Any other exception → Error log, result false; **the scrobble is dropped**
  (no retry, no queue).
- In `ScrobbleService.Scrobble` (`ScrobbleService.cs:191-211`), a scrobbler
  that *throws* (rather than returning false) sets the overall result false and
  **breaks** the scrobbler loop. `LastFmScrobbler` catches its own exceptions,
  so Last.fm failures degrade to a false result rather than aborting the
  internal `MelodeeScrobbler` (which runs first anyway).
- Legacy has **no playback-path coupling**: scrobble failure never affects
  streaming — it surfaces only as a `400` on the scrobble endpoint itself.

## 6. Configuration keys

`src/Melodee.Common/Constants/SettingRegistry.cs:128-155`; seeds in
`src/Melodee.Common/Data/MelodeeDbContext.cs:1068-1109`.

| Key | Seed default | Purpose |
|---|---|---|
| `scrobbling.enabled` | `true` | "Is scrobbling enabled." **Dead config**: never read in the scrobble path — only surfaced in DoctorService (`src/Melodee.Blazor/Services/DoctorService.cs`, `DoctorServiceBase.cs:190`). The real per-user gate is `User.IsScrobblingEnabled` via the `Scrobble` capability. |
| `scrobbling.lastFm.Enabled` | `false` | Enables `LastFmScrobbler` (`ScrobbleService.cs:50-53`). **Capital-E quirk** — sibling key `scrobbling.lastFm.apiKey` is lowercase; keep the exact legacy spelling for settings compatibility. |
| `scrobbling.lastFm.apiKey` | `""` | Last.fm API key. Also reused by the Last.fm **search-engine** plugin. |
| `scrobbling.lastFm.sharedSecret` | `""` | Shared secret for `api_sig` signing. |
| `searchEngine.lastFm.Enabled` | — | Gates the Last.fm metadata plugin (`ArtistSearchEngineService.cs:206-208`, `AlbumImageSearchEngineService.cs:62-64`). Also capital-E. |

Decision for WP-07 (recorded here per the task assignment): **`scrobbling.enabled`
is preserved as a recognized setting but not enforced**, matching legacy exactly.
Enforcing it would be a behavioral divergence from the oracle; surfacing it in a
doctor/diagnostics surface remains appropriate.

## 7. Server routes (native v1) and existing goldens

Route base `api/v{version}/scrobble`, JWT + `UserCapability.Scrobble` +
`melodee-api` rate limiting. Confirmed against
`compatibility/melodee-api-v1/contracts/route-manifest.json` and
`openapi-v1.json` in this repo.

| Route | Request | Success shape | Unconfigured / error shape |
|---|---|---|---|
| `GET /api/v1/scrobble/lastfm/auth-url?callback=<abs uri>` | query `callback` | `200 { "url": "https://www.last.fm/api/auth/?api_key=…&cb=…" }` (source-evidenced only — see capture-gap note) | invalid callback scheme → validation `ApiError`; missing apiKey → `400 { "code": "BAD_REQUEST", "message": "Last.fm not configured", "correlationId": "…" }` — golden `native-scrobble/native-scrobble-lastfm-auth-url-unconfigured.json` |
| `POST /api/v1/scrobble/lastfm/session` | `{ "token": "…" }` | `200 { "message": "Last.fm linked" }` (source-evidenced only) | exchange failure/unconfigured → `400 { "code": "BAD_REQUEST", "message": "Unable to exchange Last.fm session key", "correlationId": "…" }` — golden `native-scrobble/native-scrobble-lastfm-session-unconfigured.json` |
| `POST /api/v1/scrobble/lastfm/disconnect` | none | `200 { "message": "Last.fm disconnected" }` — golden `native-scrobble/native-scrobble-lastfm-disconnect.json` (idempotent, succeeds unconfigured and unlinked) | — |

Capturing scenario: `compatibility/scenarios/native-scrobble-extra.json`
(`native-scrobble-lastfm` flow). The captured unconfigured shapes **remain the
disabled-state contract** for WP-07 (PHASE-9-PLAN §1.3 row and §3 Last.fm
decision).

## 8. Last.fm as a metadata/search provider (caching and provenance)

`src/Melodee.Common/Plugins/SearchEngine/LastFm/LastFm.cs` implements
`IArtistSearchEnginePlugin`, `IArtistTopSongsSearchEnginePlugin` (top songs
**not implemented** — returns empty with a "LastFm Not implemented" message,
lines 218–222), and `IAlbumImageSearchEnginePlugin`:

- `artist.search`: GET `ws.audioscrobbler.com/2.0/?method=artist.search&…&format=json&limit=…`,
  cached **6 hours** via `ICacheManager` under key
  `lastfm:artist:{normalizedName}:{maxResults}` (lines 139, 214). Exact
  normalized-name matches rank 10, others 1. Results capture MusicBrainzId,
  image URLs, and a `LastFmId` extracted from the last URL path segment
  (`ExtractLastFmId`, lines 224–246).
- `album.Search` (album images): **no cache wrapper** — inconsistent with
  ADR-0003's caching guidance. Filters to exact normalized artist match;
  thumbnail = `large`, media = `extralarge` with a `300x300 → 600x600` URL
  rewrite (lines 71–73).
- Provenance: `Artist.LastFmId` / `Album.LastFmId` columns
  (`ArtistService.cs:684`, `AlbumService.cs:604`); surfaced to OpenSubsonic
  clients as `lastFmUrl = https://www.last.fm/music/{LastFmId}`
  (`OpenSubsonicApiService.cs:3412-3428`). There is no separate provenance
  table in legacy — just the ID column.

## 9. External provider inventory (PRD-EXT-001/002)

All credentials live in the Settings table per legacy ADR-0003; missing
credentials ⇒ graceful no-op. Shared infrastructure:
`searchEngine.userAgent`, `searchEngine.defaultPageSize`,
`searchEngine.maximumAllowedPageSize`; shared Polly pipeline
`src/Melodee.Common/Plugins/SearchEngine/SearchEnginePolicies.cs` (3 retries,
exponential backoff + jitter, retries network exceptions/timeouts/5xx/429,
honors `Retry-After`). Plugin composition:
`src/Melodee.Common/Services/SearchEngines/{ArtistSearchEngineService,ArtistImageSearchEngineService,AlbumImageSearchEngineService}.cs`.

| Provider | Purpose | Config keys | Legacy code location |
|---|---|---|---|
| Last.fm | Scrobble + now-playing; artist search; album images; `LastFmId` provenance; top-songs stub (unimplemented) | `scrobbling.lastFm.Enabled`, `scrobbling.lastFm.apiKey`, `scrobbling.lastFm.sharedSecret`, `searchEngine.lastFm.Enabled`, `scrobbling.enabled` (dead) | `Plugins/Scrobbling/LastFmScrobbler.cs`; `Plugins/SearchEngine/LastFm/LastFm.cs`; `Controllers/Melodee/ScrobbleController.cs` |
| MusicBrainz | Artist search against a **local imported decentdb replica** (bulk importer with batch/timestamp settings); Cover Art Archive album images; MBIDs flow into scrobbles and Last.fm mbid param | `searchEngine.musicbrainz.enabled`, `.importMaximumToProcess`, `.importBatchSize`, `.storagePath`, `.importLastImportTimestamp`, `searchEngine.artistSearchDatabaseRefreshInDays` | `Plugins/SearchEngine/MusicBrainz/**` (incl. `Data/DecentDBStreamingMusicBrainzImporter.cs`, `MusicBrainzCoverArtArchiveSearchEngine.cs`) |
| Spotify (SpotifyAPI.Web 7.4.2) | Artist search, top songs, artist/album images; client-credentials token fetched on demand and persisted back into Settings; semaphore-2 concurrency + 429 cooldown honoring Retry-After | `searchEngine.spotify.enabled`, `searchEngine.spotify.apiKey` (client id), `searchEngine.spotify.sharedSecret`, `searchEngine.spotify.accessToken` | `Plugins/SearchEngine/Spotify/Spotify.cs`, `SpotifyClientBuilder.cs` |
| Discogs | Artist search (`api.discogs.com/database/search`, optional `Discogs token=` auth header); 2 h cache, concurrency 2, Polly retry | `searchEngine.discogs.enabled`, `searchEngine.discogs.userToken`, `searchEngine.userAgent` | `Plugins/SearchEngine/Discogs/DiscogsArtistSearchEnginePlugin.cs` |
| Wikidata | Artist search via SPARQL, images/descriptions; 2 h cache, concurrency 2, Polly retry | `searchEngine.wikidata.enabled`, `searchEngine.userAgent` | `Plugins/SearchEngine/WikiData/WikiDataArtistSearchEnginePlugin.cs` |
| iTunes/Apple | Album + artist images, artist search (iTunes Search API, no key) | `searchEngine.itunes.enabled` | `Plugins/SearchEngine/ITunes/ITunesSearchEngine.cs` |
| Deezer | Album + artist images (public API, no key) | `searchEngine.deezer.enabled` | `Plugins/SearchEngine/Deezer/DeezerSearchEngine.cs` |
| Brave Search | Artist + album image search | `searchEngine.brave.enabled`, `searchEngine.brave.apiKey`, `searchEngine.brave.baseUrl`, `searchEngine.brave.imageSearchPath` | `Plugins/SearchEngine/Brave/{BraveArtistImageSearchEnginePlugin,BraveAlbumImageSearchEnginePlugin,BraveSearchClient}.cs` |
| Metal-API | Artist + album images for metal artists | `searchEngine.metalApi.enabled` | `Plugins/SearchEngine/MetalApi/**` |
| Melodee (internal) | Artist search + top songs from own database; SortOrder 0 fallback — **not external** | — | `Plugins/SearchEngine/MelodeeArtistSearchEnginePlugin.cs` |

"AMG" (named in PRD-EXT-001's prose inventory) has **no implementation** in the
legacy codebase.

## 10. Rust implementation guidance for WP-07

1. **Signature scheme:** replicate the Last.fm `api_sig` construction exactly —
   parameter name/value pairs concatenated without separators in sorted order,
   shared secret appended last, MD5 lowercase hex (legacy evidence §3;
   <https://www.last.fm/api/authentication>). MD5 here is external-protocol
   mandated, same compatibility category as the OpenSubsonic salted-password
   MD5 already in the tree; document it at the use site.
2. **Session invalidation:** treat Last.fm API errors 4 (authentication
   failed), 9 (invalid session key), and 10 (invalid API key) as
   clear-session triggers, matching legacy `NotAuthenticatedException →
   ClearSessionKeyAsync` (§5); re-linking is the only recovery (legacy
   ADR-0004: no refresh flow).
3. **Session-key storage (deliberate improvement):** store only a **digest** of
   the session key in `native_lastfm_links.session_key_digest` per ADR-0134 §7
   — never the raw key at rest in that table — improving on legacy's plaintext
   `varchar(64)` (§3 item 3). Record this improvement explicitly in the WP-07
   PR. Note for the PR author: scrobble signing needs the raw key at runtime,
   so the PR must state where the usable key is held (e.g., an encrypted secret
   store or in-memory-only retrieval at link time) and keep the raw key out of
   logs, traces, and API responses, as legacy ADR-0004 requires.
4. **Timestamp quirk (preserve):** legacy stamps scrobbles with
   `DateTime.UtcNow` at processing time and ignores client-submitted
   timestamps (§4). Preserve this as a **documented known quirk** in WP-07;
   changing it alters user-visible Last.fm history semantics and belongs in a
   separate compatibility decision.
5. **Fail-degraded:** scrobble submission must never break or delay the
   playback/streaming path (§5: legacy drops failed scrobbles; only the
   scrobble endpoint itself reports failure). The native/OS/JF surfaces share
   one scrobble pipeline, as in legacy.
6. **Caching and rate limiting:** provider responses cache in
   `external_provider_cache` per ADR-0134 §7 (provider payloads are
   cache/evidence only — never catalog identity, PRD-ARTIST-012). Honor the
   legacy TTLs as starting points (artist search 6 h; Discogs/Wikidata 2 h)
   and the shared retry posture (bounded retries, exponential backoff, honor
   `Retry-After`, no 4xx retry other than 429).
7. **No-provider-required invariant (PRD-EXT-002):** a structurally valid local
   Release enters the catalog with zero external providers configured; every
   provider gate follows legacy ADR-0003 — missing/invalid credentials ⇒
   disabled ⇒ empty/neutral result, never an ingestion failure.
8. **Provenance (PRD-EXT-004):** record which provider supplied/influenced
   metadata (legacy's `LastFmId`/MBID columns are the minimum bar); provider
   payloads in `external_provider_cache` carry the evidence trail.
9. **Settings:** keep the exact legacy key spellings, including the capital-E
   `scrobbling.lastFm.Enabled` / `searchEngine.lastFm.Enabled` quirk, and keep
   `scrobbling.enabled` recognized-but-unenforced (§6).
10. **Disabled-state contract:** the captured unconfigured golden shapes (§7)
    remain authoritative; the Rust routes must reproduce them byte-for-byte
    after normalization when Last.fm is not configured.

## 11. Uncertainties / gaps

- **Configured-state golden shapes** (`{url}` for auth-url,
  `{"message":"Last.fm linked"}` for session) are source-evidenced only; see
  the capture-gap limitation in the front matter. If a configured Last.fm test
  application becomes available, re-capture per
  `design/runbooks/PHASE-0-LEGACY-CAPTURE.md` and amend this document.
- **Hqub.Last.fm internals** (exact POST body encoding/parameter ordering for
  `track.scrobble`/`track.updateNowPlaying`, and the library's mapping of
  Last.fm error codes to `NotAuthenticatedException`) live inside the NuGet
  package, not the legacy repo; guidance §10.2 covers the expected mapping but
  should be validated against a mocked Last.fm wire fixture in WP-07.
- **Album-image search is uncached in legacy** (§8) despite ADR-0003; the Rust
  port should cache it in `external_provider_cache` and note the divergence.
- **No scrobble retry/queue exists in legacy**; adding one would be a new
  feature requiring its own ADR, not parity work.
