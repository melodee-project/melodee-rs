# Final integrated performance matrix, 2026-11-27/28

This document records the representative-scale measurement round executed on the
final integrated sources. All thirteen workloads required by RM-00 now have
final-source production-path measurements. Four are **matched baseline/final**
pairs; one has a matched pair for the media stage; the rest are final-only or
sampled for reasons stated per row. Evidence class is labelled for every row and
nothing here is a target, an estimate, or an extrapolation.

Fixture construction time is reported separately and is never presented as
import or application throughput.

## 1. Source, binary and harness provenance

| Item | Value |
| --- | --- |
| Final source commit | `bdfe774` (measurements before the retry-selector edit: `52436f5`; test/harness-only changes since `554e9d5`) |
| Final CLI proofs | `final-cli-build-2026-11-28c.json` (all runs through db competition), `...-28d.json` (retry run) |
| Final server build | `final-server-build-2026-11-28.json`, sha256 `85f734a4ca79292e…` |
| Baseline commit | `a0979cde1fd2aeff5b8027d9d59336b678f1aa12` in `../melodee-rs-perf-baseline-a0979cd` |
| Baseline CLI proofs | `final-baseline-cli-build-2026-11-27.json` (rebuild), `...-27b.json` (policy), `...-28c.json` (mixed load) |
| Baseline server build | `final-baseline-server-build-2026-11-28.json`, sha256 `98e48029c573ce36…` |
| Codec matrix executables | final `40f576d013da7c64…`, baseline `12bcba92e9763173…` |
| Merge/split benchmark | `perf_large_release_merge` (final only; the baseline has no equivalent target) |
| Harness | `scripts/perf_representative.py` sha256 `029d125041331286…` (same content copied into the baseline worktree) |

Toolchain Rust 1.99.0 (pinned), release profile, `CARGO_BUILD_JOBS=2`,
`RUST_TEST_THREADS=1`. Host: AMD Ryzen 9 3900X, 24 logical CPUs, 62 GiB RAM,
NVMe-backed btrfs (media matrix used tmpfs). PostgreSQL 18.3 from
`compose.dev.yaml` (`shared_buffers=128MB`, `work_mem=4MB`, `max_connections=100`),
read from the database inside the measured runs. One Cargo/test workload ran at
a time and no competing Cargo process was active. Cache state was uncontrolled
after each fixture seed; every case used a fresh invocation-owned database.

Base fixture (all database cases): 35,000 Artists, 50,000 Releases, 600,000
Tracks, 600,000 synthetic acoustic evidence rows, skewed genre terms, ratings
1–5, ACL groups/libraries, alternate asset links, one synthetic artwork object.
It is a database/media-reference fixture, not a 600K-file audio corpus.

**Fixture variant required by ingestion-driven workloads.** The ingestion
workload refuses a fixture that already contains work items, and the ACL
publication placeholders create `release_work_items`. Every ingestion-driven run
below (single/200-album, re-drop, genre scheduler, streaming, mixed load) used
`--without-acl-placeholders`, which omits only that synthetic ACL block. Policy
and identity use the same option; the outbox and rebuild cases use the default
fixture.

## 2. Results by workload

### 2.1 Single-album import (measured)

One generated five-second MP3 Release through the real workers, two workers per
stage, `--operation-metrics`.

- End-to-end 7.148 s to `cataloged`, including worker polling; corpus 40,682 B.
- Media operations observed exactly once each where expected: acquisition tree
  hash 6, media file hash 4, validation hash 1, media copy 1, publication copy
  1, publication hash 4, ffprobe 3, ffmpeg 1, fpcalc 1, tool-version probe 3.
  `complete=true`, no required label missing.
- Search, policy, discovery, enrichment and podcast workers disabled; no
  interactive load; cache uncontrolled.

### 2.2 200-album import (measured)

- 110.069 s for 200 cataloged Releases (alternating MP3/FLAC), 109.02
  albums/min, 1.817 Tracks/s, corpus 12,587,640 B, two workers per stage.
- No failures, no timeout, graceful stop.

