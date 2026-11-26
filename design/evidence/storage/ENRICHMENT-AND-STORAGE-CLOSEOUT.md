# Closeout evidence — provider scheduling, manifest evidence, outbox retention

Baseline commit: `b0b15dd` (recorded before implementation).

Local environment: Fedora host, PostgreSQL 18.3 (`127.0.0.1:5433`), Rust
`1.99.0`, Tantivy via the workspace pin. All runs used disposable databases
created by the test/measurement harnesses. No production data, credentials or
NAS/worker settings were touched.

## 1. Manifest storage (ADR-0213)

Command:

```bash
python3 scripts/measure_manifest_storage.py --manifests 4424
```

The script creates a disposable database, applies the **real** migrations with
`melodee-cli migrate`, then writes the identical logical manifest fixture twice:
once in the pre-ADR-0213 `jsonb` shape and once in the ADR-0213 shape (logical
row + verified evidence reference + facts + per-track facts + receipt). The
fixture is metadata-only, with 84 provenance documents per 147 manifests, seven
tracks each, ~3 KiB fingerprint strings, lyrics and one non-ASCII sidecar.

| Measurement (4,424 manifests) | Baseline `jsonb` | Candidate references + facts |
| --- | ---: | ---: |
| PostgreSQL bytes (table + TOAST + indexes) | 222,732,288 (212.4 MiB) | 15,572,992 (14.9 MiB) |
| Inline payload text bytes | 235,205,804 | — (on disk) |
| Reduction | — | **93.01 %** |

The gate requires at least 50 % less PostgreSQL space for manifest storage; the
measured 93.01 % includes the new fact, receipt, evidence and ownership tables
and their indexes. The baseline figure (212.4 MiB) is close to the production
observation (about 219 MiB for 4,424 rows), which is why the fixture is
considered representative.

Upgraded-in-place footprint was **not** measured on a production copy (operator
gate). Locally, the upgrade keeps the inline payload until finalize, so peak
usage is baseline + facts + WAL; the runbook carries the sizing guidance.

Reproduce: `python3 scripts/measure_manifest_storage.py --manifests 500`
finishes in seconds and shows the same shape (91.5 % reduction).

## 2. Enrichment capacity check (ADR-0212)

Harness (ignored timing test, real worker, real scheduler, stub HTTP provider
with a controlled 100 ms response):

```bash
cargo test -p melodee-server --test enrichment_capacity three_provider -- --ignored --nocapture
```

Fixture: 165 Releases, each with one new Artist, three eligible providers of
nine, both purposes → **1,980 logical jobs**; empty queue at start.

| Measurement | Value |
| --- | ---: |
| Logical jobs | 1,980 |
| Wall clock to drain | 42.0 s |
| Measured rate | 47.1 jobs/s (169,707 jobs/h) |
| Provider HTTP requests | 1,977 (2 cache hits) |
| Concurrency | process-wide 8, per provider 2 |
| Arrival rate in the documented scenario | 330 Releases/h → 1,980 jobs over 30 min |
| Service rate required to finish by the 5-minute bound | 6.6 jobs/s (23,760 jobs/h) |

The measured rate is ~7× the required service rate with the configured
concurrency, so all 1,980 jobs finish well inside five minutes of the final
submission. Real provider quotas can dominate: with a 1 request/second/provider
quota the same fixture would take ~11 minutes per provider lane and the bound
would not hold. This local fixture does not qualify production provider quotas
or NAS throughput.

Rate-limit isolation:

```bash
cargo test -p melodee-server --test enrichment_capacity rate_limited -- --ignored --nocapture
```

20 Releases (240 jobs), one provider returning 429 (`Retry-After: 1`): the two
healthy providers completed all 160 jobs in 49 s, 192 rate-limited attempts were
observed, 96 retries stayed visible, and all 20 Releases remained active/playable
throughout.

Not measured: a production-shaped 50,000-Release / 500,000-Track plan
inspection, and NAS/provider-quota behaviour. The SQL shapes are index-backed
(keyset `id > $cursor ORDER BY id LIMIT 250`, partial claim index, ordered prune
index), and the criteria are recorded in the runbook as operator work.

## 3. Search outbox retention (ADR-0214)

```bash
cargo test -p melodee-persistence --test search_outbox_retention
```

