# Phase 8 Report — OpenSubsonic and Jellyfin Adapters

**Status:** Captured-contract closure complete — both frozen surfaces replay in full with **zero
unexpected differences** (§ Full-golden differential replay). The only exit item outstanding is
the representative real-client smoke gate, which requires OpenSubsonic/Jellyfin client software
not available in the build environment.

**Evidence captured:** 2026-08-13

**Base commit:** `981763025c189ae3f5ae15edb9c9820be2689dca` plus the pending Jellyfin change set

**Authority:** SPEC §7.2/§7.5/§10 compatibility invariants, PRD-API-OS-001..004 and
PRD-API-JF-001..006, ADR-0102 (adapters stay separate products), ADR-0131 (Jellyfin token schema
and routing-middleware preservation), and the frozen Phase 0 captures under `compatibility/`.

## Current state

Both frozen adapter surfaces are mounted by `melodee-server` and every inventory row carries
route/endpoint-specific evidence:

| Surface | Rows | Implemented and differentially tested | Pending | Deferred |
| --- | ---: | ---: | ---: | ---: |
| OpenSubsonic endpoints | 81 | 81 | 0 | 0 |
| Jellyfin routes | 60 | 60 | 0 | 0 |

The joined views are `design/agents/PHASE-8-OPENSUBSONIC-ROUTE-MATRIX.json` and
`design/agents/PHASE-8-JELLYFIN-ROUTE-MATRIX.json`, generated and validated by
`scripts/build_phase8_route_matrix.py` from the frozen inventories and the per-route evidence
registries (`compatibility/*/contracts/phase8-route-evidence.json`). The validator rejects
unknown routes, missing legacy scenarios, missing Rust tests or requirement IDs, and
normalization/deferral claims without an accepted ADR.

The frozen legacy baseline remains commit `4028b58dbd3302ed431e2c5b32d9dde5634db218`.

## OpenSubsonic implementation

`apps/melodee-server/src/opensubsonic/` implements the captured surface (golden evidence
`compatibility/opensubsonic/golden/`):

- `auth.rs`/`envelope.rs`/`params.rs` — token+salt auth, the captured localhost/baseUrl/`/images/`
  bypass (ADR-0130 decision 7; no JWT branch, decision 1), JSON/XML/JSONP envelope writers with
  the captured XML quirks, and per-endpoint x-total-count semantics (list endpoints report their
  total; getArtist reports 1; stream error envelopes omit the header).
- `system.rs` — ping/getLicense/getMusicFolders/getOpenSubsonicExtensions with the byte-preserved
  extension advertisement (decision 2).
- `browse.rs` — getArtists/getIndexes/getMusicDirectory/getArtist/getAlbum/getSong/getTopSongs and
  the album/artist info endpoints with the captured shapes: directory artist playCount from user
  state sums, album `played` from dated play rows, exact-match getTopSongs, getAlbum-only
  contributor rendering (see the differential section), and the per-path duration rounding rules
  (half-to-even except the getArtist album entries, captured with truncation).
- `lists.rs` — getAlbumList/getAlbumList2/getStarred/getStarred2/getNowPlaying/getGenres/
  getSongsByGenre/search2/search3 with the captured sort orders, tiebreaks (decision 5), and
  now-playing field forcing.
- `annotation.rs` — star/unstar/setRating/scrobble plus the shared Child projection: global
  song playCount, row-existence `starred`/`userRating` rules (including the legacy
  `Nullable<DateTime>.ToString()` empty-string artifact), and runtime-recomputed averageRating.
- `media.rs` — stream/download with correct single-pass ranges (decision 4; the legacy
  double-range bug is not preserved), 404 + error 70 envelopes for invalid ranges/timeOffset,
  getCoverArt stored/default/304 classes (Cache-Control only on stored artwork), getAvatar with
  the captured default user image, and the captured lyrics serializers.
- `playlists.rs`/`bookmarks.rs`/`playqueue.rs`/`shares.rs`/`radio.rs`/`users.rs`/`podcasts.rs`/
  `scan.rs`/`negative.rs`/`jukebox` — the captured CRUD, role gates, error shapes, intentional
  410/501 classes, podcast raw-wrapper/conditional-off behavior, and `scanning: false` scan
  status.
