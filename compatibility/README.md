# Compatibility Baseline

This directory stores sanitized, reviewable evidence of the existing C# server's external behavior.
It is not a place for production credentials or private library dumps.

## Surfaces

- `melodee-api-v1/` — native first-party REST API.
- `opensubsonic/` — OpenSubsonic/Subsonic behavior.
- `jellyfin/` — implemented music-focused Jellyfin subset.
- `scenarios/` — declarative capture scenarios.
- `captures/` — local generated output; ignored by Git except `.gitkeep`.

## Route manifests and OpenAPI contract

Each surface's `contracts/route-manifest.json` is generated from the frozen legacy checkout
(the frozen commit and reproduction steps are documented in `scripts/legacy_oracle.py`
and its `--help` output):

```bash
python3 scripts/generate_route_manifest.py --legacy-root /path/to/melodee
python3 scripts/generate_route_manifest.py --legacy-root /path/to/melodee --check  # drift check
```

`compatibility/melodee-api-v1/contracts/openapi-v1.json` is the captured `/openapi/v1.json`
document with the volatile `servers` URL normalized to `https://{melodee-host}/`. The published
document is known to be incomplete (party, native podcasts, and queue routes are missing); the
route manifests are the authoritative route inventory. The summary of route-inventory findings
that motivated this lives in the withdrawn `design/evidence/phase-0/route-inventory-summary.md`
report (see [Withdrawn evidence reports](../design/adr/README.md#withdrawn-evidence-reports)).

## Capture public baseline

```bash
export LEGACY_MELODEE_BASE_URL=http://127.0.0.1:5157
python3 scripts/capture_baseline.py
```

For scenarios marked `auth: bearer`, set a temporary token in
`LEGACY_MELODEE_BEARER_TOKEN`. Never commit it.

## Native API v1 critical path (Issue 5)

The reproducible oracle lifecycle and native-v1 capture are one command each:

```bash
python3 scripts/legacy_oracle.py recreate   # frozen-SHA check, build, migrate, seed, start, verify
python3 scripts/legacy_oracle.py capture    # reseed + capture native-auth/catalog/stream
```

- Fixture dataset: `fixtures/catalog-v1.json` (deterministic public IDs, synthetic WAV media).
- Scenario files: `scenarios/native-auth.json`, `native-catalog.json`, `native-stream.json`
  (schemaVersion 2: flows, structure checks, capture-time sanitization).
- Golden evidence: `melodee-api-v1/golden/` with per-directory `provenance.json`.
- Unified route inventory: `melodee-api-v1/contracts/route-inventory.json`
  (`scripts/build_native_inventory.py --check` for drift).
- Findings: distilled into `COVERAGE-MATRIX.md`, the per-surface route matrices under
  `design/agents/PHASE-*-ROUTE-MATRIX.json`, and the golden captures. The original capture and
  route-inventory reports are withdrawn evidence reports.

## Golden fixture rules

A fixture promoted from `captures/` into a `golden/` directory must:

- contain no secrets, private hostnames, real user emails, or private filesystem paths;
- include request metadata and status/headers needed for comparison;
- identify volatile fields and the normalizer used;
- identify the legacy commit/version/database fixture;
- remain human-readable where practical; and
- be accompanied by a test.

Do not normalize meaningful differences merely to make tests pass.

## OpenSubsonic surface (Issue 6)

- Scenarios: `scenarios/opensubsonic-*.json` (11 files, 177 steps; system/auth/browse/lists/
  media/annotation/playlists/community/negative/conditional/conditional-off).
- Golden: `opensubsonic/golden/` with per-directory `provenance.json`.
- Endpoint inventory: `opensubsonic/contracts/route-inventory.json`
  (`python3 scripts/build_opensubsonic_inventory.py --check`).
- Findings: captured in `opensubsonic/golden/` and `COVERAGE-MATRIX.md` (auth bypass contract,
  unsigned JWT acceptance, double range-processing seek bug, 410/501 matrix, playlist mutation
  breakage; do-not-preserve decisions recorded in ADR-0113 and ADR-0130).


## Media baseline (Issue 8)

- `media/baseline.json` + `media/cases/` — 31-case synthetic corpus outcomes (extraction,
  pipeline, validation). Generator: `scripts/generate_media_corpus.py`; harness:
  `scripts/capture_media_baseline.py`. Outcomes are read from `media/baseline.json`; the
  narrative report is a withdrawn evidence report.

## Benchmarks (Issue 9)

- `benchmarks/` — three deterministic dataset tiers (15k/560k/5M songs), endpoint timing
  matrices with environment metadata, `SUMMARY.md`. Generators:
  `scripts/generate_benchmark_dataset.py`, `scripts/run_benchmarks.py`. Findings, budgets, and
  hazards live in `benchmarks/SUMMARY.md`; the original scale-baseline report is a withdrawn
  evidence report.

## Phase 0 closure (Issue 10)

- `coverage-matrix.json` + `COVERAGE-MATRIX.md` — per-surface coverage and open discrepancies
  (`scripts/build_coverage_matrix.py`). These are the canonical closure record; the original
  Phase 0 closure report is a withdrawn evidence report.
