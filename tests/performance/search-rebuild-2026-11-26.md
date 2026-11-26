# PostgreSQL search rebuild diagnostic, 2026-11-26

One measured production-path rebuild on an invocation-owned disposable database;
the matched baseline run described below adds a limited before/after comparison. The release test binary ran against a dirty
working tree based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`. Concurrent edits occurred during
compilation, so its exact source identity is not established. The binary SHA-256
was `32c7f58ce55ff2f4d7e8933a10992d0d33b6f9c08733ac7044cb67fd30c96f61`; the benchmark source SHA-256 at
execution was `7cbfc739b18f8e8998850a1a80ffe6d08a3753e8bb1deac78561b8d55f1b1c60`. The tracked diff hash at
execution is in the raw metadata below and excludes untracked files.

Fixture: 35,000 artists, 50,000 releases, 600,000 tracks, 600,000 synthetic
acoustic evidence rows, 1,200,000 genre terms and 100 synthetic users with
ratings. The source uses PostgreSQL rows and referenced assets; no physical
media or ACL bindings were supplied. Identity relationships and rating data
are synthetic. The catalog outbox was empty, so catch-up recorded zero rows.

The release build used Rust 1.99.0, `CARGO_BUILD_JOBS=2`, and one test thread.
The host reported AMD Ryzen 9 3900X, 24 logical CPUs and 62 GiB RAM; the
workspace is on NVMe-backed btrfs with compression. PostgreSQL 18.3 used
128 MB shared buffers, 4 MB work_mem and 100 max connections. Cache state was
uncontrolled after fixture generation. No ingestion, network client, workload
competition or background catalog mutation was introduced by this run.

Results (one run, 685,000 documents):

- Fixture creation: 186.5 s. Rebuild: 30.968 s,
  22119 documents/s. All three indexed counts
  matched the fixture and the generation activated.
- Snapshot lifetime from begin return through finish: 30.920 s; 69 logical
  index commits. The activation call took 0.00724 s; this is the whole call,
  not a measurement of the exclusive lock hold itself.
- The 69 commit operations took median 59.94 ms,
  p95 70.92 ms, maximum 86.48 ms. Their
  reader reloads took median 0.86 ms and maximum
  19.53 ms. Segment counts observed just after commits ranged
  3–22; this is not a count of merge operations.
- Logical index files occupied 218,358,261 bytes at the
  observation point. Peak test-process RSS was 248,976
  KiB and does not include PostgreSQL. Neither allocated disk blocks nor total
  system memory, vacuum effects, or snapshot-related dead-tuple retention were
  measured.

The observed writer-initialization time below refers to the fresh rebuild
writer only. Incremental outbox writer open frequency, per-batch writer cost,
reader request latency, multi-process contention, and a controlled multi-sample
baseline comparison remain unmeasured. This run does not justify the RM-13 writer lifecycle decision
or alone complete RM-14 acceptance.

Reproduce from a compiled migration CLI and a release build of the benchmark:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-search --test perf_rebuild --no-run
python3 scripts/perf_representative.py -- cargo test --release -p melodee-search --test perf_rebuild -- --ignored --nocapture --test-threads=1
```

The fixture program owns and removes only its invocation-created database; the
benchmark creates its own temporary search directory and verifies the database
marker before mutating it. The raw output below redacts synthetic identifiers.

