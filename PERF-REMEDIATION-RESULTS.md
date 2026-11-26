# Performance Remediation — Results

**Base revision:** `a0979cde1fd2aeff5b8027d9d59336b678f1aa12` (branch `main`)
**Toolchain:** Rust 1.99.0, PostgreSQL 18.3 (dev compose, host port 5433)
**Host:** 24 vCPU, 62 GiB RAM, NVMe `/home` (552 GiB free)
**Build profile for measurements:** PostgreSQL fixtures are live; index/plan measurements used the
development PostgreSQL. Rust test binaries were debug builds unless noted.
**Measurement classification:** every number below is labelled
MEASURED IN THIS RUN, REPRODUCED CORRECTNESS, ANALYTICALLY DERIVED, or NOT EXECUTED.

This file is the integrated result record. The per-package ledger is
`PERF-REMEDIATION-STATUS.md`; the normative standard is `PERFORMANCE.md`; the operator procedure is
`design/runbooks/PERF-REMEDIATION-UPGRADE.md`.

---

## 1. What is now fixed

### 1.1 The three release-blocking correctness outcomes

**Genre aggregate concurrency (PERF-003 / RM-03, RM-04).** The statement-level transition-table
triggers now collect the affected `(surface, display_value)` keys, acquire transaction-scoped
advisory locks in ascending *computed lock-key* order, and then recount from authoritative
`catalog_genre_terms` joined to active Releases. Each recount is a distinct SPI statement in a
`VOLATILE` function, so under READ COMMITTED it takes a fresh snapshot and observes a contribution
committed by the transaction it waited for. The previous protocol counted from the statement
snapshot and overwrote the stored row, losing a concurrent contribution.

- REPRODUCED CORRECTNESS (before): two sessions inserting the same new genre on two Releases
  stored `song_count=1, release_count=1` while authoritative terms held `2,2`.
- REPRODUCED CORRECTNESS (after): the same experiment stores `2,2`.
- Permanent regression: `catalog_concurrency::concurrent_new_genre_contributions_are_not_lost_and_repair_converges`
  (passes) also proves idempotent repair, stale zero-contributor deletion, and overlay-only
  no-change behavior.
- Repair is a first-class operator command: `melodee-cli catalog repair-genres [--apply]`, backed
  by SQL `catalog_genre_aggregate_discrepancies(limit)` and
  `repair_catalog_genre_aggregates(apply, limit)`.

**Search activation commit safety (PERF-V001 / PERF-006 / RM-05).** `catalog_projection_outbox.id`
is allocated before commit, so `max(id)` is not a commit-order watermark. The rebuild snapshot now
records exactly which outbox rows it could see (`search_rebuild_snapshot_outbox`) inside the
snapshot transaction. Every other current outbox row is a delivery obligation of the new
generation, regardless of its id, its `processed_at`, or whether the ordinary worker acknowledged
it against the previous generation. Catch-up replays obligations in id order and records durable
progress (`search_rebuild_applied_outbox`) only after the Tantivy commit. Activation takes
`ACCESS EXCLUSIVE` on `catalog_projection_outbox` (which waits for every in-flight transaction that
inserted an outbox row, and blocks new ones), under a bounded `lock_timeout`, and activates only
when no obligation is outstanding. The old `caught_up_outbox_id >= max(id)` gate is gone.

- Permanent regression: `rebuild_integration::late_lower_id_commit_is_replayed_before_generation_activation`
  drives the real `run_rebuild_once` path with an uncommitted low-id event already marked
  `processed_at`, commits later ids, advances catch-up, then commits the low id and asserts the
  activated generation contains the new revision and not the stale one.
- Legacy `load_outbox_after`/`max_outbox_id` remain for diagnostics and existing tests.

**Discovery coverage (PERF-012 / RM-06).** `native_discovery_progress` stores one durable keyset
cursor per work class (`track_popularity`, `release_popularity`, `artist_popularity`,
`missing_candidates`, `chart_relink`) with explicit wrap. Five work classes share the configured
per-pass batch cap, so a large Track population can no longer consume the budget. Progress is
stored in the same transaction as the batch effect; a crash replays an idempotent batch. A
transaction-scoped advisory lock serializes passes across processes.

