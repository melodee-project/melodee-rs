# ADR-0142: Admin browse index remediation

- **Date:** 2026-08-15
- **Status:** Accepted
- **Decision owners:** Phase 10 follow-up (admin-list index remediation)
- **Supersedes:** None
- **Superseded by:** ADR-0166 (operational application guidance only)
- **Related requirements:** PRD-API-ADMIN-002, PRD-UI-005/006/007/008/010, SPEC §23

## Context

The Phase 10 W4-5 performance evidence
(performance-report.md (historical artifact removed), §3/§4) measured every
admin list endpoint with `EXPLAIN (ANALYZE, BUFFERS)` against a synthetic catalog-scale
scratch dataset (100k artists, 100k releases, 500k tracks, 200k observed artists, 200k work
items). The Phase 7 native browse indexes (migration 0018) are **partial**
(`WHERE status NOT IN ('merged','deprecated')` on artists, `WHERE status = 'active'` on
releases) because the native v1 API never lists those rows. The Phase 10 admin listings
deliberately include them, so the partial indexes cannot serve admin browse paths. Measured
impact at synthetic scale:

- **A2** artist ILIKE browse: 342 ms @ 100k artists (partial trigram GIN unusable; no
  trigram index on `sort_name` at all); scales linearly.
- **I1/I2** identity unresolved queue (`observed_artists`): 325–365 ms @ 200k observations
  with a disk-spilling external merge sort (12.6 MB disk); no index on `created_at` or
  `display_value`.
- **T2** track `durationMs` sort: 153 ms @ 500k tracks (full seq scan); ≈ 3 s projected at
  the SPEC §23 10M-track reference point.
- **R1/R2/R4** release listing sorts: 49–82 ms @ 100k releases (partial browse indexes and
  partial trigram GIN unusable).
- **A1/A3** artist unfiltered sorts: 63–77 ms seq scans.
- **W2/W3** work-item unfiltered sorts: 27 ms, memory-bounded top-N (lowest priority per
  the report; only the default `created_at` sort gets an index here).

SPEC §23 requires indexed server-side pagination; the report verdict was "partially holds"
because of these gaps. A schema change requires an accepted ADR per the database rules;
this is that ADR.

Two empirical findings shaped the final index set (measured on the report's scratch
dataset before this migration was written; see the report addendum):

1. **The artist ILIKE filter is not index-servable as written.** The store's filter is
   `canonical_name ILIKE $1 OR sort_name ILIKE $1 OR EXISTS (artist_aliases …)`. An OR
   whose arms are not all index scans cannot become a BitmapOr, so full trigram GINs on
   `artists (canonical_name)` and `(sort_name)` are never selected by the planner for this
   query shape (verified: seq scan unchanged at 207 ms with both GINs present). The report
   recommended those GINs; the measurement shows they would only add write amplification.
   A2 remediation therefore requires flattening the OR/EXISTS shape — a
   `catalog_browse.rs` query change, out of this index-only package.
2. **`NULLS LAST` ordering needs a matching index ordering.** The stores sort the two
   nullable columns as `release_date DESC NULLS LAST` / `duration_ms DESC NULLS LAST` (and
   ASC NULLS LAST). A plain btree serves only ASC NULLS LAST; verified by EXPLAIN that
   `(duration_ms DESC NULLS LAST, id)` drops T2 from a 122 ms parallel seq scan to a 0.1 ms
   index-only scan. One btree cannot serve both NULLS LAST directions; DESC is both the
   measured hot path (T2/R4) and the operator-common direction (longest/newest first), so
   these two indexes are declared `DESC NULLS LAST` and the ASC variants keep the previous
   LIMIT-bounded top-N fallback.

## Decision

Add **full (non-partial) indexes** in one additive migration
(`0029_phase10_admin_browse_indexes.sql`) serving exactly the sort/filter columns used by
the Phase 10 admin stores (`catalog_browse.rs`, `identity_review.rs`, `work_items.rs`).
The existing partial indexes are kept unchanged — they remain the cheaper, smaller indexes
for the hot non-merged native paths, and PostgreSQL prefers them whenever their predicate
holds.

Indexes created (all plain `CREATE INDEX`, matching existing migration style; no table
rewrites, no constraint changes):

