# ADR-0168: Upgrade pinned DecentDB replica reader to 2.17.1

- **Date:** 2026-08-26
- **Status:** Superseded
- **Decision owners:** Repository dependency maintenance
- **Supersedes:** ADR-0163
- **Superseded by:** ADR-0179
- **Related requirements:** PRD-ARTIST-012, PRD-EXT-001, PRD-EXT-003, PRD-CONFIG-001

## Context

ADR-0163 pinned the Apache-2.0 `decentdb` reader at version `2.16.1` and a fixed Git commit so
configured MusicBrainz `.ddb` replicas could be queried without floating an external storage
engine dependency. Version `2.17.1` is now the latest stable upstream release. Its `2.17.x`
line preserves the public read API used by Melodee while adding typed failure propagation and
multiple WAL, checkpoint, and recovery correctness fixes.

The exact commit remains architecture- and data-safety-sensitive even though Melodee opens the
replica read-only. The new pin therefore supersedes, rather than silently edits, the original
version decision.

## Decision

Pin `decentdb` version `2.17.1` at commit
`5b47323b8dedcd1051b036bbbcc66f6905578a63`, with default features disabled. Retain every
boundary from ADR-0163: reads execute in a bounded blocking task, only the bounded
`Artist`/`ArtistAlias` evidence query is permitted, Lua extensions remain disabled, and the
configured replica is never modified.

## Alternatives considered

### Keep version 2.16.1

Rejected. It avoids immediate lockfile churn but leaves the repository on an older storage-engine
release after upstream shipped typed error propagation and durability/recovery fixes.

### Follow an upstream branch or moving tag

Rejected. A floating source would make builds non-reproducible and could silently change the
replica parser or its transitive FOSS surface.

## Rationale

The fixed `2.17.1` commit keeps the reproducibility and feature restrictions required by ADR-0163
while taking the latest stable correctness work. The upgrade does not add a service, change the
provider boundary, or grant write access to the source replica.

## Consequences

### Positive

- Uses the latest stable DecentDB release and its error-propagation and durability fixes.
- Preserves an exact, auditable source revision and the existing read-only provider contract.

### Negative / trade-offs

- Changes a format-sensitive runtime dependency and its transitive lockfile surface.
- Requires the real DecentDB provider fixture and FOSS audit to be rerun.

### Follow-up work

- Re-evaluate and supersede this ADR before changing the pinned commit again.

## Validation

Run the provider's temporary DecentDB `Artist`/`ArtistAlias` fixture through the production
local-replica path. Run the full workspace test, Clippy, formatting, and `cargo deny` gates against
the regenerated lockfile.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0163](ADR-0163-rust-native-musicbrainz-decentdb-replica-reads.md)
- [DecentDB 2.17.1 release](https://github.com/sphildreth/decentdb/releases/tag/v2.17.1)
- [DecentDB changelog](https://decentdb.org/about/changelog/)
