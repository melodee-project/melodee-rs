# Feature Completion Program — Phase 1 Report

**Status: COMPLETE — local evidence closed 2026-08-25.** Phase 1 closed its assigned
implementation and local-evidence scope before Phase 2 identity/provider work was evaluated.
The parity rows that still say `IMPLEMENTED — UNPROVEN` remain so where a current-C#
differential, external credential, physical-client, or production-library leg was not executed;
that is a truthful residual, not a pending section in this report.

## Objective and terminal decision

Phase 1 delivered the quick wins and the RELEASE-002 P0 save-path correction, retained embedded
MusicBrainz observations, and proved the existing AudioIdentity proposal boundary without making
ambiguous catalog claims. The phase is accepted as complete for its assigned scope. Its provider
and cross-Release identity work was intentionally handed to Feature-Completion Phase 2, which
adds the production resolver/enrichment composition and the additive evidence schema under
[ADR-0162](../../../adr/ADR-0162-durable-provider-enrichment-tasks-and-release-external-evidence.md).

No media bytes were moved, rewritten, deleted, or quarantined by the Phase 1 evidence work.

## Workstream terminal dispositions

| Workstream | Disposition | Evidence |
|---|---|---|
| A — RELEASE-002 | Complete for the implemented metadata contract; legacy tag-rewrite transforms remain the accepted post-parity deferral | `apps/melodee-server/tests/admin_catalog_api.rs::metadata_update_routes_auth_success_and_errors`; `crates/melodee-persistence/tests/catalog_metadata.rs` (4 store tests); `admin-ui/e2e/catalog.spec.ts` metadata group (9 tests, including browser reload persistence) |
| B — TRACK-001/002 | Complete | `crates/melodee-media/src/lib.rs` MusicBrainz tests; `crates/melodee-persistence/tests/catalog_import.rs::catalog_import_persists_all_embedded_musicbrainz_families_and_enrichment_work`; `crates/melodee-persistence/tests/catalog_audio_identity.rs` |
| C1 — six service rows | Local production-path evidence complete; current-C# differential remains a separately tracked residual | Focused playback, transcode, podcast, discovery, and community suites listed below |
| C2 — OPS-001 + MIGRATE-001/002 | Local/rehearsal evidence consolidated; operator-authorized R4/R5 production legs remain external | `security_headers_test`, migration-0034 integration evidence, `design/evidence/r3/README.md`, `design/evidence/r4/README.md` |
| D — evidence and traceability | Complete | This report, parity audit addendum, matrix, generated requirements index, and status overrides |

## Workstream A — RELEASE-002 metadata editing

The server route, admin API DTO field names, store mutation, validation, revision bump, search
outbox event, and audit event are exercised together. The focused server test proves authorized
Release/Artist/Track success, invalid-request 400 responses with no mutation, and not-found
behavior. The persistence suite proves Release, Artist, and Track round trips, partial patches,
stable revisions on rejected values, and audit rows. The browser suite proves the SPA sends the
canonical `title`, `displayArtist`, `releaseDate`, `canonicalName`, `artistType`, and `ordinal`
fields, avoids legacy aliases, and reads the changed Release title after a page reload.

The C# `MediaEditService` transforms that rewrite source tags (featuring-artist removal,
promotion, bulk renumbering, mapping, and similar operations) remain outside this contract by the
Phase 10 decision log. They are not represented as silently completed parity.

## Workstream B — embedded external IDs and AudioIdentity

`melodee-media` passes the following tests:

- MP3/ID3v2 TXXX observations, FLAC/Vorbis observations, and M4A/MP4 observations retain all
  six families: Artist, Release Artist, Release, Release Group, Track/Recording, and Work.
- Untagged files retain an empty observation object; malformed values are retained verbatim as
  observations and are not promoted to trusted UUID identifiers.
- The inspected manifest retains the same six-family object.

The PostgreSQL catalog import fixture then persists Artist and Release external evidence with
provider/source/confidence, persists one AudioIdentity evidence row per retained family, and
schedules 36 idempotent provider tasks without requiring provider availability. The focused
AudioIdentity integration test proves evidence survives merge/split and media paths remain
unchanged. Proposal creation is bounded, duplicate pair keys are idempotent, and ambiguous
evidence never receives an automatic confirmed decision or fabricated negative
`no_catalog_match` assertion.

## Workstream C1 — nine previously `IMPLEMENTED — UNPROVEN` rows

The six service rows now have reproducible local production-path evidence. Their matrix status is
not upgraded to parity solely from local tests where a current-C# differential remains open.

