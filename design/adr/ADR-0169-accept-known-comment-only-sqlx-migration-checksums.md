# ADR-0169: Accept known comment-only SQLx migration checksums

- **Date:** 2026-08-26
- **Status:** Accepted
- **Decision owners:** Database and release maintenance
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARCH-003, PRD-DEPLOY-005

## Context

SQLx records a SHA-384 checksum for every applied migration and refuses startup when the embedded
migration bytes differ. That fail-closed behavior protects authoritative PostgreSQL state and must
remain the default.

Migrations 24 through 29 were nevertheless edited in place by commits `594053d6`, `97cc5485`, and
`26cb87c6` after their original additions. Git-history review confirms every edit to those six
files changed comments only; their executable SQL remained identical. Databases created at those
revisions therefore have the same schema and seed behavior but carry different checksums. A current
server fails before binding its listener at the first mismatch, then exposes the next mismatch only
after the previous one is addressed.

Deleting or resetting an affected PostgreSQL volume would discard authoritative state. Rewriting
`_sqlx_migrations` would conceal the actual applied artifact. Ignoring migration checksums globally
would discard SQLx's tamper protection.

## Decision

Freeze the current bytes and canonical SHA-384 checksums of migrations 24 through 29. Before
running SQLx migrations, Melodee may substitute a database's stored checksum into a one-run
in-memory migrator only when the version/checksum pair exactly matches a comment-only artifact
proven by Git history. The complete full-length allowlist and canonical checksums live beside the
migrator and in its regression fixture; the recognized lineages are the original migration files,
the `594053d6` comment refactor, and the `97cc5485` comment refactor. Migration 29 has no distinct
`97cc5485` checksum because that commit did not edit it.

The compatibility path does not update `_sqlx_migrations`, execute any applied migration again, or
weaken validation for other versions. An unknown checksum continues to return SQLx
`VersionMismatch(version)`. A compile-time checksum for migrations 24–29 different from its frozen
canonical checksum fails startup before compatibility is considered, reinforcing the rule that
future schema or comment changes require a new numbered migration.

## Alternatives considered

### Reset affected databases

Rejected. A reset is acceptable only for explicitly disposable local data and is never a safe
upgrade procedure for an authoritative database.

### Rewrite the stored checksum

Rejected. Updating `_sqlx_migrations` would erase which artifact actually ran and would turn an
application startup into an undocumented metadata repair.

### Disable or broadly normalize checksum validation

Rejected. SQLx checksum mismatches are valuable tamper and migration-drift evidence. Compatibility
is limited to the exact checksums whose Git history proves executable-SQL equivalence.

## Rationale

An exact allowlist preserves existing PostgreSQL data and truthful migration metadata while keeping
unknown drift fail-closed. The canonical checksum guard also prevents the compatibility exception
from becoming permission to edit migrations 24 through 29 again.

## Consequences

### Positive

- Databases created from every proven repository variant start and continue through later migrations.
- No user, catalog, media, or migration-history data is rewritten.
- Unknown checksum drift remains an actionable startup failure.

### Negative / trade-offs

- The migrator carries a narrowly scoped historical compatibility exception.
- The allowlist and regression fixture must remain until every supported database lineage has
  passed through a release containing this behavior.

### Follow-up work

- Never edit migrations 24 through 29 again. Add a new numbered migration for every future change.
- Remove the exception only through a superseding ADR that defines the supported upgrade floor.

## Validation

Integration tests construct databases for every historical checksum lineage, verify a normal
migrate run succeeds without rewriting any stored checksum, and verify an unknown checksum still
returns the matching `VersionMismatch(version)`. Separate assertions freeze all six canonical
embedded checksums. The documented README startup path must succeed against the retained local
database whose migrations 24–29 use the `97cc5485` lineage.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0105: PostgreSQL is the authoritative state store](ADR-0105-postgresql-is-authoritative-state-store.md)
- [ADR-0166: Require maintenance fencing for blocking schema upgrades](ADR-0166-require-maintenance-fencing-for-blocking-schema-upgrades.md)