### 2.3 Unchanged reimport (measured: idempotent retry, plus non-idempotent re-drop finding)

Two distinct operations were measured, and they behave differently by design:

- **Same work item re-delivered** (`perf_catalog_retry`, 50 samples on the
  cataloged occurrence): p50 3,045 µs, p95 3,977 µs, max 7,624 µs;
  `inserted=false` every time, same Release and Track IDs, no new
  `catalog_imports` rows, and the identity-reconciliation seed set exactly
  unchanged. This is the avoided-work/idempotency path.
- **Identical bytes re-dropped as a new occurrence** (`--redrop`, one album):
  total 13.285 s with the second pass at 6.211 s, both cataloged, but every
  media operation repeats (2 ffmpeg, 2 fpcalc, 8 media file hashes, 2
  publication copies) and the Release identifiers are **not** unchanged. Per
  ADR-0116 a distinct occurrence receives a distinct Release, so this is a
  duplicate import, not an unchanged reimport; it is recorded so the row is not
  misread as evidence of media-work avoidance.

### 2.4 Policy evaluation and dry-run (matched baseline/final)

Three timed iterations per case per side after untimed samples; one injected
cross-Release acoustic match; 600K background Tracks.

| Metric | Baseline | Final |
| --- | --- | --- |
| Single Release evaluate (3 samples) | 1,229.6 / 1,203.0 / 1,278.9 ms | 8.21 / 9.66 / 8.54 ms |
| SQL rows returned | 600,014 | 7 |
| Retained acoustic digests / bytes | 600,008 / 38,400,512 | 1 / 64 |
| Bounded 8-Release dry-run (3 samples) | 16,363.8 / 15,806.8 / 16,313.5 ms | 396.4 / 374.7 / 382.3 ms |
| Dry-run retained digests / bytes | 4,800,057 / 307,203,648 | 1 / 64 |

SQL statements are 13 (single) and 3 (dry-run) on both sides; the scoped query
returns the same cross-Release match. Evidence:
`final-search-rebuild-*`, policy logs summarized here; CLI proof
`...-27e`/baseline `...-27b`.

### 2.5 Identity reconciliation (final-only)

The pre-remediation revision has no scoped candidate methods, so the harness
cannot run against it. Augmented fixture: 650,004 observations, 601,002 Track
credits, 11,087 Artist and 16,668 Release external IDs; 8 durable seeds.

| Candidate query | Scoped ms | Global ms |
| --- | ---: | ---: |
| Strong Artist | 6.530 | 59.718 |
| Same-Release Track Artist | 15.775 | 2,963.826 |
| Repeated Artist credit | 6.322 | 1,365.018 |
| Strong Release | 2.238 | 17.901 |
| Name proposal | 5.682 | 718.751 |

Drain: one bounded turn 3,188.3 ms (3 Artist merges, 1 Release merge, 68
follow-ups); closure quiescent after one further 32.1 ms turn. Evidence:
`final-identity-600k-2026-11-27v2.json`.

### 2.6 Media processing (matched baseline/final, stage workload)

Four codecs × one/two/four concurrent Release workers × two samples, 60 s mono
source, unmodified production FFmpeg options and subprocess handling;
normalization timer only. Baseline production code `a0979cd`; the same benchmark
test and harness were copied into that worktree without changing production
sources.

| Codec | Workers | Baseline rel/s | Final rel/s | Baseline peak RSS MiB | Final peak RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| MP3 | 1 | 1.116 | 1.257 | 73.3 | 75.7 |
| MP3 | 2 | 2.257 | 2.430 | 138.3 | 107.3 |
| MP3 | 4 | 4.377 | 3.956 | 244.6 | 182.1 |
| FLAC | 1 | 0.978 | 1.089 | 74.6 | 74.7 |
| FLAC | 2 | 1.949 | 2.088 | 143.1 | 143.8 |
| FLAC | 4 | 3.773 | 3.525 | 279.3 | 194.4 |
| OGG | 1 | 0.996 | 1.109 | 74.2 | 73.9 |
| OGG | 2 | 1.993 | 2.164 | 141.7 | 137.0 |
| OGG | 4 | 3.865 | 3.524 | 277.1 | 187.2 |
| WAV | 1 | 0.979 | 1.089 | 76.3 | 73.8 |
| WAV | 2 | 1.960 | 2.061 | 141.0 | 130.9 |
| WAV | 4 | 3.853 | 3.522 | 276.0 | 208.0 |

