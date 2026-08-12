# ADR-0127: Native public ID migration and ID-preserving schema for API v1

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (phase governance)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-M1-004, PRD-PLAYLIST-003, PRD-USER-001..005

## Context

Public IDs are external contracts used by clients, URLs, and existing links. Any migration that changes identifiers breaks compatibility.

## Decision

- Preserve existing public IDs for users, artists, releases, tracks, playlists, shares, and related resources unless migration evidence and PRD explicitly require translation.
- Any new persistence objects adopt internal immutable IDs, and public IDs are mapped in API-facing layers.
- Backfill and reconciliation tests are required for all migrated/public-facing resources.
- Existing Phase 4 Artist/Release/Track public IDs remain authoritative. Phase 7 User, Playlist,
  Share, and other resources that expose captured public UUIDs use UUIDv7 internal primary keys while
  those legacy public UUIDs are immutable and unique. Internal playlist items and sessions do not
  acquire invented wire IDs. Playlist ordering uses an explicit bounded integer position with a
  deferrable per-playlist uniqueness invariant so set-based reorder is atomic.
- A migration import binding records the legacy source kind/key and imported internal/public IDs;
  exact retry adopts an identical mapping and conflicting retry fails closed. Names, ordering, and
  mutable display fields never form identity keys.
- No Phase 7 migration rewrites or regenerates an existing catalog public ID. All foreign keys use
  internal IDs; wire adapters resolve public IDs explicitly and follow accepted identity redirects.

## Alternatives considered

### Alternative A

Regenerate all public IDs at migration time.

- Rejected: breaks client references and route-shared URLs.

### Alternative B

Treat all IDs as opaque and mutable.

- Rejected: destroys traceability of migration and causes URL breakage.

## Rationale

Preserving existing IDs provides stable compatibility for clients and offline artifacts.

## Consequences

### Positive

- No widespread client URL invalidation.
- Shared fixture and cache compatibility across cutover.

### Negative / trade-offs

- Additional reconciliation checks required in migration and schema evolution.

### Follow-up work

- Add migration smoke tests with identity round-trip assertions for all affected domain objects.
- Add invariants to prevent normalized names from being used as authoritative keys.

## Validation

- Migration/integrity tests on known public IDs.
- Integration tests for old URLs, cached stream URLs, share URLs, and playlist routes.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
