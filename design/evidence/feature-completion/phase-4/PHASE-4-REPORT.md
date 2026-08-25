# Feature Completion Program — Feature-Completion Phase 4 Report

**Status:** COMPLETE WITH EXPLICIT ACCEPTED DIFFERENCES — local implementation and validation
closed 2026-08-25. Migration, cutover, production-library mutation, and other operator-owned
legs remain outside this decision.

## Phase objective and exit decision

Phase 4 closed the implementation-level operational gaps that are safe to complete before
production migration: reachable MPD/MPV control, release-scoped metadata transformations and
safe rewriting, durable job visibility and maintenance controls, stored-artwork sizing, verified
Admin UI gaps, local authentication depth, scripting/CLI/configuration residuals, and local
compatibility evidence.

The exit decision is terminal for every Phase 4-owned row. Each row is either locally implemented
and evidenced, or has an explicit accepted difference/external disposition below. No listener SPA,
migration, production cutover, or destructive operation against an existing media library was
started.

## Baselines and worktree state

| Repository | Entry HEAD | Entry branch | Entry worktree | Final observation |
|---|---|---|---|---|
| Rust rewrite | `096819bde23c92b281ddce9d635d9824adf729b2` | `main` | clean | Phase 4 changes present; unrelated changes preserved |
| C# oracle | `4028b58dbd3302ed431e2c5b32d9dde5634db218` | `sph.2026-07-11.01` | clean; read-only | unchanged and clean |
| MeloAmp | `096c76d810af91205aeee54da58131766b0f076b` | `sph.2026-07-14.01` | clean; read-only | unchanged and clean |

`mpv` is installed locally. `mpd` is not installed, so no MPD daemon smoke was claimed. The
deterministic fake-server evidence remains the portable gate for both protocols.

## Entry-wave reconciliation

The 50-row parity inventory was recalculated from current production callers, tests, Phase 1/2
evidence, and the C# reachability audit. The historical matrix prose remains unchanged; current
dispositions are recorded in the Phase 4 plan, the current matrix section, and this report.

The Phase 2 OpenSubsonic replay discrepancy is resolved as harness drift. The original replay
reported 158 unexpected differences because the disposable Rust server's background Tantivy
search worker held a PostgreSQL snapshot while the replay reseeded the database. The replay was
rerun with search disabled for the disposable database: **zero unexpected differences** and **321
enumerated intentional differences**. No public contract was changed to satisfy stale fixtures.

The `ADMINUI-001` re-audit found no basis for a duplicate listener application. Libraries/source
topology remains configuration and operator infrastructure; playlists, podcasts, shares, and
listener requests remain MeloAmp/native-client workflows under ADR-0161. Current Admin UI gaps
with material operator impact—metadata transforms and scheduled jobs—were implemented and tested.
Charts, identity/artwork curation, users, groups, profiles, and review workflows already have
current server/UI support. The historical C# media-artist and listener pages were not copied.

The requirement override file was audited and remains structurally current: all 239 requirement
IDs have overrides; the generated requirements index reports current. Existing requirement
statuses already reflect the local implementations and accepted external boundaries, so no
historical status was rewritten merely to make this report green.

## Workstream evidence and terminal dispositions

### A — MPD and MPV playback backends (`PLAYBACK-002`)

Implemented in `apps/melodee-server/src/playback.rs` under ADR-0156:

- object-safe Rust-native `PlaybackBackend`, typed failures, capabilities/status, initialization,
  shutdown, cancellation, and bounded command/reconnect timeouts;
- MPD TCP greeting/password/status/play/pause/stop/seek/volume handling;
- MPV Unix IPC JSON request/response handling, status queries, loadfile, pause/stop/seek/volume,
  reconnect, and malformed-response handling;
- layered `jukebox`, `mpd`, and `mpv` settings with disabled, MPD-selected, and MPV-selected
  composition;
- additive native control/status routes while preserving the existing registered-endpoint and
  heartbeat routes and their DTOs; and
- backend failure isolation from catalog and streaming availability, with redacted structured
  diagnostics.

Deterministic fake MPD and MPV integration tests cover initialize, command, status, disconnect,
reconnect, shutdown, unavailable process, timeout, malformed response, cancellation, and
configuration. The server library playback suite passed (136 tests total). A real local MPV
process smoke passed for IPC startup, property query, volume command, and clean exit. MPD smoke
is explicitly unavailable because the binary is absent. Windows named-pipe transport and
operator-owned MPV executable/audio-device launch settings remain documented configuration
boundaries, not untested claims of daemon management.

