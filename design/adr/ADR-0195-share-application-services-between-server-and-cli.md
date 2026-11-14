# ADR-0195: Share application services between server and CLI

- **Date:** 2026-11-13
- **Status:** Accepted
- **Decision owners:** User-directed review remediation program
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARCH-006, PRD-CLI-002, PRD-SCRIPT-004, PRD-MIG-003

## Context

The CLI imports the server application for podcast recovery, scripting, notifications, onboarding,
and avatar migration. This compiles protocol adapters into an administrative application and makes
shared operational behavior depend on the HTTP composition root. SPEC section 7 already defines a
shared application boundary; there are now concrete users rather than speculative empty modules.

## Decision

Create `melodee-application` for these real shared services and use it from both executables.
Move podcast worker/recovery, scripting, onboarding, and notifications into the crate. Share resource
gauges and task-local correlation propagation without moving HTTP middleware or protocol DTOs.
Keep server module reexports to avoid unnecessary adapter churn. Protocol-specific responses remain
in the server. Share bounded avatar normalization and ZIP entry decoding at the application boundary
so migration and HTTP callers cannot diverge in safety enforcement.

SQL implementations and decoded database rows remain in `melodee-persistence` (`podcast_work` and
`onboarding`); application services retain transaction boundaries and network/filesystem orchestration.
A boundary regression checks application source and manifests and the CLI dependency direction.

The application crate depends on existing domain, persistence, ingestion, media and search crates;
none depends back on the application. Existing external dependencies retain their central exact pins.
The script-worker binary is the bounded process boundary specified by ADR-0193.

## Alternatives considered

### Extract only podcast recovery

Leaves four other CLI-to-server dependencies intact, including executable scripting behavior.
Rejected because it does not remove the architectural coupling reported by the review.

### Copy helpers into the CLI

Duplicates safety logic and permits drift. Rejected.

### Split every protocol adapter now

Not necessary to remove the demonstrated application dependency. Existing server adapters retain
separate DTOs and modules; further crate splitting should have a concrete boundary benefit.

## Consequences

The CLI no longer depends on `melodee-server` or its Axum routing. Shared services have one tested
implementation. No wire route, public identifier, database schema, or migration disposition changes
because of the move. Build and packaging commands include the script worker. Normal application
startup and shutdown contracts remain intact; a generic worker trait is not introduced solely to
remove a handful of heterogeneous shutdown wrappers.

## Validation

Run application library tests, server API tests that consume reexports, CLI migration and recovery
tests, workspace Clippy, and dependency-boundary inspection. Verify avatar and theme positive cases
and script resource failures against the shared implementation.

## References

- [SPEC.md](../../SPEC.md)
- [PRD.md](../../PRD.md)
- [ADR-0193](ADR-0193-enforce-terminable-script-execution-resource-limits.md)
