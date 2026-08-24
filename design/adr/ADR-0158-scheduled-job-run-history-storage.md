# ADR-0158: Scheduled-job run history and operator visibility

- **Date:** 2026-08-24
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-WORK-003, PRD-WORK-005, PRD-OBS-001, PRD-UI-004, PRD-TEST-006

## Context

C# Melodee schedules all background work through **Quartz.NET** (`src/Melodee.Common/Jobs/`):
`JobBase` (an abstract `IJob` with a `DoCreateJobHistory` flag), a `JobHistoryListener`
(`JobHistoryListener.cs:24`) that intercepts every Quartz execution and writes a `JobHistory`
row (`Data/Models/JobHistory.cs`: `JobName`, `StartedAt`, `CompletedAt`, `DurationInMs`,
`Success`, `ErrorMessage`, `WasManualTrigger`), and a cron-expression-per-job configuration in
the Settings table (`JobRegistry`, `JobKeyRegistry`, `JobMapNameRegistry`). The Blazor admin
`/admin/jobs` page (`Pages/Admin/Jobs.razor`) renders a jobs data grid with **scheduler status,
total jobs, running, paused, history records, and 24-hour failure count**, plus per-job
pause/resume and a manual "trigger now" action.

Rust has the **worker-pool pause/resume** seam (`worker_pool_state`, migration 0023, ADR-0134 §8;
the `discovery`/`podcast`/`policy` pools are seeded and pause-aware) and the **lease-based release
work items** (`release_work_items` + `release_work_item_events`), but **no scheduled-job run
history**: there is no `job_history` table, no per-job `StartedAt/CompletedAt/Duration/Success/
ErrorMessage/WasManualTrigger` record, no 24-hour failure aggregation, and no admin surface that
lists job runs. The admin SPA (Phase 10) has a jobs *screen skeleton* but it has no durable
history to render. PRD-WORK-003/005, PRD-OBS-001 (durable, queryable observability), and the
operator-visibility requirement
therefore have an unimplemented job-history half.

## Decision

1. **History table.** A new additive migration `0035_job_run_history.sql` creates a
   `job_run_history` table:
   - `id` (PK, `uuid` v7), `job_name` (text, matches the pool/worker key), `started_at`
     (`timestamptz`), `completed_at` (`timestamptz` NULL), `duration_ms` (`double precision`
     NULL), `success` (`boolean` NULL until completion), `error_message` (`text` NULL),
     `was_manual_trigger` (`boolean` NOT NULL DEFAULT false), `triggered_by` (`text` NULL —
     the principal name for a manual trigger).
   - Indexes: `(job_name, started_at DESC)` for per-job history and `(started_at DESC)` for the
     global recent-runs and the 24-hour failure aggregation.
   - A `success`/`error_message` invariant: a failed run always has a non-NULL
     `error_message`; a completed run always has a non-NULL `completed_at` and `duration_ms`.
   - A retention policy: runs older than a configurable retention window (default 90 days) are
     purged by a scheduled housekeeping pass; the purge is itself a job run and is audited.
2. **Recording semantics.** Every scheduled worker pool (the existing `discovery`/`podcast`/
   `policy` pools and any future pool) records **one `job_run_history` row per scheduled tick or
   per manual trigger**:
   - `started_at` is set when the worker claims the tick (the first claim batch for that tick).
   - `completed_at`/`duration_ms`/`success`/`error_message` are set when the tick finishes
     (success: all claims settled without a fatal error; failure: a fatal error that aborts the
     tick). Individual per-item lease outcomes remain in `release_work_item_events`; the
     `job_run_history` row is the **tick-level** rollup, not a per-item log.
   - `was_manual_trigger` is true when the tick was requested via the admin "run now" route
     (triggered by a named principal) rather than the scheduler.
   - High-frequency pools that would flood the table (e.g., a sub-minute cleanup) set
     `do_create_run_history = false` in their pool registration, mirroring the C#
     `JobBase.DoCreateJobHistory` flag.
3. **Scheduler model.** A lightweight, in-process scheduler (tokio `interval` per pool, driven by
   a **cron or fixed-interval expression** from the layered settings service) owns tick
   generation. The existing `worker_pool_state` pause row is the authoritative gate: a paused pool
   does not generate ticks and records no run. The scheduler does **not** use an external broker
   (no Redis/Quartz/Temporal); it is a tokio task per pool that respects the pause row and a
   single-instance `pg_advisory_lock` so a multi-process deployment does not double-run.
4. **Cron / interval configuration.** Per-pool cadence (a cron expression or an interval in
   seconds) is read from the layered settings service (preserved from the C# per-job cron in
   Settings); changing it takes effect on the next tick without a restart. The schedule is
   server-level, not per-user.
