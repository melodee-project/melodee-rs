# Phase 1C evidence — media inspection and versioned Release manifests

## Scope

Phase 1C adds the read-only media observation stage after Phase 1B settling:

```text
Ready -> Inspecting -> Normalizing
```

`Normalizing` is the durable handoff state only; media normalization/transcoding is deliberately not
implemented here.

## Architecture

- `crates/melodee-media` is the compile-time boundary around Lofty and ffprobe. It returns Melodee-owned
  manifest types and never exposes third-party parser/probe structures.
- `crates/melodee-ingestion/src/inspect.rs` claims Ready work with the Phase 1A PostgreSQL lease, checks
  the Phase 1B settling signature before and after inspection, runs blocking media work on the Tokio
  blocking pool, and releases or fails the lease through the existing store.
- `release_work_item_manifests` stores one schema-versioned JSONB manifest per work item. Publication,
  `manifest_hash`, `Inspecting -> Normalizing`, lease release, and history event are one PostgreSQL
  transaction.
- Files are classified by observed role: audio, artwork, control marker, CUE, playlist, NFO, checksum,
  lyrics, or unknown. Sidecars and unknown files remain in the manifest inventory.

## Manifest contract

`CURRENT_MANIFEST_VERSION` is `1`. The manifest contains source/provenance, release observations, sorted
file observations, track observations, artwork observations, sidecars, structured findings, and inspection
context. Observed tag strings remain observations; no canonical Artist identity is created.

The semantic manifest hash is BLAKE3 over deterministic JSON after removing volatile ids, timestamps,
source snapshot/provenance, and tool provenance. Individual audio files also carry streaming BLAKE3 byte
hashes. These hashes prove exact bytes only; they are not acoustic fingerprints or catalog identities.

## Media strategy and safety

Lofty 0.25.0 is the native fast path for common tags and embedded pictures. ffprobe 8.1.2 is invoked
directly with `Command::new` and separate arguments, never through a shell. Its JSON and diagnostics are
bounded; processes have a timeout. Artwork is observed with size bounds and lightweight header dimensions;
no source artwork is extracted, rewritten, resized, or deleted. The source tree is never modified.

Valid tagless WAV is still structurally valid when ffprobe can identify its audio stream. Corrupt or
unsupported media produces structured findings and a durable failure; no partial manifest is published.
If the source signature changes before or during inspection, the result is discarded and the work item
returns to Settling.

## Restart and concurrency proof

- `crates/melodee-persistence/tests/manifests.rs` proves durable JSONB publication and atomic state/lease
  handoff using real PostgreSQL.
- `crates/melodee-ingestion/tests/inspection.rs` proves successful inspection, no-manifest source-change
  handling before inspection, and a source change during a deliberately delayed ffprobe call.
- The same test creates 200 stable synthetic Releases, repeatedly claims/inspects them, and asserts 200
  work items, 200 manifests, and zero duplicate manifest rows. PostgreSQL's unique `(work_item_id)`
  manifest constraint and Phase 1A lease claim provide the concurrent-worker proof.
- A worker crash before publication leaves `Inspecting` leased; Phase 1A lease recovery records the loss
  and schedules retry. Publication checks lease ownership again, so expired workers cannot publish stale
  results.

## Tooling and performance evidence

Tool versions used on the evidence host are recorded in `performance.json`. The benchmark harness is the
ignored `phase1c_performance_evidence` test and is run explicitly; correctness tests remain part of the
normal workspace suite. Measurements are observations of this development host, not product thresholds.

## Known limitations

- Final normalization/transcoding, acoustic fingerprints, identity resolution, policy, catalog import,
  and canonical artwork publication remain later phases.
- The current manifest is stored in PostgreSQL JSONB. A future accepted ADR may add a filesystem work-area
  copy for very large manifests, but no second authoritative store exists now.
- Sidecars are inventoried in Phase 1C; complete CUE/NFO/playlist/lyrics semantic parsing remains later
  work.

## Requirements and references

Traceability is recorded in `design/requirements/status-overrides.json` and the generated
`design/requirements/REQUIREMENTS-INDEX.md`. The material architecture decision is Proposed in
`design/adr/ADR-0115-media-inspection-boundary-and-manifest-byte-hashing.md`.
