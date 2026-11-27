# Bounded identity closure on 600K Tracks, 2026-11-27

One release-build production-drainer workload ran against an invocation-owned
catalog with 35,000 Artists, 50,000 Releases and 600,000 Tracks. This is an
intermediate correctness/performance sample, not final integrated acceptance,
a pre-remediation comparison, or an end-to-end import measurement. The base
fixture was created in 184.6 seconds; identity augmentation time was not
recorded separately or included in the timings below. The fixture includes
synthetic ratings, ACL/provenance placeholders, alternate-asset references and
one generated artwork object. Those placeholders are not real import receipts;
no 600K-file audio corpus or media processing was exercised.

The isolated, write-protected source worktree was based on dirty HEAD
`37f29889fb05d1825faecb8b597387f0eb971b5e`. All 573 Rust, SQL, Cargo and
embedded input hashes matched between the separately recorded release
[migration CLI build](identity-closure-cli-build-2026-11-27.json) and
[identity test build](identity-closure-test-build-2026-11-27.json). Their
binary SHA-256 values were
`29825cb1c31ea19325ff34dfe1fd1c495f68ab864fe6df76b899fa1770405093` and
`58c308d9899b19c71e067ce1388b4a1409872b02103644e8aa75e8f5b04df0b8`,
respectively. Three Python scripts were updated in the private snapshot after
the CLI build to include reviewed fixture and owned-process safety fixes, and
one collector parser was subsequently fixed for the exact libtest prefix. The
separately hashed [first](identity-closure-script-reattestation-2026-11-27.json),
[second](identity-closure-script-reattestation-v2-2026-11-27.json), and
[third](identity-closure-script-reattestation-v3-2026-11-27.json) records
preserve those changes. The fixture validated the migration CLI build proof
before and after migration, and the collector checked its proof digest and the
actual executable hash. These hashes do not exclude transient edits or attest
all dependency binaries. The matching [tiny smoke](identity-closure-smoke-2026-11-27.json)
ran before this case.

After augmentation, the measured database held 650,004 observations and
current Artist decisions, 601,002 Track credits, 11,087 Artist external IDs
and 16,668 Release external IDs. The synthetic candidate scenario starts with
8 durable seeds referencing 6 Artist IDs and 2 Release IDs. One bounded
production turn took **3,263.770 ms**, recorded three Artist merges and one
Release merge, and left 68 seeds. The first closure sample showed **zero of
the 8 original seed IDs remained**: the 68 were follow-up work, rather than
unacknowledged originals. One subsequent bounded turn consumed the remaining
queue; the closure sample reached zero in **32.029 ms** (32.036 ms including
the sampling overhead). The number 68 counts seed rows, not distinct identity
relationships. The evidence establishes convergence for this particular
synthetic closure, not a bound for all catalog graphs or interrupted sessions.

| Production candidate query | Scoped ms | Global ms | Rows |
| --- | ---: | ---: | ---: |
| Strong Artist | 7.488 | 55.209 | 1 |
| Same-Release Track Artist | 19.491 | 2,930.589 | 1 |
| Repeated Artist credit | 6.478 | 1,357.601 | 1 |
| Name-only proposal | 5.119 | 89.211 | 4 |
| Strong Release | 2.188 | 18.230 | 1 |

Each scoped vector matched the corresponding untruncated global vector. The
[raw sanitized result and ten scoped/global EXPLAIN plans](identity-closure-600k-2026-11-27.json)
include SQL timing, buffer counts, closure samples, source/script hashes and
limitations. The exact candidate SQL was re-executed for plans with identical
binds. This is one sequential sample per query: global statements follow their
scoped counterparts, cache state was uncontrolled, and plan executions were
separate. PostgreSQL configuration, database-volume storage, database resource
consumption and physical I/O were not captured. Host: AMD Ryzen 9 3900X,
24 logical CPUs, 65,727,588 KiB RAM, Linux 7.3.4-300.fc45 x86-64; source
snapshot on tmpfs. Results do not establish p95/p99, concurrent imports, or
whole-system throughput. The separate small PostgreSQL
`identity_scoped_oracle` regression covers scope exclusion, manual and
negative decisions, redirects, a multi-hop closure, interruptions, late seeds,
and production global-startup outcomes; it is correctness evidence, not a
representative workload.
