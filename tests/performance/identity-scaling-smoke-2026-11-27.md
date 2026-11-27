# Identity scaling harness smoke, 2026-11-27

This is a functional harness smoke, **not** representative performance or final
acceptance evidence. The invocation-owned catalog contained 50 Artists, 50
Releases, and 150 Tracks before scenario setup. After setup, the measured
relations contained 204 observations and current decisions, 201 Track credits,
18 Artist external IDs, and 18 Release external IDs. The fixture has synthetic
identity evidence and does not model real-world ambiguity distributions.

The matching release [identity test build](identity-scaling-test-build-2026-11-27.json)
and [migration CLI build](identity-scaling-cli-build-2026-11-27.json) both used
HEAD `37f29889fb05d1825faecb8b597387f0eb971b5e` with a dirty, shared worktree.
Their full before/after fingerprinted source dictionaries are equal. Their
binary SHA-256 values are, respectively,
`529f9cefe19396b634ee72fb23ee1cd20af28adfa0340d3c8225d2a38ea7b2ee` and
`145ca42a7cd28a2b971a407072e2562f10591e8bb3e8f5767ff24d038f6628ad`;
the [smoke capture](identity-scaling-smoke-2026-11-27.json) identifies both
same hashes. Equal snapshots do not rule out transient edits, attest dependency
binaries, or establish a clean commit. This is an intermediate source snapshot;
it must not be presented as the final integrated implementation.

All five scoped and global candidate queries returned nonempty equivalent
results; the newly added same-Release case returned one row. The collector
captured ten completed SQL execution plans. A single production drain processed
8 initial seeds involving 6 Artist IDs and 2 Release IDs, recorded 3 Artist
merges and 1 Release merge, and left 32 closure seeds for later turns. These
facts verify harness operation only; they are not a throughput claim or proof
of global outcome equivalence.

The reproducible commands, executed sequentially with `CARGO_BUILD_JOBS=2` and
`RUST_TEST_THREADS=1`, were:

```text
python3 scripts/build_perf_cli.py --output tests/performance/identity-scaling-cli-build-2026-11-27.json
python3 scripts/build_perf_test.py --target identity --output tests/performance/identity-scaling-test-build-2026-11-27.json
python3 scripts/perf_representative.py --artists 50 --releases 50 --tracks-per-release 3 --migration-cli target/release/melodee-cli -- python3 scripts/run_perf_identity_case.py --test-binary target/release/deps/perf_identity_reconciliation-03b25fe2e11ac309 --build-provenance tests/performance/identity-scaling-test-build-2026-11-27.json --output tests/performance/identity-scaling-smoke-2026-11-27.json --timeout-seconds 180
```

The fixture owns its disposable PostgreSQL database. The CLI's build wrapper
has focused safety tests in `tests/python/test_build_perf_cli.py`; the case
collector's evidence and ownership checks have tests in
`tests/python/test_run_perf_identity_case.py`. A representative 60K/600K
multi-cardinality run, final-source provenance and broader workload evidence
remain required. No long-running load was started during this smoke.
