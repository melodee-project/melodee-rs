# Scoped identity discovery at two catalog sizes, 2026-11-27

These are two sequential, single-sample **intermediate** release-build
measurements, not final integration acceptance or a pre-remediation baseline.
The 60K case used 3,500 Artists, 5,000 Releases and 60,000 Tracks; the 600K
case used 35,000, 50,000 and 600,000. Both used the same prebuilt executable,
the same fixture and workload scripts, and an affected graph of the same
logical shape; generated UUIDs differ. They used independent invocation-owned
PostgreSQL databases. Base-fixture creation took 18.2 and 178.9 seconds,
respectively; those durations are not included in the query or drain times.
Identity augmentation time was not recorded separately.

The test and migration CLI were release builds from the same 569-file
fingerprint snapshot, based on dirty HEAD
`37f29889fb05d1825faecb8b597387f0eb971b5e`. Their build records and exact
binary digests are in the [test build](identity-scaling-test-build-2026-11-27.json)
and [CLI build](identity-scaling-cli-build-2026-11-27.json) records. The
workloads ran in a separate write-protected detached worktree whose 569 input
hashes, attested script hashes, HEAD and copied binary hashes were checked
against these records. The test executable SHA-256 was
`529f9cefe19396b634ee72fb23ee1cd20af28adfa0340d3c8225d2a38ea7b2ee`;
the CLI SHA-256 was
`145ca42a7cd28a2b971a407072e2562f10591e8bb3e8f5767ff24d038f6628ad`.
The case collector recorded the CLI hash supplied by the fixture but did not
automatically consume the CLI build proof. That linkage was independently
checked by comparing the captured CLI hash and complete source dictionary to
the build record. This distinction is explicit in each JSON capture. The
current shared harness now supports automated CLI-proof validation for a future
final-source run; that later code was not used in these measurements.

After scenario setup the smaller/larger cases had 65,004/650,004 Artist
observations and current decisions, 61,002/601,002 Track credits,
1,112/11,087 Artist external IDs and 1,668/16,668 Release external IDs. The
fixture has no representative negative decisions, manual locks or redirect
histories. It includes a synthetic, nonempty same-Release Track-credit pair,
a repeated-credit pair, strong Artist and Release pairs, and name-only review
proposals. The source snapshot and temporary workspace were on tmpfs; the
PostgreSQL volume's underlying storage was not independently characterized.
Host information read during the run: AMD Ryzen 9 3900X, 24 logical CPUs,
65,727,588 KiB RAM, Linux 7.3.4-300.fc45 x86-64. PostgreSQL version, settings,
server resource consumption and cache residency were not captured for these
runs and are not inferred here.

| Production candidate query | 60K scoped ms | 60K global ms | 600K scoped ms | 600K global ms | Returned rows |
| --- | ---: | ---: | ---: | ---: | ---: |
| Strong Artist | 9.476 | 8.310 | 7.517 | 55.138 | 1 |
| Same-Release Track Artist | 14.821 | 258.924 | 16.575 | 2,899.335 | 1 |
| Repeated Artist credit | 5.058 | 89.219 | 9.784 | 1,385.596 | 1 |
| Name-only proposal | 5.159 | 12.538 | 5.605 | 748.443 | 4 |
| Strong Release | 2.393 | 2.112 | 2.142 | 16.158 | 1 |

The five scoped/global result vectors matched at each size. The captured
production statements were then rerun separately with `EXPLAIN (ANALYZE,
BUFFERS, FORMAT JSON)` and identical binds. Their top-level shared buffer
hit/read counts are below, not elapsed-query measurements. Parent-node counts
include child accesses; repeated hits need not be distinct pages, and a
PostgreSQL read can still be served by the operating-system cache. Plans are
separate executions and the global query follows scoped execution, so cache
conditions are uncontrolled.

| Query | 60K scoped hit/read | 60K global hit/read | 600K scoped hit/read | 600K global hit/read |
| --- | ---: | ---: | ---: | ---: |
| Strong Artist | 96 / 0 | 220 / 0 | 68 / 0 | 1,604 / 0 |
| Same-Release Track Artist | 689 / 0 | 625,669 / 0 | 853 / 0 | 6,238,606 / 37,623 |
| Repeated Artist credit | 191 / 0 | 170,632 / 0 | 213 / 0 | 1,814,860 / 86,454 |
| Name-only proposal | 103 / 0 | 4,226 / 0 | 124 / 0 | 1,358,993 / 46,789 |
| Strong Release | 74 / 0 | 37 / 0 | 24 / 0 | 310 / 0 |

Full sanitized timings, candidate counts and plans are in the
[60K case](identity-scaling-60k-2026-11-27.json) and
[600K case](identity-scaling-600k-2026-11-27.json). Each production drain
started with eight seeds referencing six Artist IDs and two Release IDs,
recorded three Artist merges and one Release merge, and left **68 durable
seeds**. The drain took 657.015 ms and 3,122.793 ms, respectively. This is one
bounded processing turn, not completion of all closure work: the recorded
remaining count does not distinguish captured from newly enqueued seeds or
measure the number of later turns. The separate
`identity_scoped_oracle` correctness tests exercise bounded convergence,
transitive discovery, preserved decisions, interruptions and seed retention
on a small catalog, not representative closure throughput.

These plans and timings support affected-relationship access for this sampled
graph while the global queries touch substantially more buffers for several
predicates. They do not prove asymptotic bounds for every data distribution,
provide p95/p99, exercise concurrent imports or actual media, or replace the
mandatory representative end-to-end workload. The two samples use different
fresh databases and uncontrolled caches; no before/after claim is made.
