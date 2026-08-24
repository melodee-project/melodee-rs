# ADR-0149: Phase 11 rehearsal, freeze, cutover, and rollback procedures

- **Date:** 2026-08-17
- **Status:** Accepted
- **Amends:** ADR-0148 §D.5 (`runs.notes` is `jsonb`, not `text`) and §D.7 (the command group
  gains `baseline` and `freeze`; the original six commands are unchanged)
- **Decision owners:** Release owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MIG-001, PRD-MIG-002, PRD-MIG-003, PRD-MIG-004, PRD-MIG-005,
  PRD-MIG-006, PRD-MIG-007

## Context

ADR-0148 fixed the Phase 11 migration contract and the six-command `melodee-cli legacy`
surface (`inventory`, `plan`, `import`, `verify`, `delta`, `status`). R4
(`IMPLMENTATION_OUTSTANDING.md` §9) requires rehearsed production replacement: an immutable
rehearsal baseline, a full production-sized rehearsal, search rebuild from migrated state,
dual-run differential traffic, real-client acceptance, production integration validation, a
final delta under write-freeze, and a rehearsed cutover/rollback.

Two capabilities need durable authority beyond ADR-0148:

1. **Baseline capture.** R4.1 requires an immutable record of source schema version, snapshot
   marker, sizes, counts, and configuration fingerprint (secrets redacted) that later deltas
   and verification compare against.
2. **Freeze and cutover acknowledgement.** ADR-0148's state machine includes `cutover-ready`,
   reachable only after a final delta under an enforced source write-freeze with operator
   acknowledgement recorded durably. The six accepted commands cannot record either fact.

Without this ADR those facts would live in runbook prose or ad-hoc SQL, which is not
auditable or fenced.

## Decision

### D.1 Additional commands

The `melodee-cli legacy` group gains exactly two commands:

- `legacy baseline` — read-only against the source; writes one durable baseline record into
  the target `legacy_migration.runs.notes` of the named run (or prints without a run):
  schema fingerprint, source identity, snapshot marker (the source's latest
  `pg_stat_xact` xid horizon or operator-supplied dump checksum), per-domain row counts, media
  file/byte totals from stored metadata, and a redacted configuration fingerprint (setting
  keys and value hashes, never values marked secret). Output is stable JSON with `--json`.
- `legacy freeze --run <uuid>` — fenced by the run lease; requires `delta-ready`. It:
  1. verifies the source snapshot marker is still current (no writes after the operator's
     enforced freeze point are importable),
  2. runs one final convergent `delta`,
  3. re-runs the verification totals,
  4. records the freeze marker and wall-clock duration in the run `notes`, and
  5. with `--acknowledge-cutover --note "<operator statement>"`, transitions the run to
     `cutover-ready`; without the flag the run remains `delta-ready` and the report is the
     rehearsal evidence.

No other command transitions a run to `cutover-ready`. `complete` is recorded at cutover by
the operator runbook step (SQL `UPDATE` shown in the runbook), keeping the CLI surface
unchanged for that final fact.

### D.2 Freeze enforcement boundary

The CLI cannot enforce a write-freeze inside the legacy system. Freeze is enforced by the
operator procedure (stop legacy writers / revoke write roles / set the source database
read-only with `ALTER DATABASE ... SET default_transaction_read_only = on` on a writable
replica role) and **verified** by `legacy freeze`: the command proves the source marker is
unchanged across the final delta window and fails closed otherwise. A write observed between
the marker and the delta makes the pass non-convergent and aborts the transition.

### D.3 Rollback

Rollback restores traffic and write authority to the legacy system (proxy/DNS/container switch
back). It never destructively reverses partially imported target rows (ADR-0148 §D.9); the
abandoned Rust target is preserved for forensics. The rollback runbook records decision
owners, objective triggers, the maximum decision window, exact commands, and post-rollback
verification.

### D.4 Rehearsal environment rule

Rehearsal tooling never points at the live production database or canonical media. The source
is always an isolated snapshot or replica; media validation uses a mounted copy or the
production root mounted read-only at the rehearsal host.

## Alternatives considered

### Operator SQL/runbook steps instead of commands

Rejected: unfenced SQL breaks the lease/audit model, is unrepeatable across rehearsals, and
puts the freeze proof in prose instead of durable records.

### A separate rehearsal binary

Rejected: the rehearsal acts on the same run model; a second binary would duplicate fencing
and redaction logic for no boundary benefit.

## Rationale

Two fenced commands keep every cutover-relevant fact durable, auditable, and replayable while
leaving ADR-0148's six-command import surface untouched. Verifying — rather than claiming —
freeze enforcement keeps the trust boundary honest: the tool proves stillness, the operator
proves procedure.

## Consequences

### Positive

- Freeze/cutover readiness is durable, fenced, and auditable.
- Rehearsals are repeatable and comparable (stable JSON baseline + reports).
- Rollback remains non-destructive by construction.

### Negative / trade-offs

- Two more commands to maintain.
- Freeze enforcement still depends on the operator procedure; the tool detects but cannot
  prevent a legacy write.

### Follow-up work

- R4.1–R4.8 implement and rehearse these procedures; evidence lands in `design/evidence/r4/`.
- R6 documents the final cutover/rollback runbooks as the supported procedure.

## Validation

- Integration tests: baseline capture shape, freeze convergence, freeze abort on source
  writes, acknowledgement gating, state-machine fencing.
- Rehearsal drill evidence: full sequence (baseline → import → verify → rebuild → freeze →
  delta → cutover-ready → backup/restore → rollback) on a production-shaped synthetic source.

## References

- [PRD.md](../../PRD.md) §49 (migration requirements)
- [ADR-0148: Legacy migration contract](ADR-0148-legacy-migration-contract.md)
- [ADR-0149 decision package](../evidence/r4/adr-0149-decision-package.md)
- [IMPLEMENTATION.md](../../IMPLEMENTATION.md) §14 (Phase 11) — the release-closure ledger
  (`IMPLMENTATION_OUTSTANDING.md` §9, R4) was removed in the `a42e733` restructure; it remains
  available in git history at `ce3f0cd`. R4 evidence: [design/runbooks/CUTOVER-AND-ROLLBACK.md](../runbooks/CUTOVER-AND-ROLLBACK.md)
