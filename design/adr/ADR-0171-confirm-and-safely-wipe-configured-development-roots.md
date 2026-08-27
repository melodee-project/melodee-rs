# ADR-0171: Confirm and safely wipe configured development roots

- **Date:** 2026-08-27
- **Status:** Accepted
- **Decision owners:** User-directed fresh local setup workflow
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** None — repository-local development tooling; the safety posture follows
  SPEC-INV-008

## Context

The guided `scripts/reset_and_run_server.py` workflow recreated the local development database but
always preserved filesystem state. Catalog resets could therefore be followed by ingestion of old
inbound media, reuse of staging or normalization artifacts, stale canonical bundles and search
generations, old podcast downloads, or misleading logs. That did not provide the requested fully
fresh test environment.

Recursively removing configured storage is irreversible and the inbound root can contain the
operator's only copy of test media. A generic recursive delete, an implicit wipe, or treating path
names as sufficient authorization would be unsafe. The launcher needs a deliberate second consent
boundary and a testable target-validation contract independent of the database reset confirmation.

## Decision

The guided fresh-development launcher discovers only directory values explicitly assigned in the
repository-root `.env`. Eligible roots are logging, inbound source, durable staging, normalization
work, canonical storage, search, podcast storage, and every `root` in
`MELODEE__INGEST__SOURCES_JSON`. Inherited process-environment directory values are not wipe targets.

Before any mutation, the launcher displays the deduplicated targets and asks a separate confirmation
whose default is no. `--yes` continues to bypass only the fixed development-database confirmation;
it never bypasses the directory-wipe prompt. Declining the directory prompt preserves all directory
contents and allows the database-only fresh setup to continue.

When confirmed, the launcher preflights every target before deleting from any target. Each path must
be absolute, dedicated, non-overlapping, outside the repository, and neither a broad system/home
directory nor a path with a symbolic-link component. Existing targets must be directories and may
not contain a nested mount point. The root itself may be a mount point because it is preserved.

The implementation uses Python filesystem APIs without a shell or glob. It unlinks files and
symbolic links, recursively removes real child directories, never follows a symbolic link, preserves
each configured root, creates a missing confirmed root as an empty directory, and reports the number
of top-level entries removed. The Docker volume, repository checkout, build artifacts, and any path
not explicitly configured as an eligible `.env` root remain outside the wipe scope.

## Alternatives considered

### Always wipe configured roots during a database reset

Rejected because a database-only reset remains useful and inbound or canonical media can be costly
or impossible to reconstruct. The irreversible filesystem action requires separate explicit consent.

### Use `rm -rf`, shell globs, or delete the root directories

Rejected because shell expansion makes target proof and cross-platform behavior weaker, hidden files
are easy to mishandle, root deletion can change ownership or mount semantics, and malformed or
overlapping paths can broaden the destructive scope.

### Wipe only staging, work, and search

Rejected because the requested outcome is a system with no prior local Melodee state. Preserving
configured inbound, canonical, podcast, or log contents can immediately reintroduce or obscure prior
test-run state. The separate target list and confirmation make the broader scope visible.

## Rationale

This provides the complete fresh-start behavior without weakening the existing exact-database guard.
Explicit enumeration, full preflight, root preservation, and no-follow deletion make the target scope
legible and unit-testable. Keeping the wipe optional protects media while making a truly clean test
run a single guided workflow.

## Consequences

### Positive

- Operators can reset the database and all configured Melodee filesystem state in one workflow.
- The exact destructive scope is shown before consent and remains independent of `--yes`.
- Missing, malformed, broad, linked, mounted-below, or overlapping targets fail closed before any
  configured root is changed.
- Root directory ownership and mount identity are preserved.

### Negative / trade-offs

- Confirming the wipe permanently deletes inbound and canonical media along with disposable state;
  recovery requires an external backup.
- Conservative target validation rejects some otherwise technically usable directory layouts.
- The operation is not atomic across filesystems; an I/O failure after deletion begins can leave a
  partial wipe and must be followed by correcting the filesystem problem and rerunning setup.

### Follow-up work

- None. Production cleanup and retention behavior are intentionally unchanged.

## Validation

- Python unit tests enumerate scalar and JSON-configured roots, deduplicate exact repeats, and reject
  relative, broad, repository, home, nested, and symlink-component targets.
- Filesystem tests prove hidden files and nested directories are deleted, root directories and
  external symlink targets survive, and embedded-mount detection preflights all roots before the
  first deletion.
- Orchestration tests prove the wipe is separately confirmed and occurs before database recreation,
  while administrator credentials remain scoped to the CLI environment.
- Repository and documentation validation exercise safe `--help` behavior and the published warning.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Guided launcher](../../scripts/reset_and_run_server.py)
- [Launcher tests](../../tests/python/test_reset_and_run_server.py)
