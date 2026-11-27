# Identity and NOWAIT scrobble cursor plan, 2026-11-26

This is one exploratory release-build run, not release acceptance or a
pre-remediation comparison. The prebuilt executable SHA-256 was
`0d5a5de459b67f40cb557b5578d2784d8660c8e58725407f5cc6f02610c95aaf`.
It was built from a dirty working tree based on
`8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`, not a committed revision. The
[build manifest](identity-nowait-source-hashes-2026-11-26.json) records hashes
for 524 Rust, SQL and Cargo files before and after compilation; no changes were
detected during that interval. The workload scripts are separately hashed in
that manifest. These hashes are not a complete committed source snapshot.

The disposable database held 35,000 Artists (8,051 active), 50,000 Releases,
600,000 Tracks, 50,000 Release observations, 600,000 Track observations,
650,000 current decisions, 600,000 Track credits, 11,085 Artist external IDs,
16,666 Release external IDs and 349 same-name groups before the test scenario.
The test then added two Artists and Release observations, one strong Artist
pair, one repeated-credit pair, one strong Release pair, and 1,000 additional
primary Artist credits on one Track. Names and provider IDs are synthetic.

The host was an AMD Ryzen 9 3900X with 24 logical CPUs and 65,727,588 KiB
reported RAM, Linux 7.3.4-300.fc45 x86-64, and PostgreSQL 18.4-alpine. The
repository was on btrfs; the PostgreSQL Docker volume's underlying storage was
not independently characterized. Detailed PostgreSQL settings and cache state
were not captured. A four-connection pool used a 20-second statement timeout.
Base fixture creation took 152.9 seconds; augmentation duration was not
separately recorded. Neither is part of the workload timings. Queries ran as
single sequential samples without cold-cache control.

| Production candidate query | Scoped, ms | Global, ms | Returned rows |
| --- | ---: | ---: | ---: |
| Strong Artist | 6.175 | 57.260 | 1 |
| Same-Release Track Artist | 15.527 | 2,968.200 | 0 |
| Repeated Artist credit | 7.122 | 1,317.434 | 1 |
| Name-only proposal | 5.614 | 90.291 | 2 |
| Strong Release | 2.451 | 17.008 | 1 |

All scoped/global result vectors matched in this narrow scenario. Each scoped
SQL statement was captured from SQLx and run again under `EXPLAIN (ANALYZE,
BUFFERS, FORMAT JSON)` with identical parameters. Complete plans are in
[identity-nowait-2026-11-26-plans.json](identity-nowait-2026-11-26-plans.json).
The separate plan executions' planning/execution times were 4.498/0.595 ms,
11.645/1.526 ms, 4.314/0.359 ms, 4.039/0.145 ms and 1.570/0.084 ms in table
order. Plans are separate, warmer executions. They support indexed access for
these sampled seeds; they do not prove bounded work or result equivalence for
all identity distributions.

A production drain took 2,459.191 ms with six initial seeds, two Artist
merges, one Release merge and 62 new retained seeds. A separate 51.564 ms drain
retained a lower-ID seed committed after capture and acknowledged a higher-ID
seed. This is not end-to-end import, full concurrent-import load, crash/retry,
or general global-oracle acceptance evidence.

The benchmark also extracted the current unsorted Artist cursor SELECT and
executed full `EXPLAIN ANALYZE` plans. The ordinary Track had one primary credit:
0.047 ms planning and 0.009 ms execution. The pathological Track had 1,001
primary credits: 0.030 ms planning and 0.194 ms execution. Both plans were a
single index-only scan on `track_artists_primary_artist_order_idx`; neither
contained Sort or Materialize. This shows the selected plan on this fixture,
not a guarantee for every PostgreSQL plan or data distribution. It does not
measure incremental FETCH latency, lock waits, NOWAIT retry behavior, full
scrobble performance, or concurrent playback. The earlier phased cursor's
separate [smoke result](scrobble-cursor-smoke-2026-11-26.md) used a different
fixture size and is not a comparable before/after measurement.

The synthetic corpus lacks realistic negative assertions, manual locks,
redirect histories, multiple ordinary Track credits and transitive clusters.
The global queries ran after their respective scoped query and plan probe, so
their cache conditions may differ. No sample percentiles, pre-remediation
baseline, broad identity oracle or large-scale fault evidence were collected.
