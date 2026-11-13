# ADR-0117: Use a PostgreSQL catalog, identity, and contribution schema with set-based manifest import

- **Date:** 2026-08-10
- **Status:** Superseded
- **Decision owners:** User-directed Phase 4 authority work
- **Supersedes:** None
- **Superseded by:** [ADR-0189](ADR-0189-reuse-an-existing-artist-by-exact-normalized-name-at-import.md) — only the "creates provisional Artists when no credible or locked match exists rather than merging by name" import rule; every schema and identity invariant below stands
- **Related requirements:** PRD-ARTIST-001 through PRD-ARTIST-012, PRD-RESOLVE-001 through
  PRD-RESOLVE-005, PRD-CREDIT-001 through PRD-CREDIT-008, PRD-CATALOG-001 through
  PRD-CATALOG-005, PRD-INGEST-007 through PRD-INGEST-010, PRD-API-M1-004, PRD-MIG-002,
  PRD-MIG-004

> Only the import rule in decision item 4 below is superseded, by
> [ADR-0189](ADR-0189-reuse-an-existing-artist-by-exact-normalized-name-at-import.md). Every schema,
> identity, and audit invariant in this ADR remains binding.

## Context

Phase 3 publishes an immutable Release manifest v3 and reserves one UUIDv7 `ReleaseId`, but it
deliberately does not create catalog Releases, Tracks, Artists, contributions, or identity decisions.
The only existing PostgreSQL tables are ingestion/publication state and evidence. Phase 4 needs the
first authoritative semantic catalog schema and an idempotent `Published -> Cataloging -> Cataloged`
consumer.

Names cannot be identity keys. Existing public UUIDs are migration data, not necessarily internal
IDs. A Release work item, manifest, publication plan, legacy public ID, and catalog Release have
different lifecycles. The Phase 3 Release ID is nevertheless already committed to canonical storage
and must become the catalog Release's internal ID. Catalog import also needs to remain bounded and
set-based at 200-Release bursts and future multi-million-Track scale.

This ADR is the required Phase 4 schema decision. It fixes relational responsibilities and integrity
rules; individual migrations may refine non-semantic index details using measured query plans.

## Decision

### Identity and public-ID namespaces

- PostgreSQL is authoritative for the semantic catalog, identity decisions, and audit history.
- `Artist`, `Release`, `Track`, `MediaAsset`, and `AudioIdentity` use immutable UUIDv7 internal IDs.
  `Release.id` for a manifest import is exactly `manifest.publication.release_id`; import must never
  allocate a replacement.
- `Artist`, `Release`, and `Track` have a separate immutable, unique `public_api_id`. Legacy imports
  preserve the legacy UUID verbatim. For a newly created entity with no migrated identifier,
  `public_api_id` initially equals its internal ID. Adapters remain responsible for protocol-specific
  representation.
- `MediaAsset` and `AudioIdentity` are not legacy client resources and have no required legacy public
  ID. A future external contract may add a separate mapping without changing their internal IDs.
- Work-item IDs, work-item API keys, manifest IDs, publication authorization/plan IDs and hashes,
  canonical paths, external-provider IDs, and catalog/public IDs remain distinct namespaces.

### Authoritative relational model

Phase 4 migrations create normalized relations for:

- `artists`, `artist_aliases`, `artist_external_identities`, `artist_relationships`, and
  `group_memberships`;
- `releases`, `tracks`, `media_assets`, and `track_media_assets`;
- `release_artists`, `track_artists`, and `contributions`, including stable presentation order,
  exact display values, and join phrases;
- `observed_artists`, `artist_match_evidence`, `artist_match_decisions`, and
  `artist_negative_assertions`;
- append-only `artist_merge_events`, `artist_split_events`, and catalog/identity audit events;
- rebuildable `artist_track_associations` for direct and date-valid inferred participation;
- `catalog_imports` binding one work item, v3 manifest identity/hash, publication plan/receipt, and
  catalog Release; and
- a transactional catalog outbox whose consumers arrive in later phases.

The migrations enforce these invariants:

1. Artist canonical and normalized names are indexed but never unique. An alias means the same
   identity; collaboration/billing text is not an alias.
