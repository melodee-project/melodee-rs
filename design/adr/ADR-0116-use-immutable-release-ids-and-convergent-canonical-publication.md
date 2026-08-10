# ADR-0116: Use immutable Release IDs and convergent canonical publication

- **Date:** 2026-08-10
- **Status:** Accepted
- **Decision owners:** User-directed Phase 3.3 implementation work
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-005, PRD-INGEST-006, PRD-INGEST-007,
  PRD-INGEST-008, PRD-INGEST-009, PRD-INGEST-010, PRD-STORAGE-001,
  PRD-STORAGE-002, PRD-STORAGE-004

## Context

ADR-0106 fixes the Release-centric storage principle but deliberately leaves the exact layout and
publication protocol for implementation. Phase 3.2 now produces verified normalized assets in an
attempt-isolated work area, but the durable manifest has no catalog Release identity, canonical file
mapping, publication receipt, or crash-convergence protocol.

A `ReleaseWorkItem` is processing identity and must not silently become the catalog `Release`.
PostgreSQL and a filesystem rename cannot share one atomic transaction. The Phase 5 policy engine is
also not available yet, while SPEC section 11.2 requires an accept decision before staging and SPEC
section 11.7 forbids source mutation before a durable decision and successful publication.

## Decision

### Identity and admission

- Reserve a distinct UUIDv7 `ReleaseId` exactly once when a structurally valid work item receives a
  durable publication authorization. Retries and reprocessing reuse it. Phase 4 adopts it as the
  catalog Release's internal identity. Work-item IDs, operator API keys, manifest IDs, hashes, paths,
  and legacy public IDs remain separate identities.
- Phase 3 validation is structural/media validation only. It does not perform classification,
  novelty analysis, or destructive disposition policy.
- A structurally complete manifest receives a versioned, non-destructive system authorization named
  `phase3.structural-publication` version 1. This is the narrow `Accept` needed to stage canonical
  bytes; it is not the Phase 5 general policy engine. Missing or ambiguous required local
  presentation/ordering facts go to `NeedsReview` rather than being guessed.
- Publication always records the inbound source disposition as `retained`. Moving or deleting inbound
  media requires a later, separate durable decision. Delete additionally requires the complete
  PRD-POLICY-007 audit.

### Canonical layout v1

Canonical storage uses lowercase ASCII paths derived only from immutable IDs and byte hashes:

```text
<storage-root>/
  releases/
    <shard-0>/
      <shard-1>/
        rel_<release-uuid-simple>/
          manifest.json
          media/<blake3>.mp3
          artwork/<blake3>.jpg
```

`shard-0` and `shard-1` are the first two bytes of BLAKE3 over the Release UUID's raw 16 bytes,
encoded separately as two lowercase hexadecimal characters. Artist names, titles, source paths, and
other mutable display values never determine physical identity. The manifest maps each source and
normalized logical path to its canonical Release-relative asset path. Identical bytes within one
Release may share a physical asset; global physical deduplication is not required.

The configured storage root is physical provenance and is not embedded in semantic hashes. Database
paths are storage-root-relative and portable. Existing canonical media may later be registered as a
`legacy` location without re-encoding or unnecessary movement.

### Manifest and hashes

- Manifest schema v3 adds the stable Release ID, validation profile/result, publication authorization
  reference, source disposition, and a sorted canonical asset inventory with kind, relative path,
  byte size, and BLAKE3 hash. Schema v1/v2 remain readable and explicitly upgradeable.
- The semantic manifest hash continues to describe observed media facts. Volatile worker, attempt,
  time, absolute-root, and filesystem provenance remain excluded.
- A separate publication-plan hash covers the layout version, Release ID, input semantic manifest
  hash, authorization reference, and sorted canonical asset inventory. `manifest.json` is not
  recursively included as one of its own hashed assets.

### Publication protocol

1. Under a valid lease, persist an immutable publication plan, Release ID, target relative path,
   authorization, and plan hash before copying bytes.
2. Copy only planned regular non-symlink assets into a deterministic hidden incoming directory on
   the target filesystem. Per-file temporary names are never visible in the final tree.
3. Verify every size and streamed BLAKE3 hash, write the v3 manifest, fsync files and directories
   where supported, and verify the sealed bundle.