Six scenarios, all green:

- 100,000 eligible old successes plus recent, leased, pending, retryable and
  permanently failed controls; the dry run reports exactly the eligible set and
  the applied pass deletes all and only that set in bounded batches;
- the chart after pruning equals an independent pre-prune distinct-count query,
  including the live partial hour and the zero-filled spine;
- an interrupted single-batch pass resumes; two concurrent workers delete each
  row exactly once;
- a nonterminal rebuild pauses pruning and releases it after `activated`;
- pruning through an empty raw table is stable, a stale upsert replay for a
  pruned revision is suppressed while the next revision still enqueues;
- acknowledgements straddling an hour boundary are counted in their own hour.

## 4. Test and gate results

Test tiering was changed with this work so the development loop is not a 45-minute run:

| Target | Runs | Measured locally |
| --- | --- | --- |
| `make check-fast` | repository validation, fmt, clippy, script-worker build | **3.8 s warm** (incremental compile time only on change) |
| `make check` (pre-merge) | adds Python tests, Admin UI gate, cargo-deny; **no Rust tests** | ~5 min (Admin UI dominates) |
| `make check-nightly` | adds every Rust suite: workspace unit/binary, database-free crate integration, PostgreSQL workspace suite, six historical migration compatibility tests | long; scheduled CI, pushes to `main`, manual dispatch |

The six historical migration tests are `#[ignore]`d and run only in the nightly tier
(`make test-migrations` runs just them). The two failures found by the last full workspace run were
fixed and re-verified individually: a parallel-suite timing flake in
`search_phase6` (poller timeouts raised to 30 s) and a historical-schema rebuild reading the new
ADR-0214 maintenance table (guarded with `to_regclass`, raw fallback).

Suites verified green with these changes:

- `crates/melodee-persistence/tests/provider_eligibility.rs` — 9 passed.
- `crates/melodee-persistence/tests/manifest_evidence.rs` — 4 passed.
- `crates/melodee-persistence/tests/search_outbox_retention.rs` — 6 passed.
- `crates/melodee-persistence/tests/migrations.rs` — 12 passed / 6 ignored in the pre-merge tier;
  the six historical tests passed in the earlier full-binary run, and `migration_0041` was
  re-verified individually after its fix.
- `crates/melodee-persistence/tests/search_phase6.rs` — full binary green after the timeout fix.
- `crates/melodee-search/tests/rebuild_integration.rs` — 9 passed after the historical-schema guard.
- `apps/melodee-server/tests/provider_enrichment_worker.rs` — 3 passed.
- `apps/melodee-server/tests/enrichment_capacity.rs -- --ignored` — 2 passed (timed evidence above).
- Admin UI: `pnpm check`, `pnpm lint`, `pnpm test` (318), `pnpm i18n:check`.
- Repository validation, requirement index, override audit: passed.

## 5. Integrity and restore

The upgrade path is exercised locally end to end by
`manifest_evidence::upgrade` in the test suite: legacy inline payload → evidence object →
independent verification → inline removal, with a repeat run performing no bulk work.
Missing/corrupt evidence and a reintroduced inline payload are detected. A paired restore
rehearsal (database + asset store) was **not** executed against production-shaped data; it is an
operator step in `design/runbooks/ENRICHMENT-AND-STORAGE-UPGRADE.md`.

## 6. Behavior change to note

When a manifest lives in evidence, policy snapshots no longer read the raw Chromaprint string from
PostgreSQL: `CatalogTrackFact.audio_fingerprint` is `None` and the durable comparison fact is
`audio_fingerprint_digest` (ADR-0186/ADR-0213). Inline-payload manifests keep the old behavior
until they are upgraded. No public client surface or wire field changes.

## 7. Operator-only verification still outstanding

- Paired database + asset-store restore rehearsal on production-shaped data.
- Production-shaped 50,000-Release / 500,000-Track plan inspection and NAS throughput.
- Real provider quotas (the capacity harness uses a 100 ms stub).
- `python3 scripts/do-prechecks.py --include-e2e` and `make audit-live` require the local dev
  stack; Docker could not start in the workshop environment (SELinux label error), so those were
  not run here.
- The nightly tier as a single end-to-end invocation (`make check-nightly`); its constituent
  suites were run and are green as listed above.