- Permanent regression: `native_discovery_phase7::discovery_progress_is_durable_fair_and_eventually_complete`
  (passes) seeds 100,000 Track candidates plus Release/Artist/chart work, proves per-pass budget
  adherence, proves Release/Artist are not starved in the first pass, drives all Track candidates
  to completion across sweeps, and proves a row inserted behind the cursor is reached after
  wraparound.

### 1.2 Query-scope and index defects

- **PERF-001 / RM-01** — forward migration `0060` adds the narrow
  `tracks(playable_media_asset_id)` index. No trigger or insertion-order change.
- **PERF-002 / RM-02** — normal policy (`load_context`) and dry-run (`load_test_contexts`) now scope
  the acoustic comparison to the evaluated Release's own digest facts, with an empty-set
  fast path that issues no query. Same-release exclusion, duplicate handling, and the
  `evidence_kind`/JSON-key predicates are preserved.
- **PERF-003 / RM-03** — forward migration `0060` adds `catalog_genre_terms(release_id, surface,
  display_value)`; the status trigger returns immediately when no actual status changed.
- **PERF-004 / RM-09 (partial)** — forward migration `0060` adds
  `artist_match_decisions(artist_id, observed_artist_id)`; idempotent (non-`inserted`) imports
  skip reconciliation; the startup backstop now covers strong Release identity as well as Artist.
- **PERF-010 / RM-15 (partial)** — forward migration `0060` adds a partial leading
  `native_artist_user_states(artist_id) INCLUDE (user_id, rating) WHERE rating > 0` index for the
  correlated `AVG(rating)` aggregate.

### 1.3 Worker, media, and write-path work

- **PERF-009 / RM-07** — scheduler result retention is a bounded 64-entry ring
  (`MAX_RETAINED_SCHEDULER_RESULTS`), with a permanent 10,000-tick unit test. Persistent lease/job
  history was deliberately not touched.
- **PERF-005 / RM-08** — the search scheduler drains consecutive full outbox batches without the
  fixed idle sleep (bounded turn budget), and gates the hourly retention check with a monotonic
  timer while the durable `last_prune_at` remains authoritative.
- **PERF-006 / RM-14 (partial)** — a fresh empty generation uses `insert_*_fresh` without a
  per-document existing-revision lookup; revision fencing remains on the catch-up/replay `upsert_*`
  path.
- **PERF-013 / RM-17 (partial)** — blacklist refresh uses a single-flight gate; concurrent requests
  no longer stampede identical settings queries.
- **PERF-014 / RM-18** — acoustic and embedded-identifier evidence are inserted with one set-based
  statement each instead of one round trip per Track/identifier, using typed `unnest` arrays and
  server-side `jsonb_build_object`.
- **PERF-V002 / RM-19** — release merge and split use two set-based `unnest` statements instead of
  one round trip per Track and per observation, preserving the exact ordinal mapping and per-row
  release guards.
- **PERF-011 / RM-16 (partial)** — forward migration `0060` drops the three schema-proven exact
  duplicate indexes.

---

## 2. Finding / package state