2. An Artist merge retains the old row and public ID as a tombstone/redirect through
   `merged_into_artist_id`. A split creates a new identity and moves only explicitly selected catalog
   relationships in one audited transaction. Neither operation changes `MediaAsset` locations.
3. Manual match decisions and explicit negative assertions are durable and may be locked. Provider
   refreshes cannot overwrite a locked decision.
4. Artist external identities are normalized rows. A partial uniqueness rule applies to
   `(provider, external_id)` only when that row declares that the provider guarantees Artist-ID
   uniqueness; providers that do not make that guarantee may retain multiple evidenced candidates.
5. Relationships connect distinct Artists and never imply equality. Group-membership start/end dates
   are inclusive; `NULL` is an unbounded side and a closed interval requires `start_date <= end_date`.
   Inference uses an explicit Track date when present, otherwise its Release date. A bounded
   membership with no comparable Track/Release date is not inferred. An explicit
   `(group_membership_id, track_id)` exclusion always wins.
6. Tracks have one importer-assigned ordinal unique within their Release. Observed disc/track
   numbers remain nullable, non-identity metadata and are not assumed unique.
7. Every playable Track references at least one MediaAsset through `track_media_assets`. The model is
   many-to-many so future deduplication can share an asset. Byte hashes are indexed evidence, not a
   globally unique semantic key. A canonical storage-relative path is unique; repeated manifest map
   entries may refer to that one asset.
8. Artist types are `person`, `group`, `orchestra`, `ensemble`, `choir`, `character`, `other`, and
   `unknown`; profession is not an Artist type. Artist status is `provisional`, `verified`, `merged`,
   or `deprecated`.
9. ReleaseArtist and TrackArtist express billing/presentation. Contribution expresses participation
   or credit. The initial role vocabulary is `primary_artist`, `featured_artist`, `guest_artist`,
   `performer`, `band_member`, `vocalist`, `instrumentalist`, `composer`, `lyricist`, `producer`,
   `remixer`, `engineer`, and `other`; the first seven are performance roles. Exact `DisplayArtist`
   text and ordered join phrases are retained independently from resolved identities. Parsing text
   creates observations/candidates only.
10. Direct and inferred Artist-track associations are distinguished. Inference is limited to
   performance roles and date-valid group membership; non-performance credits remain available to an
   explicit all-credits query.

An Artist negative assertion is scoped to one `ObservedArtist` and one stable candidate key (a local
Artist ID or provider/external-ID pair). It suppresses that candidate when the same observation is
re-resolved, but never suppresses every Artist sharing a normalized name. Reversal appends a
superseding decision/audit record instead of deleting history.

### Set-based, leased catalog import

The Phase 4 catalog worker has an independently configured concurrency limit and a dedicated
PostgreSQL claim path/index for `Published` and abandoned `Cataloging` work. Claims use the existing
lease/fencing model.

For each bounded batch, the importer:

1. reads the authoritative PostgreSQL manifest v3 plus matching publication plan and receipt;
2. validates their Release ID, hashes, paths, and retained disposition without reopening audio or
   reading canonical asset bytes;
3. expands Release, Track, observed Artist, and asset rows through set-based SQL (for example,
   arrays, `jsonb_to_recordset`, staging relations, or equivalent), never a lookup/save loop;
4. creates provisional Artists when no credible or locked match exists rather than merging by name
   (**superseded by [ADR-0189](ADR-0189-reuse-an-existing-artist-by-exact-normalized-name-at-import.md)**:
   import now reuses an existing Artist when the normalized name is exactly equal and the reuse is
   unambiguous; names remain non-unique and non-authoritative);
5. inserts catalog rows, associations, initial aggregates, catalog-import record, audit evidence, and
   future-search outbox records in the same transaction;
6. transitions `Cataloging -> Cataloged` and clears the lease in that transaction.

`catalog_imports.work_item_id` and `catalog_imports.release_id` are unique. A completed import is
idempotent success; a conflicting binding fails closed. Transaction rollback exposes no partial
catalog. Lease fencing prevents a stale worker from committing or completing another worker's item.