```text
{"head": "8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c", "tracked_diff_sha256_at_run": "b42849b67224f66a120662dbd5b145c2015fa0830c4afbe31402aeb6eafd9c98", "benchmark_binary_sha256": "32c7f58ce55ff2f4d7e8933a10992d0d33b6f9c08733ac7044cb67fd30c96f61", "benchmark_source_sha256": "7cbfc739b18f8e8998850a1a80ffe6d08a3753e8bb1deac78561b8d55f1b1c60", "working_tree": "dirty; concurrent edits occurred during build, exact binary source identity unverified", "platform": "Linux-7.3.4-300.fc45.x86_64-x86_64-with-glibc2.44", "cpu_count": 24, "run": "single full disposable fixture; uncontrolled warm cache"}
Seeded 35000 artists, 50000 releases, 600000 tracks in 186.5s

running 1 test
test representative_postgres_search_rebuild ... DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.002077813
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.00017043
DEBUG melodee_search::store: search writer committed commit_seconds=0.048909805 reload_seconds=0.000411243 pending_revisions=10000 segments=3
DEBUG melodee_search::store: search writer committed commit_seconds=0.045425161 reload_seconds=0.000505349 pending_revisions=10000 segments=6
DEBUG melodee_search::store: search writer committed commit_seconds=0.046155553 reload_seconds=0.000517913 pending_revisions=10000 segments=9
DEBUG melodee_search::store: search writer committed commit_seconds=0.061939358 reload_seconds=0.003053977 pending_revisions=10000 segments=4
DEBUG melodee_search::store: search writer committed commit_seconds=0.05495548 reload_seconds=0.000520748 pending_revisions=10000 segments=7
DEBUG melodee_search::store: search writer committed commit_seconds=0.074919448 reload_seconds=0.000455917 pending_revisions=10000 segments=10
DEBUG melodee_search::store: search writer committed commit_seconds=0.064651312 reload_seconds=0.003106855 pending_revisions=10000 segments=5
DEBUG melodee_search::store: search writer committed commit_seconds=0.059695704 reload_seconds=0.000509016 pending_revisions=10000 segments=8
DEBUG melodee_search::store: search writer committed commit_seconds=0.066464948 reload_seconds=0.000693493 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.05999152 reload_seconds=0.000669287 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.057561696 reload_seconds=0.003564215 pending_revisions=10000 segments=9
DEBUG melodee_search::store: search writer committed commit_seconds=0.067396268 reload_seconds=0.000863672 pending_revisions=10000 segments=12
DEBUG melodee_search::store: search writer committed commit_seconds=0.065742973 reload_seconds=0.000880493 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.055880358 reload_seconds=0.000599455 pending_revisions=10000 segments=10
DEBUG melodee_search::store: search writer committed commit_seconds=0.063980752 reload_seconds=0.000662024 pending_revisions=10000 segments=13
DEBUG melodee_search::store: search writer committed commit_seconds=0.063231424 reload_seconds=0.000810162 pending_revisions=10000 segments=16
DEBUG melodee_search::store: search writer committed commit_seconds=0.063353284 reload_seconds=0.000654149 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.061740014 reload_seconds=0.000768313 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.061946832 reload_seconds=0.00080397 pending_revisions=10000 segments=17
DEBUG melodee_search::store: search writer committed commit_seconds=0.05837294 reload_seconds=0.003326769 pending_revisions=10000 segments=12
DEBUG melodee_search::store: search writer committed commit_seconds=0.056144563 reload_seconds=0.000769746 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.052652224 reload_seconds=0.000930498 pending_revisions=10000 segments=18
DEBUG melodee_search::store: search writer committed commit_seconds=0.0513087 reload_seconds=0.003745074 pending_revisions=10000 segments=13
DEBUG melodee_search::store: search writer committed commit_seconds=0.051982677 reload_seconds=0.000792609 pending_revisions=10000 segments=16
DEBUG melodee_search::store: search writer committed commit_seconds=0.053343212 reload_seconds=0.003947314 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.060804708 reload_seconds=0.000709282 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.059047075 reload_seconds=0.000757924 pending_revisions=10000 segments=17
DEBUG melodee_search::store: search writer committed commit_seconds=0.054946092 reload_seconds=0.000929947 pending_revisions=10000 segments=20
DEBUG melodee_search::store: search writer committed commit_seconds=0.055026083 reload_seconds=0.006251201 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.069968268 reload_seconds=0.001620193 pending_revisions=10000 segments=18
DEBUG melodee_search::store: search writer committed commit_seconds=0.063139362 reload_seconds=0.000801575 pending_revisions=10000 segments=13
DEBUG melodee_search::store: search writer committed commit_seconds=0.058202479 reload_seconds=0.000652806 pending_revisions=10000 segments=9
DEBUG melodee_search::store: search writer committed commit_seconds=0.061328871 reload_seconds=0.000600828 pending_revisions=10000 segments=12
DEBUG melodee_search::store: search writer committed commit_seconds=0.057886246 reload_seconds=0.0008145 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.06344716 reload_seconds=0.000618302 pending_revisions=10000 segments=10
DEBUG melodee_search::store: search writer committed commit_seconds=0.064960493 reload_seconds=0.000690748 pending_revisions=10000 segments=13
DEBUG melodee_search::store: search writer committed commit_seconds=0.086482947 reload_seconds=0.003692305 pending_revisions=10000 segments=8
DEBUG melodee_search::store: search writer committed commit_seconds=0.061348699 reload_seconds=0.00369444 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.06737107 reload_seconds=0.002123379 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.054151301 reload_seconds=0.000767401 pending_revisions=10000 segments=17
DEBUG melodee_search::store: search writer committed commit_seconds=0.056087247 reload_seconds=0.004482719 pending_revisions=10000 segments=12
DEBUG melodee_search::store: search writer committed commit_seconds=0.048516096 reload_seconds=0.001776747 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.060457745 reload_seconds=0.000978849 pending_revisions=10000 segments=18
DEBUG melodee_search::store: search writer committed commit_seconds=0.064873479 reload_seconds=0.000921862 pending_revisions=10000 segments=13
DEBUG melodee_search::store: search writer committed commit_seconds=0.05771316 reload_seconds=0.001092802 pending_revisions=10000 segments=16
DEBUG melodee_search::store: search writer committed commit_seconds=0.055445471 reload_seconds=0.003866964 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.059551844 reload_seconds=0.000730933 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.070917812 reload_seconds=0.000814971 pending_revisions=10000 segments=17
DEBUG melodee_search::store: search writer committed commit_seconds=0.079665462 reload_seconds=0.001022 pending_revisions=10000 segments=20
DEBUG melodee_search::store: search writer committed commit_seconds=0.065214129 reload_seconds=0.000844516 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.054503772 reload_seconds=0.000869022 pending_revisions=10000 segments=18
DEBUG melodee_search::store: search writer committed commit_seconds=0.064774934 reload_seconds=0.000882528 pending_revisions=10000 segments=21
DEBUG melodee_search::store: search writer committed commit_seconds=0.057264167 reload_seconds=0.004545378 pending_revisions=10000 segments=16
DEBUG melodee_search::store: search writer committed commit_seconds=0.059941195 reload_seconds=0.000959803 pending_revisions=10000 segments=19
DEBUG melodee_search::store: search writer committed commit_seconds=0.062799173 reload_seconds=0.001257442 pending_revisions=10000 segments=22
DEBUG melodee_search::store: search writer committed commit_seconds=0.062192494 reload_seconds=0.000979129 pending_revisions=10000 segments=17
DEBUG melodee_search::store: search writer committed commit_seconds=0.059847028 reload_seconds=0.000919647 pending_revisions=10000 segments=20
DEBUG melodee_search::store: search writer committed commit_seconds=0.05375708 reload_seconds=0.019527768 pending_revisions=10000 segments=16
DEBUG melodee_search::store: search writer committed commit_seconds=0.057411533 reload_seconds=0.003300429 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.058723037 reload_seconds=0.000632638 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.06365994 reload_seconds=0.000727035 pending_revisions=10000 segments=17
DEBUG melodee_search::store: search writer committed commit_seconds=0.060266576 reload_seconds=0.003323712 pending_revisions=10000 segments=12
DEBUG melodee_search::store: search writer committed commit_seconds=0.056064844 reload_seconds=0.000683664 pending_revisions=10000 segments=15
DEBUG melodee_search::store: search writer committed commit_seconds=0.063794723 reload_seconds=0.000844456 pending_revisions=10000 segments=18
DEBUG melodee_search::store: search writer committed commit_seconds=0.06457551 reload_seconds=0.000720854 pending_revisions=10000 segments=13
DEBUG melodee_search::store: search writer committed commit_seconds=0.058321663 reload_seconds=0.000673946 pending_revisions=10000 segments=16
DEBUG melodee_search::store: search writer committed commit_seconds=0.056048503 reload_seconds=0.003461742 pending_revisions=10000 segments=11
DEBUG melodee_search::store: search writer committed commit_seconds=0.062763436 reload_seconds=0.001619532 pending_revisions=10000 segments=14
DEBUG melodee_search::store: search writer committed commit_seconds=0.034001684 reload_seconds=0.00088386 pending_revisions=5000 segments=17
 INFO melodee_search::rebuild: search rebuild snapshot indexed job_id=<synthetic-id> generation_id=<synthetic-id> snapshot_seconds=30.920430169 commit_count=69 artists=35000 releases=50000 tracks=600000
 INFO melodee_search::rebuild: search rebuild activated job_id=<synthetic-id> generation_id=<synthetic-id> catchup_seconds=0.019237394 catchup_batches=0 catchup_rows=0 activation_seconds=0.00723522
{"workload":"postgres-search-rebuild","build_debug_assertions":false,"fixture":{"artists":35000,"releases":50000,"tracks":600000},"postgres":{"version":"18.3","shared_buffers":"128MB","work_mem":"4MB","max_connections":"100"},"snapshot_page_size":1000,"snapshot_commit_documents":10000,"snapshot_commit_interval_ms":2000,"elapsed_seconds":30.968297476,"documents_per_second":22119.394859561315,"logical_index_bytes":218358261,"peak_process_rss_kib":248976,"cpu_parallelism":24,"cache_conditions":"uncontrolled after fixture seed","sample_count":1,"limitations":"fixture metadata and relationships are synthetic; no physical media, ACLs or concurrent workloads; RSS is process-wide high-water mark; index size is logical file bytes"}
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 31.07s

exit_code=0
```