### B — metadata transformations, safe rewriting, and media sidecars (`RELEASE-002`, `MEDIA-001`)

ADR-0164 supersedes the impossible whole-file-hash invariant in ADR-0157. A tag rewrite changes
the container bytes by definition; the safety predicate is now a bounded decoded-audio BLAKE3
fingerprint, with before/after whole-file SHA-256 values retained as audit facts.

Implemented:

- release-scoped dry-run preview, apply, audit history, and revert routes;
- featuring/title cleanup, artist promotion/demotion, year updates, replacement mappings, and
  track renumbering with idempotent semantics where applicable;
- advisory serialization per Release, same-filesystem temporary files, fsync/atomic replacement,
  reopen and tag verification, decoded-audio verification, recovery copies, and rollback on
  temporary-write, rename, verification, database-commit, or audit-persistence failure;
- field-level deltas, actor, affected-file count, result, timestamps, and recovery evidence in
  migration `0037_metadata_edit_audits.sql`; and
- NFO fields, same-basename `.txt` lyric sidecars, embedded/LRC lyrics, and exact Blackbeard
  provenance classification/overrides while preserving existing CUE, playlist, lyric, and
  unknown-sidecar behavior.

The real-media suite covers MP3/ID3, FLAC, M4A/MP4, OGG/Vorbis, WAV, Unicode paths, artwork,
NFO, lyrics, and provenance. Media tests passed (49 passed, 1 intentionally ignored). Fault and
property coverage verifies dry-run/apply consistency, idempotence, recovery, temporary cleanup,
decoded-audio preservation, and semantic identity preservation. The Admin UI preview, confirmation,
failure, reload/history, and revert journeys are covered by Playwright; no production media file
was rewritten during Phase 4.

### C — scheduled jobs, maintenance, and operator visibility (`JOBS-001`, `AUDIT-001`)

ADR-0158 was implemented with the next unused migrations:

- `0036_job_run_history.sql` for durable tick/manual run records, uniqueness, completion/failure
  invariants, indexes, and cleanup audit records;
- `crates/melodee-persistence/src/job_history.rs` for bounded list/failure/cleanup operations,
  advisory/unique tick claims, manual principals, and retention limits;
- worker wiring for discovery, podcast, enrichment, and policy runs with tracing correlation;
- admin status/history/failure-summary, pause/resume, run-now, pagination, stale-run reporting,
  and authorization controls;
- stale native and podcast now-playing cleanup; and
- machine-readable `jobs history` plus confirmation-gated `jobs cleanup` CLI operations.

PostgreSQL migration/integrity, manual authorization, useful failure reason, retention, stale
now-playing, and worker recovery tests passed. The Admin UI jobs screen consumes production
history and covers running/paused/success/failed/stale states, controls, errors, pagination, and
confirmation. A durable claim prevents duplicate ownership of a scheduled tick; the advisory
lock is held for the claim transaction, not for the entire worker lifetime. An interrupted run
therefore remains visibly incomplete/stale and can be recovered by later scheduling. Retention
cleanup is an audited operator/manual job; no silent destructive automatic cleanup was added.

### D — OpenSubsonic stored-artwork sizing (`ARTWORK-001` stored-artwork portion)

ADR-0159 is implemented in the OpenSubsonic media adapter. Accepted targets are small=100,
medium=200, large=500, and extralarge=1000 pixels. Stored artwork preserves aspect ratio, never
upscales, returns size-specific bytes and ETags, supports `If-None-Match`, preserves unsized raw
stored bytes, uses a bounded 256-entry in-process variant cache, and invalidates naturally when
the source hash changes. Corrupt/unsupported sized images fail safely to the normal fallback;
admin-override artwork retains precedence. Native-v1 and Jellyfin image contracts use their
existing processing behavior and were not merged with the OpenSubsonic DTO path.

Actual stored-artwork integration used a valid stored 400x200 image and verified 100x50 sizing,
no-upscale at 1000, MIME/ETag/304 behavior, source changes, and override precedence. C# source
audit confirms the stored-artwork path and accepted target semantics; a live C# capture was not
manufactured because the frozen fixture set uses placeholders rather than stored artwork.

### E — Admin UI and operator workflow closure (`ADMINUI-001`)

Current `/admin` and API reachability were audited against C# routes and MeloAmp ownership. The
verified material gaps were metadata transformation and scheduled-job controls; both now have
production API composition, localized Carbon UI, loading/empty/validation/authorization/retry/
failure states, destructive-action impact confirmation, responsive/theme/RTL behavior, and
accessibility/visual/Playwright evidence. User/group/profile/reset flows and catalog identity,
artwork, review, chart, and diagnostics surfaces were reverified rather than duplicated.

