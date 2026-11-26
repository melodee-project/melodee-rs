# Performance Remediation — Status Ledger

**Operation:** session 3 (resumed from `ca8ef83`)
**Base revision at session start:** `ca8ef83`
**Toolchain:** Rust 1.99.0 (pinned); PostgreSQL 18.3 (dev compose, port 5433)
**Host budget:** 24 vCPU, 62 GiB RAM. `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`.
**Authority:** `PERF-REMEDIATION-SPEC.md`. **Evidence:** `PERF-REMEDIATION-RESULTS.md`.

Terminal states: **IMPLEMENTED AND VERIFIED**, **ALREADY SATISFIED AND VERIFIED**,
**NO CHANGE JUSTIFIED**, **EXTERNALLY BLOCKED**. `PARTIAL`/`NOT IMPLEMENTED` are checkpoints.

## Requirement-level state (current)

| Package | State | What exists | Exact remaining action |
|---|---|---|---|
| RM-00 | PARTIAL | bounded fixed-bucket metrics; pool-acquire metric on catalog/work/search claim paths; three correctness regressions; 50K plan evidence | Build representative 560K–600K catalog + end-to-end release-build benchmarks (import, policy, media, rebuild, interactive, streaming) |
| RM-01 | IMPLEMENTED AND VERIFIED | narrow index 0060; 50K plan 0.029 ms/3 buffers vs 7.778 ms/1303 | — |
| RM-02 | IMPLEMENTED AND VERIFIED | scoped normal + dry-run acoustic queries; plan measured; novelty suites pass | — |
| RM-03 | IMPLEMENTED AND VERIFIED | genre release/surface index; status guard; Track-move term maintenance | — |
| RM-04 | IMPLEMENTED AND VERIFIED | advisory-lock recount protocol; idempotent repair CLI; concurrency regression | — |
| RM-05 | IMPLEMENTED AND VERIFIED | snapshot-visible obligations + exclusive activation fence; two regressions incl. real old-generation consumer | — |
| RM-06 | IMPLEMENTED AND VERIFIED | durable per-class discovery cursors; 100K fairness regression | — |
| RM-07 | IMPLEMENTED AND VERIFIED | bounded 64-entry scheduler result ring; 10k-tick test | — |
| RM-08 | IMPLEMENTED AND VERIFIED | full-batch drain; monotonic external maintenance gate; durable last_prune_at | — |
| RM-09 | PARTIAL | durable seeds persisted atomically; **explicit captured-id deletion (fixes the commit-order/watermark bug)**; bounded drain batch; fixed-point iterations retain seeds on exhaustion; 30-min Artist+Release backstop; oracle equivalence by construction; seed/idempotency/late-commit tests | Scope candidate discovery to the seed set and its transitive closure instead of the catalog-wide algorithms per coalesced pass |
| RM-10 | PARTIAL | two-phase manifest with fenced revalidation + in-transaction fallback; pool-acquire instrumentation | Measured background/interactive competition and an evidence-based admission/pool decision |
| RM-11 | PARTIAL | tool-version cache (3 subprocess probes/album avoided) | Eliminate provably redundant content reads/hashes/copies with integrity/failure-injection tests |
| RM-12 | PARTIAL | `inspection_concurrency` now governs actual concurrent execution (bounded, per-item lease heartbeats); in-flight bounded by claim batch | Shared process-wide CPU/FS/subprocess admission beyond per-stage bounds; measured concurrency tuning |
| RM-13 | PARTIAL | bounded verified reader registry (reload-on-query); ordered batch hydration; batch-read fallback | Measure reader/writer overhead; long-lived writer evidence-based decision |
| RM-14 | IMPLEMENTED (measurement outstanding) | fresh-generation bulk insert; per-page blocking work; document+time commit bounds | Representative production rebuild timing/commit count |
| RM-15 | PARTIAL | partial leading artist-rating index | Measure and optimize admin stats/OpenSubsonic/native/ACL; preserve wire contracts |
| RM-16 | IMPLEMENTED AND VERIFIED | ADR-0215 opt-in bounded lease-event retention; duplicate indexes dropped | — |
| RM-17 | PARTIAL | blacklist singleflight; byte-bounded artwork cache **keyed by source content hash** with replacement test | One-reader-per-stream, combined auth lookup, rate-limit contention measurement |
| RM-18 | PARTIAL | all evidence batching; sorted-key advisory lock helper | Recommendation/play-trigger contention measurement |
| RM-19 | PARTIAL | set-based merge/split correctness | 1K (and resource-safe 10K) merge/split timing |
| RM-20 | PARTIAL | runbook/docs updated; fresh + populated upgrade verified; `make check`/`make check-nightly` pass | Final closeout only after the remaining mandatory items |

## Migrations

`0060` indexes/duplicate drops · `0061` genre correctness/repair · `0062` discovery progress ·
`0063` search activation replay · `0064` identity seeds + sorted lock helper.

## Verification (session 3)

- `catalog_importer`: 9 PASS, including new late-lower-id seed retention test.
- `melodee-search rebuild_integration` (test-support): 13 PASS with commit bounds.
- `melodee-ingestion inspection` (5) + `environment_faults` (7): PASS.
- `melodee-server` media unit tests (3) incl. artwork replacement invalidation: PASS.
- Workspace fmt/clippy: PASS.
- `make check-nightly` with `RUST_TEST_THREADS=1`: PASS. (One pre-existing podcast lease test is
  timing-sensitive to parallel in-binary tests; it passes isolated/serialized and is unrelated.)

## Remaining mandatory work (not complete)

RM-00 representative benchmark; RM-09 scoped closure; RM-10 competition measurement;
RM-11 content-read reduction; RM-12 shared admission/measured tuning; RM-13 writer evidence;
RM-15 route optimization; RM-17 streaming/auth/rate-limit; RM-18/RM-19 measurements.
