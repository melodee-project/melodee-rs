# ADR-0106: Use Release-centric canonical storage

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-STORAGE-001 through PRD-STORAGE-004

## Context

Artist-folder ownership makes collaborations, same-name Artists, merges, splits, and identity
corrections into filesystem mutations. The existing library is nearly 5 TB, so avoidable moves are
high risk.

## Decision

Proposed: shard canonical storage by stable Release ID. Artist/release relationships live in the
catalog. Optional human-readable by-Artist views are generated and disposable.

## Alternatives considered

- Preserve Artist/Release physical hierarchy.
- Content-address every MediaAsset and make Releases purely virtual.

## Validation

Prototype migration/reference behavior against the existing storage tree and verify backup,
human-operability, and cross-filesystem publication behavior before acceptance.

## Phase 0 ratification (2026-08-09)

- **Status change:** Proposed -> Accepted (Issue 10 ratification authority).
- **PRD support:** PRD-STORAGE-002 explicitly states "The preferred direction is
  Release-centric/immutable-ID-centric canonical storage" with this sharded layout as the
  example; PRD-STORAGE-001 forbids filesystem-defined Artist identity; PRD-STORAGE-003 allows
  rebuildable by-artist views.
- **Phase 0 evidence:** media baseline (withdrawn evidence report
  `design/evidence/phase-0/media-baseline.md`) measured
  the artist-folder coupling hazards this removes: artwork discovery reads the artist/album
  directory (ArtistService.cs:2300+), staging/storage naming derives from artist/album strings,
  and same-name-artist identity is schema-impossible in legacy (`IX_Artists_Name` UNIQUE) while
  normalized-name collisions exist (`Dune`/`Düné` fixture pair). Artist rename/merge under
  artist-folder ownership is a multi-terabyte filesystem mutation.
- **No contradictory evidence.** No new dependency. Alternatives documented above.
- Exact shard width/prefix cosmetics are implementation parameters settled in Phase 1 schema
  work; this ADR fixes the principle (immutable Release ID authoritative, artist views are
  projections).