Final is 9–12 % faster at one and two workers, 9–12 % slower at four workers,
with four-worker peak RSS 60–93 MiB lower. An earlier unsupervised intermediate
reached 6.65 MP3 rel/s at four workers, so this is a deliberate
throughput-for-supervision/bounded-memory trade. Evidence:
`final-codec-matrix-2026-11-27.json`, `final-codec-matrix-baseline-2026-11-27.json`.

### 2.7 Concurrent genre imports (measured)

200 generated Releases sharing one new genre, two workers per stage.

- 111.544 s, 107.58 Releases/min, 1.793 Tracks/s; final states
  `cataloged=200` (one residual `discovered=1` observation, see limitations).
- Genre projection exact on both surfaces: 200 Releases / 200 Tracks for
  `native` and `opensubsonic`.
- Server CPU 8.24 s, peak RSS 50,092 KiB; media on tmpfs.

### 2.8 Incremental indexing (final-only)

`perf_outbox` on the 600K fixture: 8 mirrored trials of 2,048 mixed
Artist/Release/Track revisions through the production outbox worker.

| Batch size | Trials | Events/s | Max end-to-end lag | SQLx statements |
| ---: | --- | ---: | ---: | ---: |
| 4 | 1, 8 | 168.6 / 137.4 | 12.5 s / 15.2 s | 2,624 |
| 32 | 2, 7 | 616.6 / 1,054.3 | 3.5 s / 2.2 s | 656 |
| 8 | 3, 6 | 3,381.8 / 3,962.7 | 0.80 s / 0.73 s | 164 |
| 128 | 4, 5 | 5,960.5 / 6,008.3 | 0.54 s / 0.55 s | 82 |

SQLx events are statements, not network round trips; mixed upserts only, no
concurrent activity or physical media.

### 2.9 Full rebuild (matched baseline/final)

| Metric | Baseline | Final |
| --- | --- | --- |
| Rebuild elapsed | 42.32 s | 35.21 s (snapshot 35.169 s) |
| Documents/second | 16,188 | 19,454 |
| Logical index bytes | 250,007,082 | 231,080,841 |
| Peak process RSS | 217,552 KiB | 279,084 KiB |
| Commits | not instrumented | 69; median 64.68 ms, p95 74.07 ms, max 82.68 ms |
| Reader reloads | not instrumented | median 0.77 ms, p95 3.97 ms, max 21.36 ms |
| Activation / catch-up | not instrumented | 6.60 ms / 17.1 ms, 0 batches, 0 rows |

### 2.10 Concurrent interactive load (matched baseline/final, bounded)

Mixed mode: 200 generated short-media Releases ingested while a real rebuild and
authenticated admin-statistics plus native Artist-browse requests run, pool
limit 8, 180 s window, loopback HTTP/1.

| Metric | Baseline | Final |
| --- | --- | --- |
| Overlap window | 0.02–114.29 s | 0.02–45.08 s |
| Admin during overlap | 101 samples, 0 errors; p50 838 ms, p95 1,052 ms, p99 1,146 ms | 51 samples, 0 errors; p50 619 ms, p95 661 ms, p99 685 ms |
| Browse during overlap | 382 samples, 0 errors; p50 30 ms, p95 126 ms, p99 224 ms | 172 samples, 0 errors; p50 10.0 ms, p95 16.1 ms, p99 19.4 ms |
| Server CPU / peak RSS | 120.3 s / 473,364 KiB | 92.4 s / 589,012 KiB |

The final rebuild finished at 45 s versus 114 s under the same load, so the
overlap windows differ; percentiles are for the observed overlap only. Baseline
lacks checkout/hold instrumentation (`required_metrics_missing` names six
metrics), so no pool-wait comparison is claimed. Evidence:
`final-db-competition-mixed-2026-11-28.json`,
`final-baseline-db-competition-mixed-2026-11-28.json`.

