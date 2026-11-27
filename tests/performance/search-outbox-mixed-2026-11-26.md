# Mixed incremental search outbox diagnostic, 2026-11-26

Three consecutive trials ran on one release-build production search worker and
one invocation-owned PostgreSQL catalog. HEAD at execution was `08de08d50cb1be6e76c66000232dae257c1a2d2f`
and the tree was dirty; the compiled binary's exact source identity is
unverified. Benchmark binary SHA-256 `8fa075f5db78b80966db5b92fc39ba19820bd595261420feff1d446cf68db355`;
benchmark source SHA-256 at run `89fa5bf0163e21936576443e34f045f6f499c8314277baf7b469560f89c0ca6b`; fixture
script SHA-256 `3e6b5305a7d420aea2881141862c3d4aa4acffe5282c89e4df106a2da7183b7d`. The tracked diff SHA-256
at execution was `ee89caaec00a02211c73cbc36fab40d1915a48648a96285fb9e7f901e9ee8c35`; it excludes untracked
files and does not establish the compiled source. The sampled events are below.

The fixture had 35,000 Artists, 50,000 Releases and 600,000 Tracks, synthetic
Artist/Track relationships, ratings, genre terms and acoustic evidence. Fixture
creation took 184.4 s. A full PostgreSQL-backed search rebuild and activation
preceded the measured trials; the three trials then reused that single
generation sequentially. PostgreSQL 18.3 used 128 MB shared buffers, 4 MB
work_mem and 100 max connections. Host: AMD Ryzen 9 3900X, 24 logical CPUs,
62 GiB RAM, Linux 7.3.4 and an NVMe-backed btrfs workspace. Cache conditions
were uncontrolled and warm after seeding/rebuild; no external clients, competing
workers, physical media or concurrent writes were included.

In each trial, bounded setup SQL changed revisions for 1,024 Artists, 512
Releases and 512 Tracks and atomically inserted 2,048 interleaved outbox events.
The measured path was the ordinary production `apply_outbox_once` worker.
All 6,144 events across the three trials were applied and acknowledged with
zero errors, and an updated Artist was verified through the active search
service after each drain. Release and Track acknowledgements were checked, but
their indexed content was not independently queried in this run. The harness
was subsequently extended with untimed visibility checks for those two kinds;
they do not retroactively change this evidence. Setup SQL is excluded from
drain time.

| Trial | Drain s | Events/s | SQLx statement events | Max created-to-processed lag ms | Process peak RSS KiB before → after |
|---|---:|---:|---:|---:|---:|
| 1 | 0.5905 | 3468 | 164 | 843.74 | 273,856 → 310,752 |
| 2 | 0.5032 | 4070 | 164 | 676.80 | 315,236 → 378,164 |
| 3 | 0.4976 | 4116 | 164 | 689.04 | 379,948 → 418,168 |

Each drain comprised eight 256-event batches. The SQLx event count reflects
logged executed statements, **not** network round trips. Lag includes queue
residence and setup effects, not HTTP transport. Peak RSS is the process high
water mark including the initial rebuild and previous trials; it excludes
PostgreSQL and does not isolate writer memory.

| Operation | Samples | Median ms | Max ms | Total ms |
|---|---:|---:|---:|---:|
| Writable index open | 24 | 0.296 | 0.381 | 7.057 |
| Writer initialization | 24 | 0.740 | 1.687 | 19.062 |
| Writer reader initialization | 24 | 3.586 | 4.705 | 82.126 |
| Commit | 24 | 15.326 | 25.880 | 362.681 |
| Commit reader reload | 24 | 1.239 | 1.826 | 31.988 |
| Cached reader reload | 33 | 0.845 | 4.378 | 35.887 |

Writer initialization totalled 19.062 ms out of 1591.283 ms
of measured drains (about 1.20% in these trials);
writable index open and reader initialization are listed separately. This is
an observation of direct initialization calls, not a proven upper bound on the
impact of any writer-reuse design. Post-commit segment counts climbed 20→41 in
the first trial and remained 41 in the next two; counts do not measure actual
merge activity. Across trials, process peak RSS rose to 418,168
KiB; its cause and a per-writer memory bound were not measured.

For the tested default 256-event sequential batches, the direct writer startup
cost is small relative to drain time. Retaining a writer between batches would
require a separately tested ownership/lease and generation-transition protocol
to avoid holding the Tantivy lock after ownership is lost. These measurements
do not assess small batches, idle lock retention, multiple processes, active
query/ingestion traffic, failure or generation changes, and do not establish a
general latency budget or prove reuse would be safe. They support a conditional
choice to retain the existing lifecycle for this tested workload only; RM-13
remains open pending the missing concurrency, memory and lifecycle evidence.

