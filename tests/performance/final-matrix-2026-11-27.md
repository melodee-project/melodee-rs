# Final integrated performance matrix (partial), 2026-11-27

This document records the representative-scale measurement round executed on the
final integrated sources of this performance-remediation operation. It does
**not** complete the RM-00 matrix: two of the thirteen required workloads now
have matched baseline/final production-path measurements at the representative
560K–600K tier, a third comparable pair exists for real media normalization
(a stage workload, not a database-scale one), one has final-only
production-path measurements with a documented reason it cannot be run against
the pre-remediation baseline, and the remaining nine were not re-executed in
this round.

Everything below is **MEASURED IN THIS RUN** unless a row says otherwise. No
number is a target, an estimate, or an extrapolation. Fixture construction time
is reported separately and is never presented as import or application
throughput.

## 1. Source, binary and harness provenance

| Item | Value |
| --- | --- |
| Final source commit | `554e9d590db9e97f7343abdcd60a22e4f3bf10f4` plus this round's benchmark-only changes (section 5) |
| Final CLI proof | `tests/performance/final-cli-build-2026-11-27e.json` (source fingerprint verified equal to the measurement tree) |
| Final identity test proof | `tests/performance/final-identity-build-2026-11-27v2.json` (fingerprint verified equal) |
| Clean-tree search-rebuild CLI proof | `tests/performance/final-cli-build-2026-11-27.json` (source fingerprint matched the clean `554e9d5` tree at build time) |
| Baseline commit | `a0979cde1fd2aeff5b8027d9d59336b678f1aa12` in `../melodee-rs-perf-baseline-a0979cd` |
| Baseline CLI proof | `tests/performance/final-baseline-cli-build-2026-11-27.json` (search rebuild), `.../final-baseline-cli-build-2026-11-27b.json` (policy; current baseline worktree fingerprint) |
| Baseline CLI sha256 | `4723be0ac750b1e6a187d7cb9d648c90e2c840b9cab595d0e2636af5fd3124b6` |
| Final CLI sha256 | `61ac4bf06269a65194e2aac8f23bbd4abaffb4f241a7893d29f08583eebdf996` |
| `perf_rebuild` (final / baseline) sha256 | `8461252a00a0f717…` / `f0fc69c71e6d4393…` |
| `perf_catalog_policy` (final / baseline) sha256 | `1bcd810506d5fa8c…` / `49c4884c2870d5a1…` |
| `perf_identity_reconciliation` (final) sha256 | `73f056f81523713b…` (full digest in the build record) |
| Codec matrix executables (final / baseline) | `40f576d013da7c64…` / `12bcba92e9763173…` |
| Harness `scripts/perf_representative.py` | sha256 `029d125041331286…` (same content copied into the baseline worktree) |

Toolchain Rust 1.99.0 (pinned), release profile, `CARGO_BUILD_JOBS=2`,
`RUST_TEST_THREADS=1`. Host: AMD Ryzen 9 3900X, 24 logical CPUs, 62 GiB RAM,
NVMe-backed btrfs with compression. PostgreSQL 18.3 from `compose.dev.yaml`
(`shared_buffers=128MB`, `work_mem=4MB`, `max_connections=100`), read from the
database inside the measured runs. One Cargo/test workload ran at a time; no
competing Cargo or rust-analyzer process was active during the measured runs
(an unrelated `cargo check` for a different repository was observed once before
the first run and had exited). Cache state was uncontrolled after each fixture
seed, and every case used a fresh invocation-owned database.

Fixture (all cases): `scripts/perf_representative.py` with 35,000 Artists,
50,000 Releases, 600,000 Tracks, 600,000 synthetic acoustic evidence rows,
skewed genre terms, ratings 1–5, ACL groups/libraries, alternate asset links,
and one synthetic artwork object. It is a database/media-reference fixture, not
a 600K-file audio corpus; no network, reverse proxy, or ingestion load was
present unless a row says so. The policy cases add `--without-acl-placeholders`
(section 5); both sides of that comparison use the same option.

## 2. Full PostgreSQL search rebuild (RM-14 / PERF-006)

One production rebuild per side, exercised through `SearchProjectionStore`
activation; no concurrent clients, no outbox catch-up backlog.

