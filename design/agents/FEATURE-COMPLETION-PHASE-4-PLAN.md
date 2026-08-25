# Feature-Completion Phase 4 Plan — Operational parity and remaining non-migration gaps

**Status:** Complete with explicit accepted differences under the explicit Phase 4 assignment
received 2026-08-25. Closeout evidence: [PHASE-4-REPORT.md](../evidence/feature-completion/phase-4/PHASE-4-REPORT.md).

## Objective and entry decision

Phase 4 closes implementation-level parity that is safe to complete before migration and
cutover. The current source audit is authoritative over older matrix prose. The matrix contains
several stale `Complete` labels: a static playback-endpoint row is not live backend control, a
script registry is not proof that every legacy hook is wired, and worker-pool pause state is not
tick-level durable job history. Phase 4 therefore reopens only the affected rows with current
evidence; it does not rewrite historical Phase 1, Phase 2, or Phase 8–10 reports.

ADR-0161 remains binding: MeloAmp owns listener workflows and the server owns APIs, operations,
storage, streaming, and `/admin`. No `web-ui/` listener SPA is in scope.

## Reconciled ownership

Every remaining non-migration blocker has one primary owner. Dependencies are listed only as
inputs and do not transfer ownership.

| Owner | Workstream | Primary rows / residuals | Dependency order |
|---|---|---|---|
| A | Native MPD/MPV playback backends | `PLAYBACK-002` and the backend-management portion of `PRD-WORK-003` | settings and existing native auth first; API composition before smoke evidence |
| B | Safe media metadata transformations | `RELEASE-002`, `MEDIA-001` transformation/sidecar portions | clarify ADR-0157 integrity invariant; storage/tag layer; persistence; APIs; Admin UI |
| C | Durable scheduled jobs and maintenance | `JOBS-001`, `AUDIT-001`, stale now-playing cleanup | migration and store; tick wrapper; controls; UI; crash/retention evidence |
| D | Stored OpenSubsonic artwork sizing | `ARTWORK-001` stored-artwork portion and the ADR-0159 compatibility behavior | source-backed capture/decision; bounded image service; adapter tests |
| E | Verified Admin UI/operator closure | `ADMINUI-001` | re-audit reachability first; implement only server-supported material gaps |
| F | Authentication and user administration depth | `AUTH-001` local portion | server authorization matrix; profile/role/group/reset UI and tests |
| G | Scripting, CLI, and configuration closure | `SCRIPT-001`, `CLI-001`, `CONFIG-001` residuals | inventory and runtime-consumer scan; hook wiring; command and settings tests |
| H | Remaining local differential evidence | `PLAYBACK-001`, `TRANSCODE-001`, `PODCAST-001`, `DISCOVERY-001`, `DISCOVERY-002`, `SOCIAL-001`, `OPS-001`, `RELEASE-001`, plus the local provider/artwork differential residuals | production-path harnesses from A–G; no external credentials or production cutover |

`PACKAGING-001`, `MIGRATE-001`, `MIGRATE-002`, production-library rehearsal, production
cutover/rollback, physical-client acceptance, live provider credentials/quotas, and real Google
OIDC are explicitly Phase 11 or operator-authorized external legs. They are not silently moved
into Phase 4 and are not counted as local passes.

## Entry-wave reconciliation tasks (closed)

1. Recalculated the 50-row matrix from production callers, tests, and Phase 1/2 reports. Current
   implemented, accepted-difference, and operator-owned dispositions are separate from preserved
   historical counts.
2. Re-audited `ADMINUI-001` against current `/admin`, Admin API routes, C# reachability, user
   impact, and existing coverage. MeloAmp-owned listener screens remain out of scope; metadata
   transforms and job controls were the verified material gaps and are now closed.
3. Reconciled roadmap text that still said Phase 2 provider, identity, metadata, or artwork work
   was absent or unwired.
4. Reproduced and classified the Phase 2 OpenSubsonic replay's 158 unexpected differences as
   disposable-harness PostgreSQL snapshot drift. Search-disabled replay produced zero unexpected
   differences and 321 intentional differences. No contract changes were made.
5. Validated ADR-0157's whole-file SHA-256 wording and superseded it with ADR-0164. Decoded-audio
   fingerprinting is the integrity predicate; whole-file hashes remain before/after audit facts.

### Current Admin UI reachability audit

The current Carbon `/admin` is not being expanded from the historical C# page inventory alone.
The C# routes were rechecked against current Rust production routes, MeloAmp ownership, and
operator impact:

| Historical surface | C# reachability | Current Rust/API evidence | Phase 4 disposition |
|---|---|---|---|
| Libraries/source management | `/data/libraries`, `/data/libraryedit/{id}` | Layered source/storage configuration, Storage diagnostics, onboarding/config validation | Configuration is the authoritative operator surface; no mutable listener/library screen is added without a source-topology decision. Documented as an operational residual rather than silently claimed complete. |
| Playlists | `/data/playlists`, `/data/playlist/{id}` | Native playlist CRUD and MeloAmp acceptance | MeloAmp-owned listener workflow under ADR-0161; no duplicate Admin UI. |
| Podcasts/episodes | `/data/podcasts`, `/data/podcasts/{id}` | Native podcast/subscription/download/resume APIs and worker | Listener/subscription workflow remains client-owned; worker recovery and diagnostics are operator surfaces. No duplicate listener application. |
| Shares/requests | `/data/shares`, `/requests/*` | Native shares and requests stores/API | Native/MeloAmp workflow is live; narrow public share/security HTML remains the only browser addition permitted by ADR-0161. |
| Charts | `/charts/*` | Durable chart import, browse, linkage, and scheduled maintenance | Import/browse/maintenance are implemented and tested; historical editor is not a required Rust Admin screen until a current mutation consumer is identified. |
| Pins/media-artist curation | user-state pins; `/admin/mediaartists` | Native pin state and Admin identity/artwork/review surfaces | Existing curation and identity workflows cover the current Rust identity model; legacy media-artist page is not copied. |
| Metadata transforms/jobs | MediaEditService; `/admin/jobs` | Phase 4 metadata routes/panel and durable job routes/screen | Implemented in workstreams B/C with preview/confirmation/history/failure evidence. |
| Users/groups/profiles | `/data/users`, `/data/useredit`, `/admin/usergroups`, `/account/profile` | Admin access/group/profile/password-reset/session stores and Users screen | Local behavior is covered; real external Google OIDC remains operator-authorized evidence only. |

This audit preserves the C# reachability facts and names the configuration/source-topology residual;
it does not convert a listener-owned workflow into a second server application or claim an unbuilt
mutation surface is complete.

## Dependency-ordered execution waves

### Wave 0 — evidence and decision closure (complete)

Created this plan and the Phase 4 report target; captured repository baselines; completed the
50-row reconciliation, Admin UI audit, OpenSubsonic discrepancy classification, C# reachability
notes, and ADR-0164 metadata-integrity clarification.

### Wave 1 — production seams and additive persistence (complete)

Implement the backend abstraction and selected-backend configuration boundary, the media-edit
tag/storage service, the next unused job-history migration (never reuse Phase 2 migration 0035),
and bounded OpenSubsonic image transformation/cache primitives. Every schema change includes
integrity tests and forward/restore rollback notes.

### Wave 2 — server behavior and operator controls (complete)

Wire MPD/MPV lifecycle and additive status/control routes; implement preview/apply/audit/revert
metadata operations with atomic same-filesystem replacement and failure recovery; wire scheduled
tick history, pause/resume/run-now, stale cleanup, retention, and failure summaries; complete
OpenSubsonic sized stored-artwork behavior; close local auth/profile/group/reset gaps; wire active
scripting hooks and CLI/config residuals.

### Wave 3 — Admin UI and compatibility proof (complete)

Reconcile and implement only verified Admin UI gaps, keeping all visible text localized and all
destructive actions impact-confirmed. Add media, job, user, and scripting journeys. Run native-v1,
OpenSubsonic, Jellyfin, and MeloAmp compatibility suites; classify all OpenSubsonic replay diffs.

### Wave 4 — closeout (complete)

Ran media corpus/property/fault/PostgreSQL/multi-process tests, Admin UI typecheck/lint/unit/
localization/accessibility/Playwright/visual suites, docs and traceability validation, `cargo
deny check`, `make check-fast`, `make test-integration`, and `make check`. Every row is terminal
through local evidence or an explicitly recorded accepted difference/operator-owned residual.

## Required safety and compatibility rules

- Native-v1, OpenSubsonic, and Jellyfin wire DTOs remain separate.
- No source media is deleted, moved, or rewritten by migration work. Phase 4 tag edits preserve
  paths, extensions, semantic IDs, and decoded audio payloads.
- External contracts change only with current behavior evidence, a product decision, an accepted
  ADR, and a client/migration plan. Additive native/admin routes are not retroactive proof of old
  routes.
- Backend/provider/job failures are isolated from catalog and streaming availability; credentials,
  secrets, unsafe paths, and script output are not emitted in diagnostics.
- Local MPD/MPV tests use deterministic fake servers. Real daemon acceptance is reported only when
  binaries are present and never manufactured when absent.

## Phase-close evidence target

`design/evidence/feature-completion/phase-4/PHASE-4-REPORT.md` records the exact baseline, row
dispositions, ADRs, changed files, commands/results, compatibility impact, database/media safety,
security/resource bounds, external legs, remaining Phase 11 scope, and the truthful exit decision.

## Closeout disposition

The phase is closed by the report linked above. The only planned implementation milestone after
this phase is Phase 11 migration/import, packaging, rehearsal, cutover, rollback, and production
library validation. Source-topology mutation remains configuration-owned pending a future product
decision; live provider credentials, real external Google OIDC, physical clients, and production
rehearsal remain operator-authorized legs. No Phase 4 workstream is left unowned.