### 2.11 Streaming during ingestion (final-only)

200 Releases ingested while one slow direct stream and repeated authenticated
probes run; 116 active-window stream samples, 463 probe samples, active window
295.0–411.3 s.

- Stream TTFB p50 3.06 ms, p95 6.18 ms, p99 6.75 ms; probe p50 0.66 ms,
  p95 1.52 ms, p99 2.44 ms; `request_failures=0`; all 200 cataloged.
- Slow stream: HTTP 200, 8,142,849 bytes, headers 3.5 ms, TTFB 3.6 ms; the
  client was closed early, so this is not an audible-underrun result.
- Server CPU 13.43 s, peak RSS 57,260 KiB; storage on btrfs.
- No baseline pair is possible: the runner requires the current server's
  bound-address startup log, which the pre-remediation binary does not emit.

### 2.12 Worker scheduling (sampled)

Measured from the genre-scheduler run (same server, same fixture):

- Ready→first-claim over 200 items: p50 28.18 s, p95 50.97 s, max 53.60 s.
- 413 state/lease polls at 0.25 s; zero lock or advisory waiters sampled.
- Per-stage passes vs claimed: inspection 215/200 (115 idle), normalization
  215/200 (115 idle), validation 210/200 (105 idle), publication 210/200
  (105 idle), catalog 209/200 (103 idle).
- State samples show `settling`/`discovered` residency during the run and
  `cataloged=200` at the end. Utilization is sampled, not continuously traced;
  the 64-entry result ring is a code invariant, not a process-lifetime
  measurement.

### 2.13 Merge/split (final-only; benchmark repaired this round)

1,000 and 10,000 Tracks moved from a controlled source Release into a target
Release and split back, with a 5 ms lock sampler. Every evidence, identity,
aggregate, genre and popularity assertion passed, including immutable
moved-Track evidence and exact round-trip.

| Case | Merge | Split | Merge statements | Split statements |
| --- | ---: | ---: | ---: | ---: |
| 1,000 Tracks | 3,556.4 ms | 6,863.7 ms | 36 | 24 |
| 10,000 Tracks | 12,215.4 ms | 9,574.9 ms | 36 | 24 |

Statement counts are constant, confirming set-based merge/split. Lock spans are
sampled approximations (567/1,098 blocked samples at 1K; 1,944/1,510 at 10K), not
exact lock durations. To obtain this, the benchmark now gives its selected
Tracks controlled credits and clears fixture ratings/plays for them (the fixture
now seeds credits for essentially every Track); those are benchmark-only edits.
An earlier run of the same binary before `cargo fmt --all` is not used.

**Baseline port (1K only).** The benchmark target was copied into the verified
baseline worktree and compiled there (benchmark-only dev-dependency
`tracing-subscriber` and test file; production sources unchanged). At 1,000
Tracks the pre-remediation revision performed merge in **633.7 ms with 3,025
SQLx statement events** and split in **618.1 ms with 3,017 events**, versus 36
and 24 statements on the final source. The baseline run then failed the
post-split assertion by design: it left **2,000 stale `catalog_genre_terms`
rows** whose `release_id` no longer matched the moved Track's Release, which is
the Track-move term-maintenance defect this program fixes; the final source
restores zero stale rows and the full immutable evidence round-trip. The pair is
therefore matched evidence for statement count and operation cost, while the
correctness assertions are final-only. Baseline 10K was not run (each case needs
a fresh owned database and a ~12-minute baseline seed); the final 10K result is
recorded above.

## 3. Benchmark defects fixed in this round (no production code changed)

1. `perf_catalog_policy.rs`: no longer assumes its own Releases are the earliest
   `catalog_imports` rows; discovers the real bounded scope through the
   production loader and measures a subject provably inside it.
2. `scripts/perf_representative.py`: `--without-acl-placeholders` (tested) so a
   bounded/ingestion workload's scope is not occupied by ACL placeholder work
   items; required by every ingestion-driven row above.
