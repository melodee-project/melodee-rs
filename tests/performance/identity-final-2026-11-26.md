# Identity and scrobble plan measurement, 2026-11-26

This is one exploratory release-build run, not release acceptance or a
pre-remediation comparison. The test binary SHA-256 was
`1bab27c35c3fe5398a3096704053491acc7d206e8aa06889d35ddf89706096be`.
It was built on a dirty working tree based on
`8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`, not a committed revision.
[Source hashes](identity-final-source-hashes-2026-11-26.json) record 522 Rust,
SQL and Cargo files immediately before compilation and changes through its end.
Only the separate `catalog_hard_delete.rs` integration test changed during the
build. These hashes support comparison with a later checkpoint; they do not
preserve a full source snapshot or prove an identical final implementation.
The runner invoked the prebuilt executable directly.

The invocation-owned catalog had 35,000 Artists (8,051 active), 50,000 Releases,
600,000 Tracks, 50,000 Release observations, 600,000 Track observations,
650,000 current decisions, 600,000 Track credits, 11,085 Artist external IDs,
16,666 Release external IDs and 349 same-name groups before the test scenario.
The test added two Artists and Release observations, one pair of strong Artist
IDs, one repeated-credit pair, one pair of strong Release IDs, and 1,000 extra
primary credits on a separate Track for the cursor plan. Names and IDs are
synthetic. The corpus has no representative negative assertions, manual locks,
redirect histories, multiple ordinary Track credits or transitive clusters.

The host was an AMD Ryzen 9 3900X with 24 logical CPUs and 65,727,588 KiB
reported RAM, Linux 7.3.4-300.fc45 x86-64, and PostgreSQL 18.4-alpine. The
repository was on btrfs; the PostgreSQL Docker volume's underlying storage was
not independently characterized. Detailed PostgreSQL settings and cache state
were not captured. A four-connection pool used a 20-second statement timeout.
The base fixture took 151.4 seconds to construct; augmentation time was not
separately recorded and neither is in the workload timings. Queries and plan
probes were sequential single samples without cold-cache control.

| Production candidate query | Scoped, ms | Global, ms | Returned rows |
| --- | ---: | ---: | ---: |
| Strong Artist | 8.783 | 59.828 | 1 |
| Same-Release Track Artist | 18.612 | 3,046.334 | 0 |
| Repeated Artist credit | 11.085 | 1,239.567 | 1 |
| Name-only proposal | 5.159 | 775.952 | 2 |
| Strong Release | 2.811 | 18.553 | 1 |

All scoped/global result vectors matched in this narrow scenario. Each exact
scoped SQL statement was captured from SQLx and run again under
`EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` with the same parameters; complete
plans are in [identity-final-2026-11-26-plans.json](identity-final-2026-11-26-plans.json).
Their separate planning/execution times were 5.454/0.779 ms, 12.284/1.833 ms,
4.222/0.373 ms, 3.997/0.140 ms and 1.602/0.066 ms in table order. These are
separate, warmer executions than the application timings. The sampled plans
used indexed Artist-name, observation, current-decision, Track-credit, Artist
provider-key and Release provider-key lookups, with sequential scans on small
seed and empty auxiliary relations. The measured case supports scoped access
for these seeds; it does not prove bounds under all identity distributions.

One production drain took 2,365.982 ms with six initial seeds, two Artist
merges, one Release merge and 62 newly retained seeds. A separate 50.481 ms
drain preserved a lower-ID seed committed after capture and acknowledged a
higher-ID seed. The timings are neither import throughput nor general
concurrent-import, crash/retry or global-oracle acceptance evidence.

The same executable extracted the then-current phased scrobble cursor's SELECT
body and executed full `EXPLAIN ANALYZE` plans. The ordinary Track had one
primary credit and produced two rows: 0.189 ms planning, 0.063 ms execution.
The pathological Track had 1,001 primary credits and produced 2,002 rows:
0.090 ms planning, 1.716 ms execution. Both plans contained an outer Sort above
Append and an inner Sort in each branch, despite index-only credit scans. The
pathological outer quicksort used 127 KiB in this run. These results do not
measure incremental FETCH latency or locks, and do not establish bounded
streaming or behavior at larger cardinalities. They apply to the hashed source
state above, not a later cursor redesign.
