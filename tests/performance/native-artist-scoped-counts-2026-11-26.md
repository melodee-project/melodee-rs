# Native Artist scoped-count query diagnostic, 2026-11-26

The restricted Native Artist DTO path recomputes per-Artist release and track
counts, applying the live `native_user_can_access_release` predicate. The
original grouped queries did not restrict their inputs to the selected Artist
IDs. The candidate adds the same selected-ID predicate inside each grouped
query, preserving the active-release, primary-credit, distinct-track and
per-release ACL predicates.

An invocation-owned PostgreSQL fixture had 35,000 artists, 50,000 releases and
600,000 tracks. The harness compared the old and candidate SQL for the first
50 Artists, with three serial EXPLAIN ANALYZE samples and an exact ordered row
hash check. It reported these execution times:

| Query | Samples (ms) |
| --- | --- |
| Original | 638.086, 598.731, 612.634 |
| Scoped | 16.556, 13.242, 13.094 |

On the first plan, original scans visited all 50,000 release-credit rows and
600,000 association rows (three parallel workers' 200,000-row scans). The
candidate used bitmap heap scans returning 100 release-credit and 1,200
association rows. The selected Artists' ordered counts matched in every
comparison. Full SQL, plans, buffers, row digests and probe/fixture hashes are
in [the JSON artifact](native-artist-scoped-counts-plan-2026-11-26.json).
The prebuilt release migration CLI was used for this fixture; its SHA-256
observed after the run was
`453d8b5a2c00d0d229a6e99f99f2f6b6d39cd322756b6dfaf776ad909fee8c51`.
The artifact predates adding this CLI hash to the reusable probe, so it does
not independently attest the CLI binary at process launch.

This is a query comparison, not an HTTP or release-build application
measurement. Runs were serial with uncontrolled cache state; old was always
first. The synthetic catalog has no import provenance and no restricted
releases, so it cannot establish access selectivity or revocation correctness.
The existing real-catalog Native ACL regression, extended with Artist list
counts, a visible guest with zero counts, and membership revocation, passed;
the complete serialized `native_catalog_api` target passed 7/7. A
source-fingerprinted in-process route benchmark remains to be run after final
integration.

Reproduce using an explicitly owned disposable database and a prebuilt
migration CLI:

```text
python3 scripts/perf_representative.py --seed-timeout 900 --migration-cli target/release/melodee-cli -- python3 scripts/perf_native_artist_scope_plan.py --output /path/to/new-result.json --iterations 3
```

The command creates and cleans its own database and clone; interruption by
SIGKILL, host failure or uncertain database-creation acknowledgment may require
operator inspection. The probe contains no server credential or media paths.

For the still-pending route measurement, build the release test with
`python3 scripts/build_perf_test.py --output <new-build-provenance.json>` while
workspace source is frozen. Its output identifies the test executable basename
and SHA-256; verify that hash before running the ignored test executable as the
direct child of `perf_representative.py`. The fixture's two real releases
supply allowed and denied ACL provenance; its remaining synthetic releases do
not. The injected principal bypasses native authentication, and timing excludes
TCP transport. The route checks the allowed Artist's counts and, for distinct
identities, that the denied Artist is absent. Build source fingerprints and
sample counts must accompany any resulting latency claim.
