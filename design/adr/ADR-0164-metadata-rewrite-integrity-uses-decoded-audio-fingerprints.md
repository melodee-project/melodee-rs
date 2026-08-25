# ADR-0164: Metadata rewrite integrity uses decoded-audio fingerprints

- **Date:** 2026-08-25
- **Status:** Accepted
- **Decision owners:** Feature Completion Phase 4 (explicit user-assigned phase)
- **Supersedes:** [ADR-0157](ADR-0157-metadata-transformation-and-safe-rewrite.md), safety-invariant 4 only
- **Superseded by:** None
- **Related requirements:**

## Context

ADR-0157 correctly requires a corruption check, but its literal requirement that a whole-file
SHA-256 remain unchanged after a tag rewrite is impossible: tags are part of the file bytes and a
successful tag edit necessarily changes that hash. Treating the changed whole-file hash as
corruption would reject every successful edit. The check must distinguish tag changes from audio
payload changes while retaining the full hashes as audit facts.

## Decision

1. A metadata rewrite records the complete pre- and post-rewrite SHA-256 values as evidence. They
   are expected to differ when a tag delta is applied and are never used as the unchanged-payload
   invariant.
2. Before writing and after writing, the server computes a bounded streaming fingerprint of the
   decoded first audio stream using the configured FFmpeg executable. The decoded stream is
   emitted as signed 16-bit little-endian PCM with its channel count and sample rate included in
   the fingerprint domain. The output is hashed incrementally; decoded audio is never retained in
   memory. The post-write fingerprint must equal the pre-write fingerprint. If FFmpeg is absent,
   times out, emits an error, or produces a malformed stream, the operation fails closed and the
   original file remains authoritative.
3. A rewrite also compares the container-level technical facts (codec, channel count, sample
   rate, stream count, and duration within the existing media tolerance). Any change outside the
   declared tag fields is a failure.
4. The atomic same-directory temporary-file, replacement, backup, audit, and revert requirements
   in ADR-0157 remain unchanged. This ADR changes only the integrity predicate in that ADR's
   safety-invariant 4.

## Alternatives considered

### Alternative A — require an unchanged whole-file hash

Rejected as mathematically incompatible with a tag rewrite. It would make every valid apply fail.

### Alternative B — compare only duration and container metadata

Rejected as insufficient: a damaged audio payload can retain the same duration and container
facts. A decoded-audio fingerprint is the stronger corruption check available without storing
another full audio copy.

## Rationale

This preserves the safety intent of ADR-0157 while making the invariant technically satisfiable.
Full-file hashes remain useful forensic evidence, and the decoded fingerprint detects corruption
even when the tag writer changes framing or container offsets.

## Consequences

### Positive

- A successful tag edit has different before/after whole-file hashes by design.
- FFmpeg becomes a bounded required dependency for apply; preview remains read-only and does not
  mutate a file.

### Negative / trade-offs

- Add media-corpus and fault-injection coverage for fingerprint mismatch, timeout, missing FFmpeg,
  and container-specific tag rewrites before expanding the operation set.

### Follow-up work

- ...

## Validation

Tests must prove that a supported-container tag rewrite changes the whole-file hash, preserves the
decoded-audio fingerprint and technical facts, and restores both tag values and fingerprints on
revert. Fault tests must prove that failed fingerprint verification restores the original file.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
