# R1 — Requirement Reconciliation and Release Truthfulness

Date: 2026-08-16. Status: Complete. Depends on: R0 (`5044f81`). Scope per
`IMPLMENTATION_OUTSTANDING.md` §6: R1.1 requirement-status reconciliation, R1.2 feature-parity
matrix reconciliation, R1.3 security baseline inventory, R1.4 historical residual disposition.

Gate: `./scripts/check.sh` green end-to-end (see §6).

## 1. R1.1 — Requirement reconciliation

Every generated non-terminal requirement was reviewed against accepted evidence and either
closed with existing or newly added proof, or assigned one owner phase and one explicit closure
method. Status counts before R1: tested 182, implemented 9, in-progress 24, planned 17,
deferred 6, captured 1 (239 total). After R1: **tested 194, implemented 9, in-progress 18,
planned 10, deferred 7, captured 1**.

### 1.1 Closed in R1 (existing evidence sufficed)

| Requirement | New status | Closure evidence |
|---|---|---|
| PRD-PRINCIPLE-001 | tested | Reconciled parity matrix (§2); differential/golden corpus proves behavioral compatibility while internals diverge |
| PRD-PRINCIPLE-002 | tested | Three shipped, frozen contract surfaces; route matrices 202/202, 81/81, 60/60; real-client acceptance |
| PRD-PRINCIPLE-003 | tested | Native API v1 first-class: Phase 7 accepted, 202/202 dispositions, Android 1.8.0 + MeloAmp acceptance |
| PRD-PRINCIPLE-004 | tested | Three independent adapter trees with independent wire DTOs; per-surface matrices + 287 golden scenarios; R1 cross-surface separation re-verified |
| PRD-PRINCIPLE-007 | tested | `deny.toml` allow-list + exact-version pinning + committed `Cargo.lock`; `cargo deny check` in `check.sh`/CI; SBOM/provenance attestation in the release workflow |
| PRD-ARCH-003 | tested | PostgreSQL authoritative across all shipped state (migrations through 0028); Tantivy rebuild proven (Phase 6); note updated past the "future phases" tail |
| PRD-API-M1-007 | tested | SPEC §19 + ADR-0102 govern evolution; drift gates (capture replay, route matrices, repository validation) fail CI on contract change |
| PRD-MEDIA-005 | tested | Phase 3 extraction/normalization + Phase 10 operator overrides/upload/removal/thumbnails under ADR-0143 with evidenced frozen-endpoint precedence |
| PRD-TEST-003 | tested | Canonical fixtures + edge cases committed for all three compatibility APIs (140 Python capture tests, 287 scenarios); OpenAPI snapshot 146/176 native paths with the remainder route-matrix dispositioned |
| PRD-TEST-004 | tested | All four named client classes passed release-gate acceptance (Android emulator + committed Auto-mode probe harness, MeloAmp, Feishin v1.15.1 12/12). Recorded limitation: physical Android Auto head-unit pass is an R4/R5 acceptance task |
| PRD-STORAGE-003 | deferred | Explicit release-1.0 exclusion: PRD marks generated/symlinked views optional ("may"); no legacy surface exists; future work requires its own ADR (canonical layout is ADR-0106) |

### 1.2 Closed in R1 (missing proof added in R1)

| Requirement | New status | Proof added in R1 |
|---|---|---|
| PRD-ARCH-001 | tested | `crates/melodee-domain/tests/dependency_boundary.rs` — executable boundary guard: pinned dependency allow-list (blake3/serde/serde_json/thiserror/time/uuid), explicit forbidden web/SQL/framework list, no sibling path dependencies. ADR-0104's "enforced by dependencies and tests" is now literally true |
| PRD-OBS-003 | tested | Server unit tests for the ADR-0145 four-role storage diagnostics (`apps/melodee-server/src/onboarding.rs`: role labels, creatable work area, missing staging, unreadable source, unconfigured guidance) + admin-ui role rendering assertions (`StorageScreen.test.tsx`, `system-screens.spec.ts`) with mocks corrected to the shipped R0 detail format |

