# Phase 3.3 Work Package — Verification and Canonical Publication

## Objective

Implement `PRD-INGEST-006` according to `SPEC.md` and ADR-0116: turn a Phase 3.2 normalized
manifest/work-area result into one verified, versioned, Release-centric canonical bundle without
mutating inbound media.

## Authority

- PRD-INGEST-005 through PRD-INGEST-010
- PRD-STORAGE-001, PRD-STORAGE-002, and the Phase 3 portion of PRD-STORAGE-004
- SPEC sections 11 through 14 and 16
- ADR-0105, ADR-0106, ADR-0108, ADR-0110, ADR-0115, ADR-0116
- IMPLEMENTATION.md Phase 3.3 and the repository Definition of Done

## Assumptions fixed before coding

- Work-item and catalog Release identities remain distinct.
- A stable Release ID is reserved once with the durable Phase 3 structural-publication
  authorization and is reused by all retries and by Phase 4.
- Structural validation does not perform Phase 5 classification/novelty policy.
- Inbound source disposition is always `retained` in this work package.
- PostgreSQL/filesystem atomicity is achieved by durable intent and idempotent convergence, not a
  fictional distributed transaction.
- Canonical layout v1 and hashing are exactly those in ADR-0116.
- CUE-driven audio splitting, global byte deduplication, catalog insertion, and stale-attempt cleanup
  are outside this sub-slice unless required to prove source/publication safety.

## Implementation slices

1. Domain manifest v3 and pure validation/publication-plan invariants.
2. Migration, durable authorization/publication journal, leased claims, fencing, and transitions.
3. Target-filesystem incoming bundle, verification/fsync, no-clobber finalize, and retry adoption.
4. Server/CLI scheduling, configuration, diagnostics evidence, and graceful cancellation.
5. Phase 3.4 representative pathology/fault tests, 200-Release closure, evidence, traceability, and
   public documentation.

## Acceptance criteria

- Manifest v1/v2 remain readable; v3 contains every fact required for Phase 4 catalog insertion.
- Validation rejects missing/mismatched/untrusted work assets before final-path I/O and routes
  metadata uncertainty to `NeedsReview` without guessing identity.
- Two processes cannot validate or publish one item authoritatively at the same time.
- Every final canonical Release is absent or complete; no partial final path is observable.
- Copy, verification, fsync, rename, lease loss, and database-completion faults are retry-safe.
- Matching final bundles are adopted; mismatching finals fail closed and are never overwritten.
- Source trees remain byte-for-byte unchanged in success, failure, cancellation, and crash tests.
- `Published` rows have a stable Release ID, portable target/manifest path, publication hash,
  retained source disposition, complete event evidence, and no active lease.
- Representative MP3/FLAC/OGG/WAV and pathology cases pass through publication.
- 200 Releases reach `Published` exactly once under bounded concurrency with zero loss, duplicate
  authoritative results, partial finals, or source mutation.
- Requirements/evidence and `/docs` are updated in the same change; all repository and docs gates
  pass.

## Required report

Report files changed, requirements satisfied, ADRs applied, tests and results, compatibility impact,
data/migration impact, documentation impact, and remaining risks. Do not call Phase 3 complete while
any exit criterion above lacks reproducible evidence.