| Row | Local terminal evidence | Residual kept visible |
|---|---|---|
| PLAYBACK-001 | `apps/melodee-server/tests/native_stream.rs`: 17 passed, including ranges, HEAD/GET, token bounds, ACL, symlink safety, bounded concurrency, and cancellation; `native_stream_acl.rs` | Per-surface current-C# differential and physical-client leg |
| TRANSCODE-001 | `apps/melodee-server/tests/transcode_service.rs`: 6 passed, including real ffmpeg/ffprobe output, template failure, capacity, cancellation, and child cleanup | Current-C# seek/download differential |
| PODCAST-001 | `native_podcasts_api.rs`: 3 passed; `native_podcasts_real_feed.rs`: 1 passed; persistence podcast suite | External discovery-provider and current-C# lifecycle differential |
| DISCOVERY-001 | `admin_radio_api.rs`: 5 passed; radio persistence and OpenSubsonic compatibility evidence | Full current-C# validation/order differential |
| DISCOVERY-002 | `native_discovery_api.rs`; `discovery_worker.rs`: 4 passed; `native_discovery_phase7.rs`: 2 passed | Current-C# chart-window and stale-data differential |
| SOCIAL-001 | `native_community_api.rs`: 2 passed; `native_community_phase7.rs`: 1 passed; `notification_wiring.rs`: 2 passed | Current-C# email/request lifecycle differential and external SMTP leg |

OPS-001 local security-header/correlation/rate-limit evidence is present in
`apps/melodee-server/tests/security_headers_test.rs` (5 passed) and the native auth suites.
MIGRATE-001/002 are terminal for the synthetic/rehearsal tooling scope: R3 import/verify and R4
backup/restore/search-rebuild/rollback artifacts are linked from the parity audit. Real-library
rehearsal, traffic cutover, production rollback, physical clients, and provider credentials remain
operator-authorized external legs and are not represented as local passes.

## Decision log

- Embedded MusicBrainz IDs remain manifest observations under the existing manifest JSON contract;
  no Phase 1 schema migration was required.
- Extraction uses the canonical Lofty keys with the recording-ID fallback documented in the media
  implementation. Raw malformed values are preserved; trust is decided later by persistence and
  identity policy.
- AudioIdentity evidence and bounded Proposed decisions are written in the catalog import
  transaction with the existing resolver version. Import/re-import is idempotent.
- Weak, ambiguous, conflicting, locked, and negatively asserted evidence remains unresolved;
  Phase 1 does not manufacture a catalog-wide negative assertion.
- External MusicBrainz lookup and acoustic scoring beyond the bounded exact-evidence proposal
  boundary remain Phase 2/provider or later work.

## Compatibility, data safety, and documentation

Native-v1, OpenSubsonic, Jellyfin, and MeloAmp wire contracts were not changed by Phase 1.
Compatibility-only legacy vocabulary remains isolated from core `Artist`/`Release`/`Track`
domain vocabulary. The Phase 1 schema work is additive evidence only, with forward migrations,
repeatability tests, and restore-based rollback guidance; it does not alter source media.

Documentation impact:
- Updated — `design/evidence/feature-completion/phase-1/PHASE-1-REPORT.md`,
  `PRD.md`, `design/requirements/FEATURE-PARITY-MATRIX.md`,
  `design/requirements/status-overrides.json`, and generated requirements indexes.

## Phase 1 gate evidence

Focused Phase 1 evidence executed on 2026-08-25:

- `cargo test -p melodee-media --lib musicbrainz` — 3 passed.
- `cargo test -p melodee-persistence --test catalog_audio_identity` — 1 passed.
- `cargo test -p melodee-persistence --test catalog_metadata` — 4 passed.
- `cargo test -p melodee-server --test admin_catalog_api metadata_update_routes_auth_success_and_errors` — 1 passed.
- `pnpm exec playwright test e2e/catalog.spec.ts -g 'metadata'` — 9 passed.
- `cargo test -p melodee-server --test native_stream` — 17 passed.
- `cargo test -p melodee-server --test transcode_service` — 6 passed.
- `cargo test -p melodee-server --test native_podcasts_api -- --test-threads=1` — 3 passed.
- `cargo test -p melodee-server --test native_podcasts_real_feed` — 1 passed.
- `cargo test -p melodee-server --test admin_radio_api` — 5 passed.
- `cargo test -p melodee-server --test native_discovery_api` — passed.
- `cargo test -p melodee-server --test discovery_worker` — 4 passed.
- `cargo test -p melodee-server --test native_community_api` — 2 passed.
- `cargo test -p melodee-server --test notification_wiring` — 2 passed.
- `cargo test -p melodee-server --test security_headers_test` — 5 passed.
- `cargo test -p melodee-persistence --test native_community_phase7` — 1 passed.
- `cargo test -p melodee-persistence --test native_discovery_phase7` — 2 passed.
- `cargo test -p melodee-search --test rebuild_integration` — 4 passed.
- `cargo test -p melodee-persistence --test migrations migration_0034_repairs_only_the_same_release_artist_identity` — 1 passed.

The repository-wide Phase 1/Phase 2 gates are recorded in the Feature-Completion Phase 2 report
after the final validation run. No Phase 1 placeholder or unsupported completion claim remains in
this report.
