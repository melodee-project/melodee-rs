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

Recorded after the final code state (see section 6 for the exact commands).

- `cargo test -p melodee-persistence --test provider_eligibility` — 9 passed.
- `cargo test -p melodee-persistence --test manifest_evidence` — 4 passed.
- `cargo test -p melodee-persistence --test search_outbox_retention` — 6 passed.
- `cargo test -p melodee-persistence --test migrations` — 18 passed (includes the
  full migration chain and the historical-schema import path).
- `cargo test -p melodee-server --test provider_enrichment_worker` — 3 passed.
- `cargo test -p melodee-server --test enrichment_capacity -- --ignored` — 2
  passed (timed evidence above).
- Admin UI: `pnpm check`, `pnpm lint`, `pnpm test` (318), `pnpm i18n:check`.
- Repository validation, requirement index, `make check-fast`, `make check`,
  `python3 scripts/do-prechecks.py --include-e2e` — see section 6.

## 5. Integrity and restore

The upgrade path is exercised locally end to end by
`manifest_evidence::upgrade` in the test suite: legacy inline payload →
evidence object → independent verification → inline removal, with a repeat run
performing no bulk work. Missing/corrupt evidence and a reintroduced inline
payload are detected. A paired restore rehearsal (database + asset store) was
**not** executed against production-shaped data; it is an operator step in
`design/runbooks/ENRICHMENT-AND-STORAGE-UPGRADE.md`.

## 6. Commands and results

Filled in by the final gate run recorded with this change. See the closeout
report in the pull request for the verbatim tail of:

```bash
cargo fmt --all
make check-fast
make check
python3 scripts/do-prechecks.py --include-e2e
make audit-live
```
