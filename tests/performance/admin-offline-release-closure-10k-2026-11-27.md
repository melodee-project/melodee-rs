# Provisional offline Release-media closure: 10,000 Tracks

This is a single measurement of a **disconnected persistence prototype**. It is
not RM-00, a merge/split benchmark, a 560K–600K catalog measurement, an import,
a filesystem deletion, or evidence that the private capability/finalization
protocol is complete. No source or production media was read or deleted.

On 2026-11-27, the following command passed against a freshly migrated,
UUID-named, disposable PostgreSQL test database. The test dropped its database
on completion. It created one synthetic Release with 10,000 Track rows and
10,000 media-asset references; it did not create corresponding physical media
files. The fixture used generated UUIDs rather than a fixed reproducibility
seed. It captured the provisional scope through the actual persistence APIs,
checked exact cardinality and the 16-entry page bound, then prepared and
revalidated one intent. It did not acknowledge or execute a filesystem page.

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release --locked \
  -p melodee-persistence --test catalog_hard_delete \
  maintenance_release_media_closure_10000_tracks_measurement -- --ignored --nocapture
```

| Observation | Actual result |
| --- | ---: |
| Samples | 1 |
| Track rows / captured media entries | 10,000 / 10,000 |
| Capture turns, including closure phase transitions | 629 |
| Snapshot capture elapsed, after fixture creation | 22,999 ms |
| Entire timed test-command wall time | 25.94 s |
| Timed Cargo/test process maximum RSS | 80,844 KiB |
| Timed Cargo/test process user / system CPU | 0.21 s / 0.24 s |

The process RSS excludes PostgreSQL and filesystem cache. The 39.41-second
release compilation ran separately and is excluded from the timed command.
SQL statement counts, lock duration/waits, PostgreSQL resource usage and query
plans were not collected. Cache state was uncontrolled; this is not a cold-cache
result or a before/after comparison. A single sample establishes neither a
latency distribution nor a performance budget.

The test ran from a dirty workspace based on commit
`37f29889fb05d1825faecb8b597387f0eb971b5e`; that commit alone does not identify
the measured source. These SHA-256 hashes identify the principal measured
inputs at execution time:

```text
1f46c793b1bd5a8040f2d82337b66d1ef93b9db8d83c8b62b3e5b4624f0e9f2e  crates/melodee-persistence/src/admin_media_snapshot.rs
a8720170739a93412738cf3bf3e22842e21427d523ff4526812061858e2972a7  crates/melodee-persistence/src/admin_media_page_intent.rs
0db58a062b8bad0b7c2b22ec27fe1c23f23cb93976e9692adeaedf8b9261b4fa  crates/melodee-persistence/migrations/0071_custody_hash_spool.sql
4e8239b85f8727c3b31bad699c108ddba3d59312ed5dd795470c415ee9dce086  crates/melodee-persistence/tests/catalog_hard_delete.rs
```

The development host reported an AMD Ryzen 9 3900X, 24 logical CPUs,
65,727,588 KiB total memory, x86-64 Linux kernel `7.3.4-300.fc45.x86_64`, and
Btrfs on an NVMe workspace volume. PostgreSQL reported version 18.3,
`shared_buffers=128MB`, `work_mem=4MB`, `effective_cache_size=4GB`,
`max_connections=100`, `fsync=on`, and `synchronous_commit=on`. PostgreSQL's
actual data-volume topology and cache state were not established. The test
used one serialized test-harness thread and the repository's default test work
configuration; the set-based fixture generator ran before the timed capture.

The shared filesystem-custody proof, all lock participation, durable per-page
filesystem execution, atomic catalog finalization, and representative
end-to-end acceptance remained incomplete when this observation was collected.
