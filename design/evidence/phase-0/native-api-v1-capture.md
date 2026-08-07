# Phase 0 Evidence — Native Melodee API v1 Critical-Path Capture

**Issue:** FIRST-10-ISSUES.md Issue 5 — Capture native Melodee API v1 critical path
**Requirements:** PRD-API-M1-001 through PRD-API-M1-006, PRD-STREAM-001 through PRD-STREAM-003
**Baseline:** legacy commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (**authoritative oracle
identifier — the branch name `sph.2026-07-11.01` is recorded but never used as the
compatibility identifier**); server version `2.2.0`; DB migration head
`20260714155936_AddDeleteSourceResidueAfterIngestSetting`
**Recorded:** 2026-08-07
**Golden evidence:** `compatibility/melodee-api-v1/golden/{native-auth,native-catalog,native-stream}/`
(each with `provenance.json`)

## 1. Reproducibility (review item 10)

The oracle no longer depends on any long-lived process. One command recreates it from nothing
but the frozen checkout and the dev Postgres:

```bash
python3 scripts/legacy_oracle.py recreate   # SHA check -> build -> reset-db -> seed -> start -> verify
python3 scripts/legacy_oracle.py capture    # reseed + capture all three scenario files
```

`recreate` **fails closed** if the oracle checkout is not exactly
`4028b58dbd3302ed431e2c5b32d9dde5634db218`. The full pipeline was executed repeatedly during
this issue, including from a dropped database; two consecutive `capture` runs compare with
zero differences after declared normalization (`scripts/compare_captures.py`).

Fixture dataset: `compatibility/fixtures/catalog-v1.json` — 10 artists / 10 albums / 19 songs /
2 playlists / 5 contributors, with deterministic public ApiKeys
(`uuid5(NAMESPACE_URL, 'https://melodee.dev/phase0-fixtures/v1')` of `<kind>:<slug>`), synthetic
deterministic WAV media, staggered `CreatedAt`/`LastPlayedAt` (tie-ordering determinism), and a
seeded expired refresh token. Covers: ordinary solo artist; **two artists sharing a normalized
name** (`Dune`/`Düné` — exact-duplicate names are impossible: `IX_Artists_Name` is UNIQUE);
multi-artist Release; Various Artists compilation; featured/guest credit; group/member
(`ArtistRelation`); Unicode (diacritics + CJK + emoji-adjacent genre `環境音楽`); multi-disc
(disc-qualified song numbers 101/102/201/202 + `PartTitles`; the schema has **no disc column**
and `(AlbumId,SongNumber)` is unique); minimal/missing metadata; seeded stars/ratings/hates;
playlists spanning four Releases.

Canonical configuration: `system.baseUrl=http://127.0.0.1:5157` is set at seed time (the legacy
`** REQUIRED: THIS MUST BE EDITED **` placeholder can never contaminate captures — the verify
step asserts its absence) and origins are aliased to `{melodee-origin}` in stored bodies and
recorded URLs. `security_secretKey` must be provided as an environment variable
(`security_secretKey`, single underscore → dot mapping in `MelodeeConfigurationFactory`) or
`mcli user create` and the server fail with "security.secretKey configuration is required" —
this requirement was undocumented until this issue.

## 2. Capture tooling changes (schemaVersion 2 scenarios)

`scripts/capture_baseline.py` now supports: multi-step flows with `{{variable}}` substitution
and `capture`/`captureRegex` extraction; `expectStatus`; `bearer-var:` auth; `{{env:VAR}}`
injection (expired fixture JWT never touches disk); HEAD requests; Range headers;
`followRedirects: false`. Sanitization is enforced **at capture time**, before anything is
written: sensitive query keys redacted; captured secrets unsubstituted to `<captured:name>`;
Authorization header values always masked; the stream HMAC path segment masked everywhere;
`maskJsonPaths` applied to stored bodies; origin aliasing. Token **values** are never golden —
`normalize.structureChecks` (`jwt`, `streamAuthToken`, `opaqueToken`) validate and record
structure instead (algorithm, claims, issuer/audience, expiry semantics, URL binding).

## 3. Authentication findings (golden: `native-auth/`, 24 steps)