## Original-revision baseline (one run)

The same fixture generation rules and catalog cardinalities were run against
baseline revision `a0979cde1fd2aeff5b8027d9d59336b678f1aa12` in a separate detached worktree and separate
release build. The baseline worktree added this benchmark test and the existing
workspace tracing-subscriber as a dev dependency; its production search code
remained at the baseline revision. Fixture creation took 179.0 s and was excluded
from the rebuild timing. The baseline binary SHA-256 was
`f0fc69c71e6d439378058c539af2b5a50cfea559635656d90e15755b8d150d85`; the benchmark test SHA-256 was
`9bf346fae2260303a52977670ac411aa0632d14a73c484d71cbbd9e4f698fa8d` and fixture script SHA-256 was
`37fdec8f3fe435c52ef5b93503304e0fc60baa973040fe2bddd423aaff5a0ade`.

| Single run | Rebuild seconds | Documents/s | Peak process RSS KiB | Logical index bytes |
|---|---:|---:|---:|---:|
| Baseline | 35.129 | 19,500 | 224,632 | 219,819,473 |
| Current dirty build | 30.968 | 22,119 | 248,976 | 218,358,261 |

Both runs verified the same final document counts and empty catch-up. These are
individual measurements with uncontrolled cache state and different generated
UUIDs, not a statistically controlled performance comparison. The current
build's exact source identity is unverified, the benchmark report format changed
only to serialize settings available at each revision, and the baseline lacks
the new snapshot, writer and activation tracing. The baseline and current
schemas differ by the intervening migrations; no concurrent workload, media
processing or network path was included. Peak RSS is process-wide and does not
include PostgreSQL. These limitations prevent a general performance claim.

