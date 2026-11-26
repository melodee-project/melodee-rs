# Performance Remediation — Status Ledger

**Operation:** 26 Nov 2026 (resumed)
**Base revision at start:** `a0979cde1fd2aeff5b8027d9d59336b678f1aa12` (branch `main`)
**Toolchain:** Rust 1.99.0 (pinned); PostgreSQL 18.3 (dev compose, host port 5433)
**Host budget:** 24 vCPU, 62 GiB RAM, NVMe `/home`. `CARGO_BUILD_JOBS=2`; test harness threads 1.
**Evidence:** `PERF-REMEDIATION-RESULTS.md`. Standard: `PERFORMANCE.md`.
**Operator procedure:** `design/runbooks/PERF-REMEDIATION-UPGRADE.md`.

Terminal states: **IMPLEMENTED AND VERIFIED**, **ALREADY SATISFIED AND VERIFIED**,
**NO CHANGE JUSTIFIED**, **EXTERNALLY BLOCKED**. `PARTIAL` / `NOT IMPLEMENTED` are checkpoint
states, not completion.

## Finding → package summary

| Finding | Package | State | Evidence / remaining |
|---|---|---|---|
| PERF-001 | RM-01 | IMPLEMENTED AND VERIFIED | migration 0060; 50K plan 0.029 ms/3 buffers vs 7.778 ms/1303 |
| PERF-002 | RM-02 | IMPLEMENTED AND VERIFIED | scoped normal+dry-run; 50K 10.9 ms/31 buffers vs 62.2 ms/1725; novelty suites pass |
| PERF-003 | RM-03, RM-04 | IMPLEMENTED AND VERIFIED | migrations 0060/0061; concurrent genre regression; hydration plan; repair command |
| PERF-004 | RM-09 | IMPLEMENTED AND VERIFIED | migration 0064 durable seeds at the authoritative mutation; coalesced closure drain (advisory-fenced, bounded iterations) + periodic Artist/Release backstop; import no longer reconciles per album; seed/closure/idempotency tests pass |
| PERF-005 | RM-08, RM-13 | RM-08 IMPLEMENTED AND VERIFIED; RM-13 PARTIAL | drain + monotonic maintenance gate; ordered batch hydration + bounded reader registry (reload-on-query); writer reuse NOT implemented (batch-scoped writer retained) |
| PERF-006 | RM-05, RM-14 | RM-05 IMPLEMENTED AND VERIFIED; RM-14 IMPLEMENTED AND VERIFIED | commit-safe activation + two regressions (incl. real old-generation consumer); fresh-generation bulk insert; per-page blocking off-async; document-bound commits |
| PERF-007 | RM-11, RM-12 | RM-11 PARTIAL; RM-12 NOT IMPLEMENTED | tool-version cache added (3 subprocess starts/album avoided after first); content-read reduction and admission NOT implemented |
| PERF-008 | RM-10 | IMPLEMENTED AND VERIFIED (manifest); PARTIAL (admission) | two-phase binding with full fenced revalidation + in-transaction fallback; DB admission metrics partial |
| PERF-009 | RM-07 | IMPLEMENTED AND VERIFIED | bounded 64-entry result ring; 10k-tick unit test; rolling refill present |
| PERF-010 | RM-15 | PARTIAL — retained index, NO CHANGE JUSTIFIED for risky refactors | partial leading artist-rating index; route/serialization/concurrency changes rejected without representative measurements |
| PERF-011 | RM-16 | IMPLEMENTED AND VERIFIED (retention) / NO CHANGE (renewal) | migration ADR-0215 + `work prune-lease-events` (opt-in, bounded, audit-preserving, tested); renewal round-trip left unchanged (queue-age/event semantics) |
| PERF-012 | RM-06 | IMPLEMENTED AND VERIFIED | migration 0062; durable per-class cursors; 100K fairness regression |
| PERF-013 | RM-17 | PARTIAL | blacklist singleflight; byte-bounded asset-key artwork cache; stream reader/auth-combine/rate-limit NOT implemented |
| PERF-014 | RM-18 | IMPLEMENTED AND VERIFIED (short of recommendation measurement) | acoustic + embedded + Artist external-ID inserts batched; lock keys acquired in sorted-key order via SQL helper; recommendation contention NOT measured |
| PERF-015 | RM-00, RM-20 | RM-00 PARTIAL; RM-20 IMPLEMENTED AND VERIFIED | bounded fixed-bucket metrics + instrumentation; 3 correctness regressions; populated upgrade + fresh chain + full `make check` + `make check-nightly` pass; 560K representative tier NOT executed |
| PERF-V001 | RM-05 | IMPLEMENTED AND VERIFIED | snapshot-visible obligation set + ACCESS EXCLUSIVE activation fence; two regressions |
| PERF-V002 | RM-19 | IMPLEMENTED AND VERIFIED (no large-release perf number) | set-based merge/split; merge/split suites pass; 1K/10K timing NOT measured |