4. Atomically rename the complete incoming Release directory to the final no-clobber path, then
   fsync its parent. Never overwrite or remove an existing final directory.
5. An existing final directory with the identical Release ID, plan hash, and complete asset
   inventory is idempotent success. Any mismatch fails closed as a target conflict.
6. Only after final verification may PostgreSQL transition `Publishing -> Published`, clear the
   lease, and record source retention.
7. Crash recovery converges by inspecting durable intent and filesystem truth: before rename only a
   hidden partial may exist; after rename a retry adopts the matching final; after database commit
   cleanup is best-effort. A stale worker may never update authoritative database state.

## Alternatives considered

### Use the work-item ID as the Release ID

Rejected because processing identity and catalog identity have different lifecycles and are already
explicitly separate in the domain contract.

### Use Artist/title directories

Rejected by ADR-0106 and PRD-STORAGE-001. Identity corrections would cause large, risky media moves.

### Publish automatically without a durable authorization

Rejected because it contradicts SPEC section 11.2. The narrow versioned structural authorization is
explicit, auditable, non-destructive, and replaceable by Phase 5 policy.

### Move or delete inbound sources after publication

Rejected for Phase 3. A successful byte publication is not authorization for destructive source
disposition.

### Treat PostgreSQL and filesystem operations as one transaction

Rejected because no such atomic boundary exists. Durable intent plus idempotent convergence is the
recoverable protocol.

## Rationale

This decision closes the final irreversible Phase 3 boundary without inventing Artist identity or
Phase 5 policy. Stable identifiers prevent storage churn, content-hash asset names avoid unsafe
display-derived paths, and the incoming-directory protocol guarantees that final Releases are absent
or complete. Keeping sources retained makes every Phase 3 outcome recoverable.

## Consequences

### Positive

- Publication is restart-safe and cannot expose a partial final Release.
- Phase 4 receives a stable Release ID and complete manifest without rereading audio.
- Existing matching finals can be adopted after a crash; conflicting finals are never overwritten.
- Source media remains byte-for-byte unchanged.

### Negative / trade-offs

- PostgreSQL can temporarily report `Publishing` after the final directory exists; recovery must
  converge this state.
- Canonical paths are intentionally less human-readable; disposable views may provide presentation.
- Phase 3 must persist a narrow authorization record before the general Phase 5 policy schema exists.
- Stale attempt cleanup remains a separate, conservative operation after publication evidence exists.

### Dependency decision

Rust 1.97 does not expose a standard-library directory rename with no-replace semantics. Phase 3.3
therefore uses `rustix` 1.1.4 with its narrow `fs` and standard-library interop features on Unix to
call safe no-follow open and `renameat_with(..., RenameFlags::NOREPLACE)` wrappers. Windows retains the platform's non-replacing
directory rename behavior. `rustix` is FOSS (`Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`),
contains the platform boundary behind a safe API, and avoids introducing local `unsafe` code. An
existence check followed by `std::fs::rename` was rejected because it can replace an empty directory
and has a time-of-check/time-of-use race.

### Follow-up work

- Phase 4 must adopt the reserved Release ID rather than allocate another identity.
- Phase 5 may supersede the structural publication authorization with configurable policy decisions;
  it may not retroactively imply source deletion.
- Phase 11 must validate registration of existing canonical media without forced relocation.

## Validation

- Golden UUID-to-layout vectors and manifest v1/v2/v3 upgrade tests.
- PostgreSQL constraints and integration tests for unique immutable Release IDs, authorization,
  exclusive claims, lease fencing, immutable plans, and atomic state transitions.
- Filesystem tests for traversal/symlink rejection, size/hash verification, no-clobber finalization,
  exact-final adoption, conflicting-final rejection, source immutability, and bounded concurrency.
- Fault injection before/during/after copy, verification, fsync, rename, and database completion.
- A 200-Release vertical scenario reaching `Published` exactly once with no source mutation.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0106](ADR-0106-use-release-centric-canonical-storage.md)
- [ADR-0115](ADR-0115-media-inspection-boundary-and-manifest-byte-hashing.md)
- [Phase 3.2 evidence](../evidence/phase-3/media-normalization/README.md)
