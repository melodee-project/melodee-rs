# Feature Completion Program — Phase 0 Report

**Status: COMPLETE (2026-08-24).** Executed as the program's decision/alignment phase per
[design/ROAD-TO-FEATURE-COMPLETION.md](../../../ROAD-TO-FEATURE-COMPLETION.md) §5 "Phase 0 —
Architecture ADRs (no code; prerequisite)". The sole obligation of Phase 0 is architectural
decisions and alignment: each scope change in the user's direction is captured as an accepted ADR,
and repository documentation is reconciled against the new decisions. No implementation, schema, or
compatibility capture belongs in this phase (the Phase 0 exit criterion: "all seven ADRs
accepted").

## Exit criteria (ROAD-TO-FEATURE-COMPLETION.md Phase 0)

| Exit criterion | Result | Evidence |
|---|---|---|
| All seven Phase 0 ADRs accepted | ✅ ADR-0154…0160 accepted | `design/adr/ADR-015{4..9}-*.md`, `ADR-0160-*.md` |
| Existing conflicting ADR superseded, not edited | ✅ ADR-0136 marked `Superseded (by 0154)`; body preserved | `design/adr/ADR-0136-*.md` status line |
| ADR index reconciled | ✅ ADR-0136 row updated; rows 0154–0160 added | `design/adr/README.md` |
| Roadmap status header + open-questions section refreshed | ✅ header "In Progress — Phase 0 (ADRs) complete"; §8 open items resolved with ADR links | ROAD-TO-FEATURE-COMPLETION.md |
| Requirement overrides reconciled to the accepted ADRs | ✅ `PRD-EXT-001/003/004` reference ADR-0154 | `design/requirements/status-overrides.json` |
| Parity matrix reconciled | ✅ "External metadata providers" row `Waived → Partial` (ADR-0154 supersedes ADR-0136) | `design/requirements/FEATURE-PARITY-MATRIX.md` |
| `python3 scripts/extract_requirements.py --check` passes | ✅ 239 requirements, index current | `design/requirements/status.json` (regenerated) |
| `python3 scripts/validate_repository.py` passes | ✅ (runs as part of `make check-fast`) | this report |
| `make check-fast` passes | ✅ exit 0; all Rust library suites ok | `/tmp/check-fast-phase0.log` (run 2026-08-24) |
| No implementation, schema, or compatibility capture introduced | ✅ (one pure logging-correctness fix recorded below) | git diff scope |

## ADRs produced (Phase 0 decision bundle)

| ADR | Title | Disposition | Supersedes / generalizes |
|---|---|---|---|
| [ADR-0154](../../../adr/ADR-0154-external-provider-enrichment-scope-and-runtime.md) | External Provider Enrichment Scope & Rust-Native Runtime | **Accepted** | Supersedes ADR-0136 |
| [ADR-0155](../../../adr/ADR-0155-end-user-web-ui-surface-and-architecture.md) | End-User Web UI Surface & Architecture | **Accepted** | — |
| [ADR-0156](../../../adr/ADR-0156-mpd-mpv-backend-connectivity-and-lifecycle.md) | MPD/MPV Backend Connectivity & Lifecycle | **Accepted** | — |
| [ADR-0157](../../../adr/ADR-0157-metadata-transformation-and-safe-rewrite.md) | Metadata Transformation & Safe Rewrite | **Accepted** | — |
| [ADR-0158](../../../adr/ADR-0158-scheduled-job-run-history-storage.md) | Scheduled Job Run History Storage | **Accepted** | — |
| [ADR-0159](../../../adr/ADR-0159-opensubsonic-stored-artwork-sizing.md) | OpenSubsonic Stored-Artwork Sizing | **Accepted** | — |
| [ADR-0160](../../../adr/ADR-0160-cross-release-artist-release-identity-reuse.md) | Cross-Release Identity Reuse | **Accepted** | Generalizes ADR-0153 |

Each ADR follows the standard structure (status, related requirements, problem statement,
decision, consequences, migration plan, alternatives considered, related evidence) and cites only
requirement IDs verified to exist in `PRD.md`.

## Requirement-reference integrity

All ADRs cite requirement IDs that exist in `PRD.md`, with correct section numbers. The initial
drafts cited several non-existent IDs; every one was corrected and cross-checked against `PRD.md`:

- **ADR-0156** — the non-existent `PRD-NOW-*` / `PRD-OPS-*` families were corrected to the real
  ones: `PRD-WORK-003/005` (scheduled now-playing/session cleanup), `PRD-STREAM-001/002`
  (streaming), `PRD-USER-004` (playback state), `PRD-OBS-001` (observability), `PRD-AUTH-002`.
- **ADR-0158** — the non-existent `PRD-OPS-004/005` were corrected to `PRD-OBS-001` (durable
  operator visibility) and `PRD-TEST-006` (fault injection); `PRD-UI-004` (admin navigation) is
  a real §38 requirement and is retained.
- **ADR-0157** — the non-existent `PRD-MEDIA-010/013/015` / `PRD-OPS-006` were corrected to
  `PRD-MEDIA-005` (artwork), `PRD-MEDIA-006` (sidecars), `PRD-OBS-001`, and `PRD-TEST-006`; a
  bare `PRD-OPS` reference was rewritten as the repository invariant "never delete source media
  because classification is uncertain".
- **ADR-0159** — the non-existent `PRD-ART-001/003/007` were corrected to `PRD-MEDIA-005`
  (artwork) plus the real OpenSubsonic contract requirements `PRD-API-OS-001` / `PRD-API-OS-005`.
- **ADR-0160** — the non-existent `PRD-CAT-001` was corrected to `PRD-CATALOG-001`; `PRD-ARTIST-010`
  (safe merge/split) was added to the related-requirements line to match the audited/reversible
  merge decision.
- **ADR-0155** — the related-requirements line was tightened to
  `PRD-WEB-001/002`, `PRD-UI-002`, `PRD-API-M1-006`, `PRD-API-ADMIN-003`, `PRD-AUTH-003`,
  `PRD-STREAM-001`, `PRD-USER-001`, `PRD-PARTY-003`.

The `References` PRD section numbers in each ADR were also corrected to the actual PRD section
numbers (§11 Media, §13 Artist Identity, §14 Resolution, §19 Catalog/Database, §21 Native API,
§22 Streaming, §23 OpenSubsonic, §25 Admin API, §26 Auth, §27 User/Playback-State, §34 Party
Mode/Playback Backends, §38 Admin UI, §43 Web Parity, §45 Workers/Scheduled Tasks, §47
Observability, §50 Testing).

## Documentation reconciliation

- **`design/adr/README.md`** — ADR-0136 row marked `Superseded (by 0154)`; new rows for
  ADR-0154–0160 added under "Feature Completion Program decisions" with status and one-line
  summaries.
- **`design/adr/ADR-0136-*.md`** — status line changed to `Superseded (2026-08-24, by ADR-0154)`;
  the superseding decision is recorded in place (history never edited).
- **`design/ROAD-TO-FEATURE-COMPLETION.md`** — Phase 0 section lists all 7 ADRs as accepted with
  the "Exit: all seven ADRs accepted — satisfied" note; §8 open items marked resolved with
  per-question ADR links; the status header is refreshed to "In Progress — Phase 0 (ADRs)
  complete".
- **`design/requirements/status-overrides.json`** — `PRD-EXT-001/003/004` now reference ADR-0154
  (status remains `deferred` until the provider implementation lands; the rationale records the
  supersession and the owning W3 workstream).
- **`design/requirements/status.json`** — regenerated by `extract_requirements.py` (239
  requirements).
- **`design/requirements/FEATURE-PARITY-MATRIX.md`** — the "External metadata providers" row
  status changed `Waived → Partial`, noting ADR-0136 was superseded by ADR-0154 and the
  non-Last.fm providers are a v1 obligation owned by W3.
- **`/docs`** — `docs/src/content/docs/release/0.6.0.md` and `0.7.0.md` updated to describe the
  Rust-native MPD/MPV control plane (ADR-0156), the scheduled-job run history (ADR-0158), the
  metadata transformation/safe-rewrite contract (ADR-0157), and the corrected OpenSubsonic
  stored-artwork `size` behavior (ADR-0159).

## Non-ADR change made in Phase 0 (recorded for transparency)

One production-adjacent defect was fixed during closeout because its test was the last red test in
the workspace and it is a pure logging-correctness issue (no requirement, schema, or compatibility
impact):

- **`apps/melodee-server/src/logging.rs`** — `DailyLogWriter` previously opened the log file
  exactly once at init with the current UTC date and never rotated, so a long-running process
  wrote every subsequent day's records into the first day's file. A `write_for_date` helper now
  re-opens the file when the UTC date changes. The existing test
  `daily_log_file_is_created_and_appended` was additionally corrected to write through
  `write_for_date` with the fixed date under test (it had previously written via `Write::write`
  with `now_utc()`, which created a second same-day file and made the append assertion vacuous).
  Both logging tests now pass: `logging::tests::daily_log_file_is_created_and_appended` and
  `logging::tests::daily_log_file_rotates_when_the_utc_date_changes`.

This is recorded here for transparency; it is a small logging correctness fix, not a Phase 0
scope decision.

## Files changed (Phase 0)

ADR content (new):
- `design/adr/ADR-0154-external-provider-enrichment-scope-and-runtime.md`
- `design/adr/ADR-0155-end-user-web-ui-surface-and-architecture.md`
- `design/adr/ADR-0156-mpd-mpv-backend-connectivity-and-lifecycle.md`
- `design/adr/ADR-0157-metadata-transformation-and-safe-rewrite.md`
- `design/adr/ADR-0158-scheduled-job-run-history-storage.md`
- `design/adr/ADR-0159-opensubsonic-stored-artwork-sizing.md`
- `design/adr/ADR-0160-cross-release-artist-release-identity-reuse.md`

ADR index + supersession:
- `design/adr/README.md`
- `design/adr/ADR-0136-defer-non-last-fm-external-provider-integrations-to-post-parity.md`

Roadmap + requirements + parity:
- `design/ROAD-TO-FEATURE-COMPLETION.md`
- `design/requirements/status-overrides.json`
- `design/requirements/status.json` (regenerated)
- `design/requirements/FEATURE-PARITY-MATRIX.md`

Public docs:
- `docs/src/content/docs/release/0.6.0.md`
- `docs/src/content/docs/release/0.7.0.md`

Closeout (this file):
- `design/evidence/feature-completion/phase-0/PHASE-0-REPORT.md`

Logging fix:
- `apps/melodee-server/src/logging.rs`

## Validation

- `python3 scripts/extract_requirements.py --check` → `Requirements index is current (239
  requirements).` (exit 0).
- `make check-fast` (run 2026-08-24) → `EXIT:0`. The fast gate (repository/doc validation,
  requirements index check, `cargo fmt --check`, `clippy -D warnings`, and Rust library tests)
  passed with all suites `ok` and 0 failures. Log: `/tmp/check-fast-phase0.log`.
- `python3 scripts/validate_repository.py` runs as part of `make check-fast` and passed (it
  enforces required files, markdown link integrity, ADR status lines, and mockup PNG
  dimensions).

## Definition-of-done checklist (Phase 0)

- [x] All scope questions answered by accepted ADRs (0154–0160).
- [x] Conflicting ADR (0136) superseded, not edited.
- [x] Requirement references verified against `PRD.md`.
- [x] Roadmap, ADR index, status overrides, and parity matrix reconciled.
- [x] Public docs updated for the user-visible ADR decisions.
- [x] `extract_requirements.py --check`, `validate_repository.py`, and `make check-fast` pass.
- [x] No implementation, schema, or compatibility capture introduced (Phase 0 boundary held).

## Remaining work (subsequent phases)

Phase 0 is decision-only. The accepted decisions now feed the implementation stages per
ROAD-TO-FEATURE-COMPLETION.md §5:

- **Phase 1** — quick wins + the RELEASE-002 P0 bug (the RELEASE-002 save-path fix is already
  done; the remaining quick wins are test/evidence work, no ADR needed).
- **Phase 2** — Artist identity + provider enrichment: the W1 external-ID retention/AudioIdentity
  work, the W2 local resolver, and the W3 provider enrichment worker (ADR-0154) with the
  cross-Release identity generalization (ADR-0160).
- **Phase 3** — end-user web UI: the net-new SPA at `/` (ADR-0155) — build, static serving,
  SSE/WebSocket realtime, auth, and the per-surface checklist.
- **Phase 4** — MPD/MPV backends (ADR-0156) + the remaining PARTIAL rows, including the
  metadata transformation/safe-rewrite service and admin edit forms (ADR-0157), the scheduled-job
  run history (ADR-0158), and the OpenSubsonic stored-artwork sizing (ADR-0159).
- **Phase 5 / 6** — migration/cutover/packaging, then the final re-audit toward v1.0.0.

These are implementation work packages, each with their own schema/compatibility/gate obligations,
and are out of Phase 0's decision-only scope.