| Behavior | Evidence |
|---|---|
| Login success | 200; `AuthenticationResponse` with user model, `serverVersion`, JWT, refresh token + expiries |
| Invalid password vs **unknown user** | **Identical** envelopes (`UNAUTHORIZED` / "Invalid credentials") — no user enumeration |
| Missing / malformed / expired access token | All 401 (`UNAUTHORIZED`); expired fixture JWT (HS256, `MelodeeApi`/`MelodeeClient`, `ValidateLifetime`, zero clock skew) rejected |
| Access-token request | `GET /user/me` 200 with roles array |
| Refresh rotation | `POST /auth/refresh-token` rotates; old token revoked with reason `rotated` |
| **Replay** | Reusing a rotated token → 401 `refresh_token_replayed` and **the entire token family is revoked** (`RevokeOnReplay=true`), so the legitimately-rotated successor is also dead — clients that lose a rotation race must re-login |
| Legacy refresh (MeloAmp) | `POST /auth/refresh` requires a **valid, unexpired** JWT — the controller remark claiming "potentially expired" JWTs work is **wrong** (401 captured with an expired fixture JWT) |
| Expired refresh token | 401 `refresh_token_invalid` ("Refresh token has expired.") — distinct from `refresh_token_replayed` and from unknown-token `refresh_token_invalid` ("Refresh token is invalid.") — message differs, code identical |
| Logout | 200; revokes **all** refresh tokens (`reason: logout`) |
| **Access token after logout** | **Still works** (200 on `/user/me`) — JWTs are stateless; logout only kills refresh tokens |
| Refresh after logout | 401 `refresh_token_replayed` |
| Revoke specific token | 200; subsequent use → 401 `refresh_token_replayed` (revoked use is treated as replay and nukes the family) |

JWT contract: HS256; iss `MelodeeApi`; aud `MelodeeClient`; claims email/name/sid (user ApiKey)/
role/theme; access lifetime 15 min; refresh lifetime 30 days; family-scoped rotation with
absolute session cap (`MaxSessionDays`).

## 4. Streaming findings (golden: `native-stream/`, 17 steps)

URL shape (from `Song.streamUrl`, never client-constructed):

```text
{baseUrl}/song/stream/{songApiKey}/{userApiKey}/{urlencode(base64(
    "{userGuid}:{songGuid}:{clientIp}|{expiryUnixSeconds}|{base64(HMAC-SHA256)}"
))}
```

Secret = the user's `PublicKey`; expiry = 7 days (`expiresInDays: 7` recorded); clientBinding =
client IP (`binding: "127.0.0.1"` recorded). Structure is validated per capture
(`structureChecks`), including `userMatchesUrl`/`songMatchesUrl`.

| Request | Result |
|---|---|
| HEAD | 200, `Accept-Ranges: bytes`, `Content-Length` = full size, `Content-Type: audio/wav`, no body |
| Full GET | 200, `Accept-Ranges: bytes`, `Content-Length` = size, `Last-Modified` present |
| `Range: bytes=0-1023` | 206, `Content-Range: bytes 0-1023/220544`, exact 1024 bytes |
| Middle / open-end / suffix (`bytes=-2048`) ranges | 206 with clamped `Content-Range` (end clamped to size-1) |
| Malformed / unsatisfiable / inverted / **multipart** ranges | **400** (`BAD_REQUEST` "Unable to load song") — **not RFC 7233 416**; multipart not supported |
| Valid token for wrong song | 401 (`userGuid`/`songGuid` binding check) |
| Unknown user GUID | 401 |
| `?jwt=<token>` appended (MeloAmp) | Tolerated; 206 as normal |
| **Corrupted base64 token** | **302** to `/Account/Login?ReturnUrl=%2FError` — `Convert.FromBase64String` throws; the Blazor error pipeline answers a redirect, not JSON |
| **`Guid.Empty` userApiKey** | **302** (same exception pipeline) while a *random* unknown GUID gets a clean 401 |

Streaming-limit/concurrency behavior (`StreamingLimiter`, 429 path) was not triggered in the
fixture environment (single-client captures); recorded as a gap for the performance phase.

## 5. Catalog findings (golden: `native-catalog/`, 77 steps)

- **Envelope:** list endpoints return `{meta:{totalCount,pageSize,currentPage,totalPages,
  hasPrevious,hasNext}, data:[...]}` — the shape both clients deserialize.
- **orderBy values are C# property names** (`Name`, `ReleaseDate`, `Title`, …); lowercase
  `name` → 400 `VALIDATION_ERROR` "Invalid orderBy value".
- **Unknown-ApiKey detail = 500, not 404.** `GET /artists|albums|songs|playlists/{unknown-guid}`
  all return `INTERNAL_ERROR` despite `ProducesResponseType(404)` and unreachable
  `ApiNotFound` code paths. Candidate known-bug **not** to preserve (product decision needed);
  note contrast: a *deleted* playlist returns a clean 404 (`NOT_FOUND` "Playlist not found")
  because the playlist service handles not-found explicitly.
- **Name-collision:** `q=Dune` returns both `Dune` and `Düné` (shared `NameNormalized=DUNE`)
  as separate identities — the exact-duplicate case is schema-impossible (`IX_Artists_Name`
  UNIQUE); normalized-name collisions are the representable hazard and must never become a
  unique key (matches the repository data-safety rules).
