# Phase 2 Evidence — Durable PostgreSQL-backed Release Work Execution

**Work package:** Phase 2 sub-slice 2.1 — durable Release work items, state
transitions, leasing, retries, recovery, failure evidence
**Date:** 2026-08-09
**Status:** Complete
**Authority:** SPEC.md section 11, PRD-INGEST-001/002, PRD-WORK-001/004/005, PRD-OBS-001,
ADR-0105 (PostgreSQL), ADR-0110 (SQLx/explicit SQL), ADR-0111 (no broker/Redis)

## 1. What exists

| Component | Location |
|---|---|
| Domain model (state machine, retry policy, failure classes, error codes, events) | `crates/melodee-domain/src/ingestion.rs` |
| Persistence (store, config, worker identity, migrations) | `crates/melodee-persistence/` |
| Migrations (explicit SQL) | `crates/melodee-persistence/migrations/0001_*.sql`, `0002_*.sql` |
| CLI diagnostics | `apps/melodee-cli/src/main.rs` (`migrate`, `work list/show/seed-test/claim-test`) |
| Integration tests (real PostgreSQL) | `crates/melodee-persistence/tests/` |
| Claim benchmark evidence | `design/evidence/phase-2/durable-work-execution/work-claim-benchmark.json` |

## 2. Schema

`release_work_items` (SPEC 11.1 fields): `id` (uuidv7 PK), `api_key` (operator-safe public
identity, unique), library ids (plain uuid columns; library tables arrive with their phase),
paths, fingerprints, `state` (text + CHECK against the 18-state domain enum), `state_reason`,
`attempt_count >= 0`, `next_attempt_at`, lease triple, `last_error_code`/`last_error_message`,
`correlation_id`, and the full timestamp set.

Constraints (measured in `tests/migrations.rs::schema_rejects_impossible_states`):

- state must be a known domain state;
- `attempt_count >= 0`;
- lease owner set **iff** both lease timestamps set;
- `next_attempt_at` only for `failed` rows (retry scheduling is always visible);
- terminal states (`indexed`/`ignored`/`deleted`) **iff** `completed_at` set.

Indexes (justified by the claim/recovery/diagnostic queries; plan evidence in
`work-claim-benchmark.json`):

- `release_work_items_claim_idx` partial on `(next_attempt_at NULLS FIRST, created_at, id)`
  where `state IN ('ready','failed')` — serves the claim selection;
- `release_work_items_lease_expiry_idx` partial on `lease_expires_at` — serves recovery;
- `release_work_items_state_idx` — serves operator/diagnostic filtering.

`release_work_item_events`: append-only history (`id` identity, `work_item_id` FK cascade,
`event_type` + state CHECK constraints, `from_state`/`to_state`, `reason_code`, `worker_id`,
`correlation_id`, `created_at`, sparse operator-safe `metadata` jsonb). No UPDATE/DELETE path
exists in the repository layer.

## 3. State machine (domain-decided, SPEC 11.2)

Three actor kinds with distinct legal sets (`IngestionState::ensure_transition`):

- **System:** Discovered→Settling, Settling→Settling (source changed), Settling→Ready,
  Failed→Ready (retry requeue at claim).
- **Worker:** the forward stage path (Ready→Inspecting→Normalizing→Validating→Staged→
  Publishing→Published→Cataloging→Cataloged→Indexing→Indexed), Validating→NeedsReview, and
  each processing stage →Failed.
- **Operator:** NeedsReview→{Staged, Quarantined, Ignored, Deleted}, Quarantined→Ready,
  Failed→Ready.

Terminal: Indexed, Ignored, Deleted. Quarantined is parked (operator reprocess exits). Failed
is retryable until attempts exhaust. There is no unrestricted set_state anywhere; illegal
transitions fail atomically with zero persisted change (proven by
`illegal_transition_fails_without_changing_state` and
`failed_transaction_leaves_no_partial_state`).

## 4. Claiming, leases, recovery

- **Claim** (`WorkItemStore::claim`): one transaction; `SELECT ... FOR UPDATE SKIP LOCKED`
  over eligible rows (ready + not-scheduled-in-future, or failed + retry-eligible + attempts
  remaining), no active lease; bounded by `claim_batch_size`; deterministic order
  (`next_attempt_at NULLS FIRST, created_at, id`); establishes `lease_owner`/`lease_acquired_at`/
  `lease_expires_at`; failed→ready requeue and claim events recorded in the same transaction.
- **Lease duration** is configuration-driven (`MELODEE__WORK__LEASE_SECONDS`, default 300s);
  no literal durations in engine code.
- **Renewal** (`renew_lease`) extends a held, unexpired lease — implemented now, proving no
  schema change is needed for heartbeats later.