The Admin UI gate passed: typecheck, lint, 45 unit-test files with 249 passing tests, localization
check, and `make admin-ui-e2e` with **304 passed and 62 skipped**. Skips are the existing visual
matrix exclusions, not failed journeys. No `web-ui/` listener SPA was created.

### F — authentication, users, roles, and profiles (`AUTH-001` local portion)

Existing native/admin authorization boundaries and Phase 7/9/10 user administration were
reverified. PostgreSQL role/group/profile, session refresh/replay, password-reset one-use,
invalid-token, expired-token, replay, social identity ownership/link/unlink, and admin access
tests passed. The Admin UI user-management journeys remain on the production session/API boundary.
Real Google OIDC validation requires operator credentials and was not executed; OIDC loopback and
failure behavior remain locally tested.

### G — scripting, CLI, and configuration (`SCRIPT-001`, `CLI-001`, `CONFIG-001`)

The active Boa pre-discovery hook is production-wired with bounded time, memory, output, and
capabilities; unrestricted filesystem/network/process/secret access is denied, failures remain
fail-safe, and audit diagnostics identify inert events rather than silently claiming execution.
The C# events without an active Rust consumer remain explicitly inert and documented. CLI command
inventory, catalog/repair/validation/maintenance operations, stable machine-readable job output,
and destructive-operation confirmation were tested. Layered settings resolve deterministically,
secret import/export is redacted, and invalid values produce actionable errors. MPV executable,
audio-device, extra-argument, and debug-output settings are intentionally operator-owned-process
settings and are reported as migrated-but-inert rather than misrepresented as server launch
controls.

### H — remaining local differential evidence

Reachable Rust paths for playback, transcoding, podcast, discovery, social, operations, release,
provider, and artwork rows were exercised with focused production-path, golden, integration, or
real-media evidence. The OpenSubsonic replay is closed as described above. Compatibility suites
for native-v1, OpenSubsonic, and Jellyfin remained green; their wire DTOs and intentional legacy
differences were preserved. Remaining live-provider, physical-client, and production-library
evidence is explicitly external and is not represented as a local pass.

## Features, requirements, and traceability

Phase 4 terminally dispositioned `PLAYBACK-001/002`, `TRANSCODE-001`, `PODCAST-001`,
`DISCOVERY-001/002`, `SOCIAL-001`, `OPS-001`, `RELEASE-001/002`, `MEDIA-001`, `JOBS-001`,
`AUDIT-001`, `ARTWORK-001`, `ADMINUI-001`, `AUTH-001`, `SCRIPT-001`, `CLI-001`, and
`CONFIG-001`. Local implementation and test status is recorded in the current parity matrix
section; external/operator legs are retained as terminal residuals rather than promoted to
unexecuted parity claims.

Relevant PRD traceability includes `PRD-MEDIA-001`, `PRD-CONFIG-001`, `PRD-CLI-001`,
`PRD-SCRIPT-001`, `PRD-AUTH-001..007`, `PRD-DISCOVERY-001..003`, `PRD-WORK-003/005`,
`PRD-OBS-001`, `PRD-UI-004`, and `PRD-TEST-006`. `validate_repository.py` and
`extract_requirements.py --check` confirm the repository documentation and generated requirement
index are current.

## ADRs applied, clarified, or superseded

- Applied: ADR-0156 (MPD/MPV), ADR-0158 (job history), ADR-0159 (stored artwork), ADR-0161
  (MeloAmp/listener ownership), plus the existing ADR-0135, ADR-0138, ADR-0139, ADR-0143, and
  ADR-0147 boundaries.
- Superseded: ADR-0164 supersedes ADR-0157's whole-file SHA-256 integrity wording with decoded
  audio fingerprint verification while retaining whole-file hashes as audit facts.
- No new public contract, external service, broker, or schema family was introduced beyond the
  two additive migrations documented above.

## Validation and exact results

- `cargo fmt --all` — passed.
- Final `make check-fast` — passed with exit code 0 after traceability updates; workspace
  formatting, Clippy, library tests, repository validation, and requirement-index checks were
  green.
- `make test-integration` — passed; all PostgreSQL-backed workspace integration targets completed
  without failures.
- `cargo clippy -p melodee-server -p melodee-persistence -p melodee-media -p melodee-cli
  --all-targets -- -D warnings` — passed.
- `cargo deny check` — passed (`advisories`, `bans`, `licenses`, and `sources` all okay;
  duplicate-dependency notices are warnings only).
