# Policy catalog diagnostic, 2026-11-26

This diagnostic measures production `PolicyStore::load_snapshot` and
`load_test_contexts` followed by `evaluate_catalog_context`, against separately
created invocation-owned PostgreSQL databases. Eight Releases are created via
the existing manifest, publication and catalog APIs; each has one Track. One
of their acoustic digests has a matching evidence row on another Release. The
test checks that the normal and dry-run evaluations return the same
first-Release disposition and include that match. The loader is the same path
used by the operator CLI, but the CLI and its persisted test-summary write are
not measured. The `EXPLAIN ANALYZE` queries supplement the measurements; they
are not substitutes for those production method calls.

Both fixtures used 35,000 Artists, 50,000 Releases, 600,000 Tracks and 600,000
synthetic background acoustic evidence rows, before creating the eight valid
handoffs and one additional matching evidence row. Data generation rules and
test source are identical; UUIDs vary. This is not a multi-Track Release or a
maximum-size dry-run case. It does not exercise media files, policy worker
scheduling, concurrent import/rebuild/UI traffic, TCP, fault recovery, or the
CLI summary write. It reports SQLx top-level rows, not network wire bytes;
the separate retained digest element count and string-byte count expose arrays
nested within dry-run rows. Client RSS and high-water values are whole-process
measurements including fixture setup and allocator retention; they are not an
isolated operator memory peak.

| Measurement | Original baseline | Current working tree |
|---|---:|---:|
| Single snapshot + evaluate, n=3 | 1,285.567–1,472.095 ms | 7.894–8.804 ms |
| SQLx statements / top-level returned rows per single evaluation | 13 / 600,014 | 13 / 7 |
| Retained acoustic digest elements / string bytes, single evaluation | 600,008 / 38,400,512 | 1 / 64 |
| Eight-Release dry-run load + evaluations, n=3 | 16,305.331–16,756.116 ms | 390.060–418.873 ms |
| SQLx statements / top-level returned rows per dry-run | 3 / 10 | 3 / 10 |
| Retained acoustic digest elements / string bytes, eight contexts | 4,800,057 / 307,203,648 | 1 / 64 |
| Whole-process high-water RSS after three dry-run samples | 624,808 KiB | 8,296 KiB |

The separate diagnostic global acoustic query plan on the baseline returned
600,008 rows, read 32,087 shared blocks and wrote 21,516 temporary blocks in
858.350 ms. On the current database, the scoped query returned one row with 43
shared hits, no shared reads and no temporary writes in 6.221 ms. Plans and
all samples are in the raw logs below; these two queries were run in a fixed
order after the timed methods with uncontrolled and unequal cache warmth. The
plans are synthetic diagnostics; they are not measured network transfer.

Environment: AMD Ryzen 9 3900X (12 cores/24 logical CPUs), 62 GiB RAM, Linux
7.3.4 Fedora x86-64, NVMe-backed btrfs, PostgreSQL 18.3 with 128 MiB
`shared_buffers`, 4 MiB `work_mem`, 100 `max_connections`, and READ COMMITTED.
Rust 1.99.0 release profile, `CARGO_BUILD_JOBS=2`, `RUST_TEST_THREADS=1`, one
pool connection for the measured test. One database was generated for each
revision, three serial samples per case, baseline first. Cache state and other
host activity were uncontrolled; elapsed ranges are observations, not SLOs or
independent percentile estimates. Setup and diagnostic plans are excluded
from each stated timed method; the fixture and benchmark include eight extra
real handoffs beyond the background cardinalities. No timeout was observed.

The baseline used detached revision `a0979cde1fd2aeff5b8027d9d59336b678f1aa12`
through migration 0059; current was a dirty working tree based on
`08de08d50cb1be6e76c66000232dae257c1a2d2f` with later migrations through 0066.
The exact overall current source state during compilation was not captured;
binary and selected source hashes below identify artifacts, not a final commit.
The two revisions use identical `catalog_common` and `phase5_common` fixture
helper bytes. This is not final integrated acceptance evidence.

| Artifact | SHA-256 |
|---|---|
| Baseline log | `be58e335c70f560aa56773309d7dbb9501574afc36e82c71ed96003bd4a4c409` |
| Current log | `1425f671931c4dc4198220d9ff06ffc3d53c4f17bdc0dcee47dfbdabbb056a20` |
| Baseline test executable | `3cac6f8a66f517234819a821f512acbd30acbb2af03220114594d84c41dfe776` |
| Current test executable | `bed96c5186cc9b9ae6e57e81a37c11964816feccffa18adcdb1d58c33318b96f` |
| Identical benchmark test source | `1b7ff381fae2caaeb09e9ede66544517092fdd4b31ebe259b53ea1cbc50e11fe` |
| Fixture script | `3e6b5305a7d420aea2881141862c3d4aa4acffe5282c89e4df106a2da7183b7d` |
| Policy runner | `f92282cf6b06ba5e86c2434756510a847e00bb8728b15c9d47610ab24590c6cc` |

Raw sanitized outputs: [baseline](policy-baseline-2026-11-26.log),
[current](policy-current-2026-11-26.log). For reproduction, first create the
detached original-revision worktree and build its debug migration CLI; run
`python3 scripts/prepare_perf_policy_baseline.py --worktree PATH_TO_BASELINE`,
then build both release tests with `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo
test --release -p melodee-policy --test perf_catalog_policy --no-run` from
each respective worktree. Run the baseline from the main checkout with:

```text
python3 scripts/perf_representative.py --seed-timeout 600 --migration-cli PATH_TO_BASELINE/target/debug/melodee-cli -- python3 scripts/run_perf_policy_case.py --baseline-worktree PATH_TO_BASELINE --test-binary PATH_TO_BASELINE/target/release/deps/PERF_TEST --timeout-seconds 600
```

Run the current case with:

```text
python3 scripts/perf_representative.py --seed-timeout 600 -- python3 scripts/run_perf_policy_case.py --test-binary target/release/deps/PERF_TEST --timeout-seconds 600
```

Replace `PERF_TEST` with the test executable emitted by Cargo. Both fixture and
workload have explicit 600-second limits; a wrapper timeout is censored and
returns 124. The fixture only creates and drops its UUID-owned database and
private temporary asset root.
