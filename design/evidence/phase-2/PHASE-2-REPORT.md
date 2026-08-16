# Phase 2 Report — Durable Ingestion Engine

**Status:** Final — Phase 2 closure (2026-08-09)
**Date:** 2026-08-09
**Authority:** PRD.md sections 10/45/56, SPEC.md section 11, IMPLEMENTATION.md section 5,
accepted ADRs (ADR-0105/0110/0111), `design/agents/DEFINITION-OF-DONE.md`

---

## Executive Summary

Phase 2 replaced broad filesystem sweeps and in-memory progress with one durable PostgreSQL work
item per Release. Each discovered Release becomes an independent, lease-backed, retryable work
item that survives process crashes, repeated restarts, and concurrent workers with zero duplicate
ownership. A diagnostics API exposes operator-visible state, evidence, and active leases. The
200-Release acceptance scenario, failure-isolation test, and crash/restart recovery all pass.

## Architecture and database schema

The durable work model lives in `crates/melodee-domain/src/ingestion.rs` (state machine, retry
policy, failure classes, events) and `crates/melodee-persistence` (SQL mapping, transactional
claims, leases, recovery). Domain rules are framework-independent; the persistence crate maps
them to SQL and never invents state transitions.

### Schema

- `release_work_items` (SPEC 11.1): `id` (uuidv7 PK), `api_key` (operator-safe public identity,
  unique), library ids, paths, fingerprints, `state` (text + CHECK against the 18-state domain
  enum), `state_reason`, `attempt_count`, `next_attempt_at`, lease triple, error fields,
  `correlation_id`, full timestamp set. Constraints enforce: known state, `attempt_count >= 0`,
  lease consistency (owner iff both timestamps), `next_attempt_at` only for `failed`, terminal
  states iff `completed_at` set. Migrations: `0001`–`0004`.
- `release_work_item_events`: append-only history (`event_type`, `from_state`/`to_state`,
  `reason_code`, `worker_id`, `correlation_id`, `created_at`, sparse operator-safe `metadata`).
  No UPDATE/DELETE path in the repository layer.
- `release_work_item_settling_snapshots` + `release_work_item_settling_snapshot_files`: durable
  settling evidence (signature, file_count, total_bytes, ready_marker_seen, per-file metadata).
- `release_work_item_manifests`: one schema-versioned JSONB manifest per work item (Phase 3
  sub-slice 3.1; included here because the migration landed with the work engine).

### Indexes (justified by measured query plans)

- `release_work_items_claim_idx` partial on `(next_attempt_at NULLS FIRST, created_at, id)` where
  `state IN ('ready','failed')` — serves the claim selection (bitmap index scans in captured EXPLAIN).
- `release_work_items_lease_expiry_idx` partial on `lease_expires_at` — serves recovery.
- `release_work_items_state_idx` — serves operator/diagnostic filtering.

## Durable work execution (sub-slice 2.1)

### State machine (domain-decided, SPEC 11.2)

Three actor kinds with distinct legal sets (`IngestionState::ensure_transition`):
- **System:** Discovered→Settling, Settling→Settling (source changed), Settling→Ready,
  Failed→Ready (retry requeue at claim).
- **Worker:** the forward stage path (Ready→Inspecting→Normalizing→…→Indexed),
  Validating→NeedsReview, and each processing stage →Failed.
- **Operator:** NeedsReview→{Staged, Quarantined, Ignored, Deleted}, Quarantined→Ready,
  Failed→Ready.

Terminal: Indexed, Ignored, Deleted. No unrestricted `set_state`; illegal transitions fail
atomically with zero persisted change (`illegal_transition_fails_without_changing_state`,
`failed_transaction_leaves_no_partial_state`).

### Leases and recovery

- **Claim** (`WorkItemStore::claim`): one transaction; `SELECT ... FOR UPDATE SKIP LOCKED` over
  eligible rows (ready + not-scheduled-in-future, or failed + retry-eligible + attempts remaining),
  no active lease; bounded by `claim_batch_size`; deterministic order
  (`next_attempt_at NULLS FIRST, created_at, id`); establishes lease triple; failed→ready requeue
  and claim events recorded in the same transaction.
- **Lease duration** is configuration-driven (`MELODEE__WORK__LEASE_SECONDS`, default 300s).
- **Renewal** (`renew_lease`) extends a held, unexpired lease (heartbeat path, no schema change).
- **Recovery** (`recover_expired_leases`): expired leases cleared; claimed-but-never-started items
  return unpenalized; mid-stage interruptions become `failed`/`lease.lost` with retry scheduled
  (attempt consumed = crash-loop protection).
- **Worker identity:** `{hostname}-{pid}-{uuidv7}`; correctness never depends on hostname.

### Retries and failure evidence