- Playlist responses order entries by song creation (the legacy `GetPlaylistAsync` re-queried
  songs by ApiKey with no ORDER BY, so `PlaylistOrder` never reached the wire); updates apply
  cleanly (the legacy cover-resolution 302 rollback is phase-0 §7 do-not-preserve evidence).

### Differential evidence

`apps/melodee-server/tests/opensubsonic_*.rs` (7 files) run against PostgreSQL fixtures and
assert the golden-verified wire rules per endpoint group; the full-golden replay below exercises
every frozen scenario end to end.

## Jellyfin implementation (WP4)

`apps/melodee-server/src/jellyfin/` implements the captured music-focused surface:

- `mod.rs` — the captured routing middleware semantics (marker detection, pre-auth allowlist,
  marker-gated bare-root probe) plus dual mounting in root form and canonical `/api/jf` form,
  matching the frozen middleware rewrite (ADR-0131).
- `discovery.rs` — System/Info, System/Info/Public, and the captured 204 System/Ping (GET/POST).
- `auth.rs` — Pw-only AuthenticateByName (a `Password`-only body is the captured 400), locked
  ordering (credentials verified before the 403), 64-hex tokens stored as HMAC-SHA256 peppered
  digests with an 8-char lookup prefix, 168h expiry, max-10 active revocation, all five captured
  token transports, and idempotent logout.
- `browse.rs` — Items dispatch (includeItemTypes then the parentId probe), Artists/AlbumArtists,
  artist detail, genre-overlap Similar, Genres/MusicGenres, Filters (lowercase keys), item detail
  (artist → album → song probe, no ETag header), Similar/InstantMix with deterministic public-id
  ordering, Refresh 204 no-op, UserViews (configured libraries, else the captured synthetic
  "Music" view), and the `/Users/{id}/Items` 307 redirect pair with the captured query
  reconstruction.
- `audio.rs` — direct-play stream/stream.{container}/universal with the captured manual range
  parser (206 Content-Range; 416 carries `Content-Range: bytes */len` only for out-of-range or
  inverted starts, never for unparseable headers), HEAD headers-only, the transcoding-rejection
  400, `.lrc` lyrics with the captured timestamp semantics, HasDownloadRole-gated Download/File,
  and GET+POST PlaybackInfo.
- `playlists.rs` — playlist list/detail (ETag + 304), create/update/delete, items with 1-based
  ParentIndexNumber and UserData hydrated only with `enableUserData=true`, move, remove, and the
  DELETE /Items/{id} delete surface.
- `images.rs` — artist → album → song probe with the captured size buckets; the resolver is the
  single wiring point for durable artwork.
- `sessions.rs` — GET /Sessions synthesized from durable now-playing state; Playing/Progress feed
  it, Stopped scrobbles unless `Failed=true`, Ping/Capabilities are captured 204s.
- `userstate.rs` — FavoriteItems/PlayedItems with durable per-user counters, the self-only 403
  gate, and the artists-are-not-playable 404.
- `model.rs` — protocol-owned PascalCase DTOs (ADR-0102), N-format GUIDs,
  `yyyy-MM-ddTHH:mm:ss.fffffffZ` dates, and the exact legacy ETag MD5 preimages so cache
  identifiers are byte-identical at fixed data.

### Differential evidence

`apps/melodee-server/tests/jellyfin_core_api.rs` (11 tests) and
`apps/melodee-server/tests/jellyfin_media_api.rs` (14 tests) run against PostgreSQL fixtures
seeded through the public Phase 3–5 catalog handoff. They assert frozen-golden wire shapes
(recursive property-set/type comparison), captured status/detail strings, ETag/304 round trips,
deterministic ordering, role gates, and the 416 header nuances. The route matrix validator checks
every evidence row against these tests.

## Full-golden differential replay

`scripts/run_phase8_differential.py` closes the loop between the frozen captures and the real
server. For each surface it:

1. Boots the real `melodee-server` with the fixture environment (JWT key, token pepper, trusted
   proxy for the captured X-Forwarded-For, podcast toggle).
