# Cutover and Rollback Procedures (R6: Operational Closure)

## Overview

This runbook documents the procedures for safely transitioning from the legacy C# Melodee
server to the Rust rewrite (v1.0), and reverting if necessary. These procedures are part of
**Phase R6 — Operational Closure**.

All tooling referenced here lives in `melodee-cli`. The commands never modify legacy C# code,
infrastructure, or external service configuration — they produce audit evidence and documented
steps the operator executes manually.

Reference: [ADR-0148](../adr/ADR-0148-legacy-migration-contract.md) section D (migration lifecycle).

## Prerequisites

Before initiating any cutover:

```bash
# Verify all subsystems are healthy
melodee doctor --deep

# Ensure database migrations are current
melodee migrate

# Verify the catalog import has completed
melodee catalog status

# Confirm all legacy migration runs are clean
melodee legacy verify <RUN_ID>
```

## Phase 1: Pre-Cutover Freeze

### 1.1 Inspect Legacy Migration State

Check that the latest migration run is in a state where freeze is possible:

```bash
melodee legacy status
```

Look for a run in `importing`, `verifying`, or `delta-ready` state. The freeze command will
refuse to proceed if no import has been completed, or if another process holds the run lease.

### 1.2 Execute Freeze

The freeze sequence performs:

1. **Source write-stillness proof** — polls legacy API endpoints every second for 10 seconds,
   confirming no writes occur. If a write is detected, the freeze fails and the operator must
   identify the source of the write before retrying.
2. **Final convergent delta sync** — applies any records changed during the stillness window
   using the legacy `delta` command (idempotent: new rows imported, changed rows updated, deletions
   reported as issues but never destructive).
3. **Post-delta verification gate** — re-runs full domain parity checks. If mismatches are found,
   freeze fails with detailed gap reporting.
4. **Durable freeze evidence recording** — stores the freeze timestamp, stillness window duration,
   and delta results in `legacy_migration.runs.notes.freeze`.

```bash
melodee legacy freeze <RUN_ID>
```

This command prompts for operator acknowledgement before proceeding. It exits non-zero if any step
fails.

### 1.3 Verify Freeze Evidence

After a successful freeze:

```bash
melodee legacy status
# The run state should now be "cutover-ready"
```

## Phase 2: Cutover Execution

When ready, the operator executes these infrastructure steps (documented by the rollback runbook):

1. Stop accepting writes on the legacy system (if not already done via write-stillness freeze)
2. Switch reverse proxy or load balancer routing to v1.0 Rust server
3. Verify client applications can authenticate and browse the catalog
4. Monitor health endpoints for anomalies

```bash
# During cutover, verify Rust server health
melodee doctor --deep
curl http://localhost:8080/healthz  # liveness
curl http://localhost:8080/readyz   # readiness (PostgreSQL + search)
curl http://localhost:8080/api/admin/v1/system/health  # full diagnostics
```

## Phase 3: Rollback Procedure

If critical issues are discovered during v1.0 operation, follow these steps in order:

### 3.1 Inspect Current State

```bash
melodee rollback status
```

This command reads the latest migration run state and reports whether PostgreSQL is reachable,
whether migrations are current, the latest cutover-ready/complete/failed/cancelled run ID, and
whether freeze evidence exists.

### 3.2 Generate Rollback Runbook

```bash
melodee rollback runbook --confirm > rollback-runbook.json
```

This produces a machine-readable JSON document containing step-by-step instructions to revert
traffic to the legacy system, prerequisite verification checks, and audit evidence recording steps.

### 3.3 Execute Rollback Steps

1. **Verify prerequisites** — Ensure PostgreSQL is reachable and migrations are current (the runbook validates these).
2. **Stop the Melodee v1.0 server** — Send SIGTERM to melodee-server or use `docker compose stop melodee`. Verify the process has exited before proceeding. The server's graceful shutdown handler will wait for in-flight requests to complete (bounded by the configured timeout).
3. **Revert reverse proxy routing** — Update nginx or equivalent configuration to route all traffic back to the legacy C# server (typically `localhost:80`). Reload without dropping active connections.
4. **Verify legacy responsiveness** — Test native API and OpenSubsonic endpoints on the legacy system. Verify a known release loads correctly in a client application.
5. **Record audit evidence** — Update the migration run state to `rollback_executed` and save the generated runbook as forensic evidence alongside other migration artifacts.

## Design Decisions

### Why no destructive database operations during rollback?

The rollback orchestrator never attempts to roll back individual database migrations or delete
imported catalog data:

1. **Data integrity risk** — partial rollback could leave the database in an inconsistent state
   (e.g., Release records with orphaned media assets).
2. **Legacy compatibility** — the legacy C# server already owns this data; deleting it would be
   unnecessary and potentially destructive.
3. **Simplicity** — rolling back traffic routing is sufficient to restore service; the Rust-side
   data simply coexists with the legacy system until manual reconciliation occurs later.

### Why is freeze evidence required for rollback?

Freeze evidence confirms that the system has completed at least one cutover sequence (freeze,
delta sync, verification). Without it, the rollback runbook is still generated but warns the
operator that no meaningful cutover has been recorded — the rollback would simply be "never cut
over, just start the legacy server."

### How does rollback differ from legacy migration?

- **Migration** (`melodee legacy import/verify/delta`) moves data FROM legacy TO Rust.
- **Rollback** (`melodee rollback status/runbook`) only documents how to revert traffic routing;
  it does not move data back or delete imported records.

## References

- [ADR-0148: Legacy Migration Contract](../adr/ADR-0148-legacy-migration-contract.md) section D
- [Implementation Plan](../../IMPLEMENTATION.md) — Phase R6 section
- `apps/melodee-cli/src/legacy/` — freeze, delta, verify implementations