`RetryPolicy` (domain, pure): bounded exponential `base * 2^(attempt-1)` capped at `max_delay`,
`max_attempts` (defaults 60s/1h/5; all configuration-driven). Failure classification: `Retryable`
(scheduled or exhausted), `Permanent` (failed, no reschedule), `NeedsReview` (parks). Every
failure persists `attempt_count`, `last_attempt_at`, `last_error_code`, `last_error_message`,
`next_attempt_at`, plus `failed`/`retry_scheduled` events — and survives reconnect
(`failure_evidence_survives_reconnect`). Time is caller-supplied everywhere; tests use fixed clocks.

### Idempotency

Every mutation and its history event commit in one transaction. File publication uses temp paths,
verification, fsync where applicable, and atomic rename (PRD-INGEST-006 — staged for Phase 3.3).
Operations require stable idempotency derived from work item and stage; the unique manifest
constraint makes publication idempotent.

### Source protection

Historical evidence note: ADR-0145 refines this completed Phase 2 interpretation. Inbound bytes
remain immutable while settling and acquisition occur, but a successfully verified durable staging
handoff drains the candidate directory from inbound. Directory renaming or tag rewriting for
normalization remains prohibited.

## Discovery and settling (sub-slice 2.2)

### Source identity and idempotent discovery

`source_library_id + source_path` is the canonical identity. `create_or_get` uses
`ON CONFLICT (...) DO NOTHING` against a deterministic identity string. Repeated scans for the
same candidate re-use a single durable item (no duplicates).

### Snapshot algorithm

Deterministically traverse every entry (`read_dir` sorted by filename) including nested files.
Build file entries with relative path, file type, file size, modified time (UTC nanos). Sort by
path/type/size. Signature = concatenation of row fields (no byte-content hashing). The signature
is compared against the latest snapshot for stability decisions.

### Settling algorithm

For each scanned candidate: ensure/create durable work item (Discovered if new), compute
snapshot, persist new snapshot when signature changed, transition logic (system-owned):
Discovered→Settling, Settling→Settling (reset stable time on change), Settling→Ready (unchanged
for quiet period), Ready→Settling (re-entrant on change). Items remain durable when source
disappears; the queue row is never deleted. Ready marker (`.melodee-ready`) can shorten the quiet
period (`MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS`).

### Filesystem safety

Root and candidate discovery are canonicalized and normalized. Relative path derivation does not
rely on simple string-prefix checks. Symlink targets that escape an inbound root are rejected.
Discovery targets immediate child directories of each inbound root. Source directories are read
during discovery/settling only; writes/reorganization is deferred to later phases.

### Watcher acceleration (optional/deferred)

PRD-INGEST-003 and SPEC 11.3 establish that filesystem watchers are **optional accelerators only**;
periodic reconciliation is the authoritative correctness mechanism. The implementation relies on
periodic reconciliation as authoritative truth and does **not** add a filesystem-watcher
dependency. This satisfies PRD-INGEST-003: missed watcher events cannot lose work because
reconciliation recovers them (proven by `lost_watcher_notification_is_recovered_by_scan`). A
watcher may be added later as an optimization that wakes/schedules reconciliation; it must never
let watcher events directly become authoritative durable state. **Watcher: optional/deferred.**

## Diagnostics API (sub-slice 2.3)

The Axum server exposes a Rust-native Admin diagnostics API under `/api/admin/v1` (separate from
Melodee API v1/OpenSubsonic/Jellyfin). Routes:

| Route | Purpose |
|---|---|
| `GET /api/admin/v1/system/info` | service/version/status |
| `GET /api/admin/v1/ingest/work-items/counts` | per-state queue counts |
| `GET /api/admin/v1/ingest/work-items` | bounded, filtered, paged listing (`state`, `limit`, `offset`) |
| `GET /api/admin/v1/ingest/work-items/{id}` | work-item detail by id or api key |
| `GET /api/admin/v1/ingest/work-items/{id}/events` | append-only event history |
| `GET /api/admin/v1/ingest/leases` | active (unexpired) lease evidence |

- **Pagination:** server-side, bounded (max 500 rows; clients cannot pull the whole queue).
- **Filters:** optional `state` filter; unknown values return 400.
- **Lease/worker semantics:** exposes active-lease evidence proven by PostgreSQL (owner, acquired,
  expiry, last activity). It is **not** a live-worker liveness claim — there is no heartbeat store,
  so the API proves "a row holds an unexpired lease", not "the owner process is alive right now".
- **Error behavior:** stable JSON envelope with `code`, `message`, `correlationId`; 400/404/500.
- **DTOs:** explicit structured DTOs (`WorkItemDto`, `WorkItemEventDto`, `ActiveLeaseDto`,
  `StateCountDto`), never raw SQLx rows. No secrets exposed (tested).
- **Tests:** 13 integration tests against real PostgreSQL
  (`apps/melodee-server/tests/admin_diagnostics_api.rs`).

## Phase 2 acceptance evidence

### 200-Release result

`two_hundred_releases_discover_and_settle_without_duplicates`:
- 200 Releases copied into inbound (varied file counts, some with ready markers, some with late
  files).
- Reconciled across multiple passes with late files added mid-flight.
- Reconciled again after a simulated restart (new pool).
- Result: 200 work items, 200 distinct source paths, 200 in `ready` state. Zero duplicates, zero
  lost.