2. Seeds a disposable PostgreSQL database from `compatibility/fixtures/catalog-v1.json` with
   direct SQL (uuidv7 internal ids, uuid5-shard public ids, manifest/genre/bit-depth evidence
   rows, dated global play rows, per-user states, roles, playlists, and contribution rows —
   artist-linked in `contributions`, name-only in `observed_artists` with the role carried in
   `source_payload`).
3. Re-seeds (TRUNCATE + re-apply) before every scenario file, mirroring the legacy oracle
   capture pipeline, so annotation flows cannot leak state into list scenarios.
4. Replays every scenario step in-process and compares each fresh capture against the frozen
   golden with the capture normalization (volatile masking) applied.
5. Classifies every residual difference against the enumerated `KNOWN_DIFFERENCES` registry;
   anything unregistered fails the run. The report lands in
   `compatibility/captures/phase8-differential/differential-report.json`.

**Result (2026-08-13, both surfaces): 0 unexpected differences; 468 enumerated known
differences.** Known classes:

| Class | Count | Authority |
| --- | ---: | --- |
| content-length of otherwise-equal bodies (masked volatile values serialize with different byte lengths) | 145+ | transport-level, non-contractual |
| legacy NULL-SortName empty-index/omission artifacts (Rust requires non-empty sort values, migration 0007) | 71 | schema invariant |
| image URL fields embed the per-run origin (unmasked capture port) | 31 | capture volatility |
| updatePlaylist cover-resolution 302 rollback not preserved | 25 | phase-0 evidence §7 "MUST NOT be preserved" |
| fixture-seeded ratings bypassed legacy CalculatedRating columns | 15+ | capture-seeding artifact |
| deterministic Storage library/folder ids | 9 | ADR-0131 D10 family |
| redirect Location forwarded-host + credential strip | 8 | ADR-0130; capture volatility |
| legacy Album.OriginalReleaseDate column absent from the Rust schema | 6 | deferred schema ADR |
| legacy unsigned-JWT auth bypass rejected | 3 | R1 / ADR-0130 decision 1 |
| legacy double-range streaming bug not preserved | 6 | ADR-0130 decision 4 (D2/R2) |

(Counts are per-difference, not per-scenario; the table rounds the two largest classes.)

The replay surfaced and fixed four contract regressions that blanket registrations had masked:
getPlaylist's nil-uuid/inaccessible 302 class (vs the deleted-id 200 + error 44), the
updatePlaylist rename (dropped by misreading the cover-bug rollback artifact as a name-ignore
quirk), the orphaned `fold_artist_name` test after the fold moved to `lists.rs`, and the
Jellyfin auth 400 test asserting a non-frozen title. It also pinned previously unseen golden
semantics: playlist entries follow song creation order (legacy ignores `PlaylistOrder` on the
wire), only getAlbum loads `Song.Contributors` (compilation `artists[]`/`contributors[]`,
including `contributor_<NORMALIZED>` name-only pseudo-artists), and getArtist's resolved
response carries `x-total-count: 1`.

## Compatibility decisions and deviations

Exact legacy behavior is the default; the deviations are deliberate and bounded:

- **D4 playlist add (do-not-preserve).** The legacy add-to-nonempty-playlist path 500s (latent
  corruption). The adapter performs the clean append the native-v1 store provides; the empty-add
  204 no-op is preserved as captured. Covered by
  `playlist_full_lifecycle_and_d4_clean_add`.
- **Deterministic ordering (D10 family).** Legacy random Similar/InstantMix ordering and
  nondeterministic DB-order ties are replaced with stable public-id tiebreaks (ADR-0131 decision
  7). Repetition determinism is asserted by the differential tests.
- **Artwork 404s at this baseline.** The frozen fixture served 200 PNGs from its artwork store;
  the Rust catalog has no durable artwork bytes yet (same documented gap as the OpenSubsonic
  adapter), so catalog entities resolve to the captured no-artwork 404 classes
  ("Artist image not found." / "Album image not found." / "Song album image not found.").
  `images.rs::resolve_image` is the single wiring point; the 200-path shape (sniffed content
  type, `Cache-Control: public, max-age=86400`, ETag/304, HEAD) is implemented behind it.
