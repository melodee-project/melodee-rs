# ADR-0222: Use shared verified capabilities for offline media deletion

- **Date:** 2026-11-27
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** RM-11, RM-12, RM-20, custody and recovery invariants in `PERF-REMEDIATION-SPEC.md`

## Context

An administrative deletion can remove staging, canonical, media and artwork
objects before its database transaction commits. The work item, its catalog
relationships or an asset's other references can change while that filesystem
work is running. An expired database lease cannot revoke a stale worker's open
file descriptors. A recorded inode, path, previous hash, successful database
query or caller-provided boolean does not prove that filesystem work finished
or that the protected namespace still names the verified object.

Provisional database code now records bounded, resumable deletion scopes and
acknowledgements. Those records are not a filesystem proof. Its test fixtures
use synthetic metadata; they do not authorize a production deletion or prove
that legacy workers have stopped. Existing clients and source media must retain
their present behavior until the full protocol is integrated and verified.

## Proposed decision

Develop a lower-level shared filesystem-custody boundary that can be used by
both ingestion and persistence without reversing the dependency between those
crates. Keep its deletion orchestrator and transaction finalization boundary
private until the following requirements are met:

1. Before any destructive filesystem operation, establish an explicit offline
   maintenance fence. Stop and externally verify the termination of every old
   binary, worker and administrative writer, record the responsible actor,
   reason, backup reference and inspection attestations, and keep the fence
   active throughout the operation and recovery. A database timestamp or an
   expired process lease is not evidence that an old writer has terminated.
2. Bind a bounded exact target set and complete affected Release, Track, Artist,
   directory, media and artwork closure to a durable reservation. Revalidate
   memberships and shared references under the maintenance fence. Capture and
   acknowledge large closures in bounded durable pages; a partially captured
   or interrupted scope must remain recoverable and must never authorize a
   deletion. Preserve native path identity and all source-media obligations.
3. Acquire the immutable per-work-item lifecycle locks in deterministic order,
   without following links, and verify the lock root, lock inode, staging root,
   occurrence and relevant namespace. Do not unlink or replace a lifecycle lock
   while stale waiters might exist. Unsupported filesystem, mount topology,
   ownership, ACL, path encoding or missing identity must fail closed. An
   ambiguous pre-existing name is preserved and quarantined.
4. Use non-`Clone` scope and page capability types with private fields. Their
   safe constructors must actually acquire and revalidate the required locks and
   perform the descriptor-relative operation and content verification for the
   exact durable reservation, claim epoch, page and object. No public constructor
   may turn a caller-supplied success flag, metadata comparison or arbitrary
   identifier into a verified capability. A capability retains its lock scope
   until the underlying blocking work, subprocesses and cleanup really finish,
   including on cancellation.
5. A database acknowledgement must consume the matching verified page proof,
   check the original DB-clock lease and exact frozen scope, and preserve the
   held scope for the next page. Finalization must consume a complete scope and
   atomically commit authoritative catalog changes, a bounded lost-ack replay
   receipt and the durable cleanup state. Expiry or cancellation cannot silently
   discard a reservation or an acknowledged page. Any page whose outcome is
   uncertain stays reserved for explicit recovery.
6. Complete a maintenance-fenced upgrade and idempotent bounded provisioning
   path for existing work items. New items must receive the same verified lock
   identity before a runtime worker can use them. Every reader, writer, retry,
   repair, review and administrative deletion path that touches protected
   storage must participate before enabling runtime enforcement. A database
   readiness flag does not fence an old binary that ignores the protocol.

The provisional metadata APIs and disconnected filesystem helpers in this
change remain prerequisites, not an implementation of this decision. In
particular, no assertion-only public database API is sufficient authorization
for a destructive action. Activation, deployment and production maintenance are
outside this proposal.

## Alternatives considered

- **Database lease and content hash alone.** These cannot stop an old process
  holding a descriptor, or prevent a namespace replacement after verification.
- **Preflight once, then delete.** Catalog relationships and shared asset
  references may change before or during filesystem work; a preflight result is
  not a durable reservation.
- **A public opaque type created from caller assertions.** Private fields do not
  establish safety if a public constructor accepts a success boolean, unchecked
  path or fabricated identity. The constructor must perform the verified work.
- **Always keep the old path disabled.** This avoids deletion but does not meet
  the operational recovery and compatibility requirements. Fail closed while
  the proposed protocol is incomplete.

## Consequences

The protocol makes ownership, backpressure, crash recovery and shared-asset
checks explicit. It adds a security-sensitive shared crate boundary, durable
metadata, a maintenance procedure, and platform-specific capability work. Large
catalogs require measured indexed plans and page bounds rather than one large
in-memory closure or database transaction. An externally enforced maintenance
window is a real operational prerequisite; the database cannot attest it.

## Validation

Before accepting this decision or enabling any route, run independent review;
real-filesystem tests on supported and unsupported mount topologies; cross-process
lock, replacement, malformed path and same-size mutation tests; blocked lease,
expiry, cancellation and lost-ack tests; concurrent catalog/asset membership
and shared-reference regressions; fresh and populated upgrade and interrupted
repair rehearsals; and representative 10,000-track closure measurements. Record
actual build profile, hardware, storage, database settings and limitations.
Verify that old binaries cannot resume against the activated deployment under
the documented external maintenance procedure. Passing synthetic database
fixtures alone does not satisfy these conditions.

## References

- [Performance remediation specification](../../PERF-REMEDIATION-SPEC.md)
- [Performance standard](../../PERFORMANCE.md)
- [ADR-0166: Maintenance fencing](./ADR-0166-require-maintenance-fencing-for-blocking-schema-upgrades.md)
- [ADR-0221: Staged file content integrity](./ADR-0221-freeze-staged-custody-file-contents-with-fs-verity-where-supported.md)
- [Upgrade and repair runbook](../runbooks/PERF-REMEDIATION-UPGRADE.md)
