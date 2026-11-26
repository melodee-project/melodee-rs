# Scoped identity reconciliation measurement, 2026-11-26

This is a single exploratory run, not release acceptance or a before/after
comparison. The release-mode test executable was built from an **unrecorded
dirty working tree** based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`.
Its SHA-256 was
`62a9ef2d928873436d28ca34874d17d6d5df2b086ae5c6a4c364b3a16a06a19a`.
The fixture runner rebuilt it while other work was in progress; exact source
identity was not captured. The pre-remediation revision was not measured.

The invocation-owned, migrated fixture contained 35,000 Artists, 50,000
Releases and 600,000 Tracks before the test added two Artists and two
observations. It contained one two-Artist strong-ID candidate and one two-Artist
repeated-credit group, with four initial affected Artist seeds. The base
fixture had no representative catalog-wide observations, track credits,
external Artist or Release identities, or negative assertions. It used
synthetic metadata and asset references; it did not use physical media.

Host: AMD Ryzen 9 3900X, 24 logical CPUs, 65,727,588 KiB reported memory,
Linux 7.3.4-300.fc45 x86-64. The repository was on btrfs; database storage was
in the configured PostgreSQL Docker volume, whose underlying storage was not
independently characterized. PostgreSQL was the development `postgres:18.4-alpine`
container. Detailed PostgreSQL settings and cache state were not captured.
The candidate queries ran sequentially, and the plan executions ran immediately
after each scoped query; they were not cold-cache samples. The connection pool
allowed four connections, with a 20-second per-statement timeout. Fixture
construction took 163.2 seconds and is excluded from measurements below.

| Candidate query | Scoped, ms | Global, ms | Returned rows |
| --- | ---: | ---: | ---: |
| Strong Artist | 9.068 | 4.313 | 1 |
| Same-Release Track Artist | 15.571 | 62.567 | 0 |
| Repeated Artist credit | 7.530 | 38.826 | 1 |
| Name-only proposal | 6.696 | 3.032 | 2 |
| Strong Release | 2.583 | 1.881 | 0 |

All five scoped and global result vectors matched in this fixture; no query
timed out. One production drain took 170.448 ms, merged two Artists, and left
four newly generated closure seeds. A lower-ID seed held uncommitted while a
higher-ID seed was drained remained durable after it committed; the visible
seed was acknowledged. That second drain took 48.819 ms. This exercises the
late-commit race but does not establish behavior under concurrent imports,
interruptions or general global-oracle equivalence.

The exact scoped statements emitted by SQLx tracing were executed again with
identical bind values using `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`. Their
complete plans are in [identity-scoped-2026-11-26-plans.json](identity-scoped-2026-11-26-plans.json).
The additional `EXPLAIN ANALYZE` execution had planning/execution times of
4.255/0.728 ms (strong Artist), 10.660/1.377 ms (same-Release), 4.305/0.464 ms
(repeated credit), 3.775/0.783 ms (name proposal), and 1.243/0.070 ms (strong
Release). Those are separate warm executions, not the application timings.

The same-Release and repeated-credit plans used indexed Artist-name lookups.
However, the plans also include sequential scans of observations, current and
historical decisions, Track credits, identity evidence and negative assertions;
these relations contained only zero to two rows in this fixture. Strong Artist
and strong Release evidence were likewise nearly empty. Therefore the plans do
**not** establish that discovery follows affected relationships on a populated
production-shaped identity graph or rule out a catalog-wide scan. The timings
and vectors do not satisfy RM-09's proportional-work, full equivalence,
concurrency or recovery acceptance criteria.