### 1.3 Remaining non-terminal requirements — owner phase and closure method

| Requirement | Status | Owner | Closure method |
|---|---|---|---|
| PRD-MEDIA-001 (practical format matrix) | captured | R2 (R2.1) | Executable per-format matrix with real/synthetic fixtures: detection, metadata, duration, streams, artwork, manifest identity, normalization input, actionable failure |
| PRD-MEDIA-006 (sidecar files) | in-progress | R2 (R2.2) | CUE splitting + mandatory sidecar behavior per PRD/SPEC and frozen captures |
| PRD-INGEST-009 (bounded resource pools) | in-progress | R2 | Bound/enforce remaining pools; prove under fault injection |
| PRD-STREAM-002 (streaming semantics) | in-progress | R2 | Transcoding-policy leg + runtime gap closure without changing captured contracts |
| PRD-OBS-001 (structured logging/correlation) | in-progress | R2 | Correlation-ID propagation across request/worker/provider boundaries with tests |
| PRD-OBS-002 (health surface) | in-progress | R2 | Health coverage for every named subsystem incl. backlog/failures/disk, closing the gap list |
| PRD-PERF-003 (perf telemetry) | in-progress | R2 | Emit the named metrics at ingestion/worker boundaries |
| PRD-PERF-004 (200-Release restart benchmark) | in-progress | R2/R4 | R0 added the functional drain/crash-matrix suite; the timed benchmark with repeated termination lands in R2, the budget assertion in R4 |
| PRD-TEST-005 (media corpus) | in-progress | R2 | Real-file/tag-pathology corpus tests (with R2.1) |
| PRD-TEST-006 (fault injection) | in-progress | R2/R4 | R0 crash matrix extended across workers; crash/retry/idempotency proofs |
| PRD-TEST-007 (invariant tests) | in-progress | R2/R4 | Property/invariant suites for state machines and parsers |
| PRD-STORAGE-004 (no re-encode/no-move migration) | in-progress | R3 | In-place adoption path in the Phase 11 migration design |
| PRD-MIG-001..005, 007 (migration contract) | planned/in-progress | R3 (design) → R4 (scale/rehearsal proof) | Migration plan, fixtures, rollback, rehearsal at scale |
| PRD-SCALE-001/002 (no whole-catalog memory; indexed/bounded queries) | in-progress | R4 | Scale-envelope proofs at synthetic multi-million scale |
| PRD-API-M1-004 (preserve public IDs incl. physical-client gate) | in-progress | R4/R5 | ID-preservation proof at migration rehearsal (R3/R4); physical Android Auto acceptance (R5 gate) |
| PRD-TEST-001 (C# oracle) | in-progress | R5 | Final oracle differential gate before release |
| PRD-TEST-002 (differential harness) | in-progress | R5 | Final differential run against the frozen baseline |
| PRD-SEC-001 (path traversal/symlink/race review) | planned | R5 | Review over the R1.3 §1 inventory incl. R2-SEC-* fixes |
| PRD-SEC-002 (no low-confidence irreversible deletion) | planned | R5 | Review over R1.3 §1.7 + Phase 5 decision contract |
| PRD-SEC-003 (upload validation) | planned | R5 | Review over R1.3 §1.3 incl. R2-SEC-3/4 |
| PRD-SEC-005 (HTTP security headers) | planned | R5 | R5-SEC-1 header policy with compatibility check on frozen surfaces |
| PRD-SEC-006 (CI dependency/advisory/license scanning) | planned | R5 | Close R1.3 §4 gaps (R5-SEC-2/3/4) or record accepted decisions |
| PRD-SEC-007 (no proprietary scanning SaaS required) | planned | R5 | Verify the shipped scanner set stays FOSS |

No requirement was closed on documentation-only evidence: the two proof-added closures shipped
executable tests; the other ten cite pre-existing executable or accepted-phase evidence.

## 2. R1.2 — Feature-parity matrix reconciliation

`design/requirements/FEATURE-PARITY-MATRIX.md` re-reconciled against accepted Phase 0–10 + R0
evidence (header updated; previous reconciliation was Phase 0–8 on 2026-08-13). Changes:

- **Corrected stale `In progress` → `Complete`** with evidence links: Charts (WP-04
  `discovery_worker.rs` scheduling), Internet radio (OS leg Phase 8 + Admin-API leg WP-06; the
  frozen native v1 has no radio routes by design), Users/roles (WP-03 breadth + Phase 10 UI),
  User avatars/profile/settings (WP-03 upload/change with `avatarLastChanged`), Scheduled
  maintenance (WP-04), CLI administrative operations (checklist 12 all-green).
- **New `Waived` status** (obligation removed by accepted ADR-level waiver): External metadata
  providers (non-Last.fm legs, ADR-0136; Last.fm leg Complete), Party Mode (ADR-0133; legacy
  web route retired by ADR-0141).
- **Metadata editing** stays `In progress` — the Phase 10 exclusion is now additionally
  recorded as an explicit release-1.0 exclusion; it is not relabeled complete.
- **Jukebox** row already carried the ADR-0133 waiver; unchanged.

Gate G now holds: no row lacks `Complete` evidence or an accepted release waiver, and no row's
status contradicts accepted evidence.

**Evidence-integrity fix:** Phase 9 checklist 01 cited a `user_admin_phase9.rs` test file that
does not exist. The cited coverage was verified to live in
`apps/melodee-server/tests/admin_access_api.rs` and
`crates/melodee-persistence/tests/user_group_admin.rs`; the citations were corrected in place
with a dated correction note at the top of the checklist. This was the only stale test citation
found in the R1 pass.

## 3. R1.3 — Security baseline inventory

Full inventory: [`security-baseline-inventory.md`](security-baseline-inventory.md). It covers
every path-consuming operation, subprocess invocation, upload, the script engine, provider
responses, archive/sidecar parsers, and filesystem removals, each mapped to containment /
symlink / race / size / timeout / cancellation / audit controls; the secrets inventory and
redaction guarantees; per-surface authentication boundaries; and the CI scanner set.

Headline findings:

- Filesystem removal is strongly guarded (direct-child + generation/tree-hash fences on
  inbound cleanup; fail-closed two-phase retirement for search generations; ownership-checked
  podcast removal; no endpoint deletes canonical media).
- Secrets: 14 registry secrets with fail-closed redaction; template-only tracing spans;
  AEAD-sealed Last.fm session keys; deliberate ADR-0130 cleartext OS secrets recorded.
- **Release-blocking inventory finding:** no HTTP security-headers policy exists on any
  surface (R5-SEC-1; frozen API surfaces need a compatibility check first).
- Five bounded-input fixes assigned to R2 (R2-SEC-1..5: Last.fm post-read cap, ffprobe `--`
  separator, playlist-image Content-Type equality, OPML explicit body limit, artwork serve
  byte-hash verification).
- CI gaps assigned to R5 (R5-SEC-2..4: secret scanning, SAST/dependency-update automation,
  docs-audit/nightly-scan decisions).

No PRD-SEC-* requirement is declared tested from the inventory alone, per the backlog; all six
are assigned to R5 with the inventory as their review map.

## 4. R1.4 — Historical residual disposition

Each remaining-risk item in the accepted phase reports is assigned an owner phase or preserved
as a documented boundary:

| Residual (source) | Disposition |
|---|---|
| Production-scale PostgreSQL snapshot contention (Phase 6) | R4 scale-envelope acceptance |
| Tantivy index disk sizing on the real catalog (Phase 6) | R4 |
| Broad low-selectivity numeric/range scans below SLO (Phase 6); artist-browse planner-bound tail (Phase 10 #5) | R4 performance envelope; documented code-level follow-ups in `phase-10/performance-report.md` §4 |
| Real podcast feed / provider behavior at production scale (Phase 7) | R4 operational observation; real-feed test leg landed Phase 9 (`native_podcasts_real_feed.rs`) |
| Live SMTP happy path (Phase 9) | R2 acceptance task (live-relay smoke) |
| Configured-state Last.fm capture gap (Phase 9) | R2 acceptance task; **external blocker possible** (needs real Last.fm application credentials; disabled-state goldens are frozen and verified) |
| Physical Android Auto head-unit (Phase 7) | R5 release-gate acceptance (PRD-TEST-004/PRD-API-M1-004 recorded limitation) |
| Production Google OIDC providers (Phase 7) | R4 operational acceptance |
| Production-scale network/filesystem behavior (Phase 7) | R4 |
| Public-share per-Track sequential DTO load (Phase 7) | R4 latency observation; bounded at 10,000 items |
| CUE splitting / mandatory sidecar behavior (Phase 3 residuals) | R2.2 work package |
| Migration-only legacy schema gaps incl. original-release-date preservation | R3 migration design |
| Lexical route-evidence validator (Phase 7) | Preserved as a documented testing boundary |
| Chromium-only visual baselines (Phase 10 #7) | Preserved as a documented testing boundary; deliberate changes require `--update-snapshots` with PR review |
| Timing-sensitive test history (`discovery_worker::scheduler_runs_periodically_and_stops_gracefully`, Phase 9) | History preserved; passes in isolation and in every full gate run since; R5 requires a deterministic suite (any recurrence must be fixed, not retried) |
| Boa mid-execution interrupt / UI-gate events inert (Phase 9, ADR-0135) | Accepted ADR trade-off + documented re-expression paths; no new work |
| Ad-hoc policy test runs not persisted (Phase 10 #3) | Phase 11+; needs a schema ADR |
| Policy hot-reload, theme token-layer scope, locale fallback (Phase 10 #2/4/6) | Documented designed behavior; no action |
| JS supply chain watch (Phase 10) | CI-gated (`pnpm audit --prod`, frozen lockfile); R5-SEC-4 extends coverage decision |

## 5. R1 exit criteria (backlog §6.6)

- ✅ Every non-terminal requirement has one owner phase and one explicit closure method (§1.3).
- ✅ Every accepted exclusion carries an ADR or accepted phase decision (PRD-STORAGE-003 →
  R1 exclusion + ADR-0106 boundary; Party/Jukebox → ADR-0133/0140/0141; non-Last.fm providers
  → ADR-0136; device-profile stream selection → ADR-0134 §5; metadata edit forms → Phase 10
  decision log + this exclusion record).
- ✅ The feature-parity matrix has no stale status contradicting accepted evidence (§2).
- ✅ Security inventory has no unassigned release-blocking control gap (§3: all gaps assigned
  to R2/R5 with IDs).
- ✅ `extract_requirements.py --check` and `validate_repository.py` pass (239 requirements).
- ✅ No implementation work falsely closed on documentation-only evidence (§1.2 shipped tests).

## 6. Tests and gate

- New: `apps/melodee-server/src/onboarding.rs` 5 unit tests (four-role diagnostics);
  `crates/melodee-domain/tests/dependency_boundary.rs` 2 tests; admin-ui
  `StorageScreen.test.tsx` role assertions + `system-screens.spec.ts` e2e update.
- `./scripts/check.sh` run at the R1 boundary: result recorded in the R1 commit message
  (933+ Rust tests incl. the 7 new, 140 Python, admin-ui gate, cargo deny).

Documentation impact:
- None for `/docs` — R1 changes no user-visible behavior; the R0 storage-diagnostics format
  was already documented in R0. Internal: this report, the security inventory, the parity
  matrix, Phase 9 checklist 01 correction note, requirement overrides/index.

Remaining risks: the R2/R4/R5 assignments in §1.3/§3/§4 carry the risk; no new unowned risk
was created in R1. The configured-state Last.fm capture gap may become an external blocker
(credentials availability) — flagged in §4.