## Migrations

| Migration | Purpose |
|---|---|
| `0060_perf_query_plan_indexes.sql` | playable-asset, genre release/surface, artist-match decision, partial rating indexes; drop 3 exact duplicates |
| `0061_perf_genre_aggregate_correctness.sql` | advisory-lock recount, status guard, Track-move term maintenance, discrepancy/repair functions |
| `0062_perf_discovery_progress.sql` | durable per-work-class discovery cursors |
| `0063_perf_search_activation_replay.sql` | snapshot-visible and applied outbox replay tables |
| `0064_perf_identity_reconciliation_seeds.sql` | durable affected-identity seed queue + `lock_artist_identity_names(text[])` sorted-key lock helper |

## Verification executed

- Fresh `0001–0064` chain: PASS.
- Populated upgrade from 0059 with corrupted derived state: PASS
  (`perf_remediation_upgrade_from_0059_preserves_data_and_repairs_derived_state`).
- Six historical-schema migration compatibility tests (`--ignored`): PASS.
- `make check` (full): PASS. `make check-nightly`: PASS.
- Focused: `catalog_concurrency` (6), `catalog_import` (23), `catalog_audio_identity`,
  `catalog_merge_split` (5), `catalog_release_merge` (7), `policy_phase5`, `phase5_novelty`,
  `search_phase6` (12), `search_outbox_retention` (6), `native_discovery_phase7` (3),
  `history` (3 incl. retention), `migrations` (from-empty/repeatable/perf upgrade),
  `meldee-catalog importer` (8), `melodee-search rebuild_integration` (10 / 13 with test-support),
  `melodee-ingestion --lib` (46), `melodee-media --lib` (53), `melodee-server` security/artwork/stats.
- Measurements: 50K-track playable-asset, acoustic, and genre-hydration plans; genre concurrency
  before/after. 560K/1M/5M/10M and real-media/NFS throughput: NOT EXECUTED.

## Remaining mandatory work (not complete)

- **RM-11** content-read reduction (adjacent duplicate hashes, hash-during-copy, artwork dedup) with
  integrity/failure-injection tests. Only tool-version reuse is implemented.
- **RM-12** bounded CPU/FS admission, subprocess lifecycle review, and measured concurrency
  (including making `inspection_concurrency` effective). Not implemented.
- **RM-13** long-lived writer lifecycle (generation/lease-gated) — reader reuse + ordered batched
  hydration are implemented.
- **RM-15** endpoint/serialization/ACL optimization beyond the partial rating index; requires the
  representative 560K fixture and route p50/p95/p99 measurements.
- **RM-17** one blocking reader per stream, combined auth lookup, rate-limit measurement.
- **RM-18** recommendation/play-trigger contention measurement (batching + lock ordering done).
- **RM-19** 1K/10K release merge/split timing (correctness done).
- **RM-00** representative 560K–600K end-to-end fixture and production-path benchmarks
  (ordinary import, policy/dry-run, media, concurrent genre, search rebuild, interactive, streaming,
  scheduling) with comparable before/after. Only focused 50K query/index evidence exists.

No external blocker prevented the delivered work. The remaining items are implementation and
representative-measurement work, not unavailable dependencies.