| Finding | Package | State |
|---|---|---|
| PERF-001 | RM-01 | IMPLEMENTED AND VERIFIED |
| PERF-002 | RM-02 | IMPLEMENTED; decision-equivalence covered by existing policy suites; new plan measured |
| PERF-003 | RM-03, RM-04 | IMPLEMENTED AND VERIFIED |
| PERF-004 | RM-09 | PARTIAL — index + no-op skip + both startup backstops; affected-identity closure scoping NOT implemented |
| PERF-005 | RM-08, RM-13 | PARTIAL — drain/maintenance done; ordered replay done; reader/writer reuse NOT implemented |
| PERF-006 | RM-05, RM-14 | RM-05 IMPLEMENTED; RM-14 PARTIAL (fresh bulk insert; off-async/commit tuning NOT implemented) |
| PERF-007 | RM-11, RM-12 | NOT IMPLEMENTED — documented as remaining |
| PERF-008 | RM-10 | NOT IMPLEMENTED — documented as remaining |
| PERF-009 | RM-07 | IMPLEMENTED AND VERIFIED (bounded results); rolling refill already present for inspection/normalization/publication |
| PERF-010 | RM-15 | PARTIAL — partial rating index added; route/serialization optimization NOT implemented |
| PERF-011 | RM-16 | PARTIAL — three exact duplicates dropped; lease renewal and history retention unchanged (documented decision) |
| PERF-012 | RM-06 | IMPLEMENTED AND VERIFIED |
| PERF-013 | RM-17 | PARTIAL — blacklist singleflight done; stream reader/auth combine/artwork cache NOT implemented |
| PERF-014 | RM-18 | IMPLEMENTED (evidence batching); advisory-lock audit and recommendation measurement NOT done |
| PERF-015 | RM-00, RM-20 | RM-20 IMPLEMENTED; RM-00 PARTIAL — regressions + focused fixture done, bounded histograms and representative 560K tier NOT executed |
| PERF-V001 | RM-05 | IMPLEMENTED; regression test added (final execution gate below) |
| PERF-V002 | RM-19 | IMPLEMENTED; covered by existing merge/split suites (final gate) |

---

## 3. Before/after evidence measured in this run

### 3.1 Playable-asset reverse lookup (PERF-001)

Fixture: 50,000 Tracks / 50,000 media assets / 5,000 Releases seeded directly with SQL into the
disposable migrated database (not an import benchmark; triggers were bypassed only during fixture
load). Same statement as the filename trigger's nested lookup, `EXPLAIN (ANALYZE, BUFFERS)`.

| Shape | Plan | Execution | Shared buffers |
|---|---|---|---|
| With `tracks_playable_media_asset_idx` | Index Scan, 1 index search | 0.029 ms | 3 |
| Without (pre-0060 shape) | Seq Scan, 49,999 rows removed by filter | 7.778 ms | 1303 |

MEASURED IN THIS RUN at 50K. The absent case is a full sequential scan whose buffers grow with the
catalog; the index case is bounded indexed access. 560K/10M-scale plan verification: NOT EXECUTED.

### 3.2 Release-scoped acoustic novelty (PERF-002)

Fixture: 50,000 acoustic evidence rows, one distinct digest per Track, 10 digests belonging to one
Release. Same statement shape as the policy query.

| Shape | Execution | Shared buffers | Rows transferred |
|---|---|---|---|
| Catalog-wide digest set (old) | 62.2 ms | 1725 | 49,990 hash-joined Tracks; all distinct digests returned |
| Release-scoped `= ANY($digests)` (new) | 10.9 ms | 31 | 10 index searches; 0 cross-release matches |

MEASURED IN THIS RUN at 50K. The scoped shape uses
`audio_identity_evidence_acoustic_digest` and transfers only relevant matches; the old shape scans
and transfers the whole catalog per Release. 560K/10M: NOT EXECUTED.

### 3.3 Genre release hydration (PERF-003 / RM-03)

Fixture: 100,000 `catalog_genre_terms` rows (native + OpenSubsonic) over the 50K-Track fixture.
Query: `SELECT DISTINCT display_value ... WHERE release_id = $1 AND surface = 'native'`.

| Shape | Plan | Execution | Shared buffers |
|---|---|---|---|
| With `catalog_genre_terms_release_surface_idx` | Index Only Scan, 1 search | 0.097 ms | 4 |
| Without (skip-scan on `..._release_match_idx`) | Skip-scan + sort | 0.219 ms | 28 |

MEASURED IN THIS RUN at 50K. PostgreSQL used skip-scan, not a sequential scan, in the absent case —
consistent with the validator's correction; the new shape is still ~2.3× faster with ~7× fewer
buffers and no sort.

### 3.4 Genre aggregate concurrency (PERF-003 / PERF-004)

