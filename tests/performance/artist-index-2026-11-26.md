# Artist index diagnostic, 2026-11-26

This is one measured run on the dirty working tree based on commit
`8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`. The build was not from a clean commit, and other edits were in progress during
compilation; its exact source identity has not been established. No before/after endpoint comparison was collected. The EXPLAIN queries
isolate rating aggregation and are not full route plans. Their one sample each
ran in the displayed order after seeding, with uncontrolled and unequal cache
warmth. Timings do not establish a general performance guarantee.

Fixture: 35,000 artists, 50,000 releases, 600,000 tracks, 100 synthetic users,
175,000 artist states (including zero ratings), deterministic arithmetic data;
50,000 releases and 12 tracks per release. Seeding and route execution use the
same invocation-owned disposable PostgreSQL database. The fixture has no physical
media, real identity relationships, or catalog ACL bindings. Artist states
are synthetic rather than observed production distributions.

Environment: AMD Ryzen 9 3900X (12 cores/24 threads), 62 GiB RAM, Linux
7.3.4 Fedora x86_64, NVMe-backed btrfs with zstd compression. PostgreSQL 18.3,
shared_buffers 128 MiB, work_mem 4 MiB, max_connections 100. Rust 1.99.0,
Cargo release profile, `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`. Six serial
samples per route/format include bcrypt authentication, router dispatch, SQL,
serialization and in-process body collection; sample 0 is a warm-up candidate
but remains in the raw data. No TCP transport, concurrent ingestion, latency
percentiles, CPU, RSS or disk usage were measured. Cache conditions are
uncontrolled. The compared SQL plans precede the route samples.

Reproduce after building the migration CLI:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-server --test perf_artist_index --no-run
python3 scripts/perf_representative.py -- cargo test --release -p melodee-server --test perf_artist_index -- --ignored --nocapture
```

Raw output follows; transient synthetic UUIDs are redacted.

```text
Seeded 35000 artists, 50000 releases, 600000 tracks in 158.3s

running 1 test
plan correlated:
Seq Scan on artists artist  (cost=0.00..159448.50 rows=35000 width=24) (actual time=0.352..114.312 rows=35000.00 loops=1)
  Filter: (status <> ALL ('{merged,deprecated}'::text[]))
  Buffers: shared hit=103888 read=1749 written=904
  SubPlan 1
    ->  Aggregate  (cost=4.51..4.53 rows=1 width=8) (actual time=0.003..0.003 rows=1.00 loops=35000)
          Buffers: shared hit=103888 read=1113 written=626
          ->  Index Only Scan using native_artist_user_states_positive_rating_idx on native_artist_user_states ratings  (cost=0.42..4.50 rows=4 width=2) (actual time=0.001..0.002 rows=4.40 loops=35000)
                Index Cond: (artist_id = artist.id)
                Filter: (user_id <> '<synthetic-user-id>'::uuid)
                Rows Removed by Filter: 0
                Heap Fetches: 0
                Index Searches: 35000
                Buffers: shared hit=103888 read=1113 written=626
Planning:
  Buffers: shared hit=466 read=22
Planning Time: 5.136 ms
Execution Time: 115.578 ms
plan grouped:
Hash Left Join  (cost=6885.87..8051.25 rows=35000 width=24) (actual time=76.207..87.258 rows=35000.00 loops=1)
  Hash Cond: (artist.id = ratings.artist_id)
  Buffers: shared hit=636 read=2161 written=1919
  ->  Seq Scan on artists artist  (cost=0.00..1073.50 rows=35000 width=16) (actual time=0.016..5.814 rows=35000.00 loops=1)
        Filter: (status <> ALL ('{merged,deprecated}'::text[]))
        Buffers: shared hit=636
  ->  Hash  (cost=6442.36..6442.36 rows=35481 width=24) (actual time=75.936..75.938 rows=35000.00 loops=1)
        Buckets: 65536  Batches: 1  Memory Usage: 2427kB
        Buffers: shared read=2161 written=1919
        ->  Subquery Scan on ratings  (cost=5555.33..6442.36 rows=35481 width=24) (actual time=52.583..70.866 rows=35000.00 loops=1)
              Buffers: shared read=2161 written=1919
              ->  HashAggregate  (cost=5555.33..6087.55 rows=35481 width=24) (actual time=52.582..68.753 rows=35000.00 loops=1)
                    Group Key: native_artist_user_states.artist_id
                    Batches: 1  Memory Usage: 5393kB
                    Buffers: shared read=2161 written=1919
                    ->  Seq Scan on native_artist_user_states  (cost=0.00..4786.00 rows=153866 width=18) (actual time=0.085..29.402 rows=153999.00 loops=1)
                          Filter: ((rating > 0) AND (user_id <> '<synthetic-user-id>'::uuid))
                          Rows Removed by Filter: 21001
                          Buffers: shared read=2161 written=1919
Planning:
  Buffers: shared hit=458 read=1 written=1
Planning Time: 1.424 ms
Execution Time: 89.223 ms
route=getArtists format=json iteration=0 elapsed_ms=444.661 bytes=9926426
route=getArtists format=json iteration=1 elapsed_ms=352.589 bytes=9926426
route=getArtists format=json iteration=2 elapsed_ms=363.501 bytes=9926426
route=getArtists format=json iteration=3 elapsed_ms=438.536 bytes=9926426
route=getArtists format=json iteration=4 elapsed_ms=401.830 bytes=9926426
route=getArtists format=json iteration=5 elapsed_ms=446.783 bytes=9926426
route=getArtists format=xml iteration=0 elapsed_ms=248.138 bytes=10330182
route=getArtists format=xml iteration=1 elapsed_ms=262.000 bytes=10330182
route=getArtists format=xml iteration=2 elapsed_ms=261.866 bytes=10330182
route=getArtists format=xml iteration=3 elapsed_ms=225.572 bytes=10330182
route=getArtists format=xml iteration=4 elapsed_ms=243.445 bytes=10330182
route=getArtists format=xml iteration=5 elapsed_ms=256.476 bytes=10330182
route=getIndexes format=json iteration=0 elapsed_ms=511.984 bytes=9926451
route=getIndexes format=json iteration=1 elapsed_ms=510.186 bytes=9926451
route=getIndexes format=json iteration=2 elapsed_ms=408.630 bytes=9926451
route=getIndexes format=json iteration=3 elapsed_ms=455.116 bytes=9926451
route=getIndexes format=json iteration=4 elapsed_ms=483.800 bytes=9926451
route=getIndexes format=json iteration=5 elapsed_ms=512.813 bytes=9926451
route=getIndexes format=xml iteration=0 elapsed_ms=396.904 bytes=9890464
route=getIndexes format=xml iteration=1 elapsed_ms=391.588 bytes=9890464
route=getIndexes format=xml iteration=2 elapsed_ms=392.065 bytes=9890464
route=getIndexes format=xml iteration=3 elapsed_ms=446.361 bytes=9890464
route=getIndexes format=xml iteration=4 elapsed_ms=487.264 bytes=9890464
route=getIndexes format=xml iteration=5 elapsed_ms=409.235 bytes=9890464
test measure_artist_indexes ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.60s


```
