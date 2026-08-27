# ADR-0163: Rust-native MusicBrainz DecentDB replica reads

- **Date:** 2026-08-25
- **Status:** Superseded
- **Decision owners:** Feature Completion Program Phase 2
- **Supersedes:** None
- **Superseded by:** ADR-0168
- **Related requirements:** PRD-ARTIST-012, PRD-EXT-001, PRD-EXT-003, PRD-CONFIG-001

## Context

The C# MusicBrainz provider uses a read-only materialized DecentDB database with `Artist` and
`ArtistAlias` tables. A JSON export is useful for fixtures and operational portability, but using
only that fallback would leave configured legacy `.ddb` replicas inert in the Rust server.

## Decision

Use the pinned Apache-2.0 `decentdb` Rust engine at commit
`eb7b7d11339e3dd1f436423d8fcf294a6ec51a82` (version `2.16.1`) with default features disabled.
The provider opens the configured replica from a bounded blocking task and issues only the bounded
Artist/ArtistAlias search query needed for provider evidence. The async provider concurrency gate
remains the outer backpressure boundary. JSON materialized exports remain a deterministic
fixture/portable fallback when no `.ddb` file is present.

DecentDB rows remain provider evidence: their MusicBrainz values are retained as observations and
can participate only in the accepted strong-evidence resolver policy. The local replica cannot
allocate Melodee identities, write catalog rows, or modify the source database.

## Alternatives considered

### Parse the DecentDB file format in Melodee

Rejected: duplicating a storage engine parser would create an unbounded compatibility and
corruption-recovery surface. The upstream Rust engine is the authoritative reader for its format.

### Keep only the JSON fallback

Rejected: it would not consume the configured C#-compatible local replica and would make the
accepted offline MusicBrainz behavior dependent on a separate export step.

## Rationale

The pinned upstream engine provides the actual local-replica behavior while keeping the rewrite's
provider boundary Rust-native and read-only. Its Apache-2.0 license and disabled optional Lua
features satisfy the repository's FOSS and dependency constraints.

## Consequences

### Positive

- Configured C#-compatible `.ddb` replicas are live in the Rust provider path.
- The local query is bounded and uses the same materialized table contract as the legacy provider.
- Replica failures remain provider failures and never block catalog import.

### Negative / trade-offs

- The workspace gains a pinned external Rust dependency and its transitive FOSS audit surface.
- DecentDB reads are synchronous inside `spawn_blocking`; the provider semaphore bounds concurrent
  readers, and cancellation drops the async task boundary.

### Follow-up work

- Operator documentation should identify the configured replica path and the JSON fixture fallback.

## Validation

The provider unit test creates a temporary DecentDB `Artist`/`ArtistAlias` fixture, reads it through
the production local-replica function, and verifies the retained external ID and name. Cargo
dependency and FOSS gates validate the pinned dependency.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0154](ADR-0154-external-provider-enrichment-scope-and-runtime.md)
- [ADR-0162](ADR-0162-durable-provider-enrichment-tasks-and-release-external-evidence.md)
- [DecentDB repository](https://github.com/sphildreth/decentdb)