### Duplicates

Zero duplicate work items for the same source identity (unique constraint + `ON CONFLICT DO
NOTHING`). Proven by `concurrent_reconcilers_do_not_create_duplicates` and the 200-Release scenario.

### Lost

Zero Releases disappear. Missing/unreadable candidates are retained (not deleted):
`missing_candidate_is_not_deleted`, `snapshot_error_for_disappeared_directory_is_survivable`.

### Failure isolation

`one_failed_release_does_not_block_others` (`crates/melodee-persistence/tests/failure_isolation.rs`):
- 200 durable Releases become independent work items (all ready).
- 3 deliberately fail permanently during `inspecting` (indices 7, 99, 150).
- 197 valid Releases progress to `normalizing`; the 3 failed items remain explicitly `failed`
  with durable evidence (`stage.failed`, operator-readable message).
- No Release disappears (200 final items).
- The global queue continues serving new work after the failures (a freshly seeded item is
  claimable and progresses).

### Crash/restart recovery

`crates/melodee-persistence/tests/recovery.rs`:
- `failure_evidence_survives_reconnect`: new pool simulates process restart; failure evidence
  persists.
- `crash_after_claim_recovers_via_expired_lease`: worker disappears post-claim → recovered
  unpenalized.
- `crash_mid_stage_marks_lease_lost_and_schedules_retry`: mid-stage crash → lease.lost + retry.
- `failed_transaction_leaves_no_partial_state`: rollback atomicity.
- The 200-Release scenario includes a simulated restart (new pool) and still reaches 200 ready.

### Global scan timestamp independence

No global scan timestamp gates discovery. Each Release has durable per-item state, attempts, and
next actions (`release_work_items` per-item fields). Discovery reuses existing work items by
stable source identity; it does not depend on a "last scanned" timestamp to decide whether a
Release exists. Proven by `idempotent` rediscovery and the 200-Release scenario.

## Requirements

Phase 2 requirements satisfied: PRD-INGEST-001 (per-Release retry), PRD-INGEST-002 (lease-based
concurrency), PRD-INGEST-003 (watchers optional — reconciliation authoritative), PRD-INGEST-004
(settling), PRD-INGEST-005 (source safety), PRD-INGEST-007 (versioned manifest — with Phase 3.1),
PRD-INGEST-008 (structural validity independent of enrichment), PRD-INGEST-009 (bounded resource
pools), PRD-INGEST-010 (operator visibility), PRD-WORK-001/003/004/005, PRD-OBS-001. See
`design/requirements/status-overrides.json`.

## Tests

| Suite | Count | Key tests |
|---|---:|---|
| `melodee-persistence/tests/claiming.rs` | 15 | exclusive claim, 1000×32 parallel queue, poll race, lease blocking/expiry, batch bounds, deterministic order |
| `melodee-persistence/tests/recovery.rs` | 8 | crash after claim, mid-stage crash, failure durability, rollback atomicity, lease renewal |
| `melodee-persistence/tests/history.rs` | 2 | full lifecycle audit trail, per-item isolation |
| `melodee-persistence/tests/transitions.rs` | 2 | operator review/reprocess, legal transitions |
| `melodee-persistence/tests/manifests.rs` | — | atomic JSONB publication, state/lease handoff |
| `melodee-persistence/tests/migrations.rs` | 3 | empty→migrated, repeatable, rejects impossible states |
| `melodee-persistence/tests/failure_isolation.rs` | 1 | 200 items, 3 fail, others progress, queue continues |
| `melodee-ingestion/tests/reconciliation.rs` | 13 | 200-Release scenario, idempotent rediscovery, settling, ready marker, missing candidate, symlinks, lost-watcher recovery |
| `melodee-ingestion/tests/inspection.rs` | — | 200-Release inspection, source-change handling |
| `melodee-server/tests/admin_diagnostics_api.rs` | 13 | empty queue, counts, listing/pagination/filter, detail, events, active leases, expired lease, failed item, 404, malformed id, no secrets |
| `melodee-domain` unit tests | 13 | state machine, retry backoff, event round-trip, active-lease semantics |

All pass against real PostgreSQL. No mocks of `WorkItemStore` semantics.

## Known limitations

- `PRD-INGEST-006` (safe staging/publishing) remains `planned` — it is Phase 3.3 work
  (atomic/resumable publication), not Phase 2. The work engine supports it; the publication
  implementation has not landed.
- Filesystem watcher is optional/deferred (see above); periodic reconciliation is authoritative.
- Snapshot comparison uses path/type/size/mtime only (no metadata parsing, no content hash) —
  sufficient for settling; content hashes arrive with media inspection (Phase 3.1, landed).
- The diagnostics API exposes lease evidence, not live-worker liveness (no heartbeat store yet).
- Admin UI is Phase 10; Phase 2 ships API/operational visibility only.

## Phase 2 is complete.

The official Phase 2 exit criteria are satisfied with reproducible evidence from a clean checkout
running `./scripts/check.sh` (exit 0) and the acceptance tests above.