- `python3 scripts/validate_repository.py` — passed: 65 ADRs, 8 mockups, 239 requirements.
- `python3 scripts/extract_requirements.py --check` — passed: requirements index current.
- `cd docs && pnpm check && pnpm check:links && pnpm build` — passed; 45 pages built and
  internal links validated.
- Admin UI: `pnpm check`, `pnpm lint`, `pnpm test`, `pnpm i18n:check` — passed; 249 unit tests;
  lint retained only two pre-existing Fast Refresh warnings.
- `make admin-ui-e2e` — passed: 304 passed, 62 skipped.
- Server library — 136 tests passed; media — 49 passed, 1 intentionally ignored; persistence
  migrations — 11 passed.
- OpenSubsonic media — 32 passed; Jellyfin media — 18 passed; admin system API — 10 passed;
  metadata rewrite unit/fault coverage and job-history migration tests passed.
- `python3 scripts/run_phase8_differential.py --only opensubsonic --skip-build
  --db-name melodee_phase4_os_diff2` — passed with zero unexpected differences and 321 known
  intentional differences.
- Final `make check` — passed with exit code 0 after the Phase 4 report, plan, roadmap, matrix,
  and documentation updates; the full workspace test suite and dependency checks were green.

## Compatibility and product impact

Native-v1 retains its existing playback-endpoint registration/heartbeat and authentication
contracts; new playback controls are additive. OpenSubsonic stored-artwork sizing is isolated to
the OpenSubsonic adapter. Jellyfin and native image behavior remain covered by their existing
suites. MeloAmp is unchanged and remains the listener workflow owner under ADR-0161. No C# or
MeloAmp repository was modified.

## Database, rollback, and media safety impact

Migrations `0036_job_run_history.sql` and `0037_metadata_edit_audits.sql` are additive and include
constraints, indexes, integrity tests, and rollback/forward notes. Phase 4 did not run migration
cutover or destructive cleanup against an existing library. Metadata rewrites operate only on an
explicit Release request, use same-filesystem temporary files and recovery copies, and verify
decoded audio and semantic identity before commit.

## Authentication and security impact

New Admin metadata/job routes are authorization-gated. Password/session/reset and identity-link
boundaries remain fail-closed. Playback diagnostics redact passwords and unsafe paths. Boa hooks
retain sandbox capability, time, memory, output, and cancellation bounds. No external identity
provider credential or secret was used.

## Performance and bounded-resource evidence

Playback operations use configured bounded timeouts and cancellation. Metadata preview is bounded
by Release scope and a 2,000-file guard; rewrite and fingerprint subprocesses are time-bounded.
Artwork variants use a 256-entry cache. History APIs cap page/retention inputs. Script execution
and output are bounded. Worker work remains lease/backpressure controlled, and catalog/search
availability is isolated from playback/provider/job failures.

## External or operator-authorized legs not executed

The following remain explicitly outside this local closeout: real MPD daemon smoke (binary absent),
production MPV audio-device playback acceptance, Windows named-pipe acceptance, live provider
credentials/quotas and production replica warmup, real Google OIDC credentials, physical-client
acceptance, production-sized replica/library rehearsal, legacy data migration/import, production
cutover, rollback, and any destructive operation against the production library.

## Remaining risks and Phase 11 scope

The remaining planned implementation milestone is Phase 11 migration/import, packaging, rehearsal,
cutover, rollback, and production-library validation under ADR-0148/0149/0150. Operator-owned
external compatibility legs listed above must remain visible in the Phase 11 runbook. The source
topology/library-management mutation surface remains configuration-owned and is not silently
claimed as an Admin UI feature; a future product decision would be required before adding it.

## Documentation impact:

Documentation impact:
- Updated - `design/agents/FEATURE-COMPLETION-PHASE-4-PLAN.md`,
  `design/evidence/feature-completion/phase-4/PHASE-4-REPORT.md`,
  `design/ROAD-TO-FEATURE-COMPLETION.md`, `design/ROAD-TO-FEATURE-PARITY.md`,
  `design/requirements/FEATURE-PARITY-MATRIX.md`, `design/requirements/status-overrides.json`,
  `design/adr/README.md`,
  `docs/src/content/docs/admin/system-diagnostics.md`, `docs/src/content/docs/api/admin-api.md`,
  `docs/src/content/docs/api/opensubsonic.md`, `docs/src/content/docs/features/discovery-and-playback.md`,
  `docs/src/content/docs/installation/configuration.md`,
  `docs/src/content/docs/library-management/inbound-processing.md`, and
  `docs/src/content/docs/library-management/release-review.md`.