| Metric | Baseline `a0979cd` | Final `554e9d5` | Note |
| --- | --- | --- | --- |
| Rebuild elapsed | 42.32 s | 35.21 s | single sample each |
| Documents/second | 16,188 | 19,454 | 685,000 documents |
| Logical index bytes | 250,007,082 | 231,080,841 | file bytes, not blocks |
| Peak process RSS | 217,552 KiB | 279,084 KiB | high-water, excludes PostgreSQL |
| Snapshot seconds | not instrumented | 35.169 | |
| Commits | not instrumented | 69; median 64.68 ms, p95 74.07 ms, max 82.68 ms | 10,000 documents per commit |
| Reader reloads | not instrumented | median 0.77 ms, p95 3.97 ms, max 21.36 ms | |
| Activation call | not instrumented | 6.60 ms | whole call, not lock hold |
| Catch-up after activation | 0 rows (not instrumented) | 17.1 ms, 0 batches, 0 rows | outbox empty |
| Fixture seed | 703.6 s (baseline schema) | 185.1 s | construction only; **not** throughput |

Evidence: `final-search-rebuild-600k-2026-11-27.json` (final) and
`final-search-rebuild-baseline-600k-2026-11-27.json` (baseline JSON, which
lacks the newer tracing fields by design). The difference in seed time reflects
the older schema and tooling, not application performance; it is reported so
the two runs are not mistaken for identical setup cost. One sample per side is
not enough for a variance or regression-budget claim.

## 3. Policy evaluation and dry-run (RM-02 / PERF-002)

Three timed iterations per case per side, after three untimed fixture samples.
The ACID context is identical in shape: eight Releases, one injected
cross-Release acoustic match, 600K background tracks with synthetic digests.

### 3.1 Single Release snapshot + evaluation (`single_evaluate`)

| Metric | Baseline | Final |
| --- | --- | --- |
| Elapsed (3 samples) | 1,229.6 / 1,203.0 / 1,278.9 ms | 8.21 / 9.66 / 8.54 ms |
| SQL statements | 13 | 13 |
| SQL rows returned | 600,014 | 7 |
| Retained acoustic digests | 600,008 | 1 |
| Retained acoustic bytes | 38,400,512 | 64 |

### 3.2 Bounded eight-Release dry-run load + evaluate (`dry_run_load_and_evaluate`)

| Metric | Baseline | Final |
| --- | --- | --- |
| Elapsed (3 samples) | 16,363.8 / 15,806.8 / 16,313.5 ms | 396.4 / 374.7 / 382.3 ms |
| SQL statements | 3 | 3 |
| SQL rows returned | 10 | 10 |
| Contexts / Track facts | 8 / 8 | 8 / 8 |
| Retained acoustic digests | 4,800,057 | 1 |
| Retained acoustic bytes | 307,203,648 | 64 |

The scoped acoustic query returns the same cross-Release match in both
revisions; the baseline materialises and retains the catalog-wide digest set
(600,008 rows per evaluation, 4,800,057 rows for a bounded eight-Release
dry-run) while the final scoped query returns and retains one digest. That is
the measured form of PERF-002. The harness change in section 5 was required to
make the dry-run scope deterministic against the ACL-bearing representative
fixture; it does not change production code.

Plan evidence is captured in the same run by `EXPLAIN (ANALYZE, BUFFERS)` for
the old global and the new scoped acoustic queries and printed by the test; the
raw plan output is deliberately not committed because it contains synthetic but
identifier-shaped values.

## 3.3 Real media normalization matrix (RM-11 / RM-12 / PERF-007)

Four generated mono codecs, 60 s of source audio per Release, eight single-Track
Releases per case, two samples, unmodified production FFmpeg options and
subprocess handling, warm source reuse. The timer covers normalization only
(inspection finishes before it starts). This is a media-stage measurement, not
an import: no PostgreSQL, HTTP, scheduler, or acquisition. Baseline production
code is `a0979cd`; the same benchmark test and harness were copied into that
worktree without changing its production sources.

| Codec | Workers | Baseline releases/s | Final releases/s | Baseline peak RSS MiB | Final peak RSS MiB |
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

