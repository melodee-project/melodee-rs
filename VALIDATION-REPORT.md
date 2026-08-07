# Scaffold Validation Report

**Generated:** 2026-08-07

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

## Rust compilation status

The artifact-generation environment did not provide the pinned Rust toolchain or network access,
so `cargo generate-lockfile`, `cargo fmt`, `cargo clippy`, `cargo test`, and `cargo deny check` were
not executed here. The repository contains a pinned toolchain, bootstrap/check scripts, and GitHub
Actions CI that execute those checks on the first push. `Cargo.lock` is intentionally generated and
committed by the first dependency-resolving run.
