# ADR-0194: Preserve catalog import references and projection revisions

- **Date:** 2026-11-13
- **Status:** Accepted
- **Decision owners:** User-directed review remediation program
- **Supersedes:** [ADR-0190](ADR-0190-name-the-imported-artist-state-active-rather-than-provisional.md), only its assertion that the status rename needs no projection refresh
- **Superseded by:** None
- **Related requirements:** PRD-ARTIST-001, PRD-ARTIST-002, PRD-ARTIST-008, PRD-ARTIST-011,
  PRD-RESOLVE-001, PRD-RESOLVE-005, PRD-CATALOG-001, PRD-CATALOG-005, PRD-SEARCH-007,
  PRD-INGEST-007

## Context

The review of ADR-0189 import reuse found three violations of its existing evidence gates:
filtering disambiguated homonyms before counting candidates, accepting a new embedded MusicBrainz
identifier when the existing Artist had none, and counting joined provider rows as distinct Artists.
It also found that the initial immutable overlay kept newly allocated Artist IDs after relational
credits were remapped to an existing Artist. Reused Artist aggregate changes were omitted from the
projection outbox, and match reasons incorrectly claimed a new identity was created.

Migration 0044 renamed an indexed Artist status without advancing its revision or scheduling a
projection update. Tantivy rejects upserts at an unchanged revision, so merely scheduling the old
revision does not correct the search document. Migration 0044 is already committed and must retain
its checksum.

## Decision

1. Enforce ADR-0189 as written: count all live homonyms, then require that the sole Artist has no
   disambiguation. Aggregate MusicBrainz identifiers per Artist. Each embedded Artist identifier
   must already belong to that candidate; no embedded identifier requires no provider evidence.
   Names remain non-unique and matching does not promote an Artist to Verified.
2. After transaction-scoped name reuse, rebuild the initial overlay from the final normalized
   decisions before computing its hash or inserting it. Record catalog reuse explicitly in the
   evidence and decision reason. This changes no sealed media manifest or historical overlay.
3. Advance each reused Artist's revision once per successful import. Enqueue all affected Artist,
   Release, and Track documents using their actual entity revisions in the same transaction.
   Retrying a completed import performs no further revision, overlay, or outbox mutation.
4. New migration 0045 advances all live Artist revisions and inserts corresponding outbox upserts.
   This conservatively repairs both migration 0044 status freshness and missed reuse updates.
   Search catches up through the existing worker; no media read, re-ingestion, or forced full
   index rebuild is required.
5. Migration 0045 inventories current overlays whose Artist references disagree with their recorded
   decisions or refer to nonexistent Artists into `catalog_overlay_repair_queue`. The migration
   runner drains at most 100 Releases per transaction through the existing bounded overlay
   constructor and hash algorithm, before startup succeeds. Each repair appends an overlay and audit
   event, advances the Release revision/pointer and projection outbox, and removes its queue row
   atomically. Historical payloads and hashes remain intact. A pointer already replaced by an
   operator correction is not overwritten. A crash rolls back its batch; startup resumes the queue.
6. The migration inventory runs once. Later starts inspect only the bounded repair queue, not every
   catalog overlay. Repair errors fail startup with their root cause; they never mark pending
   repairs complete. No domain identities, public IDs, observations, decisions, or media are deleted.
7. Artist Track-credit drill-downs return distinct Tracks, matching their aggregate counts. Multiple
   associations are collapsed before pagination/counting; performance is true if any evidence is
   performance. Direct primary/featured/guest associations take precedence for the representative
   association label, and the earliest billing supplies its display fields. Separate contribution
   evidence remains intact. This corrects duplicate rows exposed by the populated live audit.

## Alternatives considered

- Editing migration 0044 would invalidate applied checksums and would not repair already upgraded
  catalogs. A forward migration is required.
- Upserting an unchanged revision is rejected by search revision fencing. Disabling fencing would
  permit stale work to overwrite newer documents.
- Rewriting immutable JSON would erase the original evidence and violate ADR-0118. A replacement
  snapshot must retain its predecessor.
- Recreating Artist identities or re-ingesting media would change identity or byte-preservation
  contracts for a projection/evidence repair and is rejected.

## Consequences

The upgrade requires the normal maintenance window: stop writers, retain a verified PostgreSQL
backup, and start one candidate instance. The SQL revision/outbox update needs WAL and transaction
headroom proportional to live Artist count. Overlay repair consumes bounded batches and resumes
safely after failure. A failed repair prevents startup until its cause is resolved; the previous
committed batches remain valid and idempotent.

Rollback restores the matching pre-upgrade database backup and image while preserving canonical
media. Do not decrement revisions, delete retained evidence, or edit migration history in place.
An older binary is not supported against the new migration set. Forward recovery reruns the normal
migration command and drains the remaining queue.

## Validation

- `catalog_reuse` tests cover disambiguated homonyms, missing/conflicting/multiple provider IDs,
  normalized-decision/overlay correspondence, exact outbox revisions, and import retries.
- Upgrade coverage seeds a valid historical overlay containing nonexistent Artist references,
  applies 0045, and proves replacement/predecessor linkage, unchanged old payloads, projection
  revision advancement, drained queue, and repeat-start idempotence.
- The runtime audit accepts the domain's current Artist status vocabulary and reports inconsistent
  current overlay references. Python coverage detects domain/dictionary drift.
- Catalog browsing coverage proves multiple associations and repeated billing yield one Track row,
  while combined production/performance contributions retain performance truth and separate evidence.
- Existing catalog import, concurrency, and migration suites remain required regression gates.

## References

- [SPEC.md](../../SPEC.md)
- [PRD.md](../../PRD.md)
- [ADR-0118](ADR-0118-use-audioidentity-and-immutable-manifest-v3-semantic-overlays.md)
- [ADR-0189](ADR-0189-reuse-an-existing-artist-by-exact-normalized-name-at-import.md)
- [Migration 0045](../../crates/melodee-persistence/migrations/0045_repair_artist_projection_and_overlay_references.sql)
- [Upgrade guide](../../docs/src/content/docs/installation/upgrading.md)
