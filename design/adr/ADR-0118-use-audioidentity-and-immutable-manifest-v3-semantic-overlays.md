# ADR-0118: Use AudioIdentity and immutable manifest v3 semantic overlays

- **Date:** 2026-08-10
- **Status:** Accepted
- **Decision owners:** User-directed Phase 4 authority work
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-FP-001, PRD-FP-002, PRD-FP-004, PRD-FP-005,
  PRD-RESOLVE-001 through PRD-RESOLVE-005, PRD-CREDIT-003 through PRD-CREDIT-008,
  PRD-INGEST-007, PRD-ARTIST-008 through PRD-ARTIST-011

## Context

`Track` is a playable occurrence ordered on one Release. `MediaAsset` is encoded canonical bytes.
Neither is the stable concept needed to say that two Release occurrences contain the same underlying
recording/performance despite different encodings. PRD-FP-004 requires that concept and requires its
name/schema to be decided before schema freeze.

Phase 3's sealed manifest v3 is also intentionally media-observational: it records raw tags,
fingerprints, normalized assets, structural validation, immutable Release identity, and publication
facts. It does not contain resolved Artist identities, canonical contributions, manual decisions, or
later classification/policy. ADR-0116 hashes and seals v3 into a no-clobber canonical bundle; rewriting
that file after an identity correction would invalidate the publication receipt and crash-adoption
protocol.

SPEC section 13 describes an evolving manifest family that eventually carries resolved identity,
contribution, classification, and policy facts. Phase 4 needs a versioned way to add the semantic
facts without mutating the byte-publication contract.

## Decision

### AudioIdentity

The core term is `AudioIdentity`.

- An `AudioIdentity` is a Melodee-owned identity for one underlying audible recording/performance.
- A `Track` is a Release occurrence and links to one current `AudioIdentity`. A new manifest import
  conservatively creates a distinct provisional AudioIdentity for each Track unless a locked decision
  or sufficiently strong non-name evidence already confirms an existing identity.
- One AudioIdentity may be referenced by many Tracks and may have many MediaAssets through those Track
  occurrences. Multiple encodings or byte hashes therefore do not create multiple semantic identities
  by definition.
- BLAKE3 hashes establish exact byte equality. Chromaprint, duration, metadata, external IDs, and
  catalog context are evidence for AudioIdentity resolution; none is a primary key and no individual
  signal auto-merges identities.
- Alternate recordings/versions remain different AudioIdentities. Uncertain similarity remains
  evidence between provisional identities until confirmed; uncertainty never forces equality.
- AudioIdentity merge/split/correction follows the same append-only decision/audit principle as Artist
  correction and never requires moving a MediaAsset. Phase 5 owns novelty rules that consume these
  identities; Phase 4 only establishes identity, evidence, and correction semantics.
- Global physical byte deduplication remains optional. The schema permits shared MediaAssets but
  Phase 4 does not relocate or coalesce canonical files.

### Immutable v3 and semantic overlays

Published Release manifest v3 is immutable. Phase 4 must not rewrite either the authoritative
PostgreSQL v3 payload or the canonical `manifest.json`, and must not change its semantic manifest hash,
publication-plan hash, sealed-manifest hash, or receipt.

Phase 4 stores a separately versioned `CatalogSemanticOverlay` in PostgreSQL:

- overlay schema name/version and immutable overlay ID;
- Release ID, source manifest ID, source semantic manifest hash, publication plan hash, and catalog
  import ID;
- resolver/ruleset version and creation/audit provenance;
- references to the normalized Artist observations, match decisions, ReleaseArtist/TrackArtist,
  Contributions, AudioIdentity links, and current display presentation produced from that source;
- a deterministic overlay hash; and
- an optional predecessor overlay ID.