| Index | Definition | Serves |
|---|---|---|
| `artists_admin_browse_name_idx` | `artists (sort_name, id)` | `CatalogArtistSort::Name*` (A1) |
| `artists_admin_browse_created_idx` | `artists (created_at, id)` | `CatalogArtistSort::CreatedAt*` |
| `artists_admin_browse_updated_idx` | `artists (updated_at, id)` | `CatalogArtistSort::UpdatedAt*` (A3) |
| `releases_admin_browse_title_idx` | `releases (sort_title, id)` | `CatalogReleaseSort::Title*` (R1) |
| `releases_admin_browse_date_idx` | `releases (release_date DESC NULLS LAST, id)` | `CatalogReleaseSort::ReleaseDateDesc` (R4) |
| `releases_admin_browse_created_idx` | `releases (created_at, id)` | `CatalogReleaseSort::CreatedAt*` |
| `releases_admin_browse_updated_idx` | `releases (updated_at, id)` | `CatalogReleaseSort::UpdatedAt*` |
| `releases_admin_search_title_idx` | `releases USING gin (title gin_trgm_ops)` | release ILIKE filter (R2); mirrors 0018's expression minus the `WHERE` clause |
| `tracks_admin_browse_duration_idx` | `tracks (duration_ms DESC NULLS LAST, id)` | `CatalogTrackSort::DurationDesc` (T2) |
| `observed_artists_admin_queue_created_idx` | `observed_artists (created_at, id)` | `ObservedArtistSort::CreatedAt*` (I1) |
| `observed_artists_admin_queue_display_idx` | `observed_artists (display_value, id)` | `ObservedArtistSort::DisplayValue*` (I2) |
| `release_work_items_admin_browse_created_idx` | `release_work_items (created_at, id)` | `WorkItemSort::AddedAt*` default sort and deep paging (W1/W1b/W5) |
| `artist_match_decisions_observed_outcome_idx` | `artist_match_decisions (observed_artist_id, outcome)` | identity queue `pending_match_count` subplan filter (I1/I2) — the report's §4 item 2 "also consider", promoted after measurement showed the subplan dominates once the sort is index-driven (4.07 ms → 0.027 ms per row; I1 page 242 ms → 3.5 ms) |

Every btree carries the `id` tiebreaker column matching the stores'
`ORDER BY … , id` fragments so paging is fully index-ordered. All indexed columns except
`release_date`/`duration_ms` are `NOT NULL`, so those plain btrees serve both sort
directions.

Explicitly **not** in this package (recorded as follow-ups):

- **Full trigram GINs on `artists`** (report §4 item 1 recommendation): measured to be
  never selected for the OR'd ILIKE shape (finding 1 above). The artist ILIKE remediation
  is folded into the query-rewrite follow-up below. (A2 still improves 342 ms → 115 ms
  because the new `sort_name` btree turns the plan into an ordered index scan that
  early-outs after 50 matches, but the ILIKE filter itself stays a per-row evaluation.)
- The release `artistId` EXISTS-rewrite (report §4 item 6) and the artist ILIKE
  OR/EXISTS-flattening: `catalog_browse.rs` query changes, not indexes.
- `release_work_items (updated_at, id)` / `(attempt_count, id)` (W2/W3): 27 ms bounded
  top-N today; work-item volume is bounded by ingest throughput, not catalog size.

### Rollback / forward plan

Project convention: migrations are forward-only; rollback is a documented manual statement
set, not a down-migration. These indexes are additive and non-unique — dropping them
restores the pre-migration plan shapes exactly, with no data impact. ADR-0166 supersedes the
original online-operation claim: execute even this manual rollback only inside the maintenance
fence after stopping Melodee writers. Rollback statements (any order):

```sql
DROP INDEX IF EXISTS artists_admin_browse_name_idx;
DROP INDEX IF EXISTS artists_admin_browse_created_idx;
DROP INDEX IF EXISTS artists_admin_browse_updated_idx;
DROP INDEX IF EXISTS releases_admin_browse_title_idx;
DROP INDEX IF EXISTS releases_admin_browse_date_idx;
DROP INDEX IF EXISTS releases_admin_browse_created_idx;
DROP INDEX IF EXISTS releases_admin_browse_updated_idx;
DROP INDEX IF EXISTS releases_admin_search_title_idx;
DROP INDEX IF EXISTS tracks_admin_browse_duration_idx;
DROP INDEX IF EXISTS observed_artists_admin_queue_created_idx;
DROP INDEX IF EXISTS observed_artists_admin_queue_display_idx;
DROP INDEX IF EXISTS release_work_items_admin_browse_created_idx;
DROP INDEX IF EXISTS artist_match_decisions_observed_outcome_idx;
```

Forward: `sqlx migrate` applies 0029 in the normal ordered flow. The original operational
guidance here incorrectly described ordinary `CREATE INDEX` as allowing writes and suggested
manual concurrent application. ADR-0166 supersedes that guidance: 0029 is a
maintenance-fenced migration, every Melodee writer must be stopped, and operators must not
diverge migration-owned indexes from SQLx history through ad-hoc manual creation.

## Alternatives considered

### Rewrite the admin queries to satisfy the existing partial indexes

Rejected. The admin listings deliberately include merged/deprecated rows (that is their
purpose — operators remediate those rows). Rewriting queries to add
`status NOT IN ('merged','deprecated')` would silently drop exactly the rows the admin UI
exists to manage, changing user-visible behavior. A `UNION ALL` of status-filtered and
unfiltered halves to exploit the partial indexes would double query complexity in three
stores for planner-shaped, not product-shaped, reasons.

