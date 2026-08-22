# First 10 GitHub Issues

These issue descriptions are designed to be copied into GitHub after the repository is created.

## Issue 1 — Validate the bootstrap and commit the Rust lockfile

**Requirements:** PRD-PRINCIPLE-007, PRD-SEC-007  
**ADRs:** ADR-0100, ADR-0103, ADR-0104

### Work

- Run the pinned toolchain.
- Resolve the workspace.
- Fix any compile/clippy problems in the minimal server, CLI, and domain crate.
- Generate and commit `Cargo.lock`.
- Run `cargo-deny` and document any license exceptions rather than broadening the allowlist casually.
- Confirm CI works from a clean checkout.

### Acceptance

- `make check` passes.
- `/healthz` responds from the bootstrap server.
- Domain tests pass.
- Lockfile is committed.

## Issue 2 — Freeze and document the legacy baseline

**Requirements:** PRD-TEST-001, PRD-TEST-003

### Work

- Record legacy repository commit/tag, server version, database version/schema migration, configured
  feature flags, and fixture dataset counts.
- Create `design/evidence/phase-0/legacy-baseline.md`.
- Define how the baseline can be recreated without production data.

### Acceptance

- Another developer can identify and start the same baseline.
- No secrets/private paths are committed.

## Issue 3 — Capture published API specifications and route manifests

**Requirements:** PRD-API-M1-001, PRD-API-OS-001, PRD-API-JF-001

### Work

- Capture all native OpenAPI documents.
- Generate route manifests for native v1, OpenSubsonic, and Jellyfin controllers.
- Record method, path, auth, status category, feature/role conditions, and implementation status.

### Acceptance

- Contract files are stable, sanitized, and tied to the baseline commit.
- Route counts are reported and reviewed.

## Issue 4 — Define HTTP snapshot and normalization format

**Requirements:** PRD-TEST-002, PRD-TEST-003

### Work

- Define a versioned scenario/fixture format.
- Normalize only volatile fields (timestamps, correlation IDs, base URLs where appropriate).
- Preserve null/omitted distinctions and array ordering unless the contract explicitly says otherwise.
- Implement comparison tests.

### Acceptance

- One public and one authenticated scenario compare reproducibly.
- A deliberate contract difference causes a useful failure.

## Issue 5 — Capture native Melodee API v1 critical path

**Requirements:** PRD-API-M1-001 through PRD-API-M1-006, PRD-STREAM-001 through PRD-STREAM-003

### Work

Capture auth, user, search, artist, Release/Album, Track/Song, playlists, queue, scrobble, images, and
stream URL behavior used by Android and MeloAmp.

### Acceptance

- Existing Android app and MeloAmp requests are represented.
- Refresh-token lifecycle is documented.
- Stream route/range behavior is captured.

## Issue 6 — Capture OpenSubsonic critical path and negative behavior

**Requirements:** PRD-API-OS-001 through PRD-API-OS-007

### Work

Convert the current matrix into executable scenarios, including JSON/XML and 410/501 behavior.

### Acceptance

- Critical browse/search/stream/playlist/scrobble workflows are covered.
- Extension advertisement and plain/`.view` route behavior are tested.

## Issue 7 — Capture Jellyfin compatibility critical path

**Requirements:** PRD-API-JF-001 through PRD-API-JF-006

### Work

Capture discovery, auth/tokens, items, Artists, genres, audio, images, playlists, sessions, user state,
and supported-client probes.

### Acceptance

- At least one supported Jellyfin client completes a documented smoke path.
- Unsupported feature probes are classified, not treated as random failures.

## Issue 8 — Capture media normalization and metadata baseline

**Requirements:** PRD-MEDIA-001 through PRD-MEDIA-006, PRD-FP-001

### Work

- Record exact current storage conversion settings and tool versions.
- Create small licensed/synthetic MP3, FLAC, OGG, WAV fixtures.
- Capture tags, artwork, output media properties, filenames, and failure behavior.

### Acceptance

- Expected output bitrate mode, sample rate/channel behavior, and metadata retention are explicit.
- Fixtures are redistributable or generated.

## Issue 9 — Add synthetic scale generators and benchmark result schema

**Requirements:** PRD-SCALE-003, PRD-PERF-001 through PRD-PERF-006

### Work

- Generate synthetic catalog records and Release directory trees without real audio.
- Define benchmark metadata: hardware, commit, dataset, command, wall time, memory, database queries,
  files/bytes touched, percentiles.
- Add baseline commands for no-op scan, search, browse, and stream start.

### Acceptance

- Small developer dataset and large configurable dataset are reproducible.
- Benchmark results are machine-readable and human-readable.

## Issue 10 — Produce Phase 0 report and ratify Phase 1 ADRs

**Requirements:** PRD-TEST-001 through PRD-TEST-008, PRD-ARCH-001 through PRD-ARCH-006

### Work

- Summarize contract, media, scale, and client evidence.
- Review proposed ADR-0106 through ADR-0111.
- Accept, revise, reject, or defer each with evidence.
- Update traceability and Phase 1 work package.

### Acceptance

- Phase 0 exit checklist is complete.
- Phase 1 has no unresolved architecture decision that blocks its first implementation slice.