The normalized relational catalog remains query authority. The overlay is an immutable, reproducible
snapshot/evidence boundary explaining how one v3 observation became a catalog revision. Initial
cataloging creates overlay version 1 in the same transaction as catalog rows. Manual identity
correction, merge/split, or evidence re-resolution appends a new overlay revision and audit event and
atomically advances the Release's current-overlay reference. Old revisions remain readable.

Overlay facts are not inputs to the Phase 3 observed-media semantic hash or canonical publication
plan. Catalog rebuild may start from v3 plus durable manual/negative decisions and regenerate an
equivalent normalized catalog/overlay under an explicitly versioned resolver. It never needs to
re-probe audio.

This overlay is the Phase 4 member of SPEC section 13's evolving manifest family. It carries resolved
identity and contribution facts without pretending they existed during media publication. Phase 5
may append a separate versioned classification/policy overlay or extend the catalog-semantic overlay
through a new accepted ADR. A future on-disk manifest version requires a new publication/sealing ADR;
it cannot be introduced by overwriting v3 in place.

## Alternatives considered

### Name the concept TrackIdentity

Rejected because `Track` already means a Release occurrence. `TrackIdentity` is easy to confuse with
`TrackId` and obscures the cross-Release, cross-encoding purpose of the abstraction.

### Use MediaAsset or fingerprint as the underlying-audio identity

Rejected. MediaAsset is byte/container identity, while fingerprints are fallible evidence. Either
choice would conflate encoding, observation, and semantic recording identity.

### Rewrite manifest v3 after identity resolution

Rejected. It breaks ADR-0116's immutable publication plan, sealed-manifest receipt, exact-final
adoption, and no-clobber recovery guarantees. It would also make an Artist correction a filesystem
mutation.

### Keep semantic decisions only in mutable catalog rows

Rejected. Normalized rows are required for queries, but without an immutable revision/evidence
boundary it is difficult to explain, audit, reproduce, or safely retry the transformation from a
specific v3 observation.

## Rationale

AudioIdentity cleanly separates occurrence, bytes, and recording/performance identity. Conservative
provisional allocation biases against false equality while enabling later novelty analysis. Immutable
semantic overlays preserve the Phase 3 publication proof and make catalog identity corrections
auditable without coupling them to the filesystem.

## Consequences

### Positive

- The catalog can represent the same audio across Releases/encodings without requiring byte
  deduplication.
- False audio matches remain reversible provisional/evidence decisions.
- Published bundles remain stable under Artist, contribution, and AudioIdentity correction.
- Catalog transformations have a versioned, hashable, reproducible audit boundary.

### Negative / trade-offs

- PostgreSQL stores both normalized query tables and immutable overlay revisions.
- Resolver changes require explicit versioning and may append revisions for many Releases.
- A provisional AudioIdentity per imported Track creates extra rows until evidence safely converges
  them; this is the intentional cost of avoiding false merges.

### Follow-up work

- Phase 4 implements AudioIdentity/evidence/overlay schema and rebuild/correction tests.
- Phase 5 defines audio-match confidence and novelty/classification/policy consumers.
- Any future on-disk manifest v4 or overlay extension that changes the sealing boundary requires a new
  ADR and migration plan.

## Validation

- Domain and PostgreSQL tests distinguish Track, MediaAsset, and AudioIdentity IDs.
- Same/different byte and same/different fingerprint fixtures prove that evidence does not become an
  automatic identity key.
- Cross-Release tests link confirmed same audio while alternate/uncertain audio stays distinct.
- V3-to-overlay import and rebuild tests operate without reading audio and produce deterministic
  overlay hashes for the same resolver version and decisions.
- Correction/merge/split tests append revisions, retain manual/negative evidence, preserve historical
  overlays, and leave canonical manifest/media bytes and publication hashes unchanged.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0115](ADR-0115-media-inspection-boundary-and-manifest-byte-hashing.md)
- [ADR-0116](ADR-0116-use-immutable-release-ids-and-convergent-canonical-publication.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- Phase 3 report (historical artifact removed)
- Phase 4 work package
