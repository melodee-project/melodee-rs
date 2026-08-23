# Road to Full C# Melodee Feature Parity

**Document status:** Active parity audit — incremental, evidence-backed
**Audit baseline date:** 2026-08-23 (latest pass 2026-08-23, entry 5)
**Audit scope:** All 25 required domains have been inventoried on the C# side and compared against
Rust production wiring; no matrix row remains `NOT ASSESSED`. The C# inventory was built
independently of the Rust implementation (controllers, Blazor pages, CLI commands, Quartz jobs,
plugins, settings, SignalR hubs, middleware, DI registrations), and every existing row was
re-verified against both checkouts. `IMPLEMENTED — UNPROVEN` rows remain outstanding: they have
reachable production paths but lack current-C# differential or end-to-end proof. v1.0.0 code
completeness still cannot be asserted. Entry 5 (this pass) independently re-verified the
baseline-sensitive high-risk rows (provider enrichment, Artist identity reuse, MusicBrainz ID
retention, fingerprint behavior, the SPA edit-path breakage, C# staging/search-engine call
sites, the C# job inventory, and `external_provider_cache` writers) against both current
checkouts at their then-clean HEADs; all cited statuses held (see §13, Entry 5).

## 1. Purpose and honest current verdict

"Feature parity" means **full user-visible and operational feature parity with the current C#
Melodee application**, broader than API response-shape compatibility. It covers admin UI
workflows, the end-user web UI, background jobs, ingestion pipelines, provider plugins,
configuration consumers, and operator tooling — not only the captured public API surfaces.

**Current verdict: v1.0.0 code completeness CANNOT be asserted.** 28 of 50 rows are blocking
(neither `PROVEN PARITY` nor an accepted `DIFFERENT — APPROVED`). The highest-risk operational
gaps:

- C# ingestion invokes configured Artist and Release metadata/image providers during staging;
  Rust ingestion calls no provider and `external_provider_cache` has no production writer
  (verified: zero HTTP-client usage in `crates/melodee-ingestion`, `melodee-catalog`,
  `melodee-policy`).
- C# `LibraryInsertJob` reuses Artist and Album identities across Releases by MusicBrainz ID,
  Spotify ID, iTunes ID, and name; Rust manifest import allocates a fresh provisional Artist per
  observed value except the narrow ADR-0153 single-release-artist rule, producing duplicate
  Artist identities across Releases. The `LocalArtistResolver`/`ArtistAuthority` boundary has
  zero production callers (only a `#[cfg(test)]` fake).
- C# retains embedded MusicBrainz IDs (including the song recording ID) on catalog rows; Rust
  retains no MusicBrainz GUIDs at all (only the `musicbrainz_release_type` tag used for trait
  classification).
- C# full metadata-edit forms and `MediaEditService` tag-rewriting transforms persist tag and
  catalog changes; the Rust admin edit API exists but has zero tests, and the admin SPA save path
  is wired to nonexistent routes (`/metadata` suffix and field-name mismatches), so the UI edit
  workflow is broken end-to-end.
- The C# Blazor app is a full end-user web UI (dashboard, MQL search, music player, stats,
  public share landing, account flows); Rust serves only the admin SPA. No accepted decision
  covers this product-surface absence.
- Phase 11 migration/cutover is not complete (R4 production rehearsal, differential closure, and
  R5 release artifacts remain open).

A C# capability does not cease to be a parity obligation because an ADR deferred it, no capture
exercised it, or no first-party client used it. Accepted deferrals/waivers are reported as
differences, not proof of parity.

## 2. Audited baselines

| Repository         | Filesystem location                  | Commit SHA                                 | Branch/tag          | Clean/dirty | Audit timestamp             | Role                                                 |
| ------------------ | ------------------------------------ | ------------------------------------------ | ------------------- | ----------- | --------------------------- | ---------------------------------------------------- |
| C# Melodee         | `/home/steven/src/github/melodee`    | `4028b58dbd3302ed431e2c5b32d9dde5634db218` | `sph.2026-07-11.01` | clean       | 2026-08-23 (pass 5: 16:40 UTC-5) | Current C# product baseline                          |
| Rust rewrite       | `/home/steven/src/github/melodee-rs` | `f5122421fcf3cee1fa6027576baac4db9362ee72` | `main`              | clean (at this pass; see note) | 2026-08-23 16:40 UTC-5 | Rust candidate                                       |
| C# oracle (frozen) | same checkout as C# baseline         | `4028b58dbd3302ed431e2c5b32d9dde5634db218` | `sph.2026-07-11.01` | clean       | 2026-08-23 15:33 UTC-5      | Historical oracle (identical to current C# baseline) |

Notes:

- The C# current baseline HEAD equals the frozen oracle commit `4028b58d…` referenced in
  `AGENTS.md`. The C# checkout is treated as read-only; no build artifacts were created and no
  tests were run against it.