- **Recovery** (`recover_expired_leases`): expired leases are cleared; items claimed but never
  started return to the queue without penalty; items interrupted mid-stage become
  `failed`/`lease.lost` with retry scheduled (attempt consumed = crash-loop protection).
  Expired leases are also directly claimable (belt and braces).
- **Worker identity:** `{hostname}-{pid}-{uuidv7}`; correctness never depends on hostname
  (uuidv7 guarantees uniqueness), documented in `worker.rs`.

## 5. Retries and failure evidence

`RetryPolicy` (domain, pure): bounded exponential `base * 2^(attempt-1)` capped at
`max_delay`, `max_attempts` automatic attempts (defaults 60s/1h/5; all configuration-driven).
Failure classification: `Retryable` (scheduled or exhausted), `Permanent` (failed, no
reschedule), `NeedsReview` (parks; SPEC-legal from Validating). Every failure persists
`attempt_count`, `last_attempt_at`, `last_error_code`, `last_error_message`,
`next_attempt_at`, plus `failed`/`retry_scheduled` events — and survives reconnect
(`failure_evidence_survives_reconnect`). Time is caller-supplied everywhere; tests use fixed
clocks and never sleep.

## 6. Error safety

Persisted errors are operator-safe strings + machine-readable codes only. No secrets are
written to `last_error_*`, `state_reason`, event metadata, or logs: the repository never
receives credentials (the database URL lives only in the CLI pool factory and is never
printed). Secret-scan gate: `tests/python` suite + repo grep in the durable-work validation run.

## 7. Configuration

Explicit keys only (the legacy indiscriminate env absorption is not reproduced):
`MELODEE__DATABASE__URL`, `MELODEE__WORK__LEASE_SECONDS`, `MELODEE__WORK__RETRY_BASE_SECONDS`,
`MELODEE__WORK__RETRY_MAX_SECONDS`, `MELODEE__WORK__MAX_ATTEMPTS`,
`MELODEE__WORK__CLAIM_BATCH_SIZE`. Documented defaults in `config.rs`.

## 8. Observability

`tracing` events with `work_item_id`, `worker_id`, `from_state`, `to_state`, `attempt_count`,
`correlation_id`, `lease_expires_at` at: created, claim (empty polls at DEBUG), lease acquired,
renewed, released, expired-recovered, transitioned, failed, retry scheduled, completed.

## 9. Concurrency and crash proof (real PostgreSQL)

| Test | Evidence |
|---|---|
| `exclusive_claim_single_item_two_workers` | 1 item, 2 workers → exactly one claim |
| `parallel_queue_no_duplicate_ownership` | **1,000 items, 32 workers → 1,000 claims, 0 duplicates** |
| `repeated_parallel_polls_do_not_duplicate_ownership` | 64 racing polls → 1 winner |
| `active_lease_blocks_second_claim` | lease blocks until expiry |
| `expired_lease_is_claimable_by_another_worker` | recovery via claim |
| `future_next_attempt_blocks_claim_until_eligible` | no early retry; requeue event recorded |
| `exhausted_retries_are_not_claimed` | attempts exhausted → parked until operator |
| `crash_after_claim_recovers_via_expired_lease` | worker disappears post-claim → recovered unpenalized |
| `crash_mid_stage_marks_lease_lost_and_schedules_retry` | mid-stage crash → lease.lost + retry |
| `failure_evidence_survives_reconnect` | restart durability |
| `failed_transaction_leaves_no_partial_state` | rollback atomicity |
| `lease_renewal_extends_ownership` | heartbeat path |
| `claim_batch_is_bounded`, `claim_ordering_is_deterministic` | bounded, deterministic queue |
| `schema_rejects_impossible_states` | DB-level integrity |
| `migrations_run_from_empty_database`, `migrations_are_repeatable` | migration gates |

## 10. Performance evidence (not a product claim; PRD-PERF-001)

`tests/perf_claims.rs` → `work-claim-benchmark.json`: 10,000-item queue, 32 workers,
batch 4 → 10,000 claimed, **0 duplicates**, ~2,452 claims/s (debug build, localhost dev
PostgreSQL), and the claim selection uses `release_work_items_claim_idx` (bitmap index scans
in the captured EXPLAIN).

## 11. Legacy weaknesses addressed (Phase 0 media baseline constraints)

- Same-size deletion without content comparison (F2): N/A to this phase's scope; the state
  model makes per-Release failures durable and explicit instead of silent.
- Non-atomic partial mutations: every mutation + its history event commit in one transaction.
- Sweep-timestamp-only checkpoints: durable per-item state, attempts, and next actions.
- Lost per-Release failures: `last_error_*` + append-only events.
- In-process-only concurrency: all coordination is PostgreSQL row locks/leases; the CLI and
  server processes coordinate identically (PRD-WORK-004).
