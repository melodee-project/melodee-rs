# Phase 0 Evidence — Synthetic Scale and Benchmark Baseline

**Issue:** FIRST-10-ISSUES.md Issue 9 — Synthetic scale and benchmark baseline
**Requirements:** PRD-PERF-001..005, PRD-SEARCH-006 (context)
**Baseline:** legacy commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (authoritative SHA);
server version `2.2.0`; DB migration head `20260714155936_AddDeleteSourceResidueAfterIngestSetting`
**Recorded:** 2026-08-08
**Artifacts:** `compatibility/benchmarks/datasets/tier-{a,b,c}.json` (generation timings),
`compatibility/benchmarks/results/tier-{a,b,c}-*.json` (endpoint timings + environment
metadata), `compatibility/benchmarks/SUMMARY.md`. Generators:
`scripts/generate_benchmark_dataset.py`; harness: `scripts/run_benchmarks.py`.

> These measurements are reference points, not a claim that legacy Melodee is "bad." They
> exist so the Rust implementation can set meaningful budgets and prove it does not regress.

## 1. Dataset tiers (deterministic, reproducible)

| Tier | Artists | Albums | Songs | Generation time (total) | DB rows/s (songs) |
|---|---:|---:|---:|---:|---:|
| A (developer) | 1,000 | 1,500 | 15,000 | 28.1 s (incl. migrations) | 30,600 |
| B (real-world) | 35,000 | 50,000 | 560,000 | 55.2 s | 28,580 |
| C (stress) | 100,000 | 500,000 | 5,000,000 | 252.6 s | 24,600 |

Generation is set-based SQL with md5-derived deterministic UUIDs
(`md5('bench-<kind>-<i>')::uuid`) — identical rows on every regeneration; no media files
(browse/search scale). Each tier gets a fresh database (`melodee_bench_{tier}`) with EF
migrations to the frozen head, fixture settings, and the synthetic fixture user. Storage/
database-insertion throughput is roughly linear (24.6-30.6k rows/s) — this is the Phase 0
"storage/database insertion" datapoint.

Environment (full details per result file): Fedora/Linux x86_64, PostgreSQL 17.10 (Docker),
.NET 10.0.10, ffmpeg 8.1.2, Melodee 2.2.0 Kestrel (Production, QuartzDisabled,
UseForwardedHeaders), server restarted before each matrix; cold = first call, warm = median
of remaining iterations (A/B: 5 iterations; C: 3).

## 2. Endpoint timings (warm median, ms)

| Endpoint | Tier A (15k) | Tier B (560k) | Tier C (5M) |
|---|---:|---:|---:|
| native artists list (paged) | 8.6 | 9.5 | 21.2 |
| native albums list (paged) | 11.0 | 47.6 | 172.5 |
| native songs list (paged) | 16.5 | 30.7 | 110.1 |
| native albums/recent | 7.4 | 40.7 | 138.1 |
| native search exact-artist | 29.6 | 198.6 | 1076.1 |
| native search partial/track | 45.3 / 997.8 | 194.2 / 191.6 | 1023.2 / 1042.0 |
| native search common/unicode/nonexistent | ~1000 | ~1000 | ~1024-1041 |
| OS getIndexes | 266.8 | 692.7 | 1326.3 |
| OS getArtists | 261.1 | 606.6 | 1187.2 |
| OS getAlbumList2 newest | 257.7 | 329.7 | 481.8 |
| OS search3 | 277.5 | 513.3 | 2098.0 |
| **OS getRandomSongs** | 950.8 | **24,625** | **230,343 (≈4 min)** |
| OS getStarred2 | 240.6 | 254.0 | 254.1 |
| JF userViews | 3.3 | 3.2 | 6.9 |
| JF artists list | 6.2 | 24.3 | 58.7 |
| JF items (audio, paged) | 25.1 | 259.8 | 1523.4 |
| JF items (albums, paged) | 7.2 | 39.7 | 150.2 |
| JF playlists | 5.2 | 3.5 | 4.9 |

## 3. Scalability findings (ranked)

1. **`getRandomSongs` is catastrophically non-scalable**: ~1s at 15k → ~25s at 560k →
   **~4 minutes at 5M** (superlinear). After the Tier C matrix the server process sat at
   **26.2 GB RSS (peak 29.1 GB)** — the random path materializes the catalog in memory.
   Any Subsonic client with a "random mix" feature hits this.
