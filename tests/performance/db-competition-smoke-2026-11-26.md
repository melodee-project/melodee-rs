# Database competition harness smoke, 2026-11-26

This is a **36,000-Track diagnostic**, not the required representative workload
or a before/after comparison. The fresh, disposable fixture held 1,000 Artists,
3,000 Releases and 36,000 Tracks; five generated one-Track MP3/FLAC Releases
entered the regular ingestion workers while the server's startup search rebuild
and authenticated HTTP requests ran in the same process and PostgreSQL pool.
The requested pool limit was eight; stage concurrency was two. Sampling was at
0.5-second intervals, on one host, with uncontrolled caches.

The observed overlap between nonterminal ingestion and a running/validated
rebuild was about 2.07 seconds. Six completed admin-statistics requests in that
window had p50/p95/p99 **73.939/88.560/88.560 ms**; eight native Artist browse
requests had **3.950/6.839/6.839 ms**. These small, serial samples do not
support tail-latency conclusions. All five imports completed, the search job
activated, and neither probe reported an error. Thirteen database polls saw no
lock waiters and counted five lease-renewal events; polling can miss short waits.

The opt-in bounded metric export included all required classes. The full-run
histogram had five catalog import hold observations, one search snapshot hold
observation and 31 admin headline observations; the observations cover more
than the overlap window and bucket upper bounds are not exact percentiles.
Raw sanitized samples, environment details, and limitations are in
[the JSON evidence](./db-competition-isolated-smoke-2026-11-26.json).

The release binary SHA-256 is
`2f1171adfab048fddfb07bad62b6ffeff9c9e00937ddab224ef4987ebc0086a6`.
The [build record](./db-competition-build-2026-11-26.json) captures matching
workspace source hashes before and after the build, but omitted the compatibility
golden files embedded in the binary; later build records include those inputs. Subsequent discovery-worker source
changes occurred after that build, so this is not evidence for the final tree.
An empty invocation-owned server `.env` prevented its ancestor dotenv lookup,
and a synthetic environment canary was absent from the private server log,
command output and sanitized evidence. At the time of this run the fixture
migration CLI still ran from the repository directory; that isolation gap was
subsequently corrected in the harness and covered by Python tests. A separate
2-Artist/3-Release real-PostgreSQL smoke then migrated and seeded successfully
with an owned empty `.env` and asset root; synthetic unrelated environment
variables were absent from the workload and canaries from command output. This
does not audit unrelated processes.

The synthetic catalog has no physical media for its 36,000 existing Tracks,
realistic ACL distribution, provider credentials, or realistic music; policy,
recommendation maintenance, external enrichment and podcast workers were
disabled. CPU/RSS values omit PostgreSQL and child processes. The raw server
log and generated media were removed with the owned temporary tree.

A separate real-PostgreSQL clone-only smoke used a 2-Artist/3-Release parent
fixture. Two distinct sequential clones accepted a clone-only table creation,
were removed afterward, and the parent had no such table. The workload and
build-provenance checks were mocked in that test. It demonstrates the tested
clone isolation and cleanup path, not production workload performance or
interrupted-process recovery.

Separate real SIGTERM and SIGINT lifecycle checks on 2-Artist/3-Release
fixtures each started a private server in an owned clone, signalled that matrix
coordinator, and verified the server had exited and the clone was removed while
the parent fixture remained accessible. Build provenance was deliberately
mocked for this lifecycle-only test. The test did not send SIGTERM to the outer
fixture process, simulate SIGKILL or host loss, or prove cleanup after a lost
CREATE DATABASE acknowledgement.

The outer fixture path was subsequently exercised separately with real SIGTERM
and SIGINT on tiny owned fixtures. In both runs the inner coordinator reported
its server reaped and clone removed, and a read-only PostgreSQL check verified
the outer fixture database was removed. The inner build-provenance check was
mocked. These checks do not cover SIGKILL, host loss, changed process topology,
or a lost CREATE DATABASE acknowledgement.

A separate tiny schema check used the existing debug migration CLI from the
`a0979cde` baseline worktree and the current fixture generator. It created and
removed an owned database with 2 Artists, 3 Releases, 3 Tracks, 3 synthetic
acoustic evidence rows and 6 genre terms. This verifies only that the fixture
SQL can run at that tiny size against the baseline schema; it is not baseline
application timing or evidence that production-scale plans are equivalent.