Reproduce from a built migration CLI and release test executable:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-search --test perf_outbox --no-run
python3 scripts/perf_representative.py -- python3 scripts/run_perf_search_case.py --test-binary PATH_TO_PERF_OUTBOX_TEST --timeout-seconds 600
```

The wrapper kills and reaps only its test process at timeout and removes its own
temporary search tree; the outer fixture removes only its own database. The
600-second limit excludes fixture construction and includes initial rebuild.
The raw measured events follow (synthetic identifiers redacted):

```text
BEGIN_INCREMENTAL_OUTBOX_MEASUREMENT trial=1

DEBUG melodee_search::store: search index opened elapsed_seconds=0.000205877 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000563277
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.001571401
DEBUG melodee_search::store: search writer committed commit_seconds=0.014859762 reload_seconds=0.000781938 pending_revisions=256 segments=20
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000245551 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.00073467
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.001822331
DEBUG melodee_search::store: search writer committed commit_seconds=0.014596407 reload_seconds=0.001088464 pending_revisions=256 segments=23
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000243848 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000828085
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.002655186
DEBUG melodee_search::store: search writer committed commit_seconds=0.017363893 reload_seconds=0.001200925 pending_revisions=256 segments=26
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000249548 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.0007937
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.002379358
DEBUG melodee_search::store: search writer committed commit_seconds=0.016367301 reload_seconds=0.001033661 pending_revisions=256 segments=29
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000247034 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000633861
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.002530151
DEBUG melodee_search::store: search writer committed commit_seconds=0.019641159 reload_seconds=0.001028671 pending_revisions=256 segments=32
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000246613 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000703842
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.0027746
DEBUG melodee_search::store: search writer committed commit_seconds=0.012767643 reload_seconds=0.001548908 pending_revisions=256 segments=35
DEBUG melodee_search::store: search index opened elapsed_seconds=0.00031431 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000881144
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003985243
DEBUG melodee_search::store: search writer committed commit_seconds=0.011204078 reload_seconds=0.001239777 pending_revisions=256 segments=38
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000297048 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000627258
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.00331276
DEBUG melodee_search::store: search writer committed commit_seconds=0.009486933 reload_seconds=0.001167532 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000254778 writable=false
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003653098
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001017631 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000831711 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000861197 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000835038 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000819779 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000791336 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000808177 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000819178 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000824859 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000816093 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000826582 success=true
{"workload":"postgres-search-incremental","trial":1,"build_debug_assertions":false,"fixture":{"artists":35000,"releases":50000,"tracks":600000},"events":2048,"artist_events":1024,"release_events":512,"track_events":512,"batches":8,"batch_size":256,"elapsed_seconds":0.590508428,"events_per_second":3468.1977477212226,"sqlx_statement_events":164,"max_end_to_end_outbox_lag_ms":843.742,"batch_elapsed_ms":[66.737378,63.025408000000006,67.410752,163.416075,67.194125,58.865748,51.900760999999996,51.955594],"query_elapsed_ms":[6.5644149999999994,1.849352,1.6951530000000001,1.663403,1.675867,1.5336690000000002,1.5040229999999999,1.675165,1.5529160000000002,1.540864,1.660618],"peak_process_rss_before_kib":273856,"peak_process_rss_after_kib":310752,"limitations":"three consecutive trials on one process and catalog; mixed upserts only; no concurrent activity or physical media; SQL setup excluded; query timings in-process; SQLx events count statements, not network round trips; RSS is process high-water including the full rebuild"}
BEGIN_INCREMENTAL_OUTBOX_MEASUREMENT trial=2
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000307317 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.001686647
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003514559
DEBUG melodee_search::store: search writer committed commit_seconds=0.01448107 reload_seconds=0.001194563 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000380564 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000768413
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003767273
DEBUG melodee_search::store: search writer committed commit_seconds=0.017150283 reload_seconds=0.001109013 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000324509 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.001391192
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.004408738
DEBUG melodee_search::store: search writer committed commit_seconds=0.025880343 reload_seconds=0.001238976 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000344126 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000785625
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003770671
DEBUG melodee_search::store: search writer committed commit_seconds=0.013092073 reload_seconds=0.001315079 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000286578 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000733167
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003624857
DEBUG melodee_search::store: search writer committed commit_seconds=0.016300506 reload_seconds=0.001425907 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000303961 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000659238
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003425291
DEBUG melodee_search::store: search writer committed commit_seconds=0.014878657 reload_seconds=0.001151963 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000355457 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000835859
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.004704654
DEBUG melodee_search::store: search writer committed commit_seconds=0.010054118 reload_seconds=0.001584666 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000274365 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000744358
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003652317
DEBUG melodee_search::store: search writer committed commit_seconds=0.016042492 reload_seconds=0.001694102 pending_revisions=256 segments=41
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.00303616 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000981613 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.00084647 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000834677 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000808738 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000853873 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000805673 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.00080945 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000831872 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000838675 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000808879 success=true
{"workload":"postgres-search-incremental","trial":2,"build_debug_assertions":false,"fixture":{"artists":35000,"releases":50000,"tracks":600000},"events":2048,"artist_events":1024,"release_events":512,"track_events":512,"batches":8,"batch_size":256,"elapsed_seconds":0.503159559,"events_per_second":4070.279424026604,"sqlx_statement_events":164,"max_end_to_end_outbox_lag_ms":676.805,"batch_elapsed_ms":[62.627742,65.27711699999999,74.95497900000001,58.199006,65.830214,63.780136,52.51304,59.975071],"query_elapsed_ms":[4.023683999999999,1.664285,1.501129,1.46498,1.457737,1.476462,1.407813,1.4137950000000001,1.519744,1.525574,1.435786],"peak_process_rss_before_kib":315236,"peak_process_rss_after_kib":378164,"limitations":"three consecutive trials on one process and catalog; mixed upserts only; no concurrent activity or physical media; SQL setup excluded; query timings in-process; SQLx events count statements, not network round trips; RSS is process high-water including the full rebuild"}
BEGIN_INCREMENTAL_OUTBOX_MEASUREMENT trial=3
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000310603 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000682591
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003715216
DEBUG melodee_search::store: search writer committed commit_seconds=0.016149031 reload_seconds=0.001478246 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000295525 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000766469
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.004038112
DEBUG melodee_search::store: search writer committed commit_seconds=0.016545156 reload_seconds=0.00160804 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000357862 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000817105
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.004533752
DEBUG melodee_search::store: search writer committed commit_seconds=0.016000003 reload_seconds=0.001786024 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000295916 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000700756
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.00350956
DEBUG melodee_search::store: search writer committed commit_seconds=0.014021227 reload_seconds=0.00130981 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000244079 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000594276
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003337837
DEBUG melodee_search::store: search writer committed commit_seconds=0.0171492 reload_seconds=0.001196657 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000283643 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.00064423
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003551278
DEBUG melodee_search::store: search writer committed commit_seconds=0.015772675 reload_seconds=0.001772148 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000299603 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000626256
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003619767
DEBUG melodee_search::store: search writer committed commit_seconds=0.010038238 reload_seconds=0.001206986 pending_revisions=256 segments=41
DEBUG melodee_search::store: search index opened elapsed_seconds=0.000343514 writable=true
DEBUG melodee_search::store: search writer initialized elapsed_seconds=0.000859654
DEBUG melodee_search::store: search reader initialized elapsed_seconds=0.003921563
DEBUG melodee_search::store: search writer committed commit_seconds=0.012838425 reload_seconds=0.001825728 pending_revisions=256 segments=41
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.004378331 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001298568 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000844777 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001249075 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001184455 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001199241 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.00117705 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.001200624 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000898136 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000861046 success=true
DEBUG melodee_search::store: cached search reader reloaded elapsed_seconds=0.000887837 success=true
{"workload":"postgres-search-incremental","trial":3,"build_debug_assertions":false,"fixture":{"artists":35000,"releases":50000,"tracks":600000},"events":2048,"artist_events":1024,"release_events":512,"track_events":512,"batches":8,"batch_size":256,"elapsed_seconds":0.497615159,"events_per_second":4115.630247510205,"sqlx_statement_events":164,"max_end_to_end_outbox_lag_ms":689.039,"batch_elapsed_ms":[67.472298,67.12288199999999,66.391149,63.48431,64.437781,65.604942,48.213869,54.885595],"query_elapsed_ms":[5.407684000000001,2.076038,1.586168,2.1503870000000003,2.0413129999999997,2.0774399999999997,2.107316,2.020884,1.653254,1.571701,1.6751960000000001],"peak_process_rss_before_kib":379948,"peak_process_rss_after_kib":418168,"limitations":"three consecutive trials on one process and catalog; mixed upserts only; no concurrent activity or physical media; SQL setup excluded; query timings in-process; SQLx events count statements, not network round trips; RSS is process high-water including the full rebuild"}
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 35.41s

exit_code=0
```