The outbox is PostgreSQL data only in Phase 4. Tantivy schema, indexing, checkpoint consumers, and
search behavior remain Phase 6 work.

### Legacy catalog boundary

Phase 4 proves catalog/public-ID mapping against the frozen compatibility fixture and a representative
legacy catalog snapshot. It reconciles Artist/Release/Track counts and relationships and preserves
their public UUIDs. Phase 11 still owns production backup, user/application-state migration, final
delta synchronization, traffic cutover, and rollback to the C# server.

Legacy catalog rows that do not have Phase 3 publication evidence may exist without a
`catalog_imports` row. New manifest-driven ingestion requires the complete v3 plan/receipt binding.

## Alternatives considered

### Use names, normalized names, or provider IDs as catalog keys

Rejected. Same-name Artists are required, providers are evidence rather than authority, and manual
identity corrections must not rewrite primary keys or media paths.

### Reuse the work-item or legacy public ID as every internal ID

Rejected. Processing identity, catalog identity, and compatibility identity have different
lifecycles. ADR-0116 already reserves the Release's internal ID independently. Separate public-ID
columns permit exact migration without coupling new internal identity policy to a legacy API.

### Import one Release through per-row repository calls

Rejected. It recreates the legacy N+1 persistence shape, complicates atomicity, and violates the
set-based and bounded catalog requirements.

### Implement search while creating the catalog outbox

Rejected. Durable outbox creation belongs with the catalog transaction, but Tantivy and search are a
separate Phase 6 boundary.

## Rationale

The schema makes identity corrections relational and auditable while leaving immutable media where it
is. Separate internal/public namespaces preserve compatibility without constraining new identities.
Set-based, fenced import closes the Phase 3 manifest handoff with one recoverable PostgreSQL commit and
provides a clean projection boundary for later search.

## Consequences

### Positive

- Same-name Artists, aliases, provisional identities, negative assertions, and corrections are
  representable without unsafe uniqueness shortcuts.
- Phase 3 Release IDs and canonical paths remain stable through catalog import, merge, and split.
- Catalog retry cannot create duplicate authoritative rows or partially complete a Release.
- Public UUID migration can be tested before production cutover.

### Negative / trade-offs

- The schema contains explicit evidence/audit/projection relations rather than a small CRUD model.
- Public and internal UUID columns may initially contain equal values for new entities but must still
  be treated as distinct namespaces.
- The materialized participation relation needs explicit rebuild/incremental maintenance after
  membership or identity changes.
- Phase 4 creates an outbox that has no indexing consumer until Phase 6.

### Follow-up work

- Phase 4 implements and validates the schema/import/identity behavior under the assigned work
  package.
- Phase 5 consumes AudioIdentity and catalog facts for classification, novelty, and disposition.
- Phase 6 consumes the catalog outbox and proves rebuildable Tantivy search.
- Phase 11 implements the production migration/cutover and full rollback exercise.

## Validation

- Migration and PostgreSQL integrity tests for IDs, non-unique names, provider-scoped identity,
  FK/check constraints, tombstones, and no partial catalog state.
- Set-based import tests for v3 ID adoption, exact asset mapping, duplicate submission, process/lease
  loss, PostgreSQL failure, and conflict rejection.
- A 200-Release bounded catalog scenario recording elapsed time, peak concurrency, query count, and
  zero duplicate/lost/partial results.
- Identity tests for exact same names, aliases, conflicting evidence, negative assertions, manual
  locks, merge/split, public-ID preservation, and date-bounded participation.
- Frozen legacy-fixture reconciliation of public Artist/Release/Track UUIDs and relationships.
- Full repository gate and Phase 4 evidence report before the phase is declared complete.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0105](ADR-0105-postgresql-is-authoritative-state-store.md)
- [ADR-0110](ADR-0110-use-sqlx-and-explicit-sql.md)
- [ADR-0116](ADR-0116-use-immutable-release-ids-and-convergent-canonical-publication.md)
- Phase 3 report (withdrawn evidence report `design/evidence/phase-3/PHASE-3-REPORT.md`)
- Phase 4 work package
