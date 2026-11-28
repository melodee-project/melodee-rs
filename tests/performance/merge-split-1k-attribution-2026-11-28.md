# 1K merge/split cost attribution, 2026-11-28

Work order WO-M from `PERFORMANCE-REFACTOR-TASKS.md` §10.4.

**Question.** Why did a 1,000-Track Release merge go from 633.7 ms (baseline
`a0979cd`, 3,025 statements) to ~3.5 s (final, 36 statements), and how much of
the increase is necessary correctness maintenance versus avoidable overhead?

**Answer.** At 1K on the 600K-Track fixture, ~86–94 % of the elapsed time is the
Track-move statement's genre-aggregate maintenance, which the baseline did not
perform at all (it left 2,000 stale genre-term rows). The movement of 1,000
Tracks adds only about 0.13 ms per Track. No duplicated work was found in the
move path. One bounded, avoidable follow-up exists: no index leads with
`catalog_genre_terms(surface, display_value)`, so each genre recount scans the
whole `surface` partition instead of only the affected key.

## Provenance

| Item | Value |
| --- | --- |
| Source | `main` at `080a619` plus the WO-M phase-span instrumentation and the benchmark debug layer |
| Benchmark | `crates/melodee-persistence/tests/perf_large_release_merge.rs` (release build, `--ignored`) |
| Fixture | `scripts/perf_representative.py` with `--quiesce-fixture`; per-invocation UUID database |
| Phase run | 35,000 Artists / 50,000 Releases / 600,000 Tracks (`MELODEE_PERF_MERGE_TRACKS=1000`) |
| Trigger run | 3,500 / 5,000 / 60,000 Tracks, same 1,000-Track move |
| Environment | Rust 1.99.0 release profile, `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`, PostgreSQL 18.3 on port 5433, one workload at a time |
| Raw logs | session logs (not committed): `merge-phase-1k.log`, `merge-trigger-attribution.log` |

Each case used a fresh invocation-owned database with uncontrolled post-seed
cache. Lock probes are sampled every 5 ms. Statement counts exclude
`BEGIN`/trigger-internal statements. These are single samples, not a distribution.

## Phase breakdown (600K fixture, 1K merge)

`operation=merge elapsed_ms=3370.160 sqlx_query_events=36`

| Phase | Elapsed | Share |
| --- | --- | --- |
| `move_tracks` (Track `UPDATE` plus its triggers) | 3,183.0 ms | 94.4 % |
| `assets` | 39.8 ms | 1.2 % |
| `outbox` (Release + Track revisions) | 15.1 ms | 0.4 % |
| `evidence` (immutable merge event JSON) | 7.1 ms | 0.2 % |
| `aggregates` | 5.9 ms | 0.2 % |

The slow-statement stream isolates the fixed component: the 1,000-row
`UPDATE tracks ... release_id` took 3.179 s, while a **single-row** Track move
in the same fixture took 1.88 s. Split has the same shape
(`operation=split elapsed_ms=6621.299`; the 1,000-row restore `UPDATE` is
3.538 s and `UPDATE releases SET status='active'` is 2.963 s).

## Trigger attribution (60K fixture, 1,000-Track move, cache-warm run)

The same move was timed with triggers selectively disabled **only in the
disposable owned database**, then re-enabled and verified. This does not change
committed behavior.

| Configuration | Elapsed |
| --- | --- |
| All triggers | 423.3 ms |
| Statement-level genre maintenance disabled | 139.0 ms |
| Genre + recommendation statement triggers disabled | 129.0 ms |

Derived: genre maintenance ≈ 284 ms (67 %); recommendation statement trigger
≈ 10 ms; the remaining ≈ 129 ms is the row-level filename trigger plus the
actual update (≈ 0.13 ms per moved Track). Fixture at that point: 60,000 Tracks,
120,000 genre terms, 84,000 `Rock` terms, 0 plays.

## Interpretation

- The move path's cost is dominated by
  `maintain_catalog_genre_terms_on_track_move()` →
  `refresh_catalog_genre_aggregate_keys()`, introduced by the RM-03/RM-04
  correctness fix. The baseline did no such maintenance.
- The recount joins `catalog_genre_terms` on `surface` and `display_value`.
  Existing indexes lead with `surface` (or `track_id`/`release_id`); none leads
  with both predicate columns. The planner therefore scans the whole
  `surface='native'` / `'opensubsonic'` partition per affected key. That is why
  even a one-row move is O(catalog-surface), not O(moved Tracks).
- The recount-by-key protocol (deterministic advisory locks, then a recount that
  observes committed concurrent work) is the accepted RM-04 design. Replacing it
  with a delta update would be a correctness-protocol change and was **not**
  done.
- The one avoidable item is a predicate-aligned index
  `catalog_genre_terms (surface, display_value)`. It cannot remove the dominant
  `Rock`-key scan (that key spans roughly 70 % of the fixture catalog) but it
  removes the whole-partition scan for rare keys and narrows the common-key
  scan. It requires its own additive migration, populated-upgrade check and
  plan regression; it is a follow-up work order, not a regression fix.

## Limitations

- One sample per configuration; cache state is uncontrolled across cases.
- The 1K and 10K final numbers come from separate fresh fixtures, so their
  slope is indicative only.
- Trigger-disabled timings are a diagnostic, not a proposed production mode.
- The recount's own query plan was inferred from index definitions and the
  measured scaling; a captured `EXPLAIN (ANALYZE, BUFFERS)` of the recount is
  still recommended as part of the index follow-up.
