# ADR-0115: Media inspection boundary and manifest byte hashing

- **Date:** 2026-08-09
- **Status:** Proposed
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-007, PRD-INGEST-008, PRD-INGEST-009, PRD-MEDIA-003,
  PRD-MEDIA-004, PRD-MEDIA-005, PRD-MEDIA-006, PRD-FP-001, PRD-FP-002

## Context

Phase 3 needs to observe untrusted source media without coupling Melodee's durable contract to a
metadata library or ffprobe's JSON schema. It also needs a deterministic byte identity for files.
The existing accepted media-tool decision requires external ffprobe plus a native Rust metadata
reader, while the requirements distinguish byte identity from later audio identity.

## Decision

1. Add `melodee-media` as the compile-time boundary for media observation. It returns the domain
   `ReleaseManifest` model and does not expose Lofty or ffprobe types.
2. Use Lofty 0.25.0 under its MIT OR Apache-2.0 license for the native fast path, and invoke
   ffprobe as a shell-free child process for technical facts and validation/fallback.
3. Stream a BLAKE3 1.8.6 hash over each classified audio file. This is byte identity only and is
   never used as an acoustic fingerprint or catalog identity.
4. Persist the schema-versioned manifest as JSONB in PostgreSQL and publish it transactionally with
   the work-item hash and `Inspecting -> Normalizing` transition. One unique manifest row per work
   item is authoritative.

## Alternatives considered

### Alternative A — use ffprobe JSON as the domain contract

Rejected: it makes later stages depend on an external tool's evolving JSON shape and loses the
Melodee-specific distinction between observations, findings, and policy.

### Alternative B — hash with a legacy checksum or load whole files

Rejected: a legacy checksum is not an adequate integrity identity, and whole-file buffering creates
an avoidable memory hazard for large FLAC sources. BLAKE3 is FOSS, deterministic, and has a
streaming API.

## Rationale

The boundary preserves the modular-monolith rule, keeps source media read-only, and lets later
normalization/catalog phases consume durable facts without reopening every source file. BLAKE3's
hash is intentionally scoped to exact bytes; future acoustic identity remains a separate decision.

## Consequences

### Positive

- Native tags remain observations; no Artist identity or policy decision is created in inspection.
- Tool execution requires timeout, bounded diagnostics, and explicit FOSS/runtime documentation.

### Negative / trade-offs

- Re-evaluate the Proposed decisions before media normalization and catalog publication become
  irreversible consumers of the manifest.

### Review (2026-08-09, prior to Phase 3.2)

The Proposed decisions were reviewed against PRD/SPEC and implemented Phase 3.1 evidence:

| Decision | PRD/SPEC support | Evidence validates? | Hard to reverse? | Dependency current/FOSS? | Verdict |
|---|---|---|---|---|---|
| `melodee-media` boundary | PRD-MEDIA-004, SPEC §14 | Yes (`crates/melodee-media` unit/integration tests) | Low (compile-time boundary) | n/a (no external dep) | Recommend Accept |
| Lofty 0.25.0 native fast path | PRD-MEDIA-004, ADR-0108 | Yes (tagless WAV, classification, tag preservation tests) | Low (adapter-isolated) | MIT/Apache-2.0, active | Recommend Accept |
| ffprobe shell-free child process | PRD-MEDIA-003/004, ADR-0108 | Yes (bounded output, timeout, validation) | Low (external process) | LGPL/GPL FFmpeg, active | Recommend Accept |
| BLAKE3 1.8.6 byte hashing | PRD-FP-001/002, SPEC §14 | Yes (`byte_hash_distinguishes_same_size_content`) | Low (observation only) | MIT/Apache-2.0, active | Recommend Accept |
| JSONB manifest persistence | PRD-INGEST-007, ADR-0105 | Yes (`manifests.rs` atomic publication, semantic hash) | Medium (schema consumer) | PostgreSQL, ADR-0105 accepted | Recommend Accept, with note below |

**Recommendation:** all five decisions are directly supported by PRD/SPEC, validated by
implemented evidence, use current FOSS dependencies, and are either low-reversibility or already
governed by accepted ADRs (ADR-0105/0108). The reviewer recommends **Acceptance**.

**Unresolved human decision required:** repository governance has not explicitly authorized an
agent to flip ADR status to Accepted. The decision to accept ADR-0115 — or to defer acceptance
until Phase 3.2/3.3 lands more irreversible manifest consumers — rests with the project owner. The
JSONB manifest persistence is the only medium-reversibility decision: a future accepted ADR may
add a filesystem work-area copy for very large manifests, but no second authoritative store exists
now and the schema is versioned, so acceptance would not block that future change. Until the
owner decides, this ADR remains **Proposed** and Phase 3.2+ should treat the manifest contract as
stable-but-not-yet-ratified.

### Follow-up work

- Owner decision on ADR-0115 acceptance before Phase 3.2 normalization consumes the manifest.

## Validation

`crates/melodee-media` unit tests cover deterministic classification, tagless WAV, streaming
hashing, same-size collisions, and semantic manifest hashing. PostgreSQL integration tests cover
atomic publication and durable state handoff. `crates/melodee-ingestion/tests/inspection.rs`
covers successful inspection and source changes before/during inspection. Phase 3 evidence records
tool versions and corpus/performance results.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Phase 3 evidence](../evidence/phase-3/media-inspection-and-manifest/README.md)