### Do nothing and accept the latency

Rejected. Bounded memory holds (top-N sorts), so this was a real option, but A2 (342 ms),
I1/I2 (325–365 ms with disk spill), and T2 (153 ms) degrade linearly with catalog growth
and are the first paths to breach interactive latency at 10× scale (report §10). The
indexes are cheap insurance added before Phase 10 exit rather than after an incident.

### Replace the partial indexes with full ones

Rejected. The partial indexes are strictly smaller and cheaper to maintain for the hot
native-v1 paths (which never touch merged/deprecated rows). Keeping both sets lets the
planner pick the partial index whenever its predicate holds.

### Add the report's full artist trigram GINs despite the OR shape

Rejected on measurement. With both full GINs present the planner still seq-scans
`artists` for the store's `OR … EXISTS` filter (finding 1): the indexes would cost write
amplification on an ingest-written table while serving no query. Revisit together with the
query-flattening rewrite.

### Dual-direction indexes for the nullable sort columns

Rejected for size/write cost on catalog-scale tables (a second `tracks` btree is hundreds
of MB at the 10M-track SPEC §23 reference point). The `DESC NULLS LAST` direction covers
the measured hot path; ASC keeps the bounded top-N fallback (memory-bounded per SPEC §23).

## Rationale

Index-only remediation keeps this package free of store/query rewrites (frozen during the
follow-up) and fixes the plan shapes at their root cause: the admin product surface is
unfiltered by design, so its indexes must be unfiltered too. Matching each store's exact
`ORDER BY … , id` tiebreaker and the 0018 trigram expression keeps the change mechanical
and auditable against the stores' constant SQL fragments, and every index in the set was
verified by EXPLAIN to be selected for the query it targets.

## Consequences

### Positive

- Admin browse/queue paths become index-ordered: the disk-spilling identity-queue sort
  (I1/I2: 325–365 ms → 2–4 ms), the track duration seq scan (T2: 153 ms → ~1 ms),
  the release ILIKE filter (R2: 49 ms → ~11 ms), and the artist/release unfiltered sorts
  (A1/A3/R1/R4: 63–82 ms → 0.3–10 ms) are eliminated or reduced by one to two orders of
  magnitude (exact before/after numbers in the report addendum).
- No query, DTO, route, or behavior change; invisible to clients except latency.
- Partial native indexes untouched; native-v1 plan shapes are unchanged (the planner still
  prefers the smaller partial indexes for status-filtered queries).

### Negative / trade-offs

- Write amplification: one new GIN trigram index on `releases` (an ingest-written table)
  plus twelve btrees. Ingest write cost rises measurably; accepted because admin browse
  latency is a Phase 10 product requirement and ingest is throughput-bound, not
  latency-bound.
- The artist ILIKE browse (A2) remains filter-per-row (the top gap in the report) until
  the query-flattening follow-up lands; it improves 342 ms → 115 ms from the ordered
  `sort_name` scan early-out but is not trigram-backed.
- `CatalogTrackSort::DurationAsc` and `CatalogReleaseSort::ReleaseDateAsc` keep the
  previous LIMIT-bounded top-N sort (see finding 2 and the dual-direction alternative).
- Additional planner choice may shift plans for existing queries; mitigated by keeping the
  partial indexes so native paths retain their preferred access plans.

### Follow-up work

- `catalog_browse.rs` query-shape changes: release `artistId` EXISTS-rewrite (report §4
  item 6) and artist ILIKE OR/EXISTS flattening (A2; this ADR's finding 1) — one package
  with its own compatibility/perf evidence.
- Work-item `updated_at`/`attempt_count` indexes if W2/W3 become hot at real ingest
  volumes.

## Validation

- PostgreSQL data-integrity tests (`crates/melodee-persistence/tests/migrations.rs`):
  the migration applies from empty, is repeatable, and the new indexes exist as full
  (non-partial) indexes with the expected key columns and orderings; store-level browse
  tests (`catalog_browse.rs`, `review_identity_reads.rs`) continue to pass unchanged,
  proving result correctness is unaffected.
- `EXPLAIN (ANALYZE, BUFFERS)` before/after on the report's synthetic-scale scratch
  dataset (`melodee_perf_scratch`, harness `perf_seed.sql` / `perf_explain.sql`),
  recorded as an addendum in
  performance-report.md (historical artifact removed) — including the two
  findings above that narrowed the index set relative to the report's §4 candidates.
- Full workspace gates: `cargo fmt --check`, `clippy -D warnings`,
  `cargo test -p melodee-persistence`, `cargo test -p melodee-server` (every `TestDb`
  runs the new migration), `validate_repository.py`, `extract_requirements.py --check`.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md) §23
- performance-report.md (historical artifact removed) §4 (index gaps)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
  (catalog schema)
- Migration 0018 (`crates/melodee-persistence/migrations/0018_phase7_native_catalog_browse_indexes.sql`),
  the partial-index baseline this decision complements
