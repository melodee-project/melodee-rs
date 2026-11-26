# Scoped identity reconciliation with populated relationships, 2026-11-26

This exploratory run is not release acceptance or a before/after comparison.
The release executable was built from an **unrecorded dirty working tree**
based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`, with SHA-256
`62a9ef2d928873436d28ca34874d17d6d5df2b086ae5c6a4c364b3a16a06a19a`.
The exact source diff at compilation was not captured. The runner executed
that prebuilt binary directly; the pre-remediation revision was not measured.

The disposable database contained 35,000 Artists (8,051 active), 50,000
Releases and 600,000 Tracks. The identity augmentation added 50,000 Release
observations, 600,000 Track observations, 650,000 current decisions, 50,000
Release credits, 600,000 Track credits, 11,085 Artist provider IDs, 16,666
Release provider IDs and 349 same-name groups. The timed scenario additionally
created two Artists and two Release observations: one strong Artist pair and
one repeated-credit pair. All provider IDs and names were synthetic.

Hardware and execution conditions were the same as the [sparse identity
run](identity-scoped-2026-11-26.md): Ryzen 9 3900X, 24 logical CPUs, Linux
7.3.4-300.fc45, PostgreSQL 18.4-alpine, four-connection pool, release build,
20-second statement timeout, sequential single samples, cache state not
controlled. The base fixture took 165.6 seconds to build; augmentation time
was not separately recorded. Neither is included in the query timings.

| Candidate query | Scoped, ms | Global, ms | Returned rows |
| --- | ---: | ---: | ---: |
| Strong Artist | 6.845 | 58.310 | 1 |
| Same-Release Track Artist | 15.120 | 2,899.674 | 0 |
| Repeated Artist credit | 7.029 | 1,268.991 | 1 |
| Name-only proposal | 5.809 | 795.096 | 2 |
| Strong Release | 2.491 | 13.255 | 0 |

All scoped/global vectors matched in this controlled case; no query timed out.
One production drain took 167.904 ms, merged two Artists and retained four new
closure seeds. The late lower-ID commit regression passed in a second 49.527 ms
drain. This is neither a concurrent-import load test nor broad proof of global
oracle equivalence. Strong Release had no affected Release seed and returned no
candidate, so its query comparison and plan do not measure useful Release
identity discovery.

The five scoped statements were captured from SQLx and separately executed
with identical parameters under `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`;
the complete plans are in [identity-populated-2026-11-26-plans.json](identity-populated-2026-11-26-plans.json).
Their separate planning/execution times were 4.545/0.703 ms (strong Artist),
11.362/1.418 ms (same-Release), 4.648/0.386 ms (repeated credit), 4.087/0.438 ms
(name proposal), and 1.715/0.049 ms (strong Release). Plans for the four Artist
paths used indexed access to the populated Artist, observation, current-decision,
Track-credit and Artist-ID tables in this scenario. Small seed and other empty
relation scans still occur. These plans support scoped access for the sampled
Artist groups, not a universal bound or a proof for other distributions.

The synthetic data has one credit per Track, mostly unique names, and no
representative negative assertions, manual locks, redirects or transitive
identity clusters. The global queries ran after their corresponding scoped
query and plan, without controlled cache conditions. Timings are not a
pre-remediation baseline, end-to-end import, multi-sample percentile or a
performance acceptance claim. RM-09 still requires the untested Release case,
mutation coverage audit, recovery/concurrency tests and broader oracle evidence.