5. **Operator visibility (admin).** New additive native-v1 routes (admin-gated per ADR-0139):
   - `GET /api/v1/admin/jobs` — list pools with their scheduler status (enabled/paused), current
     tick state, total runs, running count, 24-hour failure count.
   - `GET /api/v1/admin/jobs/{pool}/runs?limit=&cursor=` — paginated run history for a pool.
   - `GET /api/v1/admin/jobs/runs?limit=&cursor=&since=` — recent runs across all pools (feeds
     the 24-hour failure count and the dashboard).
   - `POST /api/v1/admin/jobs/{pool}/pause` / `POST /api/v1/admin/jobs/{pool}/resume` — set the
     `worker_pool_state` row (already exists; exposed here for the admin SPA).
   - `POST /api/v1/admin/jobs/{pool}/run` — manually trigger a tick now (records a
     `was_manual_trigger=true` row).
   The admin SPA jobs screen renders these (Phase 10 skeleton now wired).
6. **Idempotency / at-most-once tick.** A tick is identified by `(pool, scheduled_tick_at)` and
   the scheduler holds the `pg_advisory_lock` for that tick for the duration of the claim batch; a
   crash mid-tick releases the lock on the next restart and the next tick proceeds (at-most-once
   for the tick rollup; per-item leases already guarantee at-least-once item processing per
   ADR-0117).
7. **Observability.** Each tick emits a `tracing` span with the pool, tick id, duration, and
   success/failure; the `job_run_history` row is the durable record the admin surface reads.

## Alternatives considered

### Adopt Quartz / Temporal / an external job scheduler

Rejected: the AGENTS.md forbids new infrastructure services and the PRD/W0 workstream does not
require sub-second precision or distributed scheduling. An in-process tokio scheduler + an
advisory lock is sufficient and keeps the dependency surface minimal (no new runtime
dependency).

### Reuse `release_work_item_events` as the history

Rejected: that table is a **per-item lease** log for the release work pipeline, not a
**tick-level** rollup for scheduled pools; conflating the two would make the 24-hour failure
aggregation and the admin jobs grid ambiguous. A dedicated table is clearer and cheaper to
aggregate.

### No history (logs only)

Rejected: PRD-OBS-001 requires durable, queryable operator visibility into job runs and
failures; `tracing` logs are not a substitute for a queryable, aggregated, retained record.

## Rationale

The C# `JobHistory` + `JobHistoryListener` model is a thin, durable tick-rollup on top of a
scheduler; the Rust worker-pool pause/resume seam and the lease-based work items already exist, so
the gap is specifically the tick-level history table, the in-process scheduler, and the admin
routes. Adding them closes the operator-visibility requirement without introducing an external
broker and without changing the existing lease contract.

## Consequences

### Positive

- Closes the job-history half of PRD-WORK-003/005 and the admin jobs screen.
- 24-hour failure aggregation and per-pool history become queryable and retained.
- Manual triggers and pause/resume are operator-visible and audited.

### Negative / trade-offs

- A new migration and a new tokio scheduler task add surface; the advisory-lock single-instance
  gate must be tested for the multi-process case.
- Run history grows with tick frequency; the retention purge must be tuned to the operator's
  retention expectation.

### Follow-up work

- Feature Completion Phase 2 (WORKS / OPS): the migration; the tokio scheduler per pool with the
  cron/interval config; the run-history writer; the housekeeping purge; the admin routes; the
  admin SPA jobs screen wiring; fault-injection tests (crash mid-tick, pause during a tick,
  multi-process double-run prevention).

## Validation

- Scheduler test: a pool with a fixed-interval schedule generates one tick per interval and
  records one `job_run_history` row per tick; a paused pool records no ticks.
- Failure test: a tick that aborts with a fatal error records `success=false` and a non-NULL
  `error_message`; the 24-hour failure count reflects it.
- Manual trigger test: `POST /admin/jobs/{pool}/run` records a `was_manual_trigger=true` row
  with the principal in `triggered_by`.
- Retention test: runs older than the retention window are purged; the purge is audited.
- Multi-process test: two processes holding the same pool do not both run the same tick
  (advisory-lock single-instance gate).
- Admin route tests: each route returns the expected shape; non-admin principal gets 403.
- No existing route/DTO/status changes (golden diff).

## References

- [PRD.md](../../PRD.md) §45 (PRD-WORK-003/005), §47 (PRD-OBS-001), §38 (PRD-UI-004),
  §50 (PRD-TEST-006)
- [ADR-0134](ADR-0134-phase-9-parity-domain-schema-for-radio-device-profiles-onboarding-notifications-scripts-and-providers.md) §8
  (worker pool pause/resume)
- [ADR-0139](ADR-0139-admin-spa-session-strategy-and-admin-api-gating.md) (admin gating for new routes)
- `src/Melodee.Common/Jobs/` (C# Quartz jobs, `JobHistory`, `JobHistoryListener`, behavioral
  oracle)
- `crates/melodee-persistence/migrations/0023_phase9_worker_pool_state.sql`
