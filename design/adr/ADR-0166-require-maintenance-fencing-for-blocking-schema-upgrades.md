# ADR-0166: Require maintenance fencing for blocking schema upgrades

- **Date:** 2026-08-25
- **Status:** Accepted
- **Decision owners:** Repository maintainers (upgrade availability remediation)
- **Supersedes:** ADR-0142 operational application guidance only
- **Superseded by:** None
- **Related requirements:** PRD-SCALE-002, PRD-OBS-002, PRD-PERF-003, PRD-MIG-007

## Context

ADR-0142 incorrectly states that ordinary PostgreSQL `CREATE INDEX` takes
`ShareUpdateExclusiveLock` and allows writes to continue. A normal index build takes a lock that
permits reads but blocks writes; `CREATE INDEX CONCURRENTLY` is the online form. Migration 0029 is
already committed and tracked by SQLx, so rewriting it would invalidate applied migration history.
The server also applies pending migrations before listening, which is safe for a new database but
does not by itself fence older instances in a rolling deployment from writing.

## Decision

Migrations remain immutable. Any upgrade containing an ordinary index build or another
write-blocking operation is a maintenance-fenced upgrade:

1. take and verify the documented backup;
2. stop every Melodee server, worker, and operator process that can write the target database;
3. run the versioned `melodee-cli migrate` command once;
4. verify `_sqlx_migrations`, catalog/public-ID counts, and readiness before restoring traffic.

The public database and upgrade guides must name this fence and must not describe ordinary index
creation as online. Operators must not manually create migration-owned indexes concurrently under
the same names because that can diverge SQLx history from schema state.

Future migrations that require truly online index construction must be designed from the outset as
an explicit, resumable, out-of-transaction operator phase with exact index-definition validation;
they may not be retrofitted by editing an applied SQLx migration.

## Alternatives considered

### Rewrite migration 0029 to use concurrent index creation

Rejected. Applied SQLx migrations are immutable, and concurrent index construction cannot run in the
transactional shape used by 0029.

### Allow ad-hoc manual concurrent creation

Rejected. It leaves index validity, names, definitions, and SQLx history capable of disagreeing and
has no fail-closed recovery contract.

## Rationale

The maintenance fence is explicit, testable, compatible with committed migration history, and honest
about availability. It avoids inventing a second partially tracked schema-management system.

## Consequences

### Positive

- Upgrade write availability is no longer overstated.
- Migration history stays immutable and reproducible from an empty database.
- Operators have one supported and recoverable procedure.

### Negative / trade-offs

- Upgrades containing blocking DDL require a maintenance window.
- Large index builds must be included in the downtime and disk/time budget.

### Follow-up work

- If production requirements later demand rolling schema upgrades, design a versioned online-DDL
  phase and supersede this ADR before adding the first such migration.

## Validation

- Repository validation checks the public upgrade guidance and ADR supersession link.
- Migration tests continue to prove empty and forward application plus exact index definitions.
- Production rehearsal records migration wall time, lock waits, disk growth, and rollback evidence.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0142](ADR-0142-admin-browse-index-remediation.md)
- [PostgreSQL CREATE INDEX](https://www.postgresql.org/docs/18/sql-createindex.html)
