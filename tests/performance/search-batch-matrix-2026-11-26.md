# Search outbox writer lifecycle and batch-size matrix, 2026-11-26

This is an intermediate, production-path measurement, not a final integrated
acceptance run and not a comparison against a long-lived writer. A release
`perf_outbox` test executed the real PostgreSQL outbox claim, batched hydration,
`apply_outbox_once` writer, Tantivy commit, and post-commit acknowledgement
paths. The fixture had 35,000 Artists, 50,000 Releases and 600,000 Tracks. An
initial production rebuild/activation preceded eight consecutive trials on one
generation. Each trial atomically changed 1,024 Artists, 512 Releases and 512
Tracks, then applied and acknowledged all 2,048 events without errors. Artist,
Release and Track content was independently queried after each drain. SQL
setup was excluded from drain time. Each drain was followed by eleven timed
in-process queries for the same Artist.

The mirrored sequence uses the existing supported `claim_batch_size` settings;
it does not change writer ownership, lease renewal, generation selection or the
commit-before-acknowledgement protocol. The sequence reduces but does not
remove time-order bias from evolving segment layout and warmed caches.

| Trial | Batch | Batches / commits | Drain s | Events/s | Max queue lag ms | Segments, first → last commit | RSS before → after KiB | Process HWM KiB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 16 | 128 | 10.023 | 204 | 10362.2 | 20 → 401 | 194908 → 463004 | 520080 |
| 2 | 64 | 32 | 3.098 | 661 | 3286.2 | 392 → 113 | 424596 → 461144 | 525840 |
| 3 | 256 | 8 | 0.814 | 2517 | 1017.8 | 104 → 41 | 420660 → 385716 | 525840 |
| 4 | 512 | 4 | 0.646 | 3171 | 842.1 | 38 → 29 | 371232 → 359980 | 525840 |
| 5 | 512 | 4 | 0.326 | 6273 | 562.5 | 29 → 29 | 336856 → 370836 | 525840 |
| 6 | 256 | 8 | 0.510 | 4016 | 738.7 | 32 → 41 | 379804 → 388636 | 525840 |
| 7 | 64 | 32 | 1.751 | 1170 | 2001.4 | 43 → 113 | 369956 → 480032 | 525840 |
| 8 | 16 | 128 | 11.335 | 181 | 11677.3 | 114 → 401 | 425696 → 500460 | 593304 |

RSS snapshots are the **whole benchmark process**, including rebuild, readers,
previous trials and allocator retention. HWM is the process high-water mark,
not a per-writer bound. Segment snapshots are counts after commits, not actual
merge counts. Lag includes setup-to-ack residence; it is not HTTP latency.
Exact unrounded samples and the full fixed-label lifecycle observations are in
[the sanitized JSON](search-batch-matrix-2026-11-26.json).

Across the two runs at each batch size, per-call timings were:

| Batch | Writer init count; median / max ms | Reader init count; median / max ms | Commit count; median / max ms | Commit reload median / max ms | Post-drain query median ms, 22 samples |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 16 | 256; 1.275 / 3.753 | 257; 22.864 / 60.231 | 256; 23.628 / 44.666 | 8.078 / 32.606 | 14.798 |
| 64 | 64; 1.093 / 2.299 | 64; 12.011 / 35.916 | 64; 20.767 / 42.930 | 4.415 / 13.166 | 5.702 |
| 256 | 16; 0.827 / 1.045 | 16; 5.225 / 13.367 | 16; 16.551 / 22.109 | 2.188 / 5.042 | 2.239 |
| 512 | 8; 0.808 / 0.952 | 8; 2.995 / 4.111 | 8; 20.617 / 24.223 | 1.285 / 1.584 | 1.437 |

The extra reader initialization in batch 16 includes the first query-side
read-only reader; writer and read-only index opens are classified separately
in the JSON. Query samples are serial repetitions of one identity after each
drain, not concurrent clients or stable tail-latency estimates. DEBUG
lifecycle tracing and SQLx event counting add unmeasured overhead.

This observation supports testing larger **bounded** claim batches as a safe
way to amortize the present writer/reader lifecycle in this particular mixed
workload; it does not prove that changing the default is appropriate under
other concurrency, fairness, memory and lease-duration conditions. The direct
writer initialization is only one part of the cost: reader initialization,
commit/reload and segment evolution are substantial at batch size 16. A
long-lived writer could change those costs, but has not been implemented or
measured, and would require generation, lease-loss, competing-writer,
cancellation and rollback fencing. These results do not close RM-13.

The release test binary SHA-256 was
`9708c07a441fb2091f7f8ae6b6163a555ce568dbb1761281e2ef8f89047838e8`.
The [build record](search-batch-matrix-build-2026-11-26.json) fingerprints its
before/after source snapshot and HEAD `37f29889fb05d1825faecb8b597387f0eb971b5e`;
the tree was dirty. Five custody/persistence files had changed by workload
start; their names and before/after hashes are in the result JSON. The fixture
migration CLI SHA-256 was
`453d8b5a2c00d0d229a6e99f99f2f6b6d39cd322756b6dfaf776ad909fee8c51`,
from a separately built snapshot. These are **not** a matching final integrated
binary and migration pair. PostgreSQL and host resources, active versions,
cache warmth and concurrent work were not remeasured by this collector; prior
host information must not be attributed to this run. No HTTP, other worker,
physical media, multiple processes, independent cache reset, or soak was tested.

For an integrated rerun, freeze source and build a fresh release test with
`python3 scripts/build_perf_test.py --target search-outbox --output <new-build.json>`.
Use the emitted executable basename and build record with the
`run_perf_search_matrix.py` collector as a direct child of
`perf_representative.py`, with a separately fingerprinted matching migration
CLI. The collector validates binary identity, bounds and removes raw logs,
removes its private search tree, and records incomplete runs as censored. The
outer fixture owns database cleanup. Both `--help` paths are safe.