3. `perf_large_release_merge.rs`: controlled credits and isolated rating/play
   state for the selected Tracks (see 2.13).
4. `perf_catalog_retry.rs`: selects the ingestion occurrence by path suffix, so
   the retry measurement works with the real ingestion workload.
5. `tests/python/test_perf_representative.py`: regression for the fixture option.

## 4. Measurement-class summary

| Workload | Class |
| --- | --- |
| Policy evaluate / dry-run | matched baseline/final |
| Media normalization | matched baseline/final (stage) |
| Full rebuild | matched baseline/final |
| Concurrent interactive load | matched baseline/final (bounded mixed) |
| Single-album import | measured |
| 200-album import | measured |
| Unchanged reimport | measured (idempotent retry + non-idempotent re-drop) |
| Concurrent genre imports | measured |
| Worker scheduling | sampled |
| Identity reconciliation | final-only (no baseline harness) |
| Incremental indexing | final-only |
| Streaming during ingestion | final-only (runner cannot drive baseline) |
| Merge/split | final-only (no baseline target) |

Single/low sample counts, uncontrolled cache state, sampled lock/CPU/RSS
figures, absent reverse proxy, loopback HTTP, and generated short media are
declared per row. No result is a release-quality regression budget or a
production default recommendation; the media and mixed-load rows in particular
need the documented repeats before any worker or pool default changes.

## 4.1 Declared regression budget and evidence-class disposition

The regression budget was declared before final acceptance testing on
2026-11-28 and is recorded in `PERFORMANCE.md` §8.1: a regression is ≥10 %
degradation of the reported central statistic over ≥5 samples per side (5–10 %
needs a repeat, <5 % is noise), ≥25 %/≥64 MiB peak-RSS increase, single-sample
rows are descriptive only, and timeouts are censored. It is not relaxed after
results.

Evidence-class disposition for the four final-only rows:

| Row | Disposition | Reason |
| --- | --- | --- |
| Identity reconciliation | final-only, accepted limitation | The baseline revision has no scoped candidate methods, so neither the comparison nor a port can exist. Scoped-vs-global measurements on the final source are the strongest available evidence. |
| Streaming during ingestion | final-only, accepted limitation | The runner verifies the current server's bound-address startup log; the pre-remediation binary does not emit it. |
| Incremental indexing | final-only unless the baseline benchmark compiles | The benchmark target is not part of the baseline revision; a port is attempted and recorded below. |
| Merge/split | final-only unless the baseline benchmark compiles | The benchmark target is not part of the baseline revision; a baseline port is being measured and recorded below. |

The final report must repeat these limitations; none of these rows may be
described as before/after evidence.

## 5. Reproduction

Build the CLI proof, the server proof, and the ignored benchmarks as documented
in `tests/performance/README.md`, then use the fixture wrapper. Representative
commands:

```text
python3 scripts/perf_representative.py --without-acl-placeholders --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <cli-record.json> -- \
  python3 scripts/perf_ingestion_workload.py --albums 200 --concurrency 2 --timeout 1500

python3 scripts/perf_representative.py --without-acl-placeholders --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <cli-record.json> -- \
  python3 scripts/perf_genre_scheduler.py --releases 200

python3 scripts/perf_representative.py --without-acl-placeholders --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <cli-record.json> -- \
  python3 scripts/perf_db_competition.py --mode mixed --albums 200 --seconds 180 \
  --pool-size 8 --output <new-result.json>

python3 scripts/perf_representative.py --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <cli-record.json> -- \
  python3 scripts/run_perf_mutation_benchmarks.py --merge-size 10000

python3 scripts/perf_representative.py --without-acl-placeholders --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <cli-record.json> -- \
  python3 scripts/perf_ingestion_workload.py --albums 1 --retry-binary <perf_catalog_retry>
```

The CLI proof binds `commit == HEAD` and the workspace source fingerprint, so it
must be regenerated after any commit or `apps/crates` edit. Baseline runs must be
launched from the verified baseline worktree so its proof applies there. A
timeout is censored evidence; none occurred in these runs.
