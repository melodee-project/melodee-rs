# MEL-14 cutover and rollback rehearsal evidence

- C# behavioral oracle: commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (frozen C# baseline)
- Rust rehearsal candidate: `target/debug/melodee-cli` built from workspace HEAD
- Decision: [ADR-0149](../../adr/ADR-0149-phase-11-rehearsal-freeze-cutover-and-rollback-procedures.md)
- Related requirement: [PRD-MIG-007 Rollback](../../requirements/REQUIREMENTS-INDEX.md) (`in-progress`)
- Related work item: MEL-14 (urgent; blocked by MEL-13, now done)

## Rehearsal summary

The Phase 11 R4 rehearsal drill (`scripts/legacy_rehearsal.py drill`) ran the complete
migration lifecycle against an isolated, synthetic production-shaped legacy source
(4000 songs, 166 artists, 333 releases, 16 users) and a disposable migrated target.
The source is a disposable database seeded from the frozen legacy DDL fixture; the
canonical root is a temporary directory of sparse synthetic files. The drill never
touches a live database or canonical media tree.

## Drill sequence and results

| Step | Result |
|---|---|
| Target database created + migrations applied | pass |
| Source inventory (52 tables, read-only confirmed) | pass |
| Plan (run `01a14cdc-f681-74d3-b738-4c79bcf9d3aa`) | pass |
| Import (bounded batches, 31s) | pass |
| Verify (0 mismatch gaps, 0 duplicate public IDs) | pass |
| Media validation (read-only; 16 adopted canonical files) | pass |
| Search rebuild from migrated state (activated) | pass |
| Search validation (all six probe classes pass) | pass |
| Backup + isolated restore (restore matches target) | pass |
| Freeze + final delta (810 ms window, cutover-ready) | pass |
| Rollback audit recorded (durable, topology-bound) | pass |
| Source counts unchanged after rehearsal | pass |
| Canonical media tree byte-identical before/after | pass |
| Failed Rust target retained for forensics | pass |
| **Overall result** | **pass** |

## Acceptance criteria

- **Approval and exact topology validation are recorded.** The drill records the run ID,
  source/target database URLs, library path map, canonical root, and operator identity in the
  durable evidence JSON (`design/evidence/feature-parity/MEL-14-CUTOVER-ROLLBACK-DRILL-EVIDENCE.json`).
  The rollback runbook is topology-bound (exact rust/legacy/traffic targets) and generation is
  read-only.
- **Timed freeze/final delta, Rust smoke, and rollback command/audit succeed.** Freeze window
  was 810 ms; verification was clean; the run reached `cutover-ready` with operator
  acknowledgement; the rollback runbook generated without state change; the rollback audit
  record was written durably to the run's `notes` and a matching `legacy_migration.issues`
  row.
- **C# writes are restored; failed Rust target is retained for diagnosis.** The source
  database is opened strictly read-only; source row counts are identical before and after the
  rehearsal; the failed target database is preserved (`failed_target_preserved: true`) and
  named as the forensic target.
- **No secret is logged and rollback preserves authority boundaries.** The drill redacts
  connection details and never logs secrets; the baseline configuration fingerprint is
  keys-only (redacted); the source is never mutated (`source_mutated_by_harness: false`);
  the canonical media tree is byte-identical before and after (`canonical_tree_unchanged: true`).

## Evidence required

- **Signed operator rehearsal record:** the durable evidence JSON above records every step,
  wall-clock timing, peak RSS, counts, and the result (`pass`). It is the rehearsal record.
- **Durable rollback audit identifier:** the run ID `01a14cdc-f681-74d3-b738-4c79bcf9d3aa`
  is the durable audit identifier; the rollback audit record is stored in the run's `notes`
  (`rollback` key) and as a `rollback.executed` issue row in `legacy_migration.issues`.

## Out of scope

Actual production cutover is not executed under this item. Production backup/restore,
traffic rollback, and restored legacy-authority verification remain external, operator-approved
gates (per the runbook in `design/runbooks/CUTOVER-AND-ROLLBACK.md`). The synthetic drill
proves the procedure and tooling; it does not prove production-sized migration or live
client acceptance.

## Commands used

```bash
# Build the CLI (already built; the drill builds if missing).
cargo build -p melodee-cli

# Run the rehearsal drill against a synthetic source.
MELODEE_TEST_DATABASE_URL="postgres://melodee:melodee-dev@127.0.0.1:5433/postgres" \
  python3 scripts/legacy_rehearsal.py drill --songs 4000

# Evidence is written to target/verification/migration/rehearsal-drill-4000.json
# and copied here for durable retention.
```

## Rust tests covering this work

- `apps/melodee-cli/tests/legacy_freeze.rs` — baseline capture with redacted fingerprint;
  freeze gating on state and acknowledgement; abort on source writes during the freeze
  window; rollback runbook generation and durable rollback audit recording.
- `apps/melodee-cli/tests/legacy.rs` — source read-only, canonical media preservation,
  repeatable migration, read-only media validation, confirmation refusal, idempotent repair.
- `apps/melodee-cli/tests/legacy_faults.rs` — fencing, restart convergence, fault safety.
