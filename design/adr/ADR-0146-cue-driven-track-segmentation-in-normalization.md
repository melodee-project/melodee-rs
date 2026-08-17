# ADR-0146: CUE-driven track segmentation in normalization

- **Date:** 2026-08-16
- **Status:** Accepted
- **Decision owners:** Product owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MEDIA-006, PRD-INGEST-009, PRD-TEST-006

## Context

The frozen legacy capture `compatibility/media/cases/cue-split.json` proves the accepted behavior:
a Release directory containing one CUE sheet and one referenced audio file produces one album whose
songs are per-track MP3 segments split with FFmpeg (`libmp3lame`, VBR q4, 48 kHz — the seeded
conversion profile), named `NNNN Title.mp3`, while the CUE and its source media are retained as
`_converted` inbound leftovers. The album exists only through the sidecar path; the tagless source
WAV alone produces no album (baseline defect F1).

The Rust pipeline deliberately differs in lifecycle: inspection observes, normalization converts
into an attempt-isolated work area, structural validation verifies and plans canonical assets. Its
accepted Phase 3 structural-validation invariants assume a 1:1 relationship between observed audio
files and playable Tracks:

1. the set of classified audio files equals the set of manifest Tracks;
2. every audio source has exactly one normalized output;
3. every audio source has exactly one acoustic fingerprint.

A CUE-governed source produces N segment outputs from one file, so these invariants must become
segment-aware without weakening them. This is a manifest/validation contract amendment and is
recorded here rather than resolved silently.

The legacy plugin also exhibits behaviors this rewrite must **not** preserve:

- It rewrites the CUE in place when the `FILE` reference is missing and converts the CUE encoding
  in place (Windows-1252 → UTF-8 overwrite). ADR-0145 forbids mutating staged source.
- It retags the split MP3s in place after encoding.
- It drops the `FF` frame component of `MM:SS:FF` index timestamps when seeking (75 fps precision
  lost). The frozen capture's tracks sit on exact minute boundaries, so the capture cannot
  distinguish; full frame precision is implemented instead.
- It flushes the final song without `SortOrder`/`Size` (captured `sortOrder: 0, size: 0` quirk)
  and stamps every song with the CUE file's CRC32 and byte size.

## Decision

1. **Governing CUE.** A CUE sidecar governs a source audio file when it parses with exactly one
   `FILE` reference, at least one `TRACK`, and the reference resolves through the owned
   staged-source resolver to a classified audio file inside the staged Release tree. A CUE with
   more than one `FILE` line is rejected with an actionable finding, matching the legacy plugin's
   "multiple FILE not supported" behavior; the referenced audio then normalizes whole-file as
   usual. A CUE whose `FILE` reference is missing, escapes the staged tree, or is not classified
   audio records `sidecar.cue.missing_source`/`sidecar.cue.unsafe_file` findings and governs
   nothing.
2. **Segment boundaries.** Each track starts at its **first** `INDEX` entry regardless of index
   number (legacy parity: an `INDEX 00` pregap start is the split point when present). Each track
   ends at the next track's start; the last track runs to end of stream. Timestamps are interpreted
   at full `MM:SS:FF` precision (frames = 1/75 s). Boundaries must be strictly increasing and the
   first boundary must lie within the probed source duration; otherwise the CUE governs nothing and
   `sidecar.cue.invalid_boundaries` is recorded. `PREGAP`/`POSTGAP` commands are parsed into the
   observation but never applied to boundaries (legacy parity).
3. **Segment generation.** Normalization generates one segment output per CUE track with direct
   FFmpeg argv (`-ss <start> [-t <duration>]`, canonical MP3 profile), writing only into the
   attempt-isolated work area under the existing `audio/` category. The staged original is never
   rewritten, retagged, split in place, or deleted. Every segment is re-probed and passes the same
   output verification as any normalized output (one MP3 audio stream, canonical sample rate,
   duration within tolerance of the declared boundary span for bounded segments).
4. **Deterministic segment identity.** A segment's logical identity is
   `<cue-relative-path>#track-<NN>` (track number from the CUE, zero-padded to two digits). The
   work-area output path is `audio/<cue-dir>/<cue-file-name>.track-NN.mp3` — derived only from the
   CUE's relative path and track number, never from tag content, so retries converge byte-for-byte
   within an attempt and validation can re-resolve them.