The final supervised/admitted path is 9–12 % faster at one and two workers and
9–12 % slower at four workers, while sampled process-tree peak RSS at four
workers falls by 60–93 MiB. An earlier unsupervised intermediate revision
reached up to 6.65 releases/s for MP3 at four workers, so the current path is a
deliberate throughput-for-supervision/bounded-memory trade, not an unexplained
regression; the 60K-fixture and full-import rows remain the ones needed before
any worker default is changed. Evidence: `final-codec-matrix-2026-11-27.json`
(final, clean tree at `9889896`) and
`final-codec-matrix-baseline-2026-11-27.json` (baseline worktree, dirty only by
the copied harness/benchmark).

## 3.4 Production scrobble/recommendation contention (RM-18 / PERF-014): final-only

`perf_recommendation_contention` ran on the final source inside the 600K fixture
(fresh owned database, release build, 8 concurrent workers, one round, three
scenarios including shared-Track, shared-Artist and concurrent Artist changes).
It reported `recommendation_consistency=exact`: recommendation counters matched
authoritative plays in every scenario after the relationship changes. Sampled
lock spans are approximate (5 ms polling) and SQLx event counts exclude BEGIN and
trigger-internal statements; network and PostgreSQL CPU/I/O are not measured.
This is a correctness-plus-contention reverification, not an interactive-load
percentile study.

## 3.5 Merge/split (RM-19 / PERF-V002): BLOCKED by fixture drift

The ignored `perf_large_release_merge` benchmark aborted before any timed
operation in this round: `measure_large_release_merge_and_split` panicked at
`crates/melodee-persistence/tests/perf_large_release_merge.rs:183`
(`assertion left == right failed: left 0, right 1001`) because its pre-timing
selection requires Tracks that have neither an `observed_artists` row nor a
`track_artists` row, and the representative fixture now seeds real Track
credits/observations for essentially every Track (601,002 credits over 600,000
Tracks). No merge/split measurement was produced; the earlier 1K/10K samples
predate the fixture change and must not be reused as final-source evidence.
The next action is to give the benchmark its own dedicated source/target
Releases with controlled credits (or to select Tracks whose fixture credits can
be moved with them), then rerun 1K and 10K. No production code is implicated.

## 4. Identity reconciliation (RM-09 / PERF-004): final-only

The pre-remediation revision has no scoped candidate methods, so this harness
cannot run against it (documented in `tests/performance/README.md`). This is a
final-only production-path measurement, not a before/after pair.

Fixture after augmentation: 35,000 Artists, 50,000 Releases, 600,000 Tracks,
650,004 observations and current Artist decisions, 601,002 Track credits,
11,087 Artist external IDs, 16,668 Release external IDs. Synthetic closure: 8
durable seeds over 6 Artist IDs and 2 Release IDs.

| Production candidate query | Scoped ms | Global ms | Rows |
| --- | ---: | ---: | ---: |
| Strong Artist | 6.530 | 59.718 | 1 |
| Same-Release Track Artist | 15.775 | 2,963.826 | 1 |
| Repeated Artist credit | 6.322 | 1,365.018 | 1 |
| Strong Release | 2.238 | 17.901 | 1 |
| Name proposal | 5.682 | 718.751 | 1 |

Production drain: one bounded turn took 3,188.3 ms, recorded three Artist
merges and one Release merge, and left 68 follow-up seeds; a subsequent bounded
turn consumed the queue in 32.1 ms (`seeds_remaining=0`, `outcome=quiescent`).
That is convergence for this synthetic closure, not a bound for arbitrary
graphs or interrupted sessions. Evidence: `final-identity-600k-2026-11-27v2.json`
(collector-validated; raw child output discarded). An earlier run of the same
binary before `cargo fmt --all` recorded 2,949.1 ms for the drain and 34.4 ms
for closure; the spread is single-sample variance, not a change.

## 5. Harness changes committed with this round

1. `crates/melodee-policy/tests/perf_catalog_policy.rs`: the bounded dry-run
   loader (`PolicyStore::load_test_contexts`) measures the earliest
   `catalog_imports` rows. The representative fixture now publishes ACL
   placeholder Releases before the benchmark runs, so the benchmark's own
   eight seeded Releases were no longer the earliest imports and the previous
   assertion panicked with `None` at representative size. The test now
   discovers the real bounded scope once, selects a subject provably inside it
   that owns an acoustic digest, and asserts scope membership and
   cross-Release matching against that subject. The production loader and the
   measurement are unchanged.
