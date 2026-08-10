# Phase 3 Evidence — Media normalization, fingerprinting, and canonical storage

## Scope

Phase 3 sub-slice 3.2 adds the media normalization stage between inspection and
validation:

```text
Normalizing -> Validating
```

During `Normalizing`, each Release's source media is processed in an isolated
work-area staging directory:

- **Audio normalization** (PRD-MEDIA-002): canonical MP3 via FFmpeg `libmp3lame`
  VBR quality 4, max 384 kbps, 48 kHz sample rate, channels preserved. Files
  already in canonical form are retained without re-encoding.
- **Acoustic fingerprinting** (PRD-FP-003/004): Chromaprint fingerprint via
  `fpcalc -json -length` on the source audio file (not the normalized output),
  for stable acoustic identity independent of encoding.
- **Artwork normalization** (PRD-MEDIA-005): embedded and external artwork
  extracted and normalized to JPEG via FFmpeg, bounded by `max_artwork_bytes`
  and a maximum dimension of 1600px.
- **Sidecar parsing** (PRD-MEDIA-006): CUE, M3U, SFV, NFO, and LRC files are
  parsed into structured `ParsedSidecar` observations with findings.

Source media is never modified, moved, renamed, or deleted. All output paths
are constrained to the work-area directory via path normalization that
resolves `..` and `.` components without filesystem access.

## Architecture

- `crates/melodee-media/src/normalize.rs` is the compile-time boundary around
  FFmpeg and fpcalc. It produces Melodee-owned manifest types and never exposes
  third-party tool structures.
- `crates/melodee-ingestion/src/normalize.rs` contains the leased normalization
  worker. It claims `Normalizing` work items with a PostgreSQL lease, renews the
  lease via a heartbeat while blocking FFmpeg/fpcalc work runs on the Tokio
  blocking pool, and publishes results atomically with the `Normalizing ->
  Validating` transition.
- `crates/melodee-persistence/migrations/0005_normalization_provenance.sql`
  adds dedicated `normalization_provenance`, `normalization_started_at`, and
  `normalization_completed_at` columns to the manifest table, plus a claim
  index for pending `normalizing` items.
- `crates/melodee-persistence/src/work_items.rs` adds `claim_normalizing` and
  `publish_normalization`. Publication re-checks lease ownership inside its
  transaction, so a worker that loses its lease can never publish stale
  authoritative results.

## Manifest contract

Normalization observations are embedded in the existing
`ReleaseManifest.normalization` field as `NormalizationObservations`:

```text
profile_version: string
outputs: Vec<NormalizedOutput>          # canonical MP3 metadata + byte hash
fingerprints: Vec<FingerprintObservation> # Chromaprint fingerprint + duration
normalized_artwork: Vec<NormalizedArtwork>
parsed_sidecars: Vec<ParsedSidecar>
provenance: NormalizationExecutionProvenance  # volatile, excluded from hash
findings: Vec<Finding>
```

The semantic manifest hash is BLAKE3 over deterministic JSON after removing
volatile fields: manifest/work-item ids, source library/path, source snapshot,
inspection context, and normalization provenance. This means normalization
observations (fingerprints, output hashes, parsed sidecars) are part of the
manifest's semantic identity, while execution metadata (tool paths, worker id,
elapsed time, timestamps) is not.

## Canonical MP3 profile

| Parameter    | Value      | Legacy evidence |
|-------------|------------|-----------------|
| Codec       | libmp3lame | PRD-MEDIA-002 captured baseline |
| VBR quality | 4          | `compatibility/media/baseline.json` env.seededSettings |
| Max bitrate | 384 kbps   | Same |
| Sample rate | 48 kHz     | Same |
| Video       | None (`-vn`) | Same |
| Channels    | Preserved  | Legacy did not force stereo |

A file already in canonical form (MP3, 48 kHz, <= 384 kbps, any channel count,
no video) is retained by copy rather than re-encoded.

## Restart and concurrency proof

- `crates/melodee-persistence/tests/normalization.rs` proves `claim_normalizing`
  leases `Normalizing` items, `publish_normalization` advances to `Validating`
  and clears the lease, and stale workers cannot publish.
- The normalization worker uses `spawn_blocking` for the blocking media work
  and a heartbeat task for lease renewal. If lease renewal fails (e.g. the row
  was transitioned by an operator), the cancellation flag is set and the
  in-flight normalization is discarded.
- A crash before publication leaves the item `Normalizing` with an active lease;
  the Phase 2 lease recovery (`recover_expired_leases`) records the loss and
  schedules retry.

## Tooling

- FFmpeg: invoked with `Command::new` and separate arguments, never through a
  shell. Output and errors are bounded. Processes have a configurable timeout.
- fpcalc: invoked with `-json -length` flags. Output is bounded and parsed
  into `FingerprintObservation`.
- Source media is read-only; only the work-area staging directory receives
  writes.

## Known limitations

- Canonical publication (Phase 3.3) is not implemented. Normalized output
  remains in the work-area staging directory rather than being promoted to the
  library.
- Physical byte deduplication across releases (PRD-FP-005) is not implemented;
  normalized outputs are hashed but not deduplicated.
- Tantivy indexing of normalization observations is deferred to later phases.

## Requirements and references

Traceability is recorded in `design/requirements/status-overrides.json` and the
generated `design/requirements/REQUIREMENTS-INDEX.md`. The canonical MP3 policy
decision is evidenced in `compatibility/media/baseline.json` and
`design/evidence/phase-0/media-baseline.md` section 4. The FFmpeg/fpcalc tool
choice is recorded in `design/adr/ADR-0108-use-ffmpeg-ffprobe-and-chromaprint-tools.md`.