5. **Manifest and validation contract (profile bump).** `NormalizedOutput` gains an optional
   `segment` identity (`cue` relative path + track number). For a governed source:
   - normalization emits the segment outputs and **no** whole-file output for that source;
   - `manifest.tracks` entries for the source are replaced by one Track per segment, keyed by the
     segment pseudo-path `<source-relative-path>#cue-track-NN`, carrying the CUE track number,
     title, performer, and observed segment duration;
   - the source file keeps exactly one whole-file acoustic fingerprint (fingerprinting segments
     is not required; the source is the fingerprinted evidence);
   - structural validation (`STRUCTURAL_VALIDATION_PROFILE_VERSION` 2) replaces the
     one-output-per-source rule for governed sources with: every segment pseudo-path has exactly
     one output, every governed source has at least one segment output, and ungoverned sources keep
     the existing 1:1 invariants. Ungoverned releases validate exactly as before under profile v1
     rules; profile v2 accepts both.
6. **Parser diagnostics.** CUE parsing stays shell-free and pure-Rust. Malformed `FILE`/`TRACK`/
   `INDEX` lines, empty sheets, multi-`FILE` sheets, unsafe references, missing sources, and
   invalid boundary ordering each produce structured findings; none abort the Release's remaining
   normalization. UTF-8 (with BOM strip) and Windows-1252 fallback decoding are unchanged.
7. **Sidecar inventory.** The accepted sidecar vocabulary remains CUE, M3U/M3U8, NFO, SFV/MD5/
   SHA1/SHA256, LRC — each with a parse/fixture — plus `.melodee-ready` control. Arbitrary
   unrecognized files remain acquired source evidence classified `Unknown`; no general sidecar
   plugin system is introduced.

## Alternatives considered

### Whole-file output plus supplementary segments

Keep the 1:1 validation invariants by emitting the whole-file MP3 as the Track asset and writing
segments as unconsumed extras. Rejected: the release would publish a spurious whole-file Track the
accepted behavior does not produce, and generated segments would be dead artifacts — the capture
shows the album exists only through the sidecar path.

### Split at inspection time

Inspectors would parse CUEs and observe segment Tracks directly. Rejected: inspection is
observation-only with no work area; segment bytes cannot exist before normalization, and making
inspection CUE-aware would couple the observation contract to conversion policy.

### New sidecar plugin abstraction

A general plugin registry for sidecar behaviors. Rejected by the work package (no general plugin
system); CUE segmentation is the single accepted behavior and is implemented directly.

## Rationale

This preserves the accepted external behavior (one album, per-track MP3s, ordering and
title/performer observations from the CUE) while upholding the rewrite's lifecycle invariants:
staged-source immutability (ADR-0145), attempt isolation, deterministic retry identity, and
structural validation that fails closed. Segment-aware invariants weaken nothing for the
overwhelming majority of releases without CUEs.

## Consequences

### Positive

- CUE releases publish per-track canonical media with verified segment outputs.
- Validation remains total: every published asset is hash-verified against the manifest.
- Legacy defects (in-place rewrite/retag, frame dropping, final-song quirk, CUE-CRC song hashes)
  are documented as not preserved.

### Negative / trade-offs

- Structural-validation profile versioning gains a second rule set; both must be maintained until
  pre-CUE manifests age out (manifests are per-attempt and transient, so this is short-lived).
- Segment duration verification uses the probed source duration; a source whose probed duration
  exceeds its decodable length fails at encode/verify time (acceptable: actionable typed error).

### Follow-up work

- Operator documentation for CUE handling (done in the same change under the docs same-change
  rule).
- Catalog/import consumes segment Tracks through the existing manifest-v3 publication path; no
  wire contract changes.

## Validation

- Unit: parser fixtures for quoted/spaces/Unicode FILE references, Windows-1252 fallback, pregaps
  (parsed, unapplied), malformed timestamps, multiple FILE rejection, empty sheet, out-of-order
  boundaries; boundary arithmetic at frame precision.
- Integration (real FFmpeg): the frozen capture shape (3-track CUE + one WAV) produces three
  verified segment MP3s with deterministic identity, no whole-file output, source tree unchanged,
  two attempts byte-identical; missing-source, traversal, and out-of-range CUEs govern nothing and
  record findings; validation passes the governed manifest and publishes segment assets.
- Compatibility: `tests/python/test_media_baseline.py` frozen-capture assertions remain green.

## References

- [PRD.md](../../PRD.md) (PRD-MEDIA-006)
- [SPEC.md](../../SPEC.md) (§14 media rules)
- [ADR-0145](ADR-0145-drain-inbound-into-durable-staging-before-ingestion.md)
- Frozen capture: `compatibility/media/cases/cue-split.json`
- Work package: `IMPLMENTATION_OUTSTANDING.md` §7.3 (R2.2)