- **Star/hate/rating** round-trips verified for songs/albums/artists (path-verb toggles).
  `userHated` is **not exposed** in the song/artist/album detail models — hate state is
  write-only from the client's perspective except via `/user/{entity}/disliked`.
- **Scrobble:** 200 for nowPlaying/played; DB play counts increment via the MelodeeScrobbler
  plugin — **but API payloads keep serving the stale count** (`SongService` caches
  `urn:song:{id}`; the scrobbler does not invalidate it). playCount in API responses is
  eventually consistent. Unknown songId → 500 (same unknown-id class).
- **Queue:** `/api/v1/queue` exists in source but is **excluded from compilation**
  (`<Compile Remove>` in `Melodee.Blazor.csproj`, together with all Party controllers) → 404
  with empty body. The static route manifest now records `excluded-from-build` (generator
  updated); 35 routes affected. Neither native client uses the queue.
- **Playlist CRUD:** create/reorder/delete work; reorder round-trips; **add-songs is broken in
  both body forms** (object → 400; correct bare `Guid[]` → 500 null-navigation). MeloAmp sends
  the object form (client/server mismatch — see native-client-route-inventory.md finding 5).
- **Images:** `/images/{type}_{apiKey}/{size}` anonymously fetchable (default images served for
  fixture entities); unknown image id → 200 fallback image, not 404. The route belongs to the
  OpenSubsonic surface (cross-surface dependency).
- **Recents:** `albums/recent` and `artists/recent` are *recently added* (`CreatedAt` DESC)
  with **no stable tiebreak** — identical timestamps yield nondeterministic order across runs
  (found via A/B capture drift; fixture now staggers `CreatedAt`, and the ordering instability
  itself is recorded here as compatibility data: clients must not depend on tie order).
- **Playlist model timestamps are ISO-8601 (`2026-08-07T20:55:38Z`)** while song/artist/album
  models use user-culture `yyyy-MM-dd HH:mm:ss` with `---` for null — an intra-API format
  inconsistency preserved in golden bodies. Nested `artist` inside album payloads can carry the
  blank-artist defaults (`createdAt: "9999-01-01 00:00:00"`) — payloads are not fully hydrated.

## 6. Public IDs (PRD-API-M1-004)

Golden fixtures demonstrate stable, deterministic public identifiers across reseeds:
fixture user `f78448cc-82ec-5864-877c-6c6163b00065`, artists/albums/songs/playlists from the
fixture UUID namespace. ID shapes: native detail routes use bare GUIDs; image URLs use
`{type}_{guid}`; playlist song entries carry both internal order and `songApiKey`. The future
Rust migration must preserve these values verbatim regardless of internal identity choice.

## 7. Unified contract inventory (review item 2)

`compatibility/melodee-api-v1/contracts/route-inventory.json` (generated by
`scripts/build_native_inventory.py`, `--check` for drift) records per route: method/path,
controller/action, auth, `inOpenApi`, `usedBy` (client evidence), scenario coverage, and
`parityStatus`:

| parityStatus | routes |
|---|---:|
| captured | 34 |
| client-used-uncaptured | 69 |
| documented-uncaptured | 45 |
| undocumented-uncaptured | 19 |
| excluded-from-build | 35 |

The published OpenAPI document is treated as one input, never the authority (it omits
queue/party/podcasts routes; party+queue are additionally not compiled at all).
`client-used-uncaptured` (MeloAmp's non-critical surfaces: shares, themes, requests,
playback-backend, equalizer, analytics, charts, recommendations, smart playlists, user-library
lists) is the prioritized backlog for later-phase capture.

## 8. Scenario and golden inventory

| File | Scenarios | Steps | Golden captures |
|---|---:|---:|---:|
| `compatibility/scenarios/native-auth.json` | 11 | 24 | `golden/native-auth/` (24) |
| `compatibility/scenarios/native-catalog.json` | 6 | 77 | `golden/native-catalog/` (77) |
| `compatibility/scenarios/native-stream.json` | 1 | 17 | `golden/native-stream/` (17) |

Legacy `Album`/`Song` vocabulary is preserved deliberately at the API boundary
(PRD-API-M1-003); `Release`/`Track` appear only in internal reasoning, never in captured
contracts.

## 9. Privacy review

Golden contains: no tokens (values masked, structure recorded), no fixture password, no
placeholder baseUrl, no raw origins, no production data. Enforced by
`GoldenFixtureTests.test_no_credential_material_anywhere` in
`tests/python/test_native_v1_capture.py` (JWT-shape regex, password scan, placeholder scan,
origin scan over every golden file).