- **Playlist move ownership hardening.** The legacy move handler did not scope to the owner;
  the adapter requires ownership like the other playlist mutations.
- **Library-scoped parentId.** The legacy partitioned songs by Library rows; the Rust catalog has
  a single canonical store, so a library parentId is accepted but not a filter.

## Data and migration impact

None beyond the accepted Phase 8 token schema: `jellyfin_access_tokens` (HMAC-SHA256 digests,
per-user salt, expiry, revocation) under ADR-0131. All catalog reads reuse Phase 4 tables; user
state reuses the Phase 7 native tables. No migration rewrites media or public ids.

## Test evidence

```text
python3 scripts/run_phase8_differential.py   # 0 unexpected, 468 enumerated known (both surfaces)
cargo test --workspace --all-features        # 669 passed, 0 failed, 4 ignored (+1 env-dependent skip)
python3 scripts/build_phase8_route_matrix.py # 81/81 OpenSubsonic + 60/60 Jellyfin validated
./scripts/check.sh                           # full workspace gate
```

## Remaining work before Phase 8 closes

- Representative real-client smoke runs (the second exit criterion): Finamp/Feishin/Streamyfin
  and an OpenSubsonic client against the documented smoke workflows, recorded like Phase 7's
  `REAL-CLIENT-ACCEPTANCE.md`. This requires client software/emulators not present in the build
  environment; every other exit criterion is met (matrix reproduced and tested, full-golden
  replay at zero unexpected differences, intentional non-music absences preserved).
- Durable artwork storage wiring for the images 200 paths (tracked with the artwork pipeline;
  the resolver hook is in place).

## Local gate note

The full gate steps pass on this change set: repository validation, requirements index, Python
capture tests (140), `cargo fmt --check`, `clippy -D warnings`, `cargo deny check`, and the
workspace test suite (671 passed, 0 failed). One pre-existing environment-dependent exception is
excluded locally: `crates/melodee-ingestion` `normalization.rs`
`real_mp3_flac_ogg_wav_unicode_artwork_and_sidecars_reach_published` asserts zero review
dispositions from real ffmpeg output; the host's ffmpeg 8.1.2 (newer than CI's distribution
ffmpeg) produces findings that route 4 items to `needs_review`, and the test binary then parks
instead of exiting after the panic. Both behaviors reproduce on the base commit tree (the change
set touches no ingestion/media code) and are unrelated to Phase 8; they are recorded here rather
than silently worked around.

## Risks and limitations

- The route-evidence validator is lexical (it rejects comments/literals/ignored symbols) rather
  than a complete Rust `cfg` evaluator.
- Genre and collection ETags with now-based preimages are volatile by design (masked in the
  frozen captures); cache hit rates for those two routes are lower than the legacy server only
  when data is static, never stale.
- `GET /Sessions` synthesizes from durable now-playing state; the legacy in-memory cache had a
  suspected staleness bug (empty after Playing reports) that is intentionally not reproduced.

Documentation impact:
- Updated — `docs/src/content/docs/api/opensubsonic.md` now describes the implemented,
  differentially verified OpenSubsonic surface (the previous stub said the matrix "will be
  published"); `docs/src/content/docs/api/jellyfin.md` and
  `docs/src/content/docs/clients/jellyfin-clients.md` describe the implemented surface;
  `design/requirements/status-overrides.json` (PRD-API-JF-001..005),
  `design/requirements/REQUIREMENTS-INDEX.md` and `requirements.json` (regenerated),
  `design/requirements/FEATURE-PARITY-MATRIX.md`,
  `compatibility/opensubsonic/contracts/phase8-route-evidence.json` (stale test references and
  rationales corrected to the golden-verified getTopSongs/getCoverArt/getAvatar behavior),
  `design/agents/PHASE-8-*-ROUTE-MATRIX.json` (regenerated), and this report. The differential
  replay changes are internal-contract refinements invisible to client documentation; `/docs`
  behavior descriptions remain accurate.