| Shape | Stored `(song_count, release_count)` | Authoritative |
|---|---|---|
| Old protocol (no per-key serialization, stale snapshot) | `(1, 1)` | `(2, 2)` |
| New protocol (advisory locks + fresh recount) | `(2, 2)` | `(2, 2)` |

REPRODUCED CORRECTNESS in this run with two coordinated PostgreSQL sessions.

### 3.5 Other

- Search late-lower-id activation: REPRODUCED CORRECTNESS via the permanent regression above.
- Discovery fairness: REPRODUCED CORRECTNESS via the permanent regression above.
- Bounded scheduler results: unit test over 10,000 ticks asserts a 64-entry bound.
- Bounded outbox drain / maintenance gate: implementation verified by compilation and existing
  scheduler tests in the final gate; no standalone throughput number claimed.

---

## 4. Upgrade and data-repair steps

Short form; the full procedure is `design/runbooks/PERF-REMEDIATION-UPGRADE.md` and the public
summary is `docs/src/content/docs/installation/upgrading.md`.

1. Verify versions; take and test-restore a PostgreSQL backup plus an asset-directory copy.
2. Enter the ADR-0166 maintenance fence (stop every writer).
3. Apply forward migrations `0060`–`0063` with one `melodee-cli migrate` invocation.
4. `melodee-cli catalog repair-genres` (check), then `--apply`, then check again until
   `genre aggregates are consistent`.
5. Discovery progress initializes itself (migration 0062 seeds `nil` cursors per class).
6. Rebuild the search projection once with `melodee-cli search rebuild --actor <operator>`; the
   corrected protocol replays late/acknowledged obligations before activation. Do not repair by
   replaying only pending rows.
7. Verify counts, index validity (`pg_index.indisvalid`), health/ready, one ordinary import, and two
   discovery intervals; resume workers in documented order.
8. Mixed old/new workers are unsupported for migration `0063`. Rollback = restore the pre-upgrade
   database and matching image. No automatic down migration.

---

## 5. Tests and rehearsals executed