The baseline used the 600-second bounded wrapper; it did not time out. Raw
output follows with synthetic identifiers redacted:

```text
{"head": "a0979cde1fd2aeff5b8027d9d59336b678f1aa12", "tracked_diff_sha256_at_run": "bdbd1725ebce7049d4e8446c125ebdcf300f6ed43b4ca6f94c674c1fcef24dbc", "benchmark_binary_sha256": "f0fc69c71e6d439378058c539af2b5a50cfea559635656d90e15755b8d150d85", "benchmark_source_sha256": "9bf346fae2260303a52977670ac411aa0632d14a73c484d71cbbd9e4f698fa8d", "fixture_script_sha256": "37fdec8f3fe435c52ef5b93503304e0fc60baa973040fe2bddd423aaff5a0ade", "working_tree": "baseline production source at recorded revision; staged benchmark test and dev dependency, other untracked fixture files", "platform": "Linux-7.3.4-300.fc45.x86_64-x86_64-with-glibc2.44", "cpu_count": 24, "run": "single full disposable fixture; uncontrolled warm cache", "rebuild_timeout_seconds": 600}
Seeded 35000 artists, 50000 releases, 600000 tracks in 179.0s

running 1 test
test representative_postgres_search_rebuild ... {"workload":"postgres-search-rebuild","build_debug_assertions":false,"fixture":{"artists":35000,"releases":50000,"tracks":600000},"postgres":{"version":"18.3","shared_buffers":"128MB","work_mem":"4MB","max_connections":"100"},"search_config":{"processed_outbox_retention_days":7,"lease_seconds":60,"claim_batch_size":256,"max_outbox_attempts":5,"poll_interval_ms":1000,"snapshot_page_size":1000,"default_limit":200,"max_limit":1000,"enabled":true},"elapsed_seconds":35.128680556,"documents_per_second":19499.7360890915,"logical_index_bytes":219819473,"peak_process_rss_kib":224632,"cpu_parallelism":24,"cache_conditions":"uncontrolled after fixture seed","sample_count":1,"limitations":"fixture metadata and relationships are synthetic; no physical media, ACLs or concurrent workloads; RSS is process-wide high-water mark; index size is logical file bytes"}
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 35.24s

exit_code=0
```
