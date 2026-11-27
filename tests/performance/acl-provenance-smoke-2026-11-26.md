# ACL provenance smoke, 2026-11-26

The release-profile `perf_interactive_routes` test ran against an invocation-owned
fixture containing 100 artists, 100 releases and 200 tracks. The existing
catalog fixture helpers built two further releases through their manifest,
normalization, publication and catalog import paths, using an invocation-owned
temporary asset root. The test created library access groups, granted the test
user access to one release, verified HTTP 200 for that release and HTTP 404 for
the restricted release, revoked membership, and verified HTTP 404 for the
formerly accessible release. This is a bounded correctness smoke, not evidence
of production-scale ACL behavior, latency distributions, or mixed load.

The run used Rust 1.99.0 release profile against PostgreSQL 18.3 on the same
host described in `interactive-routes-2026-11-26.md`. It was built from the
working tree based on `8a4cf6fa67d7977d6e1cf999dd2d66bb5a262c2c`; the exact
source snapshot was not recorded. The SQL fixture seeded in 0.5 seconds and
the test passed in 0.50 seconds. Native principal authentication was injected.

Raw ACL sample lines and test result from the run:

```text
route=native_acl_allowed iteration=0 elapsed_ms=1.522
route=native_acl_allowed iteration=1 elapsed_ms=0.848
route=native_acl_allowed iteration=2 elapsed_ms=0.859
route=native_acl_allowed iteration=3 elapsed_ms=0.836
route=native_acl_denied iteration=0 elapsed_ms=0.640
route=native_acl_denied iteration=1 elapsed_ms=0.877
route=native_acl_denied iteration=2 elapsed_ms=0.622
route=native_acl_denied iteration=3 elapsed_ms=0.562
acl_revocation_check=passed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s
```

Run the smoke after building the debug migration CLI and release benchmark:

```text
CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo test --release -p melodee-server --test perf_interactive_routes --no-run
python3 scripts/perf_representative.py --artists 100 --releases 100 --tracks-per-release 2 -- cargo test --release -p melodee-server --test perf_interactive_routes -- --ignored --nocapture
```