| Check | Result |
|---|---|
| Full migration chain from empty (disposable DB, all `0001`–`0063`) | PASS (REPRODUCED in this run) |
| `cargo check --workspace --all-targets --all-features` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo fmt --all --check` | PASS |
| `python3 scripts/validate_repository.py` | PASS (115 ADRs, 8 mockups, 239 requirements) |
| `catalog_concurrency::concurrent_new_genre_contributions_are_not_lost_and_repair_converges` | PASS |
| `native_discovery_phase7::discovery_progress_is_durable_fair_and_eventually_complete` | PASS |
| Playable-asset and acoustic EXPLAIN comparisons | EXECUTED (numbers above) |
| `rebuild_integration` (all 10, including `late_lower_id_commit_is_replayed_before_generation_activation`) | PASS |
| `search_phase6` (12 activation/snapshot/lease tests) | PASS |
| `catalog_merge_split` (5) + `catalog_release_merge` (7) | PASS |
| `catalog_import` (23, genre projection + batched evidence + reconciliation) | PASS |
| `catalog_audio_identity` (1) | PASS |
| `catalog_concurrency` (6, including the new genre regression) | PASS |
| `policy_phase5` + `phase5_novelty` (4, novelty decision equivalence) | PASS |
| `search_outbox_retention` (6, include rebuild retention pin) | PASS |
| `native_discovery_phase7` (new fairness regression) | PASS |
| `melodee-ingestion --lib` (46, including bounded-result retention) | PASS |
| `melodee-server --lib security` (7) | PASS |
| `migrations_run_from_empty_database`, `migrations_are_repeatable` | PASS |
| Fresh vs populated upgrade and interrupted-operation rehearsal | populated upgrade NOT EXECUTED; fresh chain executed |
| 560K representative tier / 1M / 5M / 10M / soak / real NFS | NOT EXECUTED |

Debug-profile test binaries were used for correctness; the plan measurements are live PostgreSQL and
do not depend on the Rust build profile. The repository's release-build `make check-nightly` gate is
the authoritative final tier and was not run in this operation.

---

## 6. Conditional proposals: retained vs rejected

| Proposal | Decision | Reason |
|---|---|---|
| `tracks(playable_media_asset_id) INCLUDE (release_id)` | Rejected | Trigger reads only `track.id`; no proven index-only consumer |
| Remove media filename INSERT trigger | Rejected | Both insertion orders are supported; the narrow index is sufficient |
| `UPDATE OF status` + transition tables | Rejected | PostgreSQL rejects transition tables with column lists |
| Naive per-track genre `song_count += delta` | Rejected | Cannot derive distinct-release counts safely |
| Sorted locks with an unchanged stale snapshot | Rejected | Lock order alone does not refresh visibility |
| Read-committed rebuild with only `max(id)` | Rejected | Ids are allocated before commit (PERF-V001) |
| Snapshot-visible obligation set + exclusive activation fence | Retained | The proven commit-safe protocol implemented here |
| Asynchronous genre maintenance | Rejected | Exact synchronous projection required; not proven necessary |
| Blanket index drops / `idx_scan=0` | Rejected | Only three exact duplicates were dropped |
| Removing lease event history | Rejected | ADR-0158 auditability; no retention policy |
| Password-success cache | Rejected | Security semantics |
| Naive per-key rate-limit upsert | Rejected | Exact multi-process admission and retained-key ceiling |
| Universal FFmpeg `-threads 1` | Rejected | Codec/filter/hardware dependent; not measured here |
| Universal `spawn_blocking` migration | Rejected | Unbounded waiting queue; admission required first |
| Asynchronous recommendation updates | Rejected | Freshness/recovery contract unproven |
| Heartbeat HOT claim | Rejected | Indexed lease/updated fields make HOT impossible |
| Fixed DB pool split | Rejected | Not evidence-based; total connection budget must be preserved |

---

## 7. Remaining work and blockers

No external blocker prevented the delivered work. The following mandatory/optional items are **not
completed** in this operation and must not be described as done:

1. **RM-09 affected-identity closure.** The durable seed queue, transitive closure expansion, and
   oracle-equivalence harness are not implemented. Release-only scoping was deliberately not
   implemented because it can miss transitive/historical identity effects. The index, the
   idempotent-import skip, and both startup backstops are in place.
2. **RM-10 manifest two-phase binding and DB admission.** Not implemented. Moving the manifest read
   outside the transaction without a complete fence/revalidation design would introduce a TOCTOU
   defect, so it was left unchanged rather than half-implemented.
3. **RM-11 / RM-12 media I/O, subprocess lifecycle, and bounded CPU/FS admission.** Not implemented.
   These require an operation inventory and integrity tests; no behavior was changed.
4. **RM-13 reader/writer reuse.** Ordered batched replay is implemented; a bounded reader registry
   and long-lived writer with generation/lease reopen were not implemented.
5. **RM-14 off-async rebuild work and commit tuning.** Fresh-generation bulk inserts are
   implemented; bounded blocking execution and commit tuning were not.
6. **RM-15 endpoint/serialization/ACL optimization.** Only the partial rating index was added; no
   route benchmark or serialization change was made.
7. **RM-16 lease renewal and history retention.** Unchanged pending a product retention decision;
   the three exact duplicate indexes were dropped.
8. **RM-17 stream reader, combined auth lookup, artwork cache.** Only blacklist singleflight was
   implemented.
9. **RM-00 bounded metrics/histograms and the 560K representative fixture.** Regression harness and
   a focused 50K fixture exist; production-path histograms and the representative tier were not
   executed.
10. **Populated-upgrade and interrupted-operation rehearsal**, `make check-nightly`, Admin UI
    gates, and long soak/NFS runs. Not executed.

The integrated result is **implementation complete for the packages marked IMPLEMENTED/PARTIAL
above and for the three release-blocking correctness outcomes**, and **focused-workload verified**
at the 50K database scale for the query/index claims. It is **not** representative-scale verified
and **not** a complete remediation of all 20 packages. See
`PERF-REMEDIATION-STATUS.md` for the resumable ledger.
