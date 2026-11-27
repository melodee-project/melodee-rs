# Intermediate admin statistics A/B measurement, 2026-11-27

The release-built authenticated admin statistics route was measured in-process
against one invocation-owned PostgreSQL fixture with 35,000 Artists, 50,000
Releases and 600,000 Tracks. The [sanitized result](admin-stats-ab-representative-2026-11-27.json)
contains every sample and source/build hashes. This is an intermediate snapshot
comparison, not final integrated-source or end-to-end import evidence.

The two builds differ only in `apps/melodee-server/src/admin_stats.rs`; the
candidate shares one grouped media-asset pass between headline totals and format
breakdown. The dirty snapshots are based on commit
`37f29889fb05d1825faecb8b597387f0eb971b5e`. Exact
[baseline](admin-stats-ab-prewarm-baseline-build-2026-11-27.json),
[candidate](admin-stats-ab-prewarm-candidate-build-2026-11-27.json), and
[migration CLI](admin-stats-ab-prewarm-cli-build-2026-11-27.json) proofs are
retained. The candidate's private target reused copy-on-write dependency files
from the baseline, but its server was explicitly recompiled; binaries differ.
The CLI's provisional ingestion build produced warnings and is not a final
quality-gate result. Build hashes do not attest dependencies/toolchain binaries
or rule out transient edits.

| Variant | Samples after warmup | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| Baseline | 16 | 615.140 ms | 640.197 ms | 640.197 ms |
| Candidate | 16 | 528.069 ms | 567.805 ms | 567.805 ms |

The candidate p50 was approximately 14.2% lower in this single ABBA sequence.
This is descriptive, not a confidence interval: there were two processes per
variant, the first response from each process was excluded as warmup, and the
nearest-rank p95 and p99 are both the maximum of 16 retained samples. Cache
residency was not controlled; no TCP traffic, simultaneous ingestion or slow
clients were exercised. One 24-logical-CPU Linux development host with about
62.7 GiB RAM was shared with an unrelated VM and desktop processes. Storage
latency and PostgreSQL settings were not recorded. The fixture had synthetic
ACL and publication references and no physical audio corpus or genuine import
receipts; its UUIDs were newly generated and no numeric random seed was used.
Fixture seeding took 187.3 seconds and is not import throughput.

The disposable database received `VACUUM (ANALYZE)` and then three consecutive
physical-size observations with no database-specific autovacuum worker; that
quiescence took 16.353 seconds. Every process prewarmed eight pool connections,
proved that a real ninth connection fails the stability gate, and then observed
exactly eight sessions three times before timing. The Rust test compares the
complete serialized response exactly across samples in each process after
validating and removing the time-varying RFC3339 `generatedAt` field. For
cross-process comparison only, `database.totalBytes` was also validated as a
nonnegative integer and replaced with an integer placeholder at its existing
key position: this PostgreSQL physical-size field changed across processes
in an earlier quiesced small smoke even though the query implementation was
identical. All remaining serialized fields, ordering, arrays and nulls matched;
`database.tables` and `poolInUse` were not excluded. The direct-child fixture
metadata is an execution assertion, not authentication against a malicious
same-UID actor.

Earlier [unquiesced failure](admin-stats-ab-representative-diagnostic-failed-2026-11-27.json),
[physical-size diagnosis](admin-stats-ab-representative-size-diagnostic-2026-11-27.json),
[quiesced pool-session failure](admin-stats-ab-representative-quiesced-failed-2026-11-27.json),
and [small cross-process size failure](admin-stats-ab-prewarm-diagnostic-failed-2026-11-27.json)
are retained rather than interpreted as performance measurements. Final source
and concurrent end-to-end verification remain open.