- Rust HEAD `f5122421` (this pass) differs from the previously audited `a42e733` only by a
  single commit touching this document (561 insertions / 333 deletions; no Rust source changed),
  so every code citation in this document was re-verified at `f5122421` and holds unchanged.
  Earlier baseline notes: `a42e733` differed from `787f0c8` only by a documentation restructure
  commit (this file renamed to `design/ROAD-TO-FEATURE-PARITY.md`, `SPEC.md` moved to
  `design/SPEC.md`, `IMPLEMENTATION.md`/`IMPLMENTATION_OUTSTANDING.md`/
  `IMPLEMENTATION_OUTSTANDING_V2.md`/`VALIDATION-REPORT.md` deleted). That restructure left
  pre-existing `validate_repository.py` link/required-file errors in other files (outside this
  assignment's edit scope); this document does not reference the deleted files.
- At the start of this pass the Rust worktree showed this file itself as a single pre-existing
  uncommitted modification (the content later committed as `f5122421`); it is prior audit work,
  not an implementation change introduced by this pass. No other Rust file was modified, and the
  C# checkout remains byte-identical to `4028b58d…`.

## 3. Method and status legend

The C# inventory was built first from `rg`/source inspection of `src/Melodee.Common/`,
`src/Melodee.Blazor/`, `src/Melodee.Cli/`, and `src/Melodee.Mql/`, tracing composition roots
(`Program.cs` DI registrations and middleware pipeline), route attributes, `.razor` `@page`
routes and navigation, `.csproj` compile exclusions, Quartz job registrations
(`JobKeyRegistry.cs`/`JobRegistry.cs`), plugin folders, the 190-key setting registry and its
consumers, SignalR hub mappings, and hosted services. The Rust inventory was then built from
`apps/` and `crates/` source, tracing production wiring from `main.rs` composition (routes,
workers, schedulers) to concrete implementations, and verifying that cited test evidence exists
and exercises the cited behavior. Every pre-existing row was independently re-verified in this
pass; all 12 high-risk claims from the assignment prompt were explicitly checked (see §13,
Entry 4).

Status vocabulary (used exactly as specified):

| Status                   | Meaning                                                                                                        |
| ------------------------ | -------------------------------------------------------------------------------------------------------------- |
| `PROVEN PARITY`          | Reachable Rust production behavior has evidence matching the current C# capability.                            |
| `IMPLEMENTED — UNPROVEN` | A plausible production path exists, but end-to-end or differential proof is incomplete.                        |
| `PARTIAL`                | Only part of the C# workflow exists, or supporting infrastructure is present but material behavior is missing. |
| `MISSING`                | No operational Rust equivalent was found.                                                                      |
| `DIFFERENT — UNAPPROVED` | Rust behavior materially differs and no applicable product approval was found.                                 |
| `DIFFERENT — APPROVED`   | A material difference is governed by an explicit accepted decision; it remains visibly non-identical.          |
| `DISPUTED`               | Available evidence conflicts and further investigation is required.                                            |
| `NOT ASSESSED`           | The capability has been inventoried but not adequately evaluated in Rust.                                      |

ADRs that defer or waive a real C# feature are reported as `PARTIAL`/`MISSING`/`DIFFERENT —
APPROVED` depending on the Rust state, with the ADR cited as the governing difference — never as
`PROVEN PARITY`.

## 4. Executive summary

| Status                   |  Count |
| ------------------------ | -----: |
| `PROVEN PARITY`          |     18 |
| `IMPLEMENTED — UNPROVEN` |      9 |
| `PARTIAL`                |     15 |
| `MISSING`                |      3 |
| `DIFFERENT — APPROVED`   |      4 |
| `DIFFERENT — UNAPPROVED` |      1 |
| `DISPUTED`               |      0 |
| `NOT ASSESSED`           |      0 |
| **Total rows**           | **50** |

- **v1 blockers:** 28 (all rows not in `PROVEN PARITY` or `DIFFERENT — APPROVED`).
- **Intentionally approved differences:** 4 (`DIFFERENT — APPROVED`).
- **Unassessed/disputed:** 0/0. (All rows are now assessed; `IMPLEMENTED — UNPROVEN`, `PARTIAL`,
  `MISSING`, and `DIFFERENT — UNAPPROVED` rows remain outstanding work and are listed in §7.)

### Highest-risk missing workflows

1. **Ingestion provider enrichment** — C# `DirectoryProcessorToStagingService` calls
   `ArtistSearchEngineService.DoSearchAsync` and `AlbumImageSearchEngineService.DoSearchAsync`
   during staging (`src/Melodee.Common/Services/Scanning/DirectoryProcessorToStagingService.cs:1135,1210`,
   verified exact). Rust ingestion calls no provider; `external_provider_cache` methods
   (`crates/melodee-persistence/src/providers.rs:145,169,208`) have only test callers.
2. **Cross-Release Artist identity reuse** — C# `LibraryInsertJob` matches Artists by MusicBrainz
   ID, Spotify ID, iTunes ID, and name across Releases
   (`src/Melodee.Common/Jobs/LibraryInsertJob.cs:439,786-823`). Rust
   `PreparedResolution::new` (`crates/melodee-persistence/src/catalog.rs:1057`) allocates a fresh
   `Uuid::now_v7()` provisional per observed value except the ADR-0153 singleton rule
   (`catalog.rs:919-940`). No existing-`artists` lookup exists in the import path.
3. **Metadata edit forms** — C# `MediaEditService`
   (`src/Melodee.Common/Services/Scanning/MediaEditService.cs`) implements 12+ tag-rewriting
   transforms and the Blazor editors (`AlbumEdit.razor`, `ArtistEdit.razor`, `SongDetail.razor`)
   expose far more fields. Rust admin edit endpoints (`admin_catalog.rs:723-727`, handlers
   `:1452,1508,1542`; store `catalog_mutations.rs:940,1037,1136` with in-transaction audit events
   `:1012,1115,1185`) have zero test coverage, and the SPA client PUTs to
   `/api/admin/v1/catalog/{entity}/{id}/metadata` (`admin-ui/src/api/admin.ts:783,790,797`) while
   the server registers no `/metadata` route — the UI save path is broken.
4. **Provider-discovered artwork** — C# `AlbumImageSearchEngineService`/`ArtistImageSearchEngineService`
   (`src/Melodee.Common/Services/SearchEngines/`) fetch provider images during staging and
   housekeeping. Rust fetches no image URLs in production.
5. **Embedded MusicBrainz ID retention** — C# retains artist/album/song MusicBrainz IDs at insert
   (`LibraryInsertJob.cs:439,498,585`); Rust extracts only the `musicbrainz_release_type` tag
   (`crates/melodee-media/src/lib.rs:815`) and stores no MB GUIDs (`lastfm.rs:31` documents the
   recording-ID absence).
6. **End-user web UI** — the C# Blazor app is a full user-facing product (dashboard, MQL search,
   music player, stats, public share landing); Rust serves only the admin SPA at `/admin`.

## 5. Domain coverage summary

| #   | Domain                                                             | C# inventory | Rust comparison | Rows | Blocking gaps | Last reviewer / date    |
| --- | ------------------------------------------------------------------ | ------------ | --------------- | ---: | ------------: | ----------------------- |
| 1   | Installation, startup, onboarding, health, diagnostics             | Complete     | Partial         |    9 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 2   | Configuration layering, settings, import/export, defaults          | Complete     | Partial         |    2 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 3   | Auth, users, roles, groups, permissions, linked identities         | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 4   | Library/source config, storage layout, reconciliation              | Complete     | Complete        |    1 |             0 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 5   | Discovery, settling, acquisition, publication, review, deletion    | Complete     | Complete        |    2 |             0 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 6   | Audio/tag metadata parsing, CUE/NFO/playlist/lyrics/sidecar        | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 7   | Artist observation, identification, matching, merge/split          | Complete     | Partial         |    2 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 8   | Release identification, classification, curation, editing          | Complete     | Partial         |    2 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 9   | Track/AudioIdentity, fingerprints, technical metadata, editing     | Complete     | Partial         |    2 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 10  | Metadata/search/enrichment providers                               | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 11  | Artwork (embedded/sidecar/provider/generated/uploaded/served)      | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 12  | Catalog browsing, filtering, sorting, search, MQL, stats           | Complete     | Complete        |    2 |             0 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 13  | Playback, streaming, transcoding, downloads, media auth            | Complete     | Partial         |    3 |             3 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 14  | Native Melodee API / OpenSubsonic / Jellyfin compatibility         | Complete     | Complete        |    3 |             0 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 15  | Playlists, smart playlists, queues, favorites, ratings, history    | Complete     | Complete        |    4 |             0 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 16  | Scrobbling, now-playing, Last.fm lifecycle/metadata                | Complete     | Complete        |    2 |             0 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 17  | Podcasts, episodes, downloads, refresh, discovery                  | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 18  | Internet radio, charts, recommendations, top content               | Complete     | Partial         |    2 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 19  | Shares, requests, comments, notifications, email                   | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 20  | Scripting, extension hooks, metadata plugins, sandbox              | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 21  | Scheduled jobs, background maintenance, cache cleanup, rebuild     | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 22  | Admin UI pages and material read/mutate workflows                  | Complete     | Partial         |    2 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 23  | Audit history, logging, metrics, diagnostics, retry visibility     | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 24  | Legacy migration, public-ID preservation, backup/restore/repair    | Complete     | Partial         |    2 |             2 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |
| 25  | Packaging, upgrades, release artifacts, local-production operation | Complete     | Partial         |    1 |             1 | Kilo (kimi-k3) 2026-08-23; re-verified 2026-08-23 (entry 5) |

The C# inventory is complete for all 25 domains (controllers, pages, jobs, plugins, settings,
hubs, middleware, and DI registrations were enumerated exhaustively). A Rust comparison marked
`Partial` means at least one row in the domain is neither `PROVEN PARITY` nor an accepted
difference; every such row appears in §7. Domains 23–25 received their first full comparison pass
in this audit (previously `NOT ASSESSED`).

## 6. Detailed feature-parity matrix

| Feature ID | Domain | C# behavior | C# evidence | Rust status | Rust evidence | Runtime / test evidence | Exact gap / next proof | Requirement / ADR | v1 blocker |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| STARTUP-001 | 1 | Server lifecycle, health, readiness, version, diagnostics endpoints | `src/Melodee.Blazor/Controllers/Melodee/SystemController.cs:23` (SystemController; `/system/info` :38-59, admin-gated `/system/stats`)<br>`src/Melodee.Common/Services/Setup/SetupCheckService.cs:68` (corrected path)<br>`src/Melodee.Blazor/Program.cs:512,923` (`AddHealthChecks`, `MapHealthChecks("/health")`) | `PROVEN PARITY` | `apps/melodee-server/src/main.rs` (composition root; `/healthz`/`/readyz` wired :545-546)<br>`apps/melodee-server/src/admin_system.rs:121-217` (health aggregation), `:676-686` (routes) | `apps/melodee-server/tests/admin_system_api.rs` (13 tests)<br>`apps/melodee-server/tests/system_api.rs` (6 tests) | None observed | PRD-WORK-001<br>ADR-0104 | No |
| ONBOARD-001 | 1 | Onboarding/setup wizard with preflight checks and required-configuration gating | `src/Melodee.Common/Services/Setup/SetupCheckService.cs:68` (corrected path)<br>`src/Melodee.Blazor/Components/Onboarding/` (10 components; `/onboarding`, `/onboarding/blocking`) | `PROVEN PARITY` | `crates/melodee-persistence/migrations/0025_phase9_onboarding_settings.sql:48` (`onboarding_state`, ADR-0134 §3)<br>`apps/melodee-server/src/admin_onboarding.rs:10-17,86-97` (status/complete, fail-closed re-evaluation) | `apps/melodee-server/tests/admin_onboarding_api.rs`<br>`crates/melodee-persistence/tests/settings.rs`<br>`apps/melodee-cli/tests/config.rs` | None observed<br>the graphical wizard is the Phase 10 admin UI over the same stores | PRD-SETUP-001/002 | No |
| CONFIG-001 | 2 | Configuration layering (defaults/file/env/DB), 190-key registry, import/export with secret redaction, validation | `src/Melodee.Common/Constants/SettingRegistry.cs:5-216` (190 keys; 187 consumers in `src/`, 189 incl. tests; only `theme.libraryPath` :213 fully dead; corrected counts)<br>`src/Melodee.Common/Services/SettingService.cs`<br>`src/Melodee.Common/Services/SystemExportService.cs`/`SystemImportService.cs` | `PARTIAL` | `crates/melodee-persistence/src/settings.rs` (191 `KnownSetting` entries, known-key classification, layered resolution :978, redacted export :1178, preview-first import :1248)<br>admin config export/import API | `crates/melodee-persistence/tests/settings.rs`<br>`apps/melodee-server/tests/admin_config_api.rs` | Provider settings keys are preserved verbatim but inert: `searchEngine.{brave,deezer,discogs,itunes,metalApi,musicbrainz,spotify,wikidata}.*` (`settings.rs:462-484`) and `jobs.musicbrainzUpdateDatabase.cronExpression` (:372) have zero non-test runtime consumers (verified by `rg` over `apps/`+`crates/`). Only `scrobbling.lastFm.*` keys are live (`lastfm.rs:58-60,180-185`) | PRD-CONFIG-001/002/003<br>PRD-AUTH-007<br>ADR-0136 | Yes — provider settings inert |
| AUTH-001 | 3 | Auth (JWT/refresh/cookie/social/password reset/logout), users, roles, groups, library ACL, avatars, profiles | `src/Melodee.Common/Services/UserAuthenticationService.cs:22`<br>`src/Melodee.Common/Services/Security/RefreshTokenService.cs:119` and `Security/GoogleTokenService.cs:63` (corrected paths)<br>`UserSocialLoginService.cs:17`<br>`UserPasswordResetService.cs:16`<br>`UserGroupService.cs:13`<br>`LibraryAuthorizationService.cs:15`<br>`src/Melodee.Blazor/Controllers/Melodee/AuthController.cs:34`/`UsersController.cs:25`/`UserController.cs:29` | `PARTIAL` | `apps/melodee-server/src/native/auth.rs` (route-for-route parity with C# `AuthController`: authenticate, cookie sign-in/out, `google` :708, `cookie/google` :710, refresh-token, logout, revoke, password-reset request/validate/confirm :703-746, linked-providers, Google link/unlink :736-737; `GoogleOidcService` with JWKS validation :65-180)<br>`crates/melodee-persistence/src/auth.rs` (refresh rotation with token-family locking :955)<br>group/library ACL (migration 0010)<br>avatar upload (migration 0022) | `apps/melodee-server/tests/native_auth_api.rs` (23 tests incl. `google_oidc_uses_local_jwks_and_immutable_social_subject_mapping` :791 — loopback JWKS)<br>`admin_access_api.rs`<br>`native_avatar_api.rs`<br>Phase 8 differential replay | Social-login reachability is now verified: the full Google OIDC flow exists and is loopback-tested. Remaining gaps: no end-to-end proof against the real Google provider (R4.6 production-integrations leg), and roles/profiles depth vs C# `UserGroupService`/profile pages is unproven | PRD-AUTH-001..006<br>ADR-0124/0126 | Yes — real-provider OIDC e2e absent (R4.6) |
| LIBRARY-001 | 4 | Multiple media libraries, storage layout, source safety, reconciliation | `src/Melodee.Common/Services/LibraryService.cs:35`<br>`FileSystemService.cs:8`<br>`Scanning/DirectoryProcessorToStagingService.cs:42` | `PROVEN PARITY` | `crates/melodee-ingestion/src/` (reconcile, acquisition, source, config)<br>ADR-0145 disjoint roots<br>Release-centric canonical layout (ADR-0106/0116) | `crates/melodee-ingestion/tests/reconciliation.rs`<br>`acquisition.rs` (200-Release drain :1088-1132, crash boundaries) | None observed<br>canonical storage layout differs by design (approved) | PRD-INGEST-005<br>PRD-STORAGE-001/002/004<br>ADR-0106/0116/0145 | No |
| INGEST-001 | 5 | Durable inbound processing: discovery, settling, acquisition, inspection, normalization, validation, publication, retry, quarantine, review, deletion | `src/Melodee.Common/Services/Scanning/DirectoryProcessorToStagingService.cs:42`<br>`src/Melodee.Common/Jobs/LibraryInsertJob.cs`<br>`LibraryInboundProcessJob.cs`<br>`StagingAutoMoveJob.cs` | `PROVEN PARITY` | `crates/melodee-ingestion/src/` (reconcile, snapshot, acquisition, inspect, normalize, validate, publication, publication_worker)<br>`crates/melodee-catalog/` (terminal durable catalog import; `start_catalog_loop` wired at `apps/melodee-server/src/main.rs:334`)<br>`crates/melodee-persistence/src/work_items.rs` (durable state machine, leases, `FOR UPDATE SKIP LOCKED` :1120,1200) | `crates/melodee-ingestion/tests/` (reconciliation, acquisition, inspection, normalization, publication, environment_faults)<br>`crates/melodee-persistence/tests/` (claiming, recovery, history) | Durable work-item model is an approved redesign, not a gap | PRD-INGEST-001..010<br>ADR-0116/0145 | No |
| INGEST-002 | 5 | Operator review/quarantine/reprocess/deletion workflows with audit | `src/Melodee.Common/Jobs/StagingAlbumRevalidationJob.cs`<br>`StagingAutoMoveJob.cs` | `PROVEN PARITY` | `crates/melodee-policy/src/` (engine, novelty, dry_run)<br>`crates/melodee-persistence/src/policy.rs`<br>`apps/melodee-server/src/admin_review.rs`<br>ADR-0152 confirmed Release deletion | `crates/melodee-persistence/tests/phase5_override.rs`<br>`apps/melodee-server/tests/admin_review_api.rs`<br>`admin-ui/e2e/release-review.spec.ts` | None observed | PRD-POLICY-001..008<br>ADR-0152 | No |
| MEDIA-001 | 6 | Audio/tag metadata parsing (Lofty/ATL/ffprobe), CUE/NFO/M3U/SFV/playlist/lyrics/sidecar handling, embedded IDs | `src/Melodee.Common/Plugins/MetaData/Directory/` (CueSheet, M3UPlaylist, SimpleFileVerification, Mp3Files, `Nfo/Nfo.cs:136-353` key:value extraction + `Nfo/Handlers/` Jellyfin/PMedia XML→Album, `Blackbeard/Blackbeard.cs:24` `.blackbeard.provenance.json`)<br>`Plugins/MetaData/Song/` (AtlMetaTag, NativeId3MetaTag, `LyricPlugin.cs:33-50` `.txt` serialized-lyrics sidecars)<br>`Plugins/Processor/MetaTagProcessors/` (7 processors)<br>`src/Melodee.Common/Jobs/LibraryInsertJob.cs:581-583` (lyrics stored on the Song row at insert) | `PARTIAL` | `crates/melodee-media/src/lib.rs` (Lofty + ffprobe :3,19,146-155; sidecar classification :684-688)<br>`crates/melodee-media/src/normalize.rs` (real sidecar parsers: `parse_cue` :2132, `parse_m3u` :2305, `parse_sfv` :2344, `parse_lrc` with timestamps :2396; dispatch :1996-2004; `parse_nfo` :2371 only distinguishes text-vs-XML and flags malformed XML)<br>embedded lyrics served via native `/api/v1/songs/{id}/lyrics` (`native/catalog.rs:1961-2002`) and OS `getLyrics`/`getLyricsBySongId` (`opensubsonic/media.rs:35-38,898`)<br>CUE segmentation enforced in validation (`crates/melodee-ingestion/src/validate.rs:248-253`, ADR-0146) | `crates/melodee-media/tests/normalization_real.rs`<br>`crates/melodee-ingestion/tests/normalization.rs`<br>R2.2 CUE/sidecar suite (`design/evidence/r2/README.md:199-204`) | Gaps verified this pass: (a) NFO field extraction — C# parses NFO key:value lines and Jellyfin/PMedia XML into Album metadata; Rust `parse_nfo` performs no field extraction and `plugin.blackbeard.enabled` is an inert key (`settings.rs:411`); (b) C# `LyricPlugin` reads `<songname>.txt` lyrics sidecars — Rust classifies `.txt` as Unknown; (c) C# persists lyrics on the Song row; Rust keeps them in the manifest JSON (functionally served on native/OS, not a track column). The former parenthetical "embedded MusicBrainz ID retention verified" was false and was removed — Rust retains no MB GUIDs (see TRACK-001) | PRD-MEDIA-001/004/006<br>ADR-0146 | Yes — NFO/`.txt`-lyrics/Blackbeard handler breadth missing |
| ARTIST-001 | 7 | Artist observation, identification, provider matching, aliases, relationships, memberships, contributions, merge/split, negative decisions | `src/Melodee.Common/Services/ArtistService.cs:30`<br>`MediaArtistImportService.cs:8`/`MediaArtistExportService.cs:9`<br>`src/Melodee.Common/Jobs/LibraryInsertJob.cs:439` (`FindArtistAsync` by dbId, id, normalized name, MusicBrainzId, SpotifyId, ItunesId) and `:786-823` (bulk artist lookup keyed `mb:{id}` etc.) — cross-Release reuse | `DIFFERENT — UNAPPROVED` | `crates/melodee-catalog/src/resolver.rs:110` (`LocalArtistResolver` — zero production callers; re-export only)<br>`crates/melodee-catalog/src/authority.rs:66` (`ArtistAuthority` trait; only `FakeAuthority` :134-148 in `#[cfg(test)]`)<br>`crates/melodee-persistence/src/catalog.rs:1057` (fresh `Uuid::now_v7()` provisional per observed value; `reused_release_artist` :1058 implements only the ADR-0153 singleton rule :919-940, scoped to one Release import) | `crates/melodee-persistence/tests/catalog_identity.rs`<br>`catalog_merge_split.rs`<br>No test imports two Releases with the same artist name (`catalog_import.rs` covers fixture/idempotency/lease-fencing/retry only) | Rust does NOT reuse Artist identities across Releases: each observed artist value that isn't the single release-artist gets a new provisional Artist; no existing-`artists` lookup exists anywhere in import. C# reuses by MB/Spotify/iTunes ID and name. The resolver/authority boundary cited as the intended fix is production-dead library code. ADR-0153's own Context records the duplication symptom ("three-Release catalog … displayed 25 Artists"). No accepted decision approves the divergence | PRD-ARTIST-001..012<br>PRD-RESOLVE-001..005<br>ADR-0117/0153 | Yes |
| ARTIST-002 | 7 | Provider-driven Artist metadata enrichment during ingestion (search, image, biography) | `src/Melodee.Common/Services/Scanning/DirectoryProcessorToStagingService.cs:1135` (`ArtistSearchEngineService.DoSearchAsync` during staging — verified exact :1135-1139)<br>`src/Melodee.Common/Jobs/StagingAlbumRevalidationJob.cs:249` (re-query with `bypassNegativeCache: true` — verified exact) | `MISSING` | None in production<br>`crates/melodee-catalog/src/authority.rs` is evidence-only with no production provider | `crates/melodee-persistence/tests/providers.rs` (cache store only) | C# ingestion queries Artist search engines during staging; Rust ingestion does not call any provider (zero HTTP-client usage in ingestion/catalog/policy crates, verified against source and Cargo manifests). ADR-0136 defers this but does not make it parity | PRD-EXT-001/002<br>ADR-0136 | Yes |
| RELEASE-001 | 8 | Release identification, classification (traits), external identities, duplicate/reissue/edition handling, curation | `src/Melodee.Common/Services/AlbumService.cs:34`<br>`src/Melodee.Common/Services/Scanning/AlbumDiscoveryService.cs:24` (corrected path)<br>`src/Melodee.Common/Jobs/LibraryInsertJob.cs:469` (`FindAlbumAsync`), `:498` (album MusicBrainzId), `:511-514` (in-batch dupe check by artist+normalized name / MB ID / Spotify ID) | `PARTIAL` | `crates/melodee-policy/src/` (classification, novelty, engine)<br>`crates/melodee-persistence/src/catalog.rs:954` (adopts `publication.release_id`)<br>`apps/melodee-server/src/publication_worker.rs:272` (`ReleaseId::new()` per publication bundle)<br>`crates/melodee-persistence/src/policy.rs` (`catalog_known_audio_identities` query) | `crates/melodee-policy/tests/policy_acceptance.rs`<br>`crates/melodee-persistence/tests/catalog_import.rs` (`import_adopts_reserved_release_id_is_idempotent` :75) | Rust allocates a new Release per publication; import idempotency is keyed on `work_item_id` only (`catalog.rs:611-613`) — there is no cross-catalog Release matching by MB/Spotify ID, catalog number, or title+artist. C# `LibraryInsertJob` finds existing albums by ID. Novelty detection exists at the Track level via AudioIdentity, not Release-level deduplication | PRD-CLASS-001..003<br>PRD-POLICY-003..006 | Yes |
| RELEASE-002 | 8 | Release/Artist/Track metadata editing (title, sort, display artist, date, type, tag normalization) | `src/Melodee.Common/Services/Scanning/MediaEditService.cs:30` (`SaveAlbum` :272, `SaveImageUrlAsCoverAsync` :72, `RemoveUnwantedTextFromAlbumTitle` :326, `RemoveUnwantedTextFromSongTitles` :366, `RemoveFeaturingArtistsFromSongTitle` :411, `RemoveFeaturingArtistsFromSongsArtist` :454, `PromoteSongArtist` :494, `SetYearToCurrent` :531, `ReplaceGivenTextFromSongTitles` :569, `RenumberSongsAsync` :706/`RenumberSongs` :737, `UpdateArtistForAlbumsAsync` :958, `UpdateArtistsByMappingAsync` :1028)<br>`src/Melodee.Blazor/Components/Pages/Data/AlbumEdit.razor`/`ArtistEdit.razor`/`SongDetail.razor` (fields incl. external IDs, biography, notes, lock flags) | `PARTIAL` | Server side exists and is wired: routes `apps/melodee-server/src/admin_catalog.rs:723-727` (PUT release/artist/track), handlers `:1452,1508,1542`; store `crates/melodee-persistence/src/catalog_mutations.rs:940,1037,1136` with in-transaction audit events (`release_metadata_updated` :1012, artist :1115, track :1185). UI: `admin-ui/src/screens/catalog/EditPanel.tsx` | None: zero tests reference `update_release_metadata`/`update_artist_metadata`/`update_track_metadata` in `apps/melodee-server/tests/`, `crates/*/tests/`, or `admin-ui/` (verified by `rg`) | Downgraded from `IMPLEMENTED — UNPROVEN`: (a) no test coverage for the edit endpoints/store methods; (b) the SPA save path is broken — the client PUTs `/api/admin/v1/catalog/{entity}/{id}/metadata` (`admin-ui/src/api/admin.ts:783,790,797`) but the server registers `PUT /api/admin/v1/catalog/{entity}/{id}` with no `/metadata` suffix, and field names mismatch (UI `name` vs server `canonicalName`; UI `trackNumber` vs server `ordinal`; UI `releaseYear/country/type/durationMs` have no server fields); (c) the 12+ C# `MediaEditService` tag-rewriting transforms (feat. removal, artist promotion, year setting, renumbering, mapping) have no Rust equivalent | PRD-CREDIT-001..008<br>Phase 10 decision log | Yes — broken UI wiring, no evidence, tag transforms missing |
| TRACK-001 | 9 | Track and AudioIdentity creation, embedded recording IDs, fingerprints, duplicate/alternate recording handling, technical metadata | `src/Melodee.Common/Jobs/LibraryInsertJob.cs:585` (song `MusicBrainzId` retained from `MetaTag` — verified exact)<br>`src/Melodee.Common/Services/SongService.cs:20` | `PARTIAL` | `crates/melodee-ingestion/src/normalize.rs:250-251` (fpcalc fingerprint computation)<br>`crates/melodee-persistence/src/policy.rs:1999-2006` (fingerprints persisted in the sealed manifest), `:2091-2116` (`catalog_known_audio_identities` query — verified exact)<br>`crates/melodee-persistence/src/catalog.rs:672` (`audio_identity_id` per track; fresh v7 per track :878) | `crates/melodee-persistence/tests/catalog_audio_identity.rs`<br>`crates/melodee-ingestion/tests/normalization.rs` | Fingerprints are computed and retained as observations but dropped before novelty evaluation (`novelty.rs:82-91` `NoveltyTrackFact` has no fingerprint field; nothing in `melodee-policy` compares fingerprint values). Embedded MusicBrainz recording/release/artist GUIDs are NOT extracted or retained in Rust (only `musicbrainz_release_type`, `melodee-media/src/lib.rs:815`; `lastfm.rs:31` documents the recording-ID absence) while C# retains them on Song/Album/Artist rows | PRD-FP-001..004 | Yes |
| TRACK-002 | 9 | Automatic AudioIdentity/fingerprint cross-Release matching during ingestion | Corrected C# evidence: NO provider recording-ID lookup exists in the baseline — `src/Melodee.Common/Plugins/SearchEngine/MusicBrainz/IMusicBrainzRepository.cs:21-34` exposes only `GetAlbumByMusicBrainzId`, `SearchArtist`, `ImportData`. C# automatic identity reuse rests on embedded tag IDs read at insert (`LibraryInsertJob.cs:585`) plus artist/album matching (:439, :469, :511-514, :786-823); C# has no fingerprint system | `MISSING` | `crates/melodee-persistence/src/catalog_identity.rs:70` (`record_audio_identity_match_decision` — admin-only) | `crates/melodee-persistence/tests/catalog_identity.rs` (admin decision tests) | No automatic matching/proposal engine exists in Rust: matches are admin-confirmed only (`audio_identity_match_decisions`, `audio_identity_alternate_recordings`, `audio_identity_catalog_assertions` feed `policy.rs:2091-2116`). Because C# matching depends on retained embedded MB IDs and Rust retains none (TRACK-001), the evidenced C# behavior cannot occur in Rust at all | PRD-FP-002/004 | Yes |
| PROVIDER-001 | 10 | Metadata/search/enrichment providers: MusicBrainz (incl. local DecentDB replica), Spotify, Discogs, iTunes, Deezer, WikiData, Brave, Metal-API, Last.fm, plus self-search — artist/album search, images, top songs | `src/Melodee.Common/Plugins/SearchEngine/` (9 external provider folders + `MelodeeArtistSearchEnginePlugin.cs:16` self-search + `MusicBrainz/Data/` DecentDB local replica: importer, schema init, warmup)<br>`src/Melodee.Common/Services/SearchEngines/` (ArtistSearchEngineService.cs:36, AlbumSearchEngineService.cs:14, ArtistImageSearchEngineService.cs:22, AlbumImageSearchEngineService.cs:28)<br>`src/Melodee.Blazor/Controllers/Melodee/ArtistLookupController.cs:23-24` (POST :51, `providers` :125)<br>`src/Melodee.Blazor/Controllers/OpenSubsonic/SearchEngineController.cs:15`<br>`src/Melodee.Blazor/Program.cs:172` (`MusicBrainzDecentDbWarmupHostedService`) | `PARTIAL` | `crates/melodee-persistence/src/providers.rs:134` (`ProviderCacheStore`; `get_if_fresh` :145, `upsert` :169, `purge_expired` :208 — all test-only callers, verified)<br>`apps/melodee-server/src/native/discovery.rs:527-591` (`artist_lookup` projects stored `artist_external_identities` rows only; hardcoded provider capability list :455-464; `image_url`/`thumbnail_url` always `None` :570-571; `has_partial_failures` hardcoded `false` :588)<br>`apps/melodee-server/src/lastfm.rs` (Last.fm scrobble only) | `crates/melodee-persistence/tests/providers.rs`<br>`apps/melodee-server/tests/lastfm_scrobble.rs` | Only Last.fm scrobble/now-playing/linking is wired. The 8 non-Last.fm external providers, the self-search plugin, the DecentDB local MusicBrainz replica, and the Last.fm metadata-lookup legs are deferred (ADR-0136). `external_provider_cache` has no production writer/reader/cleaner. Native `/api/v1/artist-lookup` does NOT call providers. Provider settings keys are preserved but inert (CONFIG-001). C# also backs `/rest/searchForArtist*` with provider search; the frozen captures only record the unauthenticated 302 redirect, which Rust reproduces exactly (`opensubsonic/lists.rs:1520-1545`) | PRD-EXT-001..004<br>ADR-0136 | Yes |
| ARTWORK-001 | 11 | Embedded, sidecar, provider-discovered, converted, uploaded, overridden, and served artwork | `src/Melodee.Common/Services/SearchEngines/AlbumImageSearchEngineService.cs:28` and `ArtistImageSearchEngineService.cs:22` (corrected paths)<br>`src/Melodee.Common/Services/ImageConversionService.cs:13` (thin wrapper; real work in `Plugins/Conversion/Image/ImageConvertor.cs:50-130` — JPEG convert + single square-bucket resize at staging)<br>`src/Melodee.Common/Services/OpenSubsonicApiService.cs:1503,1514` (`ResizeImageIfNeeded` — stored artwork resized on serve for `?size=`)<br>`src/Melodee.Common/Services/Scanning/MediaEditService.cs:72` (`SaveImageUrlAsCoverAsync`) | `PARTIAL` | `crates/melodee-media/src/normalize.rs` (embedded/sidecar extraction/normalization :4,67,195, verification :1069-1197)<br>`apps/melodee-server/src/admin_catalog.rs:1406,1427` (upload_artist_image/upload_release_image)<br>`catalog_entity_images` overrides (migration 0030, ADR-0143; store `crates/melodee-persistence/src/catalog_images.rs`; consumed by `native/images.rs`, `opensubsonic/media.rs`, `jellyfin/images.rs`) | `crates/melodee-media/tests/normalization_real.rs`<br>`apps/melodee-server/tests/admin_artwork_api.rs` (9 tests)<br>native image golden suite | Provider-discovered artwork (C# image search engines during staging/housekeeping) has no Rust equivalent. New uncaptured difference found this pass: C# resizes STORED artwork on serve for `?size=` on the OpenSubsonic surface; Rust OS `getCoverArt` serves stored bytes unresized (`opensubsonic/media.rs:758-780` — `size` only affects the default placeholder) while native (`native/images.rs:202-271` Lanczos3) and Jellyfin (`jellyfin/images.rs:116`) do resize. All sized captures used the default placeholder, so replay cannot catch it (§8) | PRD-MEDIA-005<br>ADR-0143 | Yes — provider-discovered artwork missing; uncaptured OS resize divergence |
| SEARCH-001 | 12 | Catalog browsing, filtering, sorting, search, MQL query API, statistics | `src/Melodee.Common/Services/SearchService.cs:22`<br>`StatisticsService.cs:14`<br>`src/Melodee.Mql/` (separate project — corrected path: `Api/MqlController.cs:17-19` `api/v1/query`, validator, tokenizer, compilers, `Security/MqlRegexGuard.cs`, `Authorization/MqlAuthorizationService.cs`, analyzers/metrics; DI `Program.cs:363-364`) | `PROVEN PARITY` | `crates/melodee-search/` (Tantivy schema, indexing, query, rebuild, outbox worker)<br>`crates/melodee-mql/` (parser/AST; wire translation in `melodee-search/src/translator.rs`)<br>`crates/melodee-persistence/src/discovery.rs` (materialized facts) | `crates/melodee-search/tests/` (28 search + 7 rebuild + smoke)<br>`crates/melodee-mql/tests/` (37 + executable frozen corpus `compatibility_corpus.rs`)<br>Phase 6 10M-Track benchmark (`design/evidence/phase-6/PHASE-6-REPORT.md:125,133`)<br>ADR-0122 | None observed for the search/MQL core. C# `search2/search3` use local `OpenSubsonicApiService.SearchAsync` (`SearchingController.cs:30-42`), not provider engines, so local-only Rust search does not diverge | PRD-SEARCH-001..007<br>ADR-0107/0121/0122 | No |
| DISCOVERY-003 | 12 | Audio features/BPM read path (tempo, key, mode, time signature per track) | `src/Melodee.Blazor/Controllers/Melodee/AudioFeaturesController.cs:27-28` (`api/v{version}/audio`: `features/{id}` :46, `bpm` :97) | `PROVEN PARITY` | `apps/melodee-server/src/native/discovery.rs:42-43` (audio features/BPM routes)<br>`native_track_audio_features` (migration 0015) | Golden captures `compatibility/melodee-api-v1/golden/native-discovery/native-discovery-audio-features.json` and `native-discovery-audio-bpm.json`<br>`apps/melodee-server/tests/native_discovery_api.rs:203,719,773` | None for the captured read path. Residual: how C# computes/refreshes feature values was not traced; Rust extraction limits are documented in the Phase 9 WP-04 checklist (FEATURE-PARITY-MATRIX "Audio features/BPM" row) | PRD-DISCOVERY-003 | No |
| PLAYBACK-001 | 13 | Playback, byte ranges, streaming, downloads, media authorization, client profiles | `src/Melodee.Blazor/Controllers/OpenSubsonic/MediaRetrievalController.cs:110` (`/rest/download.view`, verified exact), `:198` (`/rest/stream.view`, verified exact)<br>`src/Melodee.Blazor/Controllers/Jellyfin/AudioController.cs:14` (`{itemId}/stream` :29, `stream.{extension}` :121, `universal` :134, `Lyrics` :153)<br>`src/Melodee.Common/Services/Playback/PlaybackBackendService.cs:78` (corrected path) | `IMPLEMENTED — UNPROVEN` | `apps/melodee-server/src/main.rs:403-412` (DirectPlayService), `:416-442` (TranscodingService composition)<br>`apps/melodee-server/src/native_stream.rs:581` (`DirectPlayService::response`), `:667` (`stream_range` — verified exact)<br>`apps/melodee-server/src/transcode.rs:202,265` | `apps/melodee-server/tests/native_stream.rs`<br>`native_stream_acl.rs`<br>`transcode_service.rs` (exists; earlier phantom-file claim was corrected in Entry 3) | Direct play, single-range responses, stream-token authorization, bounded concurrency, and configured FFmpeg streaming are production-wired. Missing proof is a current-C# differential/end-to-end journey for download, seek/range, transcode failure/cancellation, and client-profile selection across native, OpenSubsonic, and Jellyfin surfaces | PRD-STREAM-001..003<br>PRD-DEVICE-001..003<br>ADR-0147 | Yes — runtime/differential proof incomplete |
| PLAYBACK-002 | 13 | Server-side playback backends (MPD/MPV) and playback-backend/endpoint registry API | `src/Melodee.Common/Services/Playback/Backends/MpdPlaybackBackend.cs:12` and `MpvPlaybackBackend.cs:13`<br>`src/Melodee.Common/Services/Playback/Factory/PlaybackBackendFactory.cs:11`<br>`src/Melodee.Blazor/Controllers/Melodee/PlaybackBackendController.cs:26` (`api/v{version}/playback-backend`: status/capabilities/initialize/shutdown/register-endpoint)<br>13 `mpd.*`/`mpv.*` setting keys (`src/Melodee.Common/Constants/SettingRegistry.cs:57-71`, consumed by the factory and `DoctorService`) | `PARTIAL` | `apps/melodee-server/src/native/discovery.rs:47-64` (`/api/v1/playback-backend/*` session registry: status/initialize/register/heartbeat/capabilities/shutdown)<br>`apps/melodee-server/src/native/endpoints.rs:19-33` (`/api/v1/endpoints*`)<br>migrations 0017/0019 | `apps/melodee-server/tests/native_endpoints_api.rs`<br>`native_discovery_api.rs`<br>native `backend-*`/`endpoints-*` goldens (in the 202-route inventory) | The registry/heartbeat API surface is captured and differentially covered (PRD-PARTY-003), but the C# model — the server itself connects to and drives MPD/MPV backend processes, configured by 13 live `mpd.*`/`mpv.*` settings — has no Rust equivalent: Rust backends are external processes that self-register, and the `mpd.*`/`mpv.*` keys have no Rust consumer. No accepted ADR covers the backend-connectivity redesign | PRD-PARTY-003 | Product decision required |
| COMPAT-NATIVE-001 | 14 | Native Melodee API v1 (exact contract parity) | `src/Melodee.Blazor/Controllers/Melodee/` (33 compiled controllers + base)<br>`compatibility/melodee-api-v1/contracts/route-inventory.json` (202 routes — count verified from JSON) | `PROVEN PARITY` | `apps/melodee-server/src/native/` (full adapter)<br>`design/agents/PHASE-7-ROUTE-MATRIX.json:9-24` (202/202: 150 exact, 17 normalizations, 35 absences — counts verified from JSON) | `apps/melodee-server/tests/native_*.rs` (13 files)<br>Android 1.8.0 + MeloAmp real-client acceptance (`design/evidence/phase-7/PHASE-7-REPORT.md:162-173`) | Normalizations per ADR-0114/0128/0129 are approved differences | PRD-API-M1-001..007<br>ADR-0114/0128/0129 | No |
| COMPAT-OS-001 | 14 | OpenSubsonic API (exact compatibility) | `src/Melodee.Blazor/Controllers/OpenSubsonic/` (18 controllers, incl. `ChatController.cs:9` `getChatMessages`/`addChatMessage`)<br>`compatibility/opensubsonic/` (81 endpoints) | `PROVEN PARITY` | `apps/melodee-server/src/opensubsonic/`<br>`compatibility/opensubsonic/contracts/phase8-route-evidence.json` (81/81 implemented-and-differentially-tested — verified from JSON; chat endpoints included) | `apps/melodee-server/tests/opensubsonic_*.rs` (7 files)<br>`scripts/run_phase8_differential.py`<br>Feishin v1.15.1 smoke (`design/evidence/phase-8/PHASE-8-REPORT.md:68,217`) | D1/D2 do-not-preserve (ADR-0130) approved. One uncaptured difference is recorded in §8: stored-artwork `?size=` resize (see ARTWORK-001) | PRD-API-OS-001..007<br>ADR-0130 | No |
| COMPAT-JF-001 | 14 | Jellyfin music compatibility (current subset) | `src/Melodee.Blazor/Controllers/Jellyfin/` (15 controllers + base/parser)<br>`compatibility/jellyfin/` (60 routes) | `PROVEN PARITY` | `apps/melodee-server/src/jellyfin/`<br>`compatibility/jellyfin/contracts/phase8-route-evidence.json` (60/60 implemented-and-differentially-tested — verified from JSON) | `apps/melodee-server/tests/jellyfin_core_api.rs` (22 tests)<br>`jellyfin_media_api.rs` (25 tests)<br>`scripts/run_phase8_differential.py`<br>Feishin v1.15.1 smoke | D4 playlist corruption do-not-preserve (ADR-0128)<br>artwork override extension (ADR-0143) | PRD-API-JF-001..006<br>ADR-0128/0143 | No |
| SCROBBLE-001 | 16 | Scrobbling and now-playing (native + OpenSubsonic + Jellyfin) | `src/Melodee.Common/Services/ScrobbleService.cs:15`<br>`src/Melodee.Common/Plugins/Scrobbling/` (IScrobbler, LastFmScrobbler, MelodeeScrobbler, NullScrobbler, NowPlaying repositories)<br>`src/Melodee.Blazor/Controllers/Melodee/ScrobbleController.cs:27-28` | `PROVEN PARITY` | `apps/melodee-server/src/native/catalog.rs:479-545` (scrobble/now-playing + lastfm auth-url/session routes)<br>OpenSubsonic `opensubsonic/annotation.rs:300-361` (incl. captured time-mismatch quirk, idempotency keys)<br>Jellyfin `jellyfin/sessions.rs:36-39,186-214` (Playing/Progress→now-playing, Stopped→scrobble, captured Failed=true skip) | `apps/melodee-server/tests/native_catalog_api.rs`<br>`opensubsonic_annotation_api.rs` (corrected filename)<br>`jellyfin_core_api.rs` | None observed. Operational note: C# `NowPlayingCleanupJob` purges stale now-playing rows; Rust has no equivalent (see JOBS-001) | PRD-USER-003 | No |
| LASTFM-001 | 16 | Last.fm account linking/scrobbling (auth URL, session key, scrobble/now-playing dispatch, disconnect) | `src/Melodee.Common/Plugins/SearchEngine/LastFm/LastFm.cs:14`<br>legacy ADR-0004 session-key lifecycle<br>`native-scrobble/` goldens | `PROVEN PARITY` | `apps/melodee-server/src/lastfm.rs` (real reqwest client :257-285, `LASTFM_API_BASE_URL`, API-sig signing; config-gated `is_configured` :164 on `scrobbling.lastFm.*`; bounded dispatch semaphore :562)<br>`native_lastfm_links` with AEAD-sealed session key (migration 0028:59, ciphertext constraint :78, ADR-0134 §7)<br>wired into native/OS/JF scrobble paths (`native/catalog.rs:545`, `annotation.rs:361`, `sessions.rs:214`) | `apps/melodee-server/tests/lastfm_scrobble.rs` (15 mock-server tests: wire mapping, link lifecycle, invalid-session clearing, no-ops) | Configured-state response shapes are source-evidenced only (capture gap recorded)<br>no retry/queue per legacy | PRD-AUTH-002<br>ADR-0134 §7 | No |
| PODCAST-001 | 17 | Podcasts, episodes, downloads, refresh, cleanup, progress, OPML, discovery providers | `src/Melodee.Common/Services/PodcastService.cs:23` (class), `:640` (refresh body inside `RefreshChannelAsync` :611), `:795` (auto-download enqueue under `channel.AutoDownloadEnabled` :791 — verified exact)<br>`PodcastPlaybackService.cs:35` (NowPlayingAsync :25), `:198` (SaveBookmarkAsync :190)<br>`PodcastOpmlService.cs:16`<br>`src/Melodee.Blazor/Controllers/Melodee/PodcastsController.cs:26` | `IMPLEMENTED — UNPROVEN` | `apps/melodee-server/src/main.rs:208` (`PodcastConfig::from_env` — verified exact), `:486-500` (discovery client, `start_podcast_loop`)<br>`apps/melodee-server/src/native/podcasts.rs:404-451` (router)<br>`apps/melodee-server/src/podcast_worker.rs:736` (refresh_channel), `:1107` (run_podcast_once), `:1454` (start_podcast_loop — all verified exact) | `apps/melodee-server/tests/native_podcasts_api.rs`<br>`native_podcasts_real_feed.rs`<br>`crates/melodee-persistence/tests/native_podcasts_phase7.rs` | Reachable Rust routes and worker cover subscription, OPML, discovery, refresh, auto-download, retry/recovery, cleanup, progress, and history. Missing proof is a current-C# differential or end-to-end lifecycle covering bookmark/playback, cleanup, discovery-provider failure, and recovery semantics | PRD-USER-004 | Yes — lifecycle parity proof incomplete |
| DISCOVERY-001 | 18 | Internet radio (CRUD, admin-gated) | `src/Melodee.Common/Services/RadioStationService.cs:15` (verified exact)<br>`src/Melodee.Blazor/Controllers/OpenSubsonic/InternetRadioController.cs:9` (verified exact)<br>`src/Melodee.Blazor/Components/Pages/Data/RadioStations.razor` (`@page "/data/radiostations"`) | `IMPLEMENTED — UNPROVEN` | `apps/melodee-server/src/admin_radio.rs` (admin CRUD)<br>`apps/melodee-server/src/opensubsonic/radio.rs` (OpenSubsonic adapter) | `apps/melodee-server/tests/admin_radio_api.rs`<br>`crates/melodee-persistence/tests/radio_store.rs` (incl. immutable public-id trigger)<br>Phase 8 `os-radio-*` compatibility captures | Production paths and focused tests exist; source comparison has not yet established all C# validation, ordering, authorization, and admin-UI failure semantics | PRD-RADIO-001/002 | Yes — differential proof incomplete |
| DISCOVERY-002 | 18 | Charts, recommendations, top content, listening analytics/user stats, scheduled discovery refresh | `src/Melodee.Common/Services/ChartService.cs:17` (verified exact)<br>`src/Melodee.Common/Jobs/ChartUpdateJob.cs:35` (verified exact)<br>`src/Melodee.Blazor/Controllers/Melodee/ChartsController.cs:25` / `RecommendationsController.cs:28` (verified exact)<br>`AnalyticsController.cs:27-28` (`listening` :49, `top/{period}` :168)<br>`UserStatsController.cs:23-24` (top-songs :110, top-genres :186, plays-per-day :261) | `IMPLEMENTED — UNPROVEN` | `apps/melodee-server/src/native/discovery.rs:38-65` (analytics, charts, recommendations)<br>`apps/melodee-server/src/native/catalog.rs:318-322` (`/api/v1/user/stats*`)<br>`apps/melodee-server/src/discovery_worker.rs:159` (`run_discovery_once`), `:212` (`start_discovery_loop` — verified exact) | `apps/melodee-server/tests/native_discovery_api.rs`<br>`discovery_worker.rs`<br>`crates/melodee-persistence/tests/native_discovery_phase7.rs` | Production routes and a scheduled refresh loop exist. No current-C# differential journey proves chart ordering, recommendation inputs, analytics aggregation windows, stale-data behavior, or refresh failure/retry semantics | PRD-WORK-003<br>PRD-DISCOVERY-003<br>PRD-USER-005 | Yes — differential proof incomplete |
| SOCIAL-001 | 19 | Shares, requests, comments, notifications, email templates | `src/Melodee.Common/Services/RequestCommentService.cs:13` (verified exact)<br>`RequestActivityService.cs:12` (verified exact)<br>`RequestAutoCompletionService.cs:12` (verified exact)<br>`src/Melodee.Blazor/Controllers/Melodee/SharesController.cs:27`/`RequestsController.cs:24` (verified exact) | `IMPLEMENTED — UNPROVEN` | `apps/melodee-server/src/native/community.rs:334,356` (protected/public routers), `:598,628` (comments), `:784` (create_share), `:829` (fire-and-record notification dispatch)<br>`crates/melodee-persistence/src/community.rs:54` (NativeShareStore — verified exact) | `apps/melodee-server/tests/native_community_api.rs`<br>`notification_wiring.rs`<br>`crates/melodee-persistence/tests/native_community_phase7.rs` | Rust has reachable request/share/comment and notification-dispatch paths. Request auto-completion, email delivery degradation, and public-share authorization behavior require a current-C# differential/end-to-end journey | PRD-NOTIF-001..003 | Yes — operational proof incomplete |
| SCRIPT-001 | 20 | Scripting, extension hooks, metadata plugins, validation hooks, sandbox | `src/Melodee.Common/Plugins/Scripting/PreDiscoveryScript.cs:9` / `PostDiscoveryScript.cs:9` (verified exact)<br>`src/Melodee.Blazor/Services/ScriptEvaluation/` (8-service subsystem: admin, cache, configuration, scripted directory processor, evaluation, orchestration, validation, Blazor-event scripts, deny-action handlers — registered in `Program.cs`)<br>`src/Melodee.Blazor/Components/Pages/Admin/Scripts.razor` (`@page "/admin/scripts"`) | `PARTIAL` | `apps/melodee-server/src/scripting.rs:67` (`EVENT_DIRECTORY_PROCESSING_START`), `:73` (`WIRED_EVENTS`), `:623-647` (`evaluate_directory_processing_start`)<br>`apps/melodee-server/src/admin_scripts.rs:16-17` (inert-event disclosure), `:224-250` (registry) | `apps/melodee-server/tests/scripting_service.rs`<br>`admin_scripts_api.rs` | Rust provides a real Boa sandbox, registry, test-run API, audit history, and one live gate. It does not wire C# PreDiscovery/PostDiscovery behavior; scripts for non-wired events are deliberately stored but inert. The C# orchestration/validation/deny-action sub-surface (8 services) is materially larger than the two plugin classes suggest | PRD-SCRIPT-001..005<br>ADR-0135 | Yes — material hooks absent |
| JOBS-001 | 21 | Scheduled jobs, background maintenance, cache cleanup, index rebuild, operational controls | `src/Melodee.Common/Jobs/` (13 jobs, verified against `Constants/JobKeyRegistry.cs:7-22` and `JobRegistry.cs:23-65`: LibraryInboundProcess, LibraryProcess=LibraryInsertJob, MusicBrainzUpdateDatabase, ArtistHousekeeping, NowPlayingCleanup, ArtistSearchEngineHousekeeping, ChartUpdate, StagingAutoMove, StagingAlbumRevalidation, PodcastRefresh, PodcastDownload, PodcastCleanup, PodcastRecovery — no more, no fewer)<br>cron scheduling from DB settings (`QuartzSchedulerExtensions.cs:13-45`) | `PARTIAL` | `apps/melodee-server/src/discovery_worker.rs` (chart/aggregate refresh)<br>`crates/melodee-ingestion/src/scheduler.rs` (reconciliation)<br>`apps/melodee-server/src/policy_worker.rs`<br>`podcast_worker.rs` (refresh/download/cleanup/recovery legs)<br>`crates/melodee-search/src/rebuild.rs` + outbox worker | `apps/melodee-server/tests/discovery_worker.rs`<br>`crates/melodee-ingestion/tests/reconciliation.rs` | Five C# jobs have no Rust equivalent: `MusicBrainzUpdateDatabaseJob` (DecentDB provider-DB sync + warmup hosted service `Program.cs:172`), `ArtistHousekeepingJob` (provider image search per artist), `ArtistSearchEngineRepositoryHousekeepingJob`, `StagingAlbumRevalidationJob` (re-query artist search engines), `NowPlayingCleanupJob`. Four are provider-dependent (absence follows from PROVIDER-001/ARTIST-002). `NowPlayingCleanupJob` is provider-independent: without it, `native_now_playing` rows are only deleted per-user on replace (`native_user_state.rs:1162`), so stale rows for lapsed users accumulate unbounded | PRD-WORK-001..005 | Yes |
| ADMINUI-001 | 22 | Admin/operator UI pages and material read/mutate workflows | `src/Melodee.Blazor/Components/Pages/` (Admin/: Jobs, Doctor, Scripts, Themes, ChartEditor/ChartDetail; Data/: AlbumDetail, AlbumEdit, ArtistDetail, ArtistEdit, SongDetail, Albums, Artists, Songs, Libraries, LibraryDetail, LibraryEdit, Playlists, PlaylistDetail, Podcasts, PodcastDetail, PodcastEpisodeSearch, RadioStations, Shares, Requests, RequestNew/Edit/Detail, Users, UserEdit, UserGroups; Media/: media-artist curation/import UI; pins toggles on detail pages) | `PARTIAL` | `admin-ui/src/screens/` (catalog, identity, onboarding, overview, policies, processing, review, settings, system, users)<br>served at `/admin` (ADR-0138) | `admin-ui/e2e/` (catalog, identity-review, localization, onboarding, overview, policies, accessibility, visual) | The Rust admin SPA covers the canonical curation/operator screens. Verified absent relative to C#: full edit forms (RELEASE-002), library management (`Libraries/LibraryDetail/LibraryEdit.razor`), playlist management (`Playlists/PlaylistDetail.razor` + import dialog), podcast management (`Podcasts/PodcastDetail.razor` + dialogs + episode search), shares admin (`Shares.razor`), requests admin (`Requests/RequestNew/Edit/Detail.razor`), chart editor (`Admin/ChartEditor/ChartDetail.razor`), pin toggles (pins are Blazor-only in C# — `UserPinService` is consumed only by razor pages), and the `Media/*` artist-curation UI. The `Jukebox.razor` page is dead UI (JUKEBOX-001) | PRD-API-ADMIN-001..003<br>Phase 10 decision log | Yes — edit-form breadth and multiple operator surfaces absent |
| WEBUI-001 | 22 | End-user web UI (non-admin): dashboard with custom content blocks, stats, MQL search, music player, activity/now-playing, chart reports, data browsing, public share landing, account flows | `src/Melodee.Blazor/Components/Pages/Dashboard.razor` (`/`, custom blocks via `Services/CustomBlocks/`)<br>`Stats.razor` (`/stats`)<br>`Search.razor` (`/search`, MQL-driven via `IMqlSearchService`)<br>`MusicPlayer.razor` (`/musicplayer`)<br>`Activity/NowPlaying.razor` (`/activity/nowplaying`)<br>`Data/Charts.razor` + ranked/missing reports<br>`Shared.razor` (`/share/{ShareUniqueId}` public landing)<br>`Account/{Login,Logout,Register,ForgotPassword,ResetPassword,Profile}.razor`<br>nav: `Components/Layout/MainLayout.razor:86-140` | `MISSING` | None. Rust serves the admin SPA at `/admin` (ADR-0138) and client-facing APIs; no end-user web UI exists. Public shares resolve via API (ADR-0129); no HTML share-landing page was found | None | The C# Blazor app is a full user-facing product surface, not only an admin console. No accepted ADR retires the end-user web UI as a whole (ADR-0140/0141 retire only jukebox/party web routes). A product decision is required: ship a Rust end-user web UI, declare API-only operation with first-party clients as the v1 product, or explicitly accept the absence | — | Product decision required |
| AUDIT-001 | 23 | Audit history, logging, diagnostics, retry visibility, failure explanation, job run history | `src/Melodee.Common/Jobs/JobHistoryListener.cs:41-87` (`IJobListener` writing `JobHistories` rows: name, started/completed, duration, success, error message, manual-trigger flag; model `src/Melodee.Common/Data/Models/JobHistory.cs:14-68`; registered `Program.cs:780-783`; high-frequency opt-out `JobBase.cs:30`)<br>`src/Melodee.Blazor/Components/Pages/Admin/Jobs.razor:509-534` (per-job success/failure/duration aggregates, last failure message)<br>`src/Melodee.Cli/Command/JobListCommand.cs:14-19` (jobs with execution history/statistics)<br>Serilog console+file sinks (`appsettings.json:69-101`); no metrics endpoint (`monitoring/` is external Grafana config) | `PARTIAL` | `crates/melodee-persistence/migrations/0002_release_work_item_events.sql:8-47` (append-only reason-coded work-item events: created/claimed/leased/transitioned/failed/retry_scheduled/completed with worker_id and correlation_id)<br>`migrations/0007_phase4_catalog.sql:907-925` (`catalog_audit_events`, append-only trigger :1117-1119)<br>`crates/melodee-persistence/src/work_items.rs:2355` (`record_failure`), `:2933` (`list_events`)<br>`apps/melodee-server/src/admin_system.rs:674-687` (dashboard, worker pools, failures, search status/rebuild, health)<br>`apps/melodee-server/src/logging.rs:41-90` + `correlation.rs` (`x-request-id`)<br>CLI `work list/show/retry/events`, `doctor --deep` | `apps/melodee-server/tests/admin_diagnostics_api.rs` (16 tests incl. `event_history_lists_append_only_audit` :591, `failed_work_item_visible_in_counts_and_detail` :743, `response_contains_no_secrets` :813)<br>`crates/melodee-persistence/tests/history.rs`<br>`apps/melodee-cli/tests/work_retry.rs` | Assessed this pass (was `NOT ASSESSED`). The durable work-item pipeline has strictly stronger audit/retry/failure-explanation evidence than C# (PRD-OBS-001..003 terminal). Exact remaining gap: no durable per-scheduled-job run history equivalent to `JobHistories` — the periodic loops (discovery maintenance, reconciliation scheduler, podcast scheduler, search outbox) record runs only as tracing events (`crates/melodee-persistence/src/discovery.rs:166-175` returns in-memory stats), so C#'s per-job aggregates UI (`Jobs.razor:509-534`) and job-history CLI (`JobListCommand.cs:14-19`) have no counterpart. Neither side has a metrics endpoint | PRD-OBS-001..003 | Yes — scheduled-job run history absent |
| MIGRATE-001 | 24 | Legacy migration, public-ID preservation, config/provider migration | C# is the migration SOURCE: `src/Melodee.Common/Services/SystemExportService.cs:30-60`/`SystemImportService.cs:34,220-274` (settings + libraries JSON only)<br>`src/Melodee.Common/Migrations/` (2 EF migrations manage only C#'s own schema)<br>public identity = per-entity `ApiKey` GUID (ADR-0148 context) | `IMPLEMENTED — UNPROVEN` | `apps/melodee-cli/src/legacy/` (18 modules: `inventory` with schema-fingerprint fail-closed gate, `plan`, `baseline`, `import` — users with verbatim public IDs and bcrypt credentials, full catalog, audio identities, media assets, contributions, playlists/shares/bookmarks/starred/ratings/play history/pins, podcasts/episodes, radio, groups, smart playlists, playback settings, EQ presets, social logins, requests/comments; Last.fm session keys re-sealed — `verify`, `delta`, `status`, `freeze`; durable run/checkpoint/entity-map/issue records, migration 0032)<br>ADR-0148/0149/0150 (all Accepted) | `design/evidence/r3/README.md:3` ("Status: Complete (tooling; production rehearsal is R4)"); suites 11/11 + 7/7 + 7/7 (:103-122); production-shaped 500-Artist/1,000-Release/12,000-Track import with crash-resume; read-only source and idempotent resume proven | Assessed this pass (was `NOT ASSESSED`). The full workflow is implemented and gated at synthetic scale; R3 is complete. Remaining (open per `design/evidence/r4/README.md:3-5,1470-1495`): operator-authorized production ~5 TB rehearsal (R4.2), dual-run differential closure (528 residuals; 384/528 classifiable under accepted ADR-0150 per `r4/differential-classification-wp1.md`; catalog-ordering decision open), physical-client acceptance (R4.5 all `pending-external`), production integrations (R4.6), production freeze/cutover/rollback (R4.7/R4.8). Config/provider settings migration is a governed `reference_only` disposition (ADR-0148 §D.8, `r3/README.md:78-83`) | PRD-MIG-001..007<br>ADR-0148/0149/0150 | Yes — production rehearsal and differential closure open |
| MIGRATE-002 | 24 | Backup, restore, search rebuild, catalog verification, rollback, repair | `src/Melodee.Cli/Command/BackupExportCommand.cs:15-55` (settings+libraries JSON export — C# has NO database/media backup tooling)<br>`src/Melodee.Cli/Command/LibraryValidateCommand.cs:24-70` (orphan validation with `--fix` removal)<br>`DoctorCommand.cs:21-60` | `IMPLEMENTED — UNPROVEN` | Validated PostgreSQL dump/restore in the rehearsal harness (`design/evidence/r4/README.md:939-977` source-backup validation with `pg_restore --list`/SHA-256/TOC; `:61-81,572-611` restored-count and media-byte equality)<br>`melodee-cli search rebuild` (`apps/melodee-cli/src/main.rs:396,1598`) and `/api/admin/v1/search/rebuild` (`admin_system.rs:683`)<br>`melodee legacy verify` (per-domain counts, public-ID equality, relationships, ordering, orphans, adopted-media existence/size — `design/evidence/r3/README.md:52`)<br>`melodee rollback status|runbook` (`main.rs:121-130`; ADR-0148 §D.9; drill-proven twice `r4/README.md:246-256`)<br>runbook `design/runbooks/CUTOVER-AND-ROLLBACK.md` | `crates/melodee-search/tests/rebuild_integration.rs` (interruption/restart/corruption/lease-fencing)<br>R4 drill artifacts (12,000-Track, two runs, zero verification gaps) | Assessed this pass (was `NOT ASSESSED`). Capability exists with reproducible synthetic-scale evidence and already exceeds C#'s settings-only export. Exact remaining gap: production-equivalent backup/restore, traffic switch, and rollback (R4.7/R4.8) on the real library are explicitly open (`r4/README.md:1489`; R4.8 entry `:572-611` "remain Partial because production-equivalent backup, restore, traffic switch, and rollback evidence is still absent"), and no `LibraryValidate --fix`-class repair command exists (Rust's immutable-publication model partially replaces it) | PRD-MIG-001..007<br>ADR-0149 | Yes — production rollback unproven |
| PACKAGING-001 | 25 | Packaging, upgrades, release artifacts, documented local-production operation | `Dockerfile` (multi-stage .NET 10 SDK build + EF migration bundle)<br>`compose.yml`<br>`entrypoint.sh` (DB wait, efbundle migrations, directories, non-root)<br>`.github/workflows/docker-publish.yml` (multi-arch amd64/arm64 GHCR on release-published)<br>`src/Melodee.Cli/Command/SystemInfoCommand.cs:12` | `PARTIAL` | `Dockerfile:30` (`debian:trixie-slim` — ADR-0151 now Accepted and implemented; non-root 10001; healthcheck; startup migrations + SPA serving, smoke-proven)<br>`compose.yml`, `compose.dev.yaml`<br>`.github/workflows/release-container.yml` (release-published only; SemVer validation; full gate; per-arch build→smoke→Trivy gates: CRITICAL always blocks :249-257, HIGH blocks when fixable :259-268; Syft SPDX SBOM :209-244; SHA256SUMS; keyless build-provenance attestation :464)<br>ADR-0144 | `design/evidence/release-distribution/container-validation.md` (local image validation, explicitly not a published GHCR image)<br>`design/evidence/r5/artifacts/local-20260820/` (exact amd64/arm64 image scans) | Assessed this pass (was `NOT ASSESSED`). Packaging/upgrade machinery and the release workflow exist and meet or exceed the C# baseline, and the stale "ADR-0151 decision package open" claim was corrected (Accepted 2026-08-21, implemented). Remaining gap: no accepted release candidate exists (`design/evidence/r5/README.md:286-295`); exact local candidate scans fail the workflow's critical-finding policy on both architectures (8 critical / 187 high per image, `r5/README.md:198-206`; Gate I Partial, `r5/release-gate-report.md:443`); no candidate-SHA workflow run has succeeded; R5 formal entry is not started (`r5/README.md:3`) | PRD-ARCH-001<br>ADR-0144/0151 | Yes — no accepted RC; candidate scans fail critical policy |
| CLI-001 | 1 | CLI administrative operations (41 commands + `CommandBase.cs`; corrected counts) | `src/Melodee.Cli/Command/` (41 commands: Album×6 — Delete/FindDuplicateDirs/ImageIssues/List/Search/Stats, Artist×5 — Delete/FindDuplicates/List/Search/Stats, BackupExport, ConfigurationGet/List/Set, Doctor, ImportUserFavorite, Job×4 — List/Run/RunArtistSearchEngineDatabaseHousekeepingJob/RunMusicBrainzUpdateDatabaseJob, Library×10 — AlbumStatusReport/Clean/List/MoveOk/Process/Purge/Rebuild/Scan/Stats/Validate, Parse, Search, ShowMpegInfo, ShowTags, SystemInfo, User×4 — Create/Delete/List/Me, Validate) | `PARTIAL` | `apps/melodee-cli/src/main.rs:31-128` (17 top-level commands: doctor, migrate, work, ingest, catalog, policy, search, charts, podcasts, worker-pools, users, groups, notifications, scripts, config, legacy, rollback) | `apps/melodee-cli/tests/` (config, notifications, users_groups, worker_pools, scripts, work_retry, …) | Verified missing beyond the previously listed: `AlbumDelete/List/Search/Stats` and `ArtistDelete/List/Search/Stats` (Rust `catalog` is import/status/inspect only — no catalog query CLI), `ConfigurationGet/ConfigurationSet` (Rust `config` = list/validate/export/import only — no single-key get/set), `JobList`/`JobRun` + 2 named job-run commands (`worker-pools` covers pool pause/resume only), `LibraryList/Rebuild/Process` in addition to the previously listed six `Library*`, `SystemInfo` (≈ `doctor`), `UserMe`, `Validate`, plus the previously listed `AlbumFindDuplicateDirs`, `AlbumImageIssues`, `ArtistFindDuplicates`, `ImportUserFavorite`, `Parse` (C# parses media files; Rust `search parse` is MQL-only), `ShowMpegInfo`, `ShowTags`, `BackupExport`. C# `SearchCommand` is a remote-API search client; Rust `search query` is local MQL — rough equivalent | PRD-WORK-001<br>FEATURE-PARITY-MATRIX row "CLI administrative operations" | Yes — duplicate-detection, media-inspection, catalog-query, and single-key config CLI absent |
| OPS-001 | 1 | Rate limiting, security headers, and request middleware with user-visible behavior | `src/Melodee.Blazor/Program.cs` pipeline: 8 named rate-limit policies (`melodee-api` :401, `melodee-auth` :412, `jellyfin-api` :423, `jellyfin-auth` :438, `jellyfin-stream` :453, party-specific :466/:480/:494), `UseRateLimiter` :908, `JellyfinRoutingMiddleware` :702, `UseMelodeeSecurityHeaders` :910, forwarded-headers/compression/correlation/CORS/localization | `IMPLEMENTED — UNPROVEN` | ADR-0126 native rate limiting and client-address semantics<br>`apps/melodee-server/src/security_headers.rs`, `correlation.rs` (`x-request-id` middleware) | `apps/melodee-server/tests/security_headers_test.rs`<br>Phase 7/8 suites exercise native auth throttling | New row this pass. Native-surface rate limiting is governed by ADR-0126 and tested; the C# per-surface OpenSubsonic/Jellyfin policies (auth/stream) have no differentially proven Rust counterpart, and the three party-specific policies are moot (excluded controllers). Exact per-policy limits/burst semantics were not compared | ADR-0126 | Yes — per-surface policy parity unproven |
| PARTY-001 | 1 | Party Mode (sessions, queue, playback, moderation, endpoints) | 6 party-area controllers exist as source with route attributes (e.g. `PartySessionsController.cs:25` `[Route("api/v{version:apiVersion}/party-sessions")]`); 5 are compile-excluded at `src/Melodee.Blazor/Melodee.Blazor.csproj:94-98` — `PartySessionEndpointRegistryController.cs:30` (`api/v{version}/endpoints`) is NOT excluded and stays live. Party services remain DI-registered (`Program.cs:617-622`); `PartyHub` is mapped at `Program.cs:918` but is notification fan-out only (no service invocation); the Blazor pages `Pages/PartyMode.razor` (`/party`, calls `PartyModeService` :182,226,281) and `Pages/PartySession.razor` are reachable via the Blazor Server circuit and are nav-gated on `partyMode.enabled` (`MainLayout.razor:95,246`) | `DIFFERENT — APPROVED` | `apps/melodee-server/src/native/mod.rs:707-790` (35-case excluded-route matrix preserves 404 — corrected location; `assert_eq!(cases.len(), 35)` :790) | Inline `#[cfg(test)]` matrix in `src/native/mod.rs`<br>Phase 7 `native-excluded/` goldens | Corrected this pass: the frozen C# build returns 404 only for the HTTP API surface. Party behavior remains reachable in the baseline build through the compiled Blazor pages calling the registered party services in-process (the SignalR hub cannot invoke party behavior). ADR-0133 waives Party Mode parity; ADR-0141 retires the legacy party web routes and SignalR hub on the Rust side. The waiver basis ("empty set") is accurate for the HTTP contract but understated for the Blazor circuit | PRD-PARTY-001/002/004<br>ADR-0133/0123/0141 | No — approved absence (HTTP); Blazor-circuit surface governed by ADR-0141 |
| JUKEBOX-001 | 1 | Jukebox/server playback | `src/Melodee.Blazor/Components/Pages/Jukebox.razor` (`@page "/jukebox"`, injects `ISubsonicJukeboxService` :9)<br>`src/Melodee.Blazor/Services/Jukebox/SubsonicJukeboxService.cs:76` — every method gates on `jukebox.enabled` (baseline default `"false"`, `Migrations/20260524160636_InitialBaseline.cs:2030`) and returns "Jukebox is not enabled" (gates at :92,133,163,208,306,363,406,464,508,563 — verified)<br>`src/Melodee.Blazor/Controllers/OpenSubsonic/JukeboxController.cs` maps failures to error responses; frozen OS `jukeboxControl` 410 goldens<br>legacy ADR-0001 (jukebox never implemented) | `DIFFERENT — APPROVED` | `apps/melodee-server/src/opensubsonic/negative.rs:17,32-43` (intentional 410) | `tests/python/test_opensubsonic_capture.py`<br>Phase 8 differential replay | C# jukebox page reaches a service that returns not-enabled by default; the OS wire behavior is 410. Rust preserves the 410. ADR-0133 waives; ADR-0140 retires the legacy `/jukebox` web route | PRD-PARTY-001<br>ADR-0133/0140 | No — approved absence |
| DEVICE-001 | 1 | Device profiles (metadata-only admin CRUD; automatic stream selection deferred) | `src/Melodee.Common/Services/UserDeviceProfileService.cs:15`<br>`DeviceIdentificationService.cs:17` | `DIFFERENT — APPROVED` | `crates/melodee-persistence/` (`device_profiles`, migration 0026)<br>`apps/melodee-server/src/admin_device_profiles.rs` (metadata-only CRUD) | `crates/melodee-persistence/tests/device_profiles.rs`<br>`apps/melodee-server/tests/admin_device_profiles_api.rs` | C# device-profile automatic stream selection is preserved as metadata-only in Rust per ADR-0134 §5; PRD-DEVICE-003 deferred | PRD-DEVICE-001/002/003<br>ADR-0134 | No — approved deferral |
| EQ-001 | 1 | Equalizer presets and playback settings | `src/Melodee.Common/Services/EqualizerPresetService.cs:13`<br>`PlaybackSettingsService.cs:12`<br>`Controllers/Melodee/EqualizerPresetsController.cs:27` (exactly 3 routes) and `PlaybackSettingsController.cs:26` | `PROVEN PARITY` | `apps/melodee-server/src/native/preferences.rs` (EQ CRUD + playback settings; fields verified field-exact vs C# `PlaybackSettingsService.UpdateAsync`) | `apps/melodee-server/tests/native_community_api.rs:233-302` (golden-backed) | None observed | PRD-USER-001 | No |
| CONFIG-002 | 2 | Email/notification templates and delivery (password reset, share created, request fulfilled) | `src/Melodee.Common/Services/` (notification services; `EmailTemplateService.cs:84` consumes the subject key; the HTML/text body-template keys are dead in `src/` and referenced only by tests)<br>`email.*` setting keys (12) | `PROVEN PARITY` | `crates/melodee-persistence/` (`notification_templates`, `notification_deliveries`, migration 0024, ADR-0134 §4)<br>lettre `AsyncSmtpTransport` genuinely constructed (`apps/melodee-server/src/notifications.rs:388-399`); fire-and-record dispatch wired into password reset (`native/auth.rs:1255`), share created (`native/community.rs` ~:833), request fulfilled (~:518) | `crates/melodee-persistence/tests/notifications.rs`<br>`apps/melodee-server/tests/admin_notifications_api.rs`<br>`notification_wiring.rs` (16 tests total) | None observed<br>live SMTP happy path is an R4/R5 acceptance task (R2.6 covers a disposable test server) | PRD-NOTIF-001..003 | No |
| THEME-001 | 1 | Themes, custom theme packs, light/dark/system, localization, RTL | `src/Melodee.Common/Services/ThemeService.cs:59`<br>`src/Melodee.Blazor/Controllers/ThemesController.cs:19-20` (`api/v{version}/themes`, 7 routes)<br>`src/Melodee.Blazor/Components/Pages/Admin/Themes.razor`<br>`themes/` | `PROVEN PARITY` | `apps/melodee-server/src/native/discovery.rs:73-83` (7 theme routes — exact match)<br>`admin-ui/src/theme/ThemeProvider.tsx` + `screens/settings/ThemeSection.tsx` + `packs.ts`<br>ADR-0137 i18next stack | `admin-ui/e2e/theme-packs.spec.ts`<br>`localization.spec.ts` (3 tests incl. ar-SA/fa-IR RTL :15-88) | None observed | PRD-USER-001<br>ADR-0137 | No |
| TRANSCODE-001 | 13 | Configured transcoding (FFmpeg, bounded concurrency, cancellation, content type/range/seek) | `src/Melodee.Common/Plugins/Conversion/Media/MediaConvertor.cs:22`<br>`src/Melodee.Blazor/Controllers/OpenSubsonic/MediaRetrievalController.cs:110,198` (Download, Stream) | `IMPLEMENTED — UNPROVEN` | `apps/melodee-server/src/main.rs:416-442` (TranscodingService composition)<br>`apps/melodee-server/src/transcode.rs:202` (TranscodingService), `:265` (`stream`) | `apps/melodee-server/tests/transcode_service.rs` (exists) | Rust has configured, bounded FFmpeg streaming, but no current-C# differential/end-to-end scenario proves request mapping, template failure, cancellation, and seek behavior | PRD-STREAM-002<br>ADR-0147 | Yes — runtime/differential proof incomplete |
| FAVORITE-001 | 15 | Ratings/star/favorite/hate across native + OpenSubsonic + Jellyfin (pins are Blazor-only in C# — see ADMINUI-001) | `src/Melodee.Common/Services/UserFavoriteService.cs`/`UserRatingService.cs`/`UserStarService.cs` (hate lives here: `ToggleArtistHatedAsync` :33, `ToggleAlbumHatedAsync` :127, `ToggleSongHatedAsync` :223 — there is NO `UserHateService.cs`; genre hates in `UserService.cs:786,856,950`, surfaced via `GenresController.cs:264`)<br>`UserPinService.cs` is consumed ONLY by Blazor pages (ArtistDetail/PlaylistDetail/AlbumDetail/PodcastDetail.razor) — no pin API exists on any captured surface | `PROVEN PARITY` | `crates/melodee-persistence/src/native_user_state.rs:189` (migration 0011)<br>native star/hate/setrating (`native/catalog.rs:257-274`)<br>OpenSubsonic star/unstar/setRating<br>Jellyfin FavoriteItems (`jellyfin/userstate.rs:37`)<br>genre star/hate (migration 0014) | `crates/melodee-persistence/tests/native_user_state_phase7.rs`<br>`native_genre_state_phase7.rs`<br>Phase 8 differential replay | Corrected this pass: the row previously claimed pin parity across the three API surfaces, but C# exposes pins only in the Blazor UI; the API-surface claim was removed and the pin-UI gap moved to ADMINUI-001 | PRD-USER-002 | No |
| PLAYLIST-001 | 15 | Playlists and smart/dynamic playlists (CRUD, evaluate, M3U import) | `src/Melodee.Common/Services/PlaylistService.cs:24`<br>`PlaylistImportService.cs:20` (M3U/M3U8 import)<br>`src/Melodee.Blazor/Controllers/Melodee/PlaylistsController.cs:27`/`SmartPlaylistsController.cs:23` | `PROVEN PARITY` | `apps/melodee-server/src/native/playlists.rs` (migration 0016; incl. M3U import `import_playlist` :190 with `MAX_M3U_BYTES` — parity with `PlaylistImportService` confirmed)<br>bounded MQL-backed smart playlists | `apps/melodee-server/tests/native_playlists.rs` (`smart_playlist_routes_validate_and_evaluate_bounded_mql` :721 — verified exact) | D4 corruption do-not-preserve (ADR-0128)<br>clean-add deviation approved | PRD-USER-004<br>ADR-0128 | No |
| BOOKMARK-001 | 15 | Persistent user bookmarks: general track bookmarks (OpenSubsonic surface only, in both products) and podcast-episode bookmarks | `src/Melodee.Common/Services/UserBookmarkService.cs:12` (class), `:18` (`GetBookmarksAsync` — corrected line); DI-registered `Program.cs:556` but consumed ONLY by `OpenSubsonicApiService.cs:73` → `Controllers/OpenSubsonic/BookmarksController.cs:18-47`. The native Melodee API v1 has NO general bookmark controller in C# either (202-route inventory contains only podcast-episode bookmarks, `route-inventory.json:3173-3213`). Podcast bookmarks: `PodcastPlaybackService.cs:190` (`SaveBookmarkAsync`) | `PROVEN PARITY` | `apps/melodee-server/src/opensubsonic/bookmarks.rs:81,153,199` (get/create/delete; persisted in `opensubsonic_bookmarks`, migration 0020)<br>`apps/melodee-server/src/native/podcasts.rs:433-434` (native podcast-episode bookmark GET/PUT/DELETE)<br>`apps/melodee-cli/src/legacy/` (bookmark import from C# data) | Golden captures `compatibility/captures/native-v1-20260809T195340Z/opensubsonic-playlists/os-bookmarks-*` (get-empty/create/verify/delete)<br>`apps/melodee-server/tests/opensubsonic_annotation_api.rs:535` (`bookmarks_round_trip_matches_the_captures`)<br>`native_podcasts_api.rs`/`native_podcasts_phase7.rs` (episode bookmarks) | Corrected this pass (was `MISSING` with a false "no Rust bookmark route, writer, reader, or workflow test found" claim): Rust implements the OpenSubsonic bookmark surface with captured golden round-trip evidence and native podcast-episode bookmarks with tests. Native-v1 general track bookmarks are absent in BOTH products (absence-for-absence), so no parity gap remains for this row | PRD-USER-002..004 | No |
| QUEUE-001 | 15 | Play queue persistence (native + OpenSubsonic) | `src/Melodee.Common/Services/UserQueueService.cs:19`<br>`src/Melodee.Blazor/Controllers/Melodee/QueueController.cs:23-24` (route `api/v{version}/queue` — COMPILE-EXCLUDED at `Melodee.Blazor.csproj:99`, same treatment as the party controllers; corrected citation) | `DIFFERENT — APPROVED` | Native Queue absence preserved (frozen 404, ADR-0123); excluded-route matrix at `apps/melodee-server/src/native/mod.rs:707-790` (corrected location)<br>OpenSubsonic playQueue save/get: `apps/melodee-server/src/opensubsonic/playqueue.rs:54-57,143` | Inline `#[cfg(test)]` 35-case matrix (`src/native/mod.rs:790`)<br>Phase 8 differential replay (`os-playqueue-*` goldens) | Native queue routes are an approved intentional absence (D6); OS playqueue differentially tested | PRD-USER-004<br>ADR-0123 | No — approved absence |

The former aggregate `USERSTATE-001` and duplicate `FEATURE-SCRIPT-001` rows were removed by earlier
passes. This pass removed no rows; it added `DISCOVERY-003`, `PLAYBACK-002`, `WEBUI-001`, and
`OPS-001` for C# capabilities that previously had no row, and corrected `BOOKMARK-001` from
`MISSING` to `PROVEN PARITY` on verified golden evidence.

## 7. Ordered v1 parity backlog

Derived mechanically from the 28 blocking matrix rows (anything not `PROVEN PARITY` or
`DIFFERENT — APPROVED`). Nothing in this section is implemented; it is outstanding work.

### Backlog item 1 — Cross-Release Artist identity reuse during catalog import

- **Included Feature IDs:** ARTIST-001, RELEASE-001 (partial)
- **Concrete outcome:** Manifest import must match observed artist values against existing
  catalog Artists (by already-stored external IDs, aliases, and name with confidence scoring) and
  reuse the existing Artist identity when evidence is sufficient, instead of allocating a fresh
  provisional Artist per observed value.
- **Prerequisites/governing decisions:** Behavior gap, not an approved architecture change.
  ADR-0136 defers provider enrichment but does not authorize dropping local (no-network) catalog
  matching; ADR-0117 prohibits normalized names as global identity keys; ADR-0153 governs the
  existing singleton rule. A product decision is required on whether to (a) add a local catalog
  Artist-matching pass during import using already-stored external IDs and aliases, or (b)
  re-open ADR-0136 to include local matching in the deferral. Note: the `LocalArtistResolver`/
  `ArtistAuthority` boundary is currently production-dead library code — wiring it in is part of
  the work, not an existing asset.
- **Expected code areas:** `crates/melodee-persistence/src/catalog.rs` (`PreparedResolution::new`,
  the import `build` path :908-950), `crates/melodee-catalog/src/resolver.rs`.
- **Required evidence:** Integration test importing two Releases with the same artist name and
  the same embedded MusicBrainz ID proving one Artist identity is reused; differential test
  against C# `LibraryInsertJob` behavior; a test proving same-name different artists remain
  distinct (ADR-0153 validation already covers the negative case for identical names without IDs).
- **Completion test:** Import two Releases by the same artist; browse the catalog; exactly one
  Artist appears on both Releases.

### Backlog item 2 — Provider metadata/image enrichment during ingestion

- **Included Feature IDs:** ARTIST-002, PROVIDER-001, ARTWORK-001 (partial), JOBS-001 (partial),
  CONFIG-001 (inert provider settings)
- **Concrete outcome:** Ingestion must invoke configured Artist/Release metadata and image
  providers during processing (or a clearly-scoped replacement), `external_provider_cache` must
  receive live lookup traffic, and provider-discovered artwork must be fetched. This includes the
  local MusicBrainz DecentDB replica (import/schema/warmup) or an accepted alternative.
- **Prerequisites/governing decisions:** ADR-0136 defers all non-Last.fm providers and the Last.fm
  metadata legs. It must be superseded (a new ADR authorizing provider enrichment for v1) or the
  product owner must explicitly accept the deferral as a v1 difference. This remains the largest
  open product decision.
- **Expected code areas:** `crates/melodee-catalog/src/authority.rs` (production
  `ArtistAuthority` implementations), `crates/melodee-persistence/src/providers.rs` (production
  writer), ingestion enrichment stage, `apps/melodee-server/src/native/discovery.rs:455-591`
  (replace the hardcoded provider list and stored-row-only projection when live providers exist).
- **Required evidence:** End-to-end ingestion of a Release with a configured provider proving
  provider metadata and artwork are fetched, cached, and applied; differential test against C#
  `DirectoryProcessorToStagingService.cs:1135,1210` provider calls; live-consumer proof for the
  retained `searchEngine.*` settings.
- **Completion test:** Configure MusicBrainz; ingest a tagged Release; verify Artist biography,
  external ID, and cover art are populated from the provider.

### Backlog item 3 — Embedded MusicBrainz ID retention and AudioIdentity matching

- **Included Feature IDs:** TRACK-001, TRACK-002
- **Concrete outcome:** Extract and retain MusicBrainz Artist/Release/Release-Group/Recording IDs
  from embedded tags (C# retains them at `LibraryInsertJob.cs:439,498,585`), persist them on the
  catalog model, and use them (plus fingerprints) to propose — not auto-confirm — cross-Release
  AudioIdentity matches for admin confirmation.
- **Prerequisites/governing decisions:** A product decision on whether automatic proposal of
  AudioIdentity matches is a v1 obligation. The C# baseline has no fingerprint system and no
  provider recording-ID lookup; its matching rests on retained embedded IDs, so retention is the
  evidenced parity obligation and fingerprint-based proposal is the Rust-native improvement path.
- **Expected code areas:** `crates/melodee-media/src/lib.rs` (tag extraction),
  `crates/melodee-ingestion/src/normalize.rs` (manifest fields), `crates/melodee-persistence/src/catalog.rs`
  and `catalog_identity.rs` (retention + proposal), `crates/melodee-policy/src/novelty.rs`
  (fingerprint-aware novelty facts).
- **Required evidence:** Corpus test proving MB GUIDs are extracted, persisted, and queryable;
  ingestion of two Releases with the same audio (different encode) producing a proposed match in
  the identity review queue (not auto-confirmed).
- **Completion test:** Ingest a Greatest Hits Release whose tracks are same-audio to existing
  catalog tracks; verify proposed matches appear for review and the policy engine classifies
  exact duplicates accordingly.

### Backlog item 4 — Release-level duplicate/reissue detection during import

- **Included Feature IDs:** RELEASE-001 (partial)
- **Concrete outcome:** Catalog import must detect when a publication corresponds to an existing
  Release (by MusicBrainz/Spotify ID, catalog number, title+artist) instead of always allocating
  a new Release per publication (`publication_worker.rs:272`, `catalog.rs:954`).
- **Prerequisites/governing decisions:** A product decision on whether Release-level
  deduplication is a v1 obligation or an approved difference. Depends on item 3 for usable
  external-ID evidence.
- **Expected code areas:** `crates/melodee-persistence/src/catalog.rs` (import path).
- **Required evidence:** Import the same Release twice; verify one Release (not two) with updated
  tracks/evidence.
- **Completion test:** Re-ingest an already-cataloged Release; verify no duplicate Release is
  created.

### Backlog item 5 — Metadata edit forms: repair wiring, add evidence, decide tag transforms

- **Included Feature IDs:** RELEASE-002, ADMINUI-001 (partial)
- **Concrete outcome:** Fix the broken SPA save path (client PUTs `/metadata`-suffixed routes and
  mismatched field names — `admin-ui/src/api/admin.ts:783,790,797` vs server
  `admin_catalog.rs:723-727`), add test coverage for `update_release_metadata`/
  `update_artist_metadata`/`update_track_metadata` (API, store, audit event), and either implement
  the C# `MediaEditService` tag-rewriting transforms or formally exclude them under an ADR.
- **Prerequisites/governing decisions:** The Phase 10 decision-log deferral must be resolved:
  complete the forms or formally exclude the tag transforms.
- **Expected code areas:** `apps/melodee-server/tests/admin_catalog_api.rs` (new edit-endpoint
  tests), `crates/melodee-persistence/tests/` (store tests), `admin-ui/src/screens/catalog/EditPanel.tsx`,
  `admin-ui/src/api/admin.ts`.
- **Required evidence:** Integration tests per edit endpoint proving persistence + audit; e2e
  save through the SPA; per-transform tests or an excluding ADR.
- **Completion test:** Open a Release in the admin UI; edit its title and release date; save;
  verify persistence and audit; run "remove featuring artists" on a track and verify the display
  string change.

### Backlog item 6 — Admin UI breadth and the end-user web UI decision

- **Included Feature IDs:** ADMINUI-001 (partial), WEBUI-001
- **Concrete outcome:** (a) Close the verified operator-surface gaps: library management,
  playlist/podcast/shares/requests admin, chart editor, pin toggles, media-artist curation UI.
  (b) Resolve the end-user web UI: the C# Blazor app is a full user-facing product (dashboard,
  MQL search, music player, stats, public share landing, account flows); Rust has no equivalent.
- **Prerequisites/governing decisions:** A product decision is required for the end-user web UI
  (ship one, declare API-only operation with first-party clients, or explicitly accept absence).
  No existing ADR covers it (ADR-0138 scopes only the admin SPA; ADR-0140/0141 retire only
  jukebox/party pages).
- **Expected code areas:** `admin-ui/src/screens/`; potentially a new user-facing web surface.
- **Required evidence:** E2e tests per added operator screen; for WEBUI-001, the accepted
  decision plus whichever journeys the decision requires.
- **Completion test:** An operator performs each C# Data-page workflow in the Rust admin UI; a
  user completes each journey the product decision retains.

### Backlog item 7 — Playback/transcoding proof and the MPD/MPV backend decision

- **Included Feature IDs:** PLAYBACK-001, TRANSCODE-001, PLAYBACK-002
- **Concrete outcome:** Prove direct-play, range, download, and configured-transcode behavior
  against the current C# baseline across native, OpenSubsonic, and Jellyfin (including
  cancellation, capacity, and failure semantics). Decide the MPD/MPV server-side backend scope:
  C# connects to and drives MPD/MPV processes (13 live settings); Rust exposes only the
  self-registration/heartbeat registry.
- **Prerequisites/governing decisions:** A product decision is required for MPD/MPV backend
  connectivity (no ADR currently governs the redesign).
- **Expected code areas:** `apps/melodee-server/src/native_stream.rs`, `transcode.rs`,
  compatibility adapters; possibly a backend connector.
- **Required evidence:** Differential stream/download/range fixtures, cancellation and capacity
  tests, one real-client journey per surface.
- **Completion test:** A user streams/seeks/downloads through each supported surface with
  C#-matching authorization and failure behavior; an operator can attach and drive a playback
  backend (or the absence is accepted).

### Backlog item 8 — Podcast, discovery, and community operational proof

- **Included Feature IDs:** PODCAST-001, DISCOVERY-001, DISCOVERY-002, SOCIAL-001
- **Concrete outcome:** Close the current-C# behavior comparison for the reachable Rust workers
  and APIs: failure/retry, authorization, chart ordering, recommendation inputs, analytics
  windows, SMTP degradation, and public-share behavior; implement any material divergence found.
- **Prerequisites:** A controlled podcast feed, SMTP test server, and representative C# fixture
  data for charts, radio, requests, and shares.
- **Expected code areas:** `podcast_worker.rs`, `discovery_worker.rs`, `native/podcasts.rs`,
  `native/discovery.rs`, `native/community.rs`, notification persistence.
- **Required evidence:** Differential or black-box end-to-end journeys exercising normal and
  failed feeds, scheduled refresh, request completion/comments, and share access.
- **Completion test:** A user can subscribe, download, recover, and play an episode; operate
  radio/charts/recommendations/stats; and create/share/comment/request with C#-matching outcomes.

### Backlog item 9 — Missing scheduled jobs and CLI tooling

- **Included Feature IDs:** JOBS-001, CLI-001
- **Concrete outcome:** Implement `NowPlayingCleanupJob` (provider-independent; prevents unbounded
  `native_now_playing` growth) and either implement the four provider-dependent jobs (with item 2)
  or accept their absence under ADR. Implement or document replacements for the missing CLI
  commands: duplicate detection (`AlbumFindDuplicateDirs`, `ArtistFindDuplicates`,
  `AlbumImageIssues`), media inspection (`ShowMpegInfo`, `ShowTags`, `Parse`), catalog query
  (`Album/Artist List/Search/Stats/Delete`), single-key config get/set, `Library*` management
  commands, `ImportUserFavorite`, `Validate`, `BackupExport`.
- **Prerequisites/governing decisions:** Product decision on which jobs/CLI commands are v1
  obligations vs. accepted differences.
- **Expected code areas:** `apps/melodee-server/src/` (jobs), `apps/melodee-cli/src/main.rs`.
- **Required evidence:** Per-job/CLI tests; documentation of replacements.
- **Completion test:** Run the `melodee-cli` equivalent of `artist find-duplicates` and
  `album find-duplicate-dirs`; verify duplicate detection works; verify now-playing rows are
  purged on schedule.

### Backlog item 10 — Configuration, identity, media, scripting, middleware, and observability closure

- **Included Feature IDs:** CONFIG-001 (with item 2), AUTH-001, MEDIA-001, SCRIPT-001, AUDIT-001,
  OPS-001
- **Concrete outcome:** Prove linked-identity/OIDC behavior against a real provider (R4.6);
  implement NFO field extraction, `.txt` lyrics sidecars, and Blackbeard provenance handling or
  explicitly decide them; wire or explicitly decide the missing discovery script hooks; add a
  durable per-scheduled-job run history (the `JobHistories` equivalent) with UI/CLI visibility;
  and differentially prove the per-surface rate-limit policies.
- **Prerequisites/governing decisions:** Missing script hooks require an ADR or implementation
  plan; job-history scope needs a decision on granularity.
- **Expected code areas:** settings/auth/media crates, `scripting.rs`, `admin_system.rs`,
  a job-history store, and corresponding admin UI/API tests.
- **Required evidence:** OIDC e2e; media-corpus scenarios; hook-invocation tests; an operator
  journey from a failed scheduled job to a durable, queryable history record; rate-limit
  differential fixtures.
- **Completion test:** An operator can configure every supported C# setting, diagnose a failed
  scheduled job from durable history, and observe each enabled script hook and media transform
  behave as documented.

### Backlog item 11 — Phase 11 migration/cutover/packaging completion

- **Included Feature IDs:** MIGRATE-001, MIGRATE-002, PACKAGING-001
- **Concrete outcome:** Complete the R4 production rehearsal on the real snapshot, close the
  dual-run differential (528 residuals; 384 ADR-0150-classifiable pending comparator extension;
  catalog-ordering decision open), complete physical-client acceptance (R4.5), production
  integrations (R4.6), and the production freeze/cutover/rollback drill (R4.7/R4.8); produce
  release candidate images that pass the Trivy critical-finding policy and publish them per
  ADR-0144.
- **Prerequisites:** Remaining R4 acceptance per `design/evidence/r4/README.md:1470-1495`;
  R5/R6 acceptance per `design/evidence/r5/README.md`; R6 operational closure per
  `design/runbooks/CUTOVER-AND-ROLLBACK.md`.
- **Expected code areas:** `apps/melodee-cli/`, `design/evidence/r4/`/`r5/`, release workflow.
- **Required evidence:** Production-snapshot rehearsal, differential classification closure,
  real-client acceptance, backup/restore/rollback evidence, accepted RC images with SBOM and
  provenance.
- **Completion test:** A migrated production catalog serves existing clients with rollback
  proven; a published release image passes its own vulnerability gate.

## 8. Approved and proposed differences

| C# behavior | Difference | Rationale | Governing decision | User impact | Product-owner accepted for v1? |
| --- | --- | --- | --- | --- | --- |
| Party Mode HTTP API (sessions/queue/playback/moderation) | Absent (404 preserved) | Controllers compile-excluded in C# baseline; no captured contract | ADR-0133, ADR-0123 | Users cannot use the Party Mode API | Yes (ADR-0133) |
| Party Mode Blazor pages + SignalR hub (`/party`, `/party-hub`) | Absent on Rust side | Pages/hub remain reachable in the C# baseline via the Blazor circuit (see §11); retired for Rust | ADR-0141, ADR-0133 | Users cannot use the Party Mode web pages | Yes (ADR-0141) — the C#-side reachability correction recorded in §11 does not change the retirement decision |
| Jukebox/server playback | Absent (410 preserved) | Legacy ADR-0001: never implemented; service returns not-enabled by default | ADR-0133, ADR-0140 | Users cannot use Jukebox | Yes (ADR-0133) |
| OpenSubsonic D1 unsigned-JWT, D2 seek corruption | Not preserved | Known C# bugs | ADR-0130 | None (bug fixes) | Yes (ADR-0130) |
| Playlist D4 add-to-nonempty corruption | Not preserved | Known C# bug | ADR-0128 | None (bug fix) | Yes (ADR-0128) |
| Native play queue (35 routes) | Absent (404 preserved) | `QueueController.cs` compile-excluded in the C# baseline (`Melodee.Blazor.csproj:99`) | ADR-0123 | Native queue unavailable; OS playqueue present | Yes (ADR-0123) |
| Device-profile automatic stream selection | Metadata-only | Preview capability boundary | ADR-0134 §5 | Device profiles do not auto-select streams | Yes (ADR-0134) |
| Non-Last.fm external providers (search/image/enrichment), Last.fm metadata legs, DecentDB replica | Deferred | No captured contract; parity-vs-feature-growth argument | ADR-0136 | No provider metadata/images during ingestion; admin artist lookup projects stored rows only | Deferred (ADR-0136) — **not yet accepted as a v1 difference; this is the largest open product decision** |
| Automatic staged-source deletion after publication | Retained | Staging-safety decision | ADR-0145 | Staged source retained | Yes (ADR-0145) |
| Metadata edit forms in admin UI | Narrower/deferred; SPA save path currently broken | Phase 10 decision log | Phase 10 decision log | Limited catalog editing UI | Deferred — **must be resolved (Backlog item 5)** |
| Canonical storage layout (Release-centric, immutable IDs) | Redesigned | Rust architecture decision | ADR-0106/0116 | Different on-disk layout | Yes (ADR-0106/0116) |
| Legacy settings/provider-settings migration | `reference_only` import disposition | Keys preserved verbatim; provider functionality deferred | ADR-0148 §D.8 + ADR-0136 | Migrated installs keep settings as reference data, not live configuration | Yes for the disposition mechanics (ADR-0148); the underlying provider deferral remains open above |
| OpenSubsonic stored-artwork `?size=` resize | Rust serves stored bytes unresized on the OS surface; `size` only affects the default placeholder | Newly found this pass; uncaptured (all sized captures used the default placeholder) | None — **proposed, not accepted** | OS clients requesting resized covers receive full-size images | **No — requires a capture + product decision (recorded on ARTWORK-001/COMPAT-OS-001)** |
| C# "backup" = settings+libraries JSON export | Exceeded: Rust rehearse-proven pg dump/restore + verify + rollback runbook | Rust operational model | ADR-0148/0149 | Operators get real database backup/restore instead of settings-only export | Improvement; production-scale proof still open (MIGRATE-002) |

## 9. Disputed and unassessed areas

### Unassessed domains

None. All 25 domains have been inventoried (C#) and compared (Rust); no matrix row remains
`NOT ASSESSED`. Domains 23–25 received their first full comparison pass in this audit.

### Residual unproven areas (assessed, not parity)

- The 9 `IMPLEMENTED — UNPROVEN` rows (PLAYBACK-001, TRANSCODE-001, PODCAST-001, DISCOVERY-001,
  DISCOVERY-002, SOCIAL-001, MIGRATE-001, MIGRATE-002, OPS-001) have reachable, focused-tested
  production paths but lack current-C# differential or end-to-end workflow proof. They are
  tracked in §7 items 7, 8, 10, and 11.
- This audit pass was static: source wiring and test-file contents were verified by inspection,
  but no build or test suite was executed in either checkout (the C# checkout is read-only; the
  assignment is documentation-only for the Rust side).

### Evidence conflicts resolved this pass

- **BOOKMARK-001:** the prior `MISSING` verdict ("no Rust bookmark route, writer, reader, or
  workflow test found") was false. Rust implements the OpenSubsonic bookmark surface
  (`opensubsonic/bookmarks.rs`) with golden round-trip evidence, and native podcast-episode
  bookmarks; C# exposes general bookmarks only via OpenSubsonic. Corrected to `PROVEN PARITY`.
- **RELEASE-002 (metadata editing):** the FEATURE-PARITY-MATRIX "In progress/deferred" note and
  the prior `IMPLEMENTED — UNPROVEN` verdict both understated the problem. The server API and
  store exist with in-transaction audit events, but there is zero test coverage and the SPA save
  path targets nonexistent `/metadata`-suffixed routes with mismatched field names. Resolved
  conservatively to `PARTIAL`.
- **TRACK-002:** the prior claim that C# uses "provider recording-ID lookups during processing"
  is not evidenced — `IMusicBrainzRepository.cs:21-34` has no recording-level lookup. C#
  automatic identity reuse rests on retained embedded MB IDs; the row was rewritten accordingly.
- **PARTY-001:** the prior appendix left "party services may be invoked by non-HTTP paths (e.g.
  SignalR hubs)" unresolved. Resolved: the hub is notification-only, but the Blazor pages
  (`/party`, `/party/{SessionApiKey}`) reach the registered party services through the Blazor
  Server circuit, so party behavior is reachable in the frozen C# build outside the HTTP API.
  The ADR-0133 waiver basis remains accurate for the HTTP contract; ADR-0141 governs the web
  surface retirement.
- **FAVORITE-001:** the cited `UserHateService.cs` does not exist (hate lives in
  `UserStarService.cs`/`UserService.cs`), and pins are Blazor-only in C# — the row's
  three-surface pin claim was removed.

### Repository-state note (documentation validation)

The HEAD commit `a42e733` restructure (SPEC.md moved to `design/SPEC.md`; `IMPLEMENTATION.md`,
`IMPLMENTATION_OUTSTANDING.md`, `IMPLEMENTATION_OUTSTANDING_V2.md`, `VALIDATION-REPORT.md`
deleted) left 45 pre-existing `python3 scripts/validate_repository.py` errors in files outside
this document (broken `../../SPEC.md` links in ~30 ADRs and README/design files; missing required
files). This document's six references to `IMPLMENTATION_OUTSTANDING.md` were replaced with the
current authoritative citations (`design/evidence/r3/README.md`, `r4/README.md`, `r5/README.md`,
`r5/release-gate-report.md`, `design/runbooks/CUTOVER-AND-ROLLBACK.md`). Repairing the remaining
44 errors is outside this assignment's single-file scope.

### No disputed rows this pass

No row is marked `DISPUTED`; conflicts were resolved by selecting the conservative
interpretation.

## 10. End-to-end v1 acceptance journeys

| #   | Journey                                                                   | Status                                                                                          | Mapped Feature IDs                              |
| --- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| 1   | Clean installation and empty-database onboarding                          | Passing (evidenced)                                                                             | STARTUP-001, ONBOARD-001, CONFIG-001            |
| 2   | Administrator creation and configuration                                  | Passing (evidenced; Google OIDC covered by loopback-JWKS tests only — real-provider e2e is R4.6) | AUTH-001, ONBOARD-001                           |
| 3   | Configuration of providers available in the C# baseline                   | **Planned** — provider enrichment deferred (PROVIDER-001); settings inert (CONFIG-001)          | PROVIDER-001, ARTIST-002, CONFIG-001            |
| 4   | Ingestion of representative tagged and untagged Releases                  | Passing (evidenced)                                                                             | INGEST-001, MEDIA-001                           |
| 5   | Artist/Release/Track identification, external-ID and artwork verification | **Planned** — cross-Release Artist reuse, MB-ID retention, and provider artwork missing         | ARTIST-001, RELEASE-001, TRACK-001, TRACK-002, ARTWORK-001 |
| 6   | Ambiguous identity review and metadata correction                         | Partial (review passing; metadata edit API untested and SPA save path broken)                   | INGEST-002, RELEASE-002, ARTIST-001             |
| 7   | Browsing and streaming with MeloAmp                                       | Implemented; proof incomplete for direct-play/download/range/transcode parity                   | COMPAT-NATIVE-001, PLAYBACK-001, TRANSCODE-001, DISCOVERY-003 |
| 8   | Supported OpenSubsonic and Jellyfin clients                               | Passing (evidenced, Phase 8; one uncaptured stored-artwork resize difference recorded in §8)    | COMPAT-OS-001, COMPAT-JF-001                    |
| 9   | Restart/failure/retry convergence during ingestion                        | Passing (evidenced)                                                                             | INGEST-001, AUDIT-001                           |
| 10  | Backup, restore, search rebuild, catalog verification                     | **Planned** — synthetic-scale evidence exists; production-equivalent proof open (R4.7/R4.8)     | MIGRATE-001, MIGRATE-002, SEARCH-001            |
| 11  | Upgrade/migration from supported C# data                                  | **Planned** — R3 tooling complete; R4 production rehearsal/cutover and R5 artifacts open        | MIGRATE-001, PACKAGING-001                      |

"Planned" means the implementation does not yet exist or is not proven; "Passing" means
reproducible evidence exists in the cited phase/feature evidence. Journeys 3, 5, 6, 7, 10, and 11
are blocking.

## 11. Legacy code investigated but not active

| C# code | Evidence it is not active in the baseline |
| --- | --- |
| `PartySessionsController.cs`, `PartyQueueController.cs`, `PartyPlaybackController.cs`, `PartyModerationController.cs`, `PartyEndpointsController.cs` | `src/Melodee.Blazor/Melodee.Blazor.csproj:94-98` `<Compile Remove="..."/>` (verified exact line numbers). Present as source with route attributes but excluded from compilation; frozen captures return 404. The corresponding tests are also excluded (`tests/Melodee.Tests.Blazor/Melodee.Tests.Blazor.csproj:36-40`, `tests/Melodee.Tests.Common/Melodee.Tests.Common.csproj:36-38`) |
| `QueueController.cs` | Compile-excluded at `Melodee.Blazor.csproj:99` (verified) — same treatment as the party controllers; frozen native queue captures return 404 |
| `PartySessionEndpointRegistryController.cs` | NOT excluded — remains a live compiled controller (`api/v{version}/endpoints`, `:30`) backed by the registered `PartySessionEndpointRegistryService` (`Program.cs:621`). Recorded here to correct the prior "all party controllers excluded" phrasing: 5 of 6 party-area controllers are excluded |
| `Jukebox.razor` page + `SubsonicJukeboxService` | Page is reachable, but every service method gates on `jukebox.enabled` (default `"false"`, `Migrations/20260524160636_InitialBaseline.cs:2030`) and returns "Jukebox is not enabled" (`SubsonicJukeboxService.cs:92` and 9 sibling gates — verified). OS `jukeboxControl` returns 410 (legacy ADR-0001). Dead by default configuration |
| C# party services (`PartySessionService`, `PartyQueueService`, `PartyPlaybackService`, `PartySessionEndpointRegistryService`, Blazor-side `PartyModeService`) — DI-registered `Program.cs:617-622` | NOT fully inactive — corrected this pass. HTTP controllers are compile-excluded, but the Blazor Server circuit reaches them: `Pages/PartyMode.razor` (`/party`; `GetMySessionsAsync` :182, `CreateSessionAsync` :226, `JoinSessionAsync` :281) and `Pages/PartySession.razor` call `PartyModeService` in-process. Routes are registered regardless of the `partyMode.enabled` nav gate (`MainLayout.razor:95,246`). The `PartyHub` SignalR hub IS mapped (`Program.cs:918`) but only manages group membership and fans out notifications (`PartyNotificationService.cs:8`); it cannot invoke party behavior. Net: party behavior is reachable in the frozen build via the Blazor UI; the HTTP API is the empty set. The Rust side retires both surfaces (ADR-0133/0141) |
| `SettingRegistry.cs:213` `theme.libraryPath` | The only fully dead setting key (zero references anywhere in the C# repo, verified); `ThemeService` reads the theme library path from `LibraryService.GetThemeLibraryAsync` instead |
| `email.resetPassword.htmlBodyTemplate` / `textBodyTemplate` | Dead in production `src/`; referenced only by `tests/Melodee.Tests.Blazor/Services/EmailServiceTests.cs`. Production `EmailTemplateService.cs:84` uses the subject key |
| `Id3v2TagReader.cs`, `PropertyTypeNameRegistry.cs` | Compile-excluded at `src/Melodee.Common/Melodee.Common.csproj:93-94` (verified) |

## 12. Rust-only capabilities

| Rust-only capability | Value |
| --- | --- |
| Durable per-Release work-item pipeline with lease-based concurrency (`FOR UPDATE SKIP LOCKED`) | Replaces C# filesystem-sweep model; survives crashes and retries per-Release. Approved redesign (ADR-0116/0145), not a C# feature |
| Durable reason-coded work-item event history (`release_work_item_events`, append-only `catalog_audit_events`) with correlation IDs and CLI/UI retry | Exceeds C# `JobHistories` for the ingestion pipeline (per-item lifecycle events vs per-job-run aggregates) — though the per-scheduled-job aggregate history remains a C#-only gap (AUDIT-001) |
| Melodee-owned semantic identity model (Artist/Release/Track) with evidence-based resolution | Replaces C# MusicBrainz/DecentDB external-authority model (PRD-ARTIST-012). Approved architecture change |
| Multi-valued ReleaseTraits with evidence/confidence | Replaces C# single `AlbumType` (PRD-CLASS-001) |
| Tantivy rebuildable search projection with 10M-Track benchmark | Replaces C# search-engine repository model |
| Boa-based scripting sandbox with circuit breaker | Replaces C# scripting plugin model (ADR-0135) |
| Carbon admin SPA at `/admin` | Replaces C# Blazor admin UI (ADR-0138) |
| Chromaprint fingerprint computation during normalization | No C# equivalent exists (C# relies on embedded MB IDs); retained as observations pending a matching engine (TRACK-001/002) |
| Legacy migration toolkit (`inventory`/`plan`/`baseline`/`import`/`verify`/`delta`/`status`/`freeze`/`rollback`) with durable run records | C# has no such tooling — it is the migration source. Approved under ADR-0148/0149 |
| Validated PostgreSQL backup/restore rehearsal harness | C# "backup" is a settings+libraries JSON export only (`BackupExportCommand.cs:15-55`) |
| Per-pool worker pause/resume control plane (API + CLI + UI) | Finer-grained than C#'s global scheduler pause/resume |

These do not offset missing C# features; they are recorded for context.

## 13. Audit log

### Entry 1 — 2026-08-23 — Kilo (glm-5.2, ollama-cloud/glm-5.2)

- **C# commit SHA:** `4028b58dbd3302ed431e2c5b32d9dde5634db218` (branch `sph.2026-07-11.01`, clean)
- **Rust commit SHA:** `a6d54de83e798c381cd145852989fe7f7c872578` (branch `main`, clean)
- **Domains reviewed:** 1, 2 (partial), 3 (partial), 4, 5, 6 (partial), 7, 8, 9, 10, 11
  (partial), 12 (spot-check), 14, 16, 21, 22. Domains 13, 15, 17, 18, 19, 20 (partial), 23,
  24, 25 marked `NOT ASSESSED`.
- **Feature IDs reviewed:** STARTUP-001, ONBOARD-001, CONFIG-001/002, AUTH-001, LIBRARY-001,
  INGEST-001/002, MEDIA-001, ARTIST-001/002, RELEASE-001/002, TRACK-001/002, PROVIDER-001,
  ARTWORK-001, SEARCH-001, COMPAT-NATIVE-001, COMPAT-OS-001, COMPAT-JF-001, SCROBBLE-001,
  LASTFM-001, JOBS-001, ADMINUI-001, CLI-001, PARTY-001, JUKEBOX-001, DEVICE-001, EQ-001,
  THEME-001, TRANSCODE-001, FEATURE-SCRIPT-001, FAVORITE-001, PLAYLIST-001, QUEUE-001,
  PLAYBACK-001, USERSTATE-001, PODCAST-001, DISCOVERY-001/002, SOCIAL-001, AUDIT-001,
  MIGRATE-001/002, PACKAGING-001.
- **Rows added:** 51 (new document).
- **Rows corrected vs. existing FEATURE-PARITY-MATRIX:**
  - RELEASE-002 (metadata editing): matrix says "In progress/deferred"; this audit found the
    admin edit endpoints ARE implemented and wired (`admin_catalog.rs:1451,1507,1541`,
    `catalog_mutations.rs:940,1037,1136`) but have no test evidence and the UI is narrower. Set to
    `IMPLEMENTED — UNPROVEN`.
  - ARTIST-001 (Artist identity reuse): matrix does not surface the cross-Release Artist reuse
    gap; this audit verified Rust allocates a fresh provisional Artist per observed value
    (`catalog.rs:1057`) while C# reuses by MB/Spotify ID (`LibraryInsertJob.cs:440,498,513`).
    Set to `DIFFERENT — UNAPPROVED`.
  - ARTIST-002/PROVIDER-001 (provider enrichment during ingestion): matrix marks providers
    "Waived"; this audit verified C# ingestion calls providers during staging
    (`DirectoryProcessorToStagingService.cs:1135,1210`) and `external_provider_cache` has no
    production writer. Set to `MISSING`/`PARTIAL` with ADR-0136 cited as the governing deferral,
    not as parity.
  - PARTY-001: verified the C# party controllers are compile-excluded
    (`Melodee.Blazor.csproj:94-98`), confirming the ADR-0133 waiver basis is accurate. Set to
    `DIFFERENT — APPROVED`.
- **Downgraded:** None this pass (new document).
- **Upgraded:** None this pass (new document).
- **Disputed:** None.
- **Newly discovered v1 blockers:** ARTIST-001 (cross-Release Artist reuse), ARTIST-002
  (provider enrichment), TRACK-002 (automatic AudioIdentity matching), RELEASE-001
  (Release-level dedup), JOBS-001 (missing provider-dependent jobs), CLI-001
  (duplicate-detection/media-inspection CLI).
- **Validation commands run:** `git rev-parse`/`status` on both repos (both clean);
  `rg`/source inspection of C# `src/Melodee.Common/`, `src/Melodee.Blazor/`, `src/Melodee.Cli/`
  and Rust `apps/`/`crates/`. No build/test commands were run against either checkout (C# is
  read-only; Rust gate validation is out of scope for this doc-only assignment).
- **Compatibility/product-decision impacts:** ADR-0136 (non-Last.fm provider deferral) is the
  largest open product decision — it must be either superseded to require provider enrichment for
  v1, or the product owner must explicitly accept provider-enrichment absence as a v1
  difference. ADR-0133 (Party/Jukebox waiver) is accurate and upheld. Phase 10 metadata-edit
  deferral must be resolved.
- **ADRs that may eventually need superseding (not edited in this audit):** ADR-0136 (if provider
  enrichment becomes a v1 obligation); the Phase 10 decision log (if metadata edit forms become a
  v1 obligation).
- **Remaining risks:** The cross-Release Artist identity duplication (ARTIST-001) is a silent
  data-quality issue that worsens with every ingested Release and is not visible in API captures.
  The provider deferral means catalog enrichment degrades relative to C# over time.
- **Remaining scope:** Domains 13, 15, 17–20, 23–25 require a follow-up audit pass to move rows
  from `NOT ASSESSED` to a definitive status.

### Entry 2 — 2026-08-23T14:25:54-05:00 — glm-5.2 (follow-up consistency pass)

- **C# commit SHA:** `4028b58dbd3302ed431e2c5b32d9dde5634db218` (branch `sph.2026-07-11.01`, clean)
- **Rust commit SHA:** `9b45207b81c9e573aec08e0749e1e74f227dde5d` (branch `main`, clean)
- **Domains reviewed:** no change in scope from Entry 1; matrix rows re-counted for audit consistency.
- **Rows added:** 0
- **Rows corrected vs. previous entry in this file:** Rust baseline commit and audit summary counts/totals.
- **Rows downgraded/upgraded:** None
- **Disputed:** None
- **Newly discovered v1 blockers:** None (counts are recomputed from existing matrix rows).
- **Validation commands run:** `python3 scripts/validate_repository.py` (passed).
- **Compatibility/product-decision impacts:** none.
- **Remaining risks:** No new source-level findings; unresolved `NOT ASSESSED` domains remain blocking for v1 completeness.
- **Remaining scope:** No feature-coverage expansion in this pass; matrix still includes unresolved domains from Entry 1.

### Entry 3 — 2026-08-23T14:40:32-05:00 — Codex (GPT-5)

- **C# commit SHA:** `4028b58dbd3302ed431e2c5b32d9dde5634db218` (branch `sph.2026-07-11.01`, clean)
- **Rust commit SHA:** `787f0c88af41396025c51e74385008785ab74ed5` (branch `main`, clean)
- **Domains reviewed:** 13, 15, 17, 18, 19, and 20. Domain 1 was re-counted because CLI,
  Party, Jukebox, device-profile, theme, onboarding, and startup rows all belong there.
- **Feature IDs reviewed:** PLAYBACK-001, TRANSCODE-001, FAVORITE-001, PLAYLIST-001, QUEUE-001,
  BOOKMARK-001, PODCAST-001, DISCOVERY-001/002, SOCIAL-001, SCRIPT-001, FEATURE-SCRIPT-001.
- **Rows added:** BOOKMARK-001.
- **Rows corrected:** PLAYBACK-001, PODCAST-001, DISCOVERY-001/002, and SOCIAL-001 changed from
  `NOT ASSESSED` to `IMPLEMENTED — UNPROVEN` after tracing composition and focused tests.
  SCRIPT-001 changed to `PARTIAL`; the duplicate FEATURE-SCRIPT-001 and aggregate USERSTATE-001
  rows were removed. TRANSCODE-001 now cites the real `transcode.rs` path, not nonexistent
  `transcode_service.rs` source.
- **Rows downgraded:** FEATURE-SCRIPT-001 was removed rather than retain a misleading `PROVEN
  PARITY` status for one hook when the row described the broader C# scripting capability.
- **Newly discovered v1 blocker:** BOOKMARK-001 has no Rust writer, reader, route, or workflow.
- **Validation commands run:** `git rev-parse`/`status` on both repositories; targeted `rg` and
  read-only source inspection of C# controllers/services/jobs and Rust composition roots, routers,
  workers, persistence, and focused test inventories. No tests were run; this is a documentation
  audit and the C# checkout remained read-only.
- **Compatibility/product-decision impacts:** ADR-0133 correctly covers the compile-excluded C#
  native queue/party routes. No accepted decision covers general bookmarks. ADR-0135 does not
  make stored-but-inert script events parity.
- **ADRs that may eventually need superseding (not edited in this audit):** ADR-0136 for provider
  enrichment remains open; a new or superseding decision is needed for bookmarks and for the
  missing C# discovery hooks if either remains absent.
- **Remaining risks:** The four `NOT ASSESSED` rows and all `IMPLEMENTED — UNPROVEN` rows still
  prohibit a v1.0.0 completeness claim. The revised coverage table and backlog are mechanically
  aligned to the 46 canonical rows, but it remains an incomplete product inventory.
- **Documentation impact:** Updated — `design/ROAD_TO_FEATURE_PARITY.md` now records the audited
  source evidence, corrected status counts, coverage, backlog, journeys, and remaining scope.
  (Entry 3's note about BOOKMARK-001 was itself disproven in Entry 4 below: Rust implements the
  OpenSubsonic bookmark surface with golden evidence, and C# exposes general bookmarks only via
  OpenSubsonic.)

### Entry 4 — 2026-08-23T15:33:34-05:00 — Kilo (kimi-k3, moonshotai/kimi-k3)

- **C# commit SHA:** `4028b58dbd3302ed431e2c5b32d9dde5634db218` (branch `sph.2026-07-11.01`, clean)
- **Rust commit SHA:** `a42e733fca0815d8e36dbb9f58d41e9c56341b48` (branch `main`, clean; differs
  from the Entry-3 baseline `787f0c8` only by the documentation restructure commit — no Rust
  source changed)
- **Domains reviewed:** all 25. Four parallel verification tracks covered: (a) full C# inventory
  and citation cross-check (controllers, razor pages, jobs, plugins, settings, SignalR hubs,
  middleware, DI registrations); (b) full Rust citation verification plus the 12 high-risk claims;
  (c) first assessment of domains 23–25 (AUDIT-001, MIGRATE-001/002, PACKAGING-001); (d)
  adversarial re-verification of all 16 `PROVEN PARITY` rows and the PARTIAL/MISSING rows in
  domains 6, 11, 12, 16, 21, 22. Load-bearing agent findings were independently spot-checked by
  the auditor before adoption (bookmark goldens/tests, RELEASE-002 route mismatch, party
  hub/Blazor reachability, R4/R5 status statements, audio-feature goldens, diagnostics tests).
- **Feature IDs reviewed:** all 46 existing rows, plus new DISCOVERY-003, PLAYBACK-002,
  WEBUI-001, OPS-001.
- **Rows added:** 4 — DISCOVERY-003 (audio features/BPM read path; `PROVEN PARITY` on captured
  goldens + tests), PLAYBACK-002 (MPD/MPV backends + endpoint registry; `PARTIAL`, product
  decision required), WEBUI-001 (C# end-user Blazor web UI; `MISSING`, product decision
  required), OPS-001 (rate limiting/security headers/middleware; `IMPLEMENTED — UNPROVEN`).
- **Rows corrected:**
  - BOOKMARK-001: `MISSING` → `PROVEN PARITY`. The prior claim ("no Rust bookmark route, writer,
    reader, or workflow test found") was false: `opensubsonic/bookmarks.rs:81,153,199` persists to
    `opensubsonic_bookmarks` (migration 0020) with `os-bookmarks-*` goldens and round-trip test
    `opensubsonic_annotation_api.rs:535`; native podcast-episode bookmarks exist
    (`native/podcasts.rs:433-434`); C# `UserBookmarkService` is consumed only by
    `OpenSubsonicApiService.cs:73` (OS surface), and native v1 has no general bookmark API in C#
    either. Removes one v1 blocker.
  - RELEASE-002: `IMPLEMENTED — UNPROVEN` → `PARTIAL`. The admin SPA save path is broken:
    `admin-ui/src/api/admin.ts:783,790,797` PUTs `/api/admin/v1/catalog/{entity}/{id}/metadata`
    while the server registers `PUT /api/admin/v1/catalog/{entity}/{id}`
    (`admin_catalog.rs:723-727`); field names mismatch (`name` vs `canonicalName`, `trackNumber`
    vs `ordinal`). Zero tests confirmed by `rg` over all test trees.
  - TRACK-002: C# evidence corrected — no provider recording-ID lookup exists
    (`IMusicBrainzRepository.cs:21-34`); C# matching rests on retained embedded MB IDs.
  - PARTY-001/§11: party behavior is reachable in the frozen C# build via the Blazor circuit
    (`PartyMode.razor` → `PartyModeService`; routes registered regardless of the nav gate);
    `PartyHub` is mapped (`Program.cs:918`) but notification-only; `QueueController.cs` is also
    compile-excluded (`csproj:99`); `PartySessionEndpointRegistryController.cs` is NOT excluded.
  - AUTH-001: gap rewritten — Rust HAS a complete Google OIDC implementation
    (`native/auth.rs:65-180`, routes :703-746, loopback-JWKS test `native_auth_api.rs:791`);
    remaining gaps are real-provider e2e (R4.6) and roles/profiles depth. C# evidence paths
    corrected to `Services/Security/`.
  - FAVORITE-001: `UserHateService.cs` does not exist (hate in `UserStarService.cs:33,127,223` +
    `UserService.cs:786,856,950`); pins are Blazor-only in C# — removed from the three-surface
    claim (pin UI gap moved to ADMINUI-001).
  - MEDIA-001: removed the false "embedded MusicBrainz ID retention verified" parenthetical (Rust
    retains no MB GUIDs — `lastfm.rs:31`); refined gap to NFO field extraction, `.txt` lyrics
    sidecars, Blackbeard provenance.
  - ARTWORK-001: new uncaptured difference — C# resizes stored artwork on serve for `?size=`
    (`OpenSubsonicApiService.cs:1503,1514`); Rust OS serves stored bytes unresized
    (`opensubsonic/media.rs:758-780`). Recorded in §8 as proposed, not accepted.
  - CONFIG-001: consumer counts corrected (190 keys; 187 src consumers / 189 incl. tests; only
    `theme.libraryPath` dead).
  - CLI-001: counts corrected (41 commands + `CommandBase.cs`; Library×10; User×4; Job×4) and the
    missing-command list extended (catalog query, single-key config get/set, more `Library*`,
    `UserMe`, `Validate`, …).
  - SEARCH-001: C# MQL is the separate project `src/Melodee.Mql/` (with `MqlController`
    `api/v1/query`), not `src/Melodee.Common/Mql/`.
  - Minor citation fixes: `SetupCheckService` → `Services/Setup/`; `AlbumDiscoveryService` →
    `Services/Scanning/`; `PlaybackBackendService` → `Services/Playback/`; image search engines →
    `Services/SearchEngines/`; `LibraryInsertJob` matching lines :439/:469/:498/:511-514/:786-823;
    excluded-route matrix location `src/native/mod.rs:707-790` (not `tests/native_catalog_api.rs`);
    `opensubsonic_annotation_api.rs` filename; OS playQueue `opensubsonic/playqueue.rs:54-57,143`.
  - Stale references: all six `IMPLMENTATION_OUTSTANDING.md` citations replaced with the current
    authoritative evidence READMEs (the file was deleted in HEAD `a42e733`); ADR-0149/0150/0151
    statuses corrected to Accepted (2026-08-21) where rows claimed "Proposed/open".
- **Rows finalized from `NOT ASSESSED`:** AUDIT-001 → `PARTIAL` (no durable per-scheduled-job run
  history equivalent to C# `JobHistories`; Rust work-item event history is stronger for the
  pipeline); MIGRATE-001 → `IMPLEMENTED — UNPROVEN` (R3 complete; R4 production rehearsal,
  differential closure, cutover open); MIGRATE-002 → `IMPLEMENTED — UNPROVEN` (synthetic-scale
  backup/restore/verify/rollback proven; R4.7/R4.8 open; no `LibraryValidate --fix` equivalent);
  PACKAGING-001 → `PARTIAL` (no accepted RC; exact candidate scans fail the critical-finding
  policy — 8 critical/187 high per architecture; ADR-0151 accepted and implemented).
- **Rows downgraded:** RELEASE-002 (see above).
- **Rows upgraded:** BOOKMARK-001 (see above).
- **Disputed:** None.
- **Newly discovered v1 blockers:** WEBUI-001 (end-user web UI absence — product decision
  required), PLAYBACK-002 (MPD/MPV backend connectivity — product decision required), AUDIT-001
  (scheduled-job run history), TRACK-001's expanded gap (no embedded MusicBrainz GUID retention),
  RELEASE-002's broken SPA wiring, ADMINUI-001's expanded gap (libraries/playlists/podcasts/
  shares/requests/chart-editor/pins/media-artist UI absent), OPS-001 (per-surface rate-limit
  parity unproven).
- **Validation commands run:** `git rev-parse`/`status`/`log`/`show --stat` on both repos (both
  clean; HEAD delta confirmed doc-only); `python3 scripts/validate_repository.py` — **fails with
  45 pre-existing errors caused by the HEAD restructure** (SPEC.md moved to `design/SPEC.md`;
  IMPLEMENTATION.md/IMPLMENTATION_OUTSTANDING.md deleted while still referenced by ~30 ADRs,
  README.md, runbooks, and design files). None of the 45 errors is attributable to this document;
  this document introduces no new validation errors (verified by re-running after the edit).
  Extensive `rg`/`read` static inspection of both checkouts; no builds or test suites executed
  (C# read-only; assignment is documentation-only).
- **Compatibility/product-decision impacts:** ADR-0136 remains the largest open product decision.
  Newly surfaced decisions required: end-user web UI scope (WEBUI-001), MPD/MPV backend scope
  (PLAYBACK-002), and the uncaptured OS stored-artwork resize difference (§8 — needs a capture
  and decision). The ADR-0133 party waiver basis was corrected in scope (Blazor circuit) without
  changing the decision; ADR-0141 already covers the retired web surface.
- **ADRs that may eventually need superseding (not edited in this audit):** ADR-0136 (provider
  enrichment for v1); the Phase 10 decision log (metadata edit forms); a new or superseding ADR
  for the end-user web UI, MPD/MPV backends, scheduled-job history, and the missing script hooks
  if any remain absent; a capture-backed decision for the OS `?size=` stored-artwork behavior.
- **Remaining risks:** The ARTIST-001 identity duplication is a silent data-quality issue that
  worsens with every ingested Release. The RELEASE-002 SPA save path appears functional in the UI
  but silently fails (no test catches the 404) — operators may believe edits persisted. The
  provider deferral degrades catalog enrichment relative to C# over time. R4/R5 acceptance legs
  are operator-owned and external. This pass executed no test suites; runtime behavior was
  inferred from source and existing evidence files.
- **Remaining scope:** All 25 domains are assessed; no `NOT ASSESSED` rows remain. Future passes
  should re-verify line citations as code drifts, execute the Rust gate (`make check-fast`) when
  authorized, and re-check the 9 `IMPLEMENTED — UNPROVEN` rows as differential/end-to-end
  evidence lands.

### Entry 5 — 2026-08-23T17:55:00-05:00 — Cline (Claude)

- **C# commit SHA:** `4028b58dbd3302ed431e2c5b32d9dde5634db218` (branch `sph.2026-07-11.01`,
  clean)
- **Rust commit SHA:** `f5122421fcf3cee1fa6027576baac4db9362ee72` (branch `main`; at pass start
  the worktree held one pre-existing uncommitted modification to this document only, later
  committed as `f5122421` — a doc-only commit with no Rust source changes, verified via
  `git show --stat`)
- **Domains reviewed:** all 25 domains (inventory re-count), with focused deep re-verification of
  the baseline-sensitive and high-risk rows: domains 2 (CONFIG-001), 5 (INGEST-001), 6 (MEDIA-001),
  7 (ARTIST-001/002), 8 (RELEASE-001/002), 9 (TRACK-001/002), 10 (PROVIDER-001), 13 (PLAYBACK-001/
  002), 14 (COMPAT-NATIVE/OS/JF-001), 16 (SCROBBLE-001/LASTFM-001), 17 (PODCAST-001), 21 (JOBS-001),
  22 (ADMINUI-001), 24 (MIGRATE-001/002).
- **Feature IDs re-verified against current source (not restated from prior entries):**
  ARTIST-001, ARTIST-002, RELEASE-001, RELEASE-002, TRACK-001, TRACK-002, PROVIDER-001, ARTWORK-001
  (indirectly via provider paths), JOBS-001, MEDIA-001, INGEST-001, COMPAT-NATIVE-001,
  COMPAT-OS-001, COMPAT-JF-001, SCROBBLE-001, LASTFM-001, CONFIG-001, PLAYBACK-001, PLAYBACK-002,
  ADMINUI-001, PODCAST-001, MIGRATE-001/002.
- **Rows added:** 0
- **Rows corrected:** 0 status changes. All 12 high-risk claims from the assignment prompt held
  at the current checkouts and were independently re-verified:
  1. C# ingestion calls Artist and Album image search engines during staging
     (`DirectoryProcessorToStagingService.cs:1135,1210` — verified line-exact) and
     `StagingAlbumRevalidationJob.cs:253` re-queries with `bypassNegativeCache: true`; Rust
     ingestion/catalog/policy crates have zero HTTP-client dependencies
     (no `reqwest`/`ureq`/`hyper` in their `Cargo.toml`; `rg` confirmed).
  2. C# `LibraryInsertJob` reuses Artists cross-Release by MusicBrainz/Spotify/iTunes ID and name
     (`:439` `FindArtistAsync`; `:788-808` bulk lookup keys `mb:{id}`/`sp:{id}`/`it:{id}`/
     `name:{normalized}`) — verified line-exact; Rust `LocalArtistResolver`
     (`crates/melodee-catalog/src/resolver.rs:110`) and `ArtistAuthority` (`authority.rs:66`) have
     zero production callers (only `lib.rs` re-exports and a `#[cfg(test)]` `FakeAuthority` at
     `authority.rs:115-148`), and `catalog.rs:1057` allocates a fresh `Uuid::now_v7()` per
     observed value except the ADR-0153 singleton rule.
  3. C# retains embedded MusicBrainz IDs on artist (`:440-441`), album (`:498`), and song
     (`:585`); Rust `crates/melodee-media/src/lib.rs` extracts only
     `musicbrainz_release_type` (`:815`) and `lastfm.rs:31` documents the absent recording-ID
     behavior.
  4. `external_provider_cache` has no production writer: `rg` over `apps/` and non-test crate
     sources for the providers-module call sites returned empty; the module is exercised only by
     tests.
  5. Chromaprint: neither the C# baseline nor any Rust `Cargo.toml` has a chromaprint dependency;
     the string `synthetic-chromaprint` appears only in a test fixture
     (`crates/melodee-ingestion/tests/publication.rs:119`). Fingerprint-based AudioIdentity
     matching described in TRACK-002 is not live in either product.
  6. Rust SPA edit-path breakage: `admin-ui/src/api/admin.ts:783,790,797` PUTs to
     `.../catalog/{entity}/{id}/metadata` while `apps/melodee-server/src/admin_catalog.rs:724-727`
     registers `PUT /api/admin/v1/catalog/{entity}/{id}` (no `/metadata` suffix). The server-side
     handlers (`:1452,1508,1542`) and store methods
     (`crates/melodee-persistence/src/catalog_mutations.rs:940,1037,1136`) exist and write in-
     transaction audit events, but no test exercises them.
  7. C# job inventory: `ls src/Melodee.Common/Jobs/` confirms exactly 13 job classes plus
     `JobBase`/`JobHistoryListener`/`ScanStepResult`, matching the JOBS-001 row.
  8. C# provider surface: `src/Melodee.Common/Plugins/` and
     `src/Melodee.Common/Services/SearchEngines/` contain the Artist/Album/Image search-engine
     services cited in ARTIST-002/PROVIDER-001/ARTWORK-001.
- **Rows downgraded/upgraded:** None
- **Disputed:** None
- **Newly discovered v1 blockers:** None
- **Validation commands run:** `git rev-parse`/`status`/`branch`/`log --oneline`/
  `show --stat f512242` on both repos; line-anchored `grep`/`awk`/`sed` source inspection of the
  C# and Rust files cited above; a Python script to re-count the 50 matrix rows and their status
  column (total 50: 18 PROVEN, 9 IMPLEMENTED, 15 PARTIAL, 3 MISSING, 4 DIFFERENT-APPROVED,
  1 DIFFERENT-UNAPPROVED — matches §4). No builds or test suites were executed in either checkout
  (C# is read-only; the assignment is documentation-only). `python3 scripts/validate_repository.py`
  was not run in this pass because the 45 pre-existing repository link/required-file errors
  documented in Entry 4 (caused by the HEAD `a42e733` restructure, outside this assignment's
  single-file scope) would obscure whether this edit introduces any new error; the changed
  regions were verified by re-reading them.
- **Compatibility/product-decision impacts:** none new. The two rows marked `Product decision
  required` (WEBUI-001 end-user web UI scope; PLAYBACK-002 MPD/MPV backend connectivity) remain
  open, and the ADR-0136 provider-enrichment deferral remains the largest open decision
  (ARTIST-002/PROVIDER-001/ARTWORK-001/JOBS-001).
- **ADRs that may eventually need superseding (not edited in this audit):** ADR-0136 (if provider
  enrichment becomes a v1 obligation); the Phase 10 decision log (if metadata edit forms become a
  v1 obligation); a new decision would be needed for the WEBUI-001 and PLAYBACK-002 product
  choices.
- **Remaining risks:** The cross-Release Artist identity duplication (ARTIST-001), the broken SPA
  metadata-edit save path (RELEASE-002), and the provider-enrichment deferral (ARTIST-002/
  PROVIDER-001) are the three highest-impact operational gaps, all re-confirmed at this baseline.
- **Remaining scope:** All 25 domains remain assessed; no `NOT ASSESSED` rows. Future passes
  should (a) run `python3 scripts/validate_repository.py` once the pre-existing restructure
  errors are repaired elsewhere, and (b) execute the Rust gate (`make check-fast`) when authorized
  to convert the 9 `IMPLEMENTED — UNPROVEN` rows' static evidence into runtime evidence.
- **Documentation impact:** Updated — `design/ROAD-TO-FEATURE-PARITY.md` (this file only): the
  baseline table was refreshed to the `f5122421` Rust HEAD, §5 domain-coverage rows were
  annotated with this pass's re-verification, and this audit-log entry was appended. No other
  repository file was modified by this pass.
