# ADR-0214: Bound processed search-outbox history with exact hourly totals

- **Date:** 2026-11-25
- **Status:** Accepted
- **Decision owners:** search/projection maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SEARCH-007, PRD-WORK-003, PRD-PERF-001,
  PRD-PERF-006, PRD-INGEST-010
- **Amends:** ADR-0121 (search projection lifecycle), ADR-0184 (activity chart)

## Context

All 137,664 completed search-outbox events in production occupied about 49 MiB
and existed only to prove that a Tantivy commit preceded the SQL
acknowledgement. The Admin activity chart reads one number per hour: distinct
Track IDs successfully indexed. Retaining one permanent row per event to
produce one number per hour is unbounded growth with no queryable value after a
short operational window.

Pruning successful rows is dangerous in four ways:

1. A chart assembled from raw rows silently loses history.
2. Deleting before an hour closes loses a count from a late acknowledgement.
3. `max(id)` stops being a completeness proof, so rebuild high-water marks and
   checkpoints become wrong.
4. A stale producer replay could resurrect an event whose unique row was
   deleted.

## Decision

1. **Successful detail is retained 7 days by default**, measured from
   `processed_at` in UTC. The retention is the ordinary Admin-editable setting
   `search.processedOutboxRetentionDays`
   (`MELODEE__SEARCH__PROCESSED_OUTBOX_RETENTION_DAYS`), range 2–90. Pending,
   leased, retryable and permanently failed rows are never candidates.
2. **Exact hourly totals are retained 400 days**, one row per projection and
   UTC hour: `count(DISTINCT entity_id)` for `entity_type = 'track'`. Two
   revisions in an hour count once; one Track in two hours counts once in each.
   Zero-filled hours and the live partial hour remain query behavior.
3. **Finalize before prune.** A closed hour is finalized from the full raw hour
   before any of its rows can be deleted. Finalization takes an exclusive
   advisory key while `acknowledge_outbox_with_generation` and
   `acknowledge_outbox` take the shared key in their transaction, so an
   acknowledgement that is in flight when an hour closes is visible to the
   aggregate and a late commit cannot lose a count. Aggregate insert and
   watermark advance are idempotent.
4. **A projection may not be pruned during a rebuild.** Pruning pauses while
   `search_rebuild_jobs` has a `requested`, `running` or `validated` job.
   Registration takes the same exclusive advisory key, so the decision is
   serialized with new registrations. The reason is reported.
5. **Deletion requires a durable checkpoint.** Each row is deleted only if it
   is processed, older than the cutoff, in an hour at or below
   `finalized_through`, and at or below the projection's
   `last_processed_outbox_id`. Batches are at most 1,000 ordered rows per
   transaction and yield between batches. The maintenance row is locked, so one
   process owns a projection's batch.
6. **`highest_pruned_id` is written with the delete.** Every high-water reader
   (`claim_rebuild`, `SearchProjectionStore::max_outbox_id`,
   `projection_high_water`) uses `GREATEST(max(id), highest_pruned_id)`.
7. **Replay safety survives deletion.** A bounded per-entity tombstone
   (`search_outbox_prune_tombstones`, one row per projection/entity with the
   highest pruned revision) plus a `BEFORE INSERT` trigger suppress a stale
   producer replay of an already-pruned revision and its older revisions. New
   revisions still enqueue. This is one row per entity, not one per event.
8. **Chart reads merge both sources.** Hours at or below `finalized_through`
   come from the immutable totals; later hours (including the live partial
   hour) come from raw detail. The Admin API shape is unchanged apart from
   additive retention diagnostics, and the existing 24-hour window, first-hour
   partial handling and zero-filled spine are preserved.
9. **Operator interface.** `melodee-cli search prune-outbox` (bare or
   `--dry-run`) previews eligible, protected, failed, pending and leased rows,
   the oldest retained completion, the last finalized hour, the last prune
   result and any blocking rebuild. `--apply` uses exactly the same selection,
   fencing and audit logic as the hourly scheduled pass. Admin search status and
   the System → Search panel expose the same facts.
10. **Retention never resurrects old buckets.** The finalized-through watermark
    is kept after hourly totals age out; detail released by a protected rebuild
    later is cleaned up by the same acknowledgement and replay checks without
    creating a chart bucket older than the retained window.

## Alternatives considered

### Keep every processed row

Simple and already correct for the chart, but unbounded and contradicts the
database-growth objective; 49 MiB for a number that fits in a few thousand rows.

### Delete rows and rebuild the chart from transition logs

The transition log records work-item state, not distinct Tracks committed to
Tantivy, so it would change what the chart means.

### Keep only a running counter per hour

Not idempotent across retries and conflicting with "distinct Track per hour";
the aggregate row is already tiny.

## Rationale

Retention and statistics have different lifetimes. Successful detail is
operational evidence for a short window; exact hourly counts are product
history for a long one. Computing the aggregate from the full raw hour before
deleting, with database-enforced serialization, makes the two consistent even
under late acknowledgements and interrupted passes.

## Consequences

### Positive

- Processed history is bounded by a configurable window.
- The Admin chart stays exact across arbitrary pruning.
- Rebuilds and acknowledgements cannot lose counts.
- Replays after pruning cannot resurrect old events.

### Negative / trade-offs

- One extra aggregate table and one maintenance row per projection.
- Two advisory lock keys on the acknowledgement path (shared, cheap).
- Sub-hour chart boundaries older than the retained window are unavailable by
  design; the hourly chart is unaffected.
- An acknowledgement that commits with a `processed_at` inside an
  already-finalized hour would be excluded; the lock makes that impossible for
  the documented flow, and the retention window (≥ 2 days) is far longer than
  any lease.

### Follow-up work

- Optional per-projection retention overrides if a second projection is added.

## Validation

- `crates/melodee-persistence/tests/search_outbox_retention.rs`: 100,000
  eligible successes plus recent/leased/pending/retryable/failed controls;
  dry-run counts; all-and-only eligible deletion; bounded batches; independent
  distinct-count comparison; identical chart after pruning; empty raw table;
  stale upsert replay suppression and current-revision enqueue; interrupted
  single-batch pass resumed; two concurrent workers deleting each row once;
  rebuild blocking and release; acknowledgements across an hour boundary kept
  in their own hour.
- `crates/melodee-search` rebuild tests continue to assert generation counts
  and queries over pruned and unpruned outbox states.
- `scripts/audit_runtime_invariants.py` checks retention and aggregate
  consistency on the development instance.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0121](./ADR-0121-search-projection-schema-and-lifecycle.md)
- [ADR-0184](./ADR-0184-admin-ui-reference-alignment-and-surface-system.md)
