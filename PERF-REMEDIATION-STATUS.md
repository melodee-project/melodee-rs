# Performance Remediation — Status Ledger

**Operation:** 26 Nov 2026
**Base revision:** `a0979cde1fd2aeff5b8027d9d59336b678f1aa12` (branch `main`)
**Toolchain:** Rust 1.99.0 (pinned); PostgreSQL 18.3 (dev compose, host port 5433)
**Host budget:** 24 vCPU, 62 GiB RAM, NVMe `/home` 552 GiB free. `CARGO_BUILD_JOBS=2`,
`RUST_TEST_THREADS=1` for test harnesses.

Terminal states: **IMPLEMENTED AND VERIFIED**, **ALREADY SATISFIED AND VERIFIED**,
**NO CHANGE JUSTIFIED**, **EXTERNALLY BLOCKED**. `IN PROGRESS` / `PARTIAL` / `NOT IMPLEMENTED`
mean unfinished work and are not completion claims. Full evidence: `PERF-REMEDIATION-RESULTS.md`.

## Finding → package summary

| Finding | Package | State | Durable evidence / next action |
|---|---|---|---|
| PERF-001 | RM-01 | IMPLEMENTED AND VERIFIED | migration 0060; 50K plan 0.029 ms / 3 buffers vs 7.778 ms / 1303 buffers |
| PERF-002 | RM-02 | IMPLEMENTED (verification pending final gate) | scoped normal + dry-run queries; 50K 10.9 ms / 31 buffers vs 62.2 ms / 1725 |
| PERF-003 | RM-03, RM-04 | IMPLEMENTED AND VERIFIED | migration 0060/0061; concurrency regression passes; hydration plan measured |
| PERF-004 | RM-09 | PARTIAL | index 0060, idempotent-import skip, startup Artist+Release backstops; affected-closure scoping not implemented |
| PERF-005 | RM-08, RM-13 | PARTIAL | full-batch drain + monotonic maintenance gate; ordered obligation replay; reader/writer reuse not implemented |
| PERF-006 | RM-05, RM-14 | RM-05 IMPLEMENTED (final gate); RM-14 PARTIAL | commit-safe activation protocol + regression; fresh-generation bulk insert; off-async/commit tuning not done |
| PERF-007 | RM-11, RM-12 | NOT IMPLEMENTED | no behavior changed; requires operation inventory + integrity boundaries |
| PERF-008 | RM-10 | NOT IMPLEMENTED | TOCTOU-safe two-phase binding not designed/implemented |
| PERF-009 | RM-07 | IMPLEMENTED AND VERIFIED | bounded 64-entry result ring + 10,000-tick unit test; rolling refill already present |
| PERF-010 | RM-15 | PARTIAL | partial leading rating index; endpoint/serialization/ACL work not done |
| PERF-011 | RM-16 | PARTIAL | three exact duplicate indexes dropped; lease renewal and history retention unchanged (no retention policy) |
| PERF-012 | RM-06 | IMPLEMENTED AND VERIFIED | migration 0062; durable per-class cursors; 100K fairness regression passes |
| PERF-013 | RM-17 | PARTIAL | blacklist singleflight; stream reader/auth combine/artwork cache not done |
| PERF-014 | RM-18 | IMPLEMENTED (final gate) | acoustic + embedded evidence batched; external-ID loops and advisory-lock audit not done |
| PERF-015 | RM-00, RM-20 | RM-20 IMPLEMENTED; RM-00 PARTIAL | PERFORMANCE.md + AGENTS.md + runbook + docs; 3 permanent regressions; bounded histograms and 560K tier not executed |
| PERF-V001 | RM-05 | IMPLEMENTED (final gate) | snapshot-visible obligation set + ACCESS EXCLUSIVE activation fence + regression |
| PERF-V002 | RM-19 | IMPLEMENTED (final gate) | set-based merge/split; existing merge/split suites |

## Migration inventory

| Migration | Purpose |
|---|---|
| `0060_perf_query_plan_indexes.sql` | playable-asset, genre release/surface, artist-match decision, partial rating indexes; drop 3 exact duplicates |
| `0061_perf_genre_aggregate_correctness.sql` | advisory-lock recount protocol, status guard, Track-move term maintenance, discrepancy/repair functions |
| `0062_perf_discovery_progress.sql` | durable per-work-class discovery cursors |
| `0063_perf_search_activation_replay.sql` | snapshot-visible and applied outbox replay tables |

All four apply cleanly on a fresh `0001`–`0063` chain (executed in this run). Populated-upgrade
rehearsal: NOT EXECUTED.

## Final gate (deferred to end checks per operator instruction)

Focused end-check tests executed (all PASS):

- `melodee-search --test rebuild_integration` (10, includes PERF-V001 regression)
- `melodee-persistence --test search_phase6` (12)
- `--test catalog_merge_split` (5), `--test catalog_release_merge` (7)
- `--test catalog_import` (23), `--test catalog_audio_identity` (1)
- `--test catalog_concurrency` (6, includes the genre regression)
- `--test policy_phase5 --test phase5_novelty` (4)
- `--test search_outbox_retention` (6)
- `--test native_discovery_phase7` (fairness regression)
- `melodee-ingestion --lib` (46), `melodee-server --lib security` (7)
- `--test migrations` filtered to from-empty and repeatable

Still not executed (deferred as repository-wide / long gates):

- `make check` (full), `make check-nightly`
- Populated-upgrade and interrupted-operation rehearsal
- 560K representative tier, 1M/5M/10M, soak, real NFS
- Admin UI gates (`make audit-live`), E2E/visual

A historical-schema search test (`migration_0045_refreshes_artist_status_in_an_existing_generation`)
builds a schema truncated at migration 0044 and runs current worker code; the new replay tables are
required there, so the fixture now includes migrations ≥0060 (they are independent of 0045). This
is a test-fixture correction, not a production compatibility claim: the application has never
supported running against a schema older than its embedded migration set.

## Preserved / rejected remedies

See `PERF-REMEDIATION-RESULTS.md` §6. Notably retained: consistent snapshot + commit-safe
obligation replay + exclusive activation fence. Notably rejected: read-committed `max(id)` rebuild,
metadata-for-content, password cache, naive rate-limit upsert, blanket index removal, universal
`-threads 1`, unbounded `spawn_blocking`, asynchronous recommendations.