2. **OpenSubsonic browse/search endpoints are 10-25x slower than native equivalents** at
   the same scale (getArtists 607ms vs native artists list 9.5ms at Tier B): serialization/
   query patterns, not raw DB scale.
3. **Native global search has a ~1s floor for broad/no-result query classes** at every tier
   (common word, Unicode, nonexistent ≈ 1000ms; exact/partial grow from ~30ms (A) to ~1s (C)).
   The ~1s class is query-dependent and scale-insensitive up to B, then dominant at C —
   consistent with a fixed external/DecentDB penalty plus contains-scan cost. **Against the
   PRD-SEARCH-006 target (warm p95 < 50 ms on 10M tracks), the legacy server is ~20x off at
   5M tracks** — the primary evidence for the Tantivy projection (ADR-0108).
4. **Native paged lists scale well** (9-173ms through Tier C) — pagination + indexes work.
5. **Jellyfin paged items scale moderately** (audio items 1.5s at 5M due to the
   SongNumber-only sort (no tiebreak index); artists/albums fine).

## 4. Ingestion measurements

- Storage/database insertion (set-based synthetic): 24.6-30.6k rows/s linear (above).
- Inbound pipeline processing (Issue 8 corpus, real ffmpeg conversion + validation, offline
  search engine): **~3 s per Release** wall time (per-album processing runs; includes
  per-file ATL reads, conversion, artwork, validation). This is NOT a throughput-optimized
  figure — it establishes the per-Release cost shape. A 200-Release burst (PRD reference
  scenario) at this shape is ~10 minutes of pipeline time; crash/restart behavior is
  non-transactional (media-baseline.md §11) — the durable-work-item motivation (Phase 2).
- No-op/already-processed scan: directories with an up-to-date melodee.json and no modified
  files are skipped by mtime rules (code-verified, FileSystemDirectoryInfoExtensions.cs:303-386);
  timing at scale was not meaningfully measurable in the fixture environment and is recorded
  as a gap rather than invented.

## 5. Proposed Rust performance budgets

| Budget | Value | Status |
|---|---|---|
| Warm search p95 (10M-track catalog) | < 50 ms | **Requirement already defined** (PRD-SEARCH-006) |
| Search rebuild from PostgreSQL provable | required | **Requirement already defined** (ADR-0108, Phase 5 exit) |
| API browse p95 at 560k tracks (paged lists) | ≤ 50 ms | **Proposed target requiring ADR/PRD ratification** — legacy native already achieves ~10-48ms; Rust should not regress |
| Random-songs endpoint p95 at 560k tracks | ≤ 500 ms | **Proposed target requiring ADR/PRD ratification** — legacy is 25s; anything reasonable is a 50x improvement; must not regress |
| Server memory ceiling at 5M tracks | bounded (no full-catalog materialization; target < 4 GB) | **Proposed target requiring ADR/PRD ratification** — legacy measured 26-29 GB after random queries |
| 200-Release inbound burst | durable across restarts (no lost/duplicated work) | **Requirement already defined** (PRD.md §200-Release scenarios, PRD-PERF-004) |
| DB bulk-insert throughput for catalog rebuild | ≥ 25k rows/s | **Proposed target requiring ADR/PRD ratification** (matches measured PostgreSQL set-based inserts; relevant to Tantivy/migration rebuilds) |

## 6. Hazards for the Rust rewrite

- Never materialize catalog-scale result sets in memory (randoms, exports) — stream/sample.
- Do not give OS-adapter serialization a per-row DB access pattern (N+1) at catalog scale.
- Search must not degrade to contains-scans or fixed external penalties at 5M+ tracks.
- Keep paged-list index health (the one thing legacy does well).
- Tiebreak all list sorts (recall the Issue 6/7 nondeterministic-order findings).

## 7. Reproducing

```bash
python3 scripts/generate_benchmark_dataset.py --tier B
python3 scripts/run_benchmarks.py --tier B            # --iterations 5 (3 for C)
```

Datasets regenerate identically; timings vary with host. Result JSONs carry full environment
metadata; SUMMARY.md accumulates rows per run.
