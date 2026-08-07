# Scaffold Validation Report

**Generated:** 2026-08-07
**Updated:** 2026-08-07 (Rust gate executed on the pinned toolchain; see "Rust gate" below)

This report records checks run while producing the starter ZIP.

## Repository structure

- Command: `/opt/pyvenv/bin/python3 scripts/validate_repository.py`
- Result: PASS

```text
Repository validation passed (12 ADRs, 8 mockups, 232 requirements).
```

## Python tooling tests

- Command: `/opt/pyvenv/bin/python3 -m unittest discover -s tests/python -p test_*.py -v`
- Result: PASS

```text
test_public_scenarios_are_valid_json_array (test_scripts.CaptureSafetyTests.test_public_scenarios_are_valid_json_array) ... ok
test_sensitive_query_values_are_redacted (test_scripts.CaptureSafetyTests.test_sensitive_query_values_are_redacted) ... ok
test_slugify (test_scripts.NewAdrTests.test_slugify) ... ok
test_prd_has_large_unique_requirement_inventory (test_scripts.RequirementExtractionTests.test_prd_has_large_unique_requirement_inventory) ... ok

----------------------------------------------------------------------
Ran 4 tests in 0.012s

OK
```

## Python syntax compilation

- Command: `/opt/pyvenv/bin/python3 -m compileall -q scripts tests/python`
- Result: PASS

```text
(no output)
```

## Rust gate

Executed 2026-08-07 on the pinned toolchain (rustc/cargo 1.97.1, cargo-deny 0.20.2, Python 3.14.6):

- Command: `./scripts/check.sh`
- Result: PASS (exit 0) — repository validation, requirements index check, Python tests,
  `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
  `cargo test --workspace --all-features`, and `cargo deny check` all pass.
- `Cargo.lock` is committed and resolves cleanly from a fresh `cargo` invocation.
- cargo-deny reports `advisories ok, bans ok, licenses ok, sources ok`; the only warnings are benign
  `license-not-encountered` notices for allowlisted licenses with no current dependent crate. No
  license exceptions are required at this time.

## Bootstrap server smoke check

- Command: `cargo run -p melodee-server`, then `curl http://127.0.0.1:8080/healthz`
- Result: HTTP 200, body
  `{"service":"melodee-server","version":"0.1.0","status":"ok","phase":"phase-0-bootstrap"}`
- SIGINT triggers graceful shutdown and the process exits 0.

## Historical note

The original scaffold-generation environment did not provide the pinned Rust toolchain or network
access, so the Rust checks were first executed in the follow-up session recorded above.