2. `scripts/perf_representative.py`: new opt-out
   `--without-acl-placeholders` omits only the synthetic ACL publication
   placeholder block (it still exists for every default/ACL run). It is needed
   because those placeholders occupy the earliest `catalog_imports` rows; the
   policy dry-run scope would otherwise measure placeholders instead of a
   workload. The fixture's ACL coverage tests continue to run with the default
   (placeholders present).
3. `tests/python/test_perf_representative.py`: unit test proving the option
   removes the ACL placeholder imports and keeps the catalog/Track seeding.

## 6. Not executed in this round (mandatory RM-00 rows)

| Workload | Status | Existing evidence and gap |
| --- | --- | --- |
| Single-album import | NOT EXECUTED in this round | Earlier ingestion workload evidence is intermediate and predates the final source. |
| 200-album import | NOT EXECUTED in this round | `perf_ingestion_workload.py`/`perf_genre_scheduler.py` exist; final-source release-build run is outstanding. |
| Unchanged reimport | NOT EXECUTED in this round | Needs the ingestion workload on the final source; idempotency is covered by functional tests only. |
| Concurrent genre imports | NOT EXECUTED in this round | `genre-scheduler-600k-2026-11-26.md` intermediate. |
| Incremental indexing | NOT EXECUTED in this round | `search-outbox-mixed-2026-11-26.md` intermediate; final-source rerun outstanding. |
| Concurrent interactive load | NOT EXECUTED in this round | `db-competition-baseline-2026-11-26.md` covers a baseline; final-source mixed-load pair outstanding. |
| Streaming during ingestion | NOT EXECUTED in this round | `ingestion-stream-600k-2026-11-26.md` intermediate; slow-client matrix outstanding. |
| Worker scheduling | NOT EXECUTED in this round | Needs slot-utilization/queue-bound instrumentation run on the final source. |
| Merge/split | NOT EXECUTED in this round | Earlier 1K/10K samples predate the final source; `run_perf_mutation_benchmarks.py` is ready to rerun. |

The remaining rows require no new design decision, only execution and, where
the harness needs it, the same kind of fixture-scope correction applied in
section 5. They remain mandatory for RM-00 acceptance and must not be described
as complete.

## 7. Reproduction

Final rebuild and policy (from the repository root, after
`python3 scripts/build_perf_cli.py --output <new-cli-record.json>` and
`cargo test --release -p melodee-search --test perf_rebuild --no-run` /
`cargo test --release -p melodee-policy --test perf_catalog_policy --no-run`):

```text
python3 scripts/perf_representative.py --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <new-cli-record.json> -- \
  python3 scripts/run_perf_search_case.py --test-binary <perf_rebuild binary> --timeout-seconds 600

python3 scripts/perf_representative.py --without-acl-placeholders --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <new-cli-record.json> -- \
  python3 scripts/run_perf_policy_case.py --test-binary <perf_catalog_policy binary> --timeout-seconds 600
```

Identity (after `python3 scripts/build_perf_test.py --target identity --output <new-build.json>`):

```text
python3 scripts/perf_representative.py --seed-timeout 1200 \
  --migration-cli target/release/melodee-cli \
  --migration-cli-provenance <new-cli-record.json> -- \
  python3 scripts/run_perf_identity_case.py \
  --test-binary target/release/deps/perf_identity_reconciliation-<hash> \
  --build-provenance <new-build.json> \
  --migration-cli-provenance <new-cli-record.json> \
  --output <new-case.json> --timeout-seconds 600
```

Baseline runs use the same harness files (copied into the verified
`../melodee-rs-perf-baseline-a0979cd` worktree) and must be launched from that
worktree so the migration-proof commit check applies there; the baseline CLI
proof is rebuilt in that worktree after any harness/test copy. The baseline
worktree contains only benchmark harness/test additions on top of `a0979cd`;
its application code is unchanged.

A timeout is censored evidence and was not encountered in these runs.
