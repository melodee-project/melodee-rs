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
| PERF-004 | RM-09 | PARTIAL — scoped candidates and mutation seeds require closure, concurrency and plan evidence (see current status ledger) |
| PERF-005 | RM-08, RM-13 | RM-08 IMPLEMENTED AND VERIFIED; RM-13 PARTIAL — writer lifecycle requires measured decision (see §8.3) |
| PERF-006 | RM-05, RM-14 | RM-05 IMPLEMENTED AND VERIFIED; RM-14 PARTIAL pending production rebuild measurement |
| PERF-007 | RM-11, RM-12 | RM-11 PARTIAL (tool-version reuse); RM-12 NOT IMPLEMENTED — see §8.6, §8.10 |
| PERF-008 | RM-10 | PARTIAL — preparation ordering and measured admission under correction (see §8.2) |
| PERF-009 | RM-07 | IMPLEMENTED AND VERIFIED (bounded results); rolling refill already present for inspection/normalization/publication |
| PERF-010 | RM-15 | PARTIAL — partial rating index added; route/serialization optimization NOT implemented |
| PERF-011 | RM-16 | retention IMPLEMENTED AND VERIFIED (ADR-0215); renewal NO CHANGE (see §8.7) |
| PERF-012 | RM-06 | IMPLEMENTED AND VERIFIED |
| PERF-013 | RM-17 | PARTIAL — singleflight + byte-bounded asset-key artwork cache; stream/auth/rate-limit remain — see §8.8 |
| PERF-014 | RM-18 | IMPLEMENTED (all evidence batching + sorted lock keys); recommendation measurement outstanding — see §8.6 |
| PERF-015 | RM-00, RM-20 | RM-20 IMPLEMENTED; RM-00 PARTIAL — bounded metrics added, representative 560K tier NOT executed — see §8.5, §8.9 |
| PERF-V001 | RM-05 | IMPLEMENTED AND VERIFIED (two regressions, incl. real old-generation consumer) |
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

## 7. Remaining work and blockers at the earlier checkpoint

This section is historical. The current state and next actions are in
`PERF-REMEDIATION-STATUS.md`; subsequent work has changed several entries below.

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

---

# 8. Resumed operation — deltas and final verification

This section records the work completed in the resumed operation and supersedes the earlier
`PARTIAL`/`NOT IMPLEMENTED` entries in §2. `PERF-REMEDIATION-STATUS.md` is the authoritative
per-package ledger.

## 8.1 Durable affected-identity reconciliation (PERF-004 / RM-09)

Migration `0064` adds `catalog_identity_reconciliation_seed` and the `lock_artist_identity_names`
helper. The import transaction now persists the affected identity facts (Release credits, Track
credits and their observations, raw observations, and external identifiers) atomically with the
catalog write, so a crash cannot lose reconciliation work. `import_once` no longer runs five
catalog-wide queries per successful album. Instead:

- `reconcile_affected_identities_once` elects one drainer with a session advisory lock, runs the
  complete candidate algorithms (Artist strong ID, same-Release Track credit, repeated credit,
  name-only proposals, strong Release ID) once per coalesced batch, iterates to a bounded fixed
  point for transitive merges, and clears only the seed rows at or below the watermark it captured.
- The catalog scheduler drains after a bounded number of import passes and on every tick, and keeps
  a 30-minute catalog-wide backstop for both Artist and strong-Release identity.
- Idempotent (non-`inserted`) imports persist no new observation and therefore no seeds.

At that checkpoint the drain ran global algorithms per coalesced pass. The current
working tree instead uses seed-scoped candidate queries; complete closure and
oracle equivalence have not yet been established.

Permanent tests:
`catalog_importer::import_persists_identity_seeds_and_the_coalesced_drain_clears_them` and
`coalesced_drain_converges_legacy_duplicate_artist_identities` (both PASS).

## 8.2 Two-phase manifest preparation (PERF-008 / RM-10)

The earlier implementation prepared and revalidated manifest evidence after
acquiring the write lock and retained a connection during file I/O. The current
working tree moves preparation before the transaction, releases the connection
before file I/O, and revalidates the binding and current database-clock lease
after locking. Focused binding, external-reference, expiry, lease-theft and
cancellation races passed. Mixed-load measurement and integrated verification
remain outstanding.

Database workload admission/pool reservation remains a measured decision; see §8.8.

## 8.3 Search reader reuse and ordered batch hydration (PERF-005 / RM-13)

- A bounded (8-entry) process registry caches a `SearchReader` keyed by canonical root + generation
  id + verified manifest hash. Activation, retirement, or a corrupt manifest changes the key and
  forces a fresh open. The reader forces `IndexReader::reload()` per query so incremental
  projection commits are visible immediately (matching the previous per-request open).
- Outbox hydration now loads all event records with one round trip per entity type
  (`load_catalog_records`), preserving per-event order, duplicates, deletes, and missing rows; a
  batch-read error falls back to the previous per-event classification.
- The current writer remains batch-scoped. No representative lifecycle measurement
  supports a decision to retain it; RM-13 remains partial pending measurement
  and the required evidence-backed lifecycle decision.

## 8.4 Search rebuild execution (PERF-006 / RM-14)

- Fresh generations use `insert_*_fresh` (no per-document revision lookup).
- Each snapshot page's document construction and commit runs on the blocking pool, one page at a
  time, so index CPU/IO no longer blocks the async runtime and the blocking queue is bounded to one
  page per rebuild. Catch-up apply+commit is likewise blocking.
- Commit frequency is the explicit snapshot-page document bound (`MELODEE__SEARCH__SNAPSHOT_PAGE_SIZE`).

## 8.5 Bounded metrics (PERF-015 / RM-00)

`melodee-persistence::metrics` provides fixed-bucket histograms for fixed metric identities
(`search_outbox_pass`, `search_rebuild_page`, `search_query`, `discovery_pass`,
`catalog_import_pass`). Cardinality is bounded by the enum; no route/path/entity/user label exists.
Recording is instrumented on the search outbox pass, rebuild page, discovery pass, and catalog
import pass. Unit tests prove fixed buckets, a single overflow bucket, and a fixed label set.
Per-class connection-checkout histograms and the representative 560K fixture remain outstanding.

## 8.6 Safe media/identity throughput at the previous checkpoint (historical)

The version-cache behavior below was later changed after an ABA review finding;
unbound executable results are currently not reused. See the current ledger.

- Tool version output is cached by canonical executable path plus file size/modification time, with
  a bounded 16-entry map. Normalization previously started ffmpeg/fpcalc/ffprobe `-version` three
  times per album; after the first album those probes are avoided. An in-place tool upgrade
  invalidates the entry through file identity.
- Artist external-identifier evidence is inserted with one set-based statement, deduplicating in
  input order to reproduce the previous first-insert-wins semantics.
- Artist identity-name advisory locks are acquired inside one SQL helper in ascending *computed
  lock-key* order (falling back to the previous loop on a pre-0064 historical schema).

## 8.7 Operator-controlled lease-event retention (PERF-011 / RM-16)

ADR-0215 and `melodee-cli work prune-lease-events --older-than-days <N> [--batch-size] [--apply]`:
read-only by default, bounded/resumable/idempotent apply, and eligible only for `lease_renewed`
rows. Transition, failure, completion, retry, and lease acquire/release history is always preserved.
Permanent test `history::lease_renewal_retention_is_opt_in_bounded_and_preserves_audit_history`.
Lease renewal round-trip redesign was evaluated and not adopted (queue-age and event semantics).

## 8.8 Artwork cache (PERF-013 / RM-17)

OpenSubsonic derived artwork is cached by stable request asset key (`album_`/`song_`/`artist_`/
`playlist_` id) + target size + format + no-upscale, replacing a per-request content hash, under a
hard 64 MiB byte ceiling plus the existing entry cap. An entry larger than the budget is served but
not retained. Blacklist refresh already uses single flight.

## 8.9 Verification gates (resumed operation)

| Gate | Result |
|---|---|
| `make check` (full: repo/docs, Python, admin UI build, fmt, clippy, cargo-deny) | PASS |
| `make check-nightly` (all Rust suites + six historical-schema migration tests) | PASS |
| Fresh `0001–0064` migration chain | PASS |
| Populated upgrade from 0059 with corrupted derived state | PASS |
| `perf_remediation_upgrade_from_0059_preserves_data_and_repairs_derived_state` | PASS |
| Historical-schema compatibility (6 `--ignored` tests) | PASS |
| Affected suites: catalog importer (8), catalog_import (23), catalog_concurrency (6), merge/split (5+7), search_phase6 (12), rebuild_integration (10/13), outbox retention (6), discovery (3), history (3), media lib (53), ingestion lib (46), server security/artwork/stats | PASS |
| 50K plan measurements (playable asset, acoustic, genre) | EXECUTED |
| 560K representative end-to-end tier, 1M/5M/10M, soak, NFS, real-media throughput | NOT EXECUTED |

Two pre-existing, unrelated gate failures were corrected minimally so the mandated gates can run:
an inconsistent settings-inventory consumer label assertion and a stats test that assumed an empty
asset registry after migration 0054. Neither touches remediation behavior.

## 8.10 Limitations at the previous checkpoint (historical)

These observations describe that checkpoint only; later implementation and evidence are recorded below and in `PERF-REMEDIATION-STATUS.md`. They are not current dispositions.

- **RM-11** content-read reduction (adjacent duplicate hashes, hash-during-copy, artwork dedup) with
  integrity/failure-injection tests is not implemented.
- **RM-12** bounded CPU/FS admission, subprocess lifecycle review, and measured concurrency
  (including making `inspection_concurrency` effective) is not implemented.
- **RM-13** long-lived writer lifecycle remains unimplemented; a measured decision is outstanding.
- **RM-15** endpoint/serialization/ACL optimization beyond the rating index needs the representative
  fixture and route percentiles.
- **RM-17** one-reader-per-stream, combined auth lookup, and rate-limit measurement are not
  implemented.
- **RM-18** recommendation/play-trigger contention is not measured.
- **RM-19** 1K/10K release merge/split timing is not measured.
- **RM-00** the representative 560K–600K end-to-end fixture and production-path benchmarks are not
  executed; only 50K query/index evidence exists.

**Status language:** implementation is complete and verified for the packages marked
IMPLEMENTED AND VERIFIED above and for the three release-blocking correctness outcomes; representative
560K-scale end-to-end verification and the RM-11/RM-12/RM-15/RM-17/RM-18/RM-19 measurement work
remain. This is not a claim that the whole remediation is complete.

---

# 9. Session 3 — correctness fixes and bounded execution (historical)

## 9.1 RM-09 seed deletion is commit-order safe

The previous drain deleted `WHERE id <= captured_watermark`. Identity values are allocated before
commit, so a lower id can become visible after the capture; that row would be deleted without ever
being reconciled. The drain now:

1. captures an explicit bounded id set (`claim_identity_reconciliation_seed_ids`, limit 10 000),
2. runs the closure to a fixed point (max 4 iterations),
3. deletes **exactly the captured ids** only when the fixed point was reached.

If the iteration budget is exhausted with candidates still present, the captured seeds are retained
durable, and the periodic global backstop retries. Seeds beyond the batch and seeds that committed
after the capture are never deleted by id order.

Test: `catalog_importer::late_committing_lower_id_seed_is_not_deleted_by_a_coalesced_drain` opens a
transaction holding a lower id, commits a higher id, drains, then commits the lower id and proves it
survives to the next drain. PASS.

## 9.2 RM-14 rebuild commit bounds

New configuration: `MELODEE__SEARCH__SNAPSHOT_COMMIT_DOCUMENTS` (default 10 000) and
`MELODEE__SEARCH__SNAPSHOT_COMMIT_INTERVAL_MS` (default 2 000). The rebuild commits when either
bound is reached and flushes before validation. At 560 K documents this reduces ~560 per-page commits
to roughly 56; the uncommitted document buffer and wall time are both explicit. Fresh-generation bulk
insert and per-page off-async indexing remain. `rebuild_integration` (13 with test-support) PASS.
Representative commit-count/timing: NOT EXECUTED.

## 9.3 RM-12 inspection concurrency is real

`inspection_concurrency` previously only sized the claim batch; the loop processed items serially.
Inspection now drives a finite `buffer_unordered(inspection_concurrency)` stream: each item keeps its
own lease heartbeat, and the in-flight set is bounded by the claimed batch. No unbounded task queue
is created, and each future holds its slot until the item's blocking work returns.
`inspection` (5) and `environment_faults` (7) PASS. A shared process-wide CPU/FS/subprocess admission
limit beyond the per-stage bounds and measured FFmpeg tuning remain outstanding.

## 9.4 RM-17 artwork cache invalidation

The derived-artwork cache key now includes the **verified source content hash** in addition to the
request asset key, size, format, and no-upscale flag. A row id alone does not prove the stored image
is unchanged. A permanent test replaces the source bytes under the same asset key and proves the
previously cached resize is not returned. Hashing a small image is cheap relative to the file read
and to the resize the cache exists to avoid.

## 9.5 RM-10 pool acquisition instrumentation

A `pool_acquire` histogram (fixed buckets) now records connection begin/checkout latency for catalog
claim, work claim, and search outbox claim, separate from query time. Measured background/interactive
competition and any pool topology/admission change remain outstanding; the topology is retained
pending that measurement.

## 9.6 Re-verified gates for the touched areas

`catalog_importer` (9), `rebuild_integration` (13 with test-support), ingestion `inspection` +
`environment_faults` (12), server media unit tests (3), and workspace fmt/clippy all pass.

**Final gate:** `make check-nightly` passes with `RUST_TEST_THREADS=1` (the specification's
recommended test setting). Without that variable, one pre-existing timing-sensitive podcast lease
test (`native_podcasts_api::all_native_podcast_routes_are_tenant_scoped_bounded_and_authorized`)
fails only because its three tests run in parallel within the binary; it passes in isolation and
with serialized test threads. It is not affected by remediation changes.

### Review of the two test-assertion corrections (from session 2)

- `admin_runtime_diagnostics_api::effective_settings_are_redacted_and_explained` expected the short
  `podcast_worker` consumer label, but the settings inventory records
  `melodee-application:podcast_worker` (the dominant convention in that file). The assertion now
  matches the inventory; it still verifies that the row exposes its consumer, so coverage is not
  weakened.
- `admin_stats_api::stats_report_library_listening_and_database_panels` asserted
  `assetFileBytes == 0`, but the fixture publishes a media asset that migration 0054 registers in
  `asset_files`. The assertion now requires the API value to equal the actual registry aggregate
  (`count(*)`, `sum(size_bytes)`), which is stronger: it still fails on invented values and no
  longer encodes an invalid empty-store assumption.

## 9.7 Still not complete

The representative 560K–600K end-to-end benchmark, seed-subgraph-scoped RM-09 discovery, substantive
RM-11 content-read elimination, shared RM-12 media admission and tuning, RM-13 writer evidence,
RM-15 route optimization, RM-17 streaming/auth/rate-limit, and RM-18/RM-19 measurements remain. This
document does not claim full remediation completion.

## Current execution: partial media operation inventory

The following is a source inventory, not a measured operation count. Conditional
branches, retries and failed stages change the actual number of operations.

| Path | Reads, verification and temporary operations | External work |
|---|---|---|
| Same-filesystem acquisition | Rename into owned staging; hash staged tree and write a durable receipt. Recovery verifies a pre-existing receipt. | None |
| Cross-filesystem acquisition | Hash source tree; copy to a partial tree; hash source and copy again; sync and rename; recheck source at cleanup. | None |
| Inspection | Verify owned tree before and after inspection; read and hash media files; parse tags and artwork; retain a manifest. | Probe audio using ffprobe. |
| Retained MP3 | Hash source before and after; decode-check, copy to a unique temporary output, hash and probe output; fingerprint source. | ffprobe, ffmpeg decode check and fpcalc. |
| FLAC and other transcoded audio | Hash source before and after; write, hash and probe an encoded output; fingerprint source. | ffprobe, ffmpeg encoding and fpcalc. |
| CUE | Hash and probe source; process and verify individual outputs; recheck source after segment work. | ffprobe, ffmpeg and fingerprint work depend on segments. |
| External and embedded artwork | Read bounded external input for hash/header checks; extract embedded images from source; normalize and verify output; recheck source when embedded extraction read it. | ffmpeg artwork processing. |
| Validation | Read and cryptographically hash normalized assets against the manifest. | No new audio probe in this stage. |
| Publication and retry | Copy assets while hashing, seal and verify bundle; adoption or recovery independently verifies existing bytes and receipts. | None |

This inventory does not prove that every remaining read is necessary or that a
given build performed it. Operation-count instrumentation and fault tests are
still needed before RM-11 can be closed.

Source inspection, not a measured I/O count: same-filesystem acquisition renames
and hashes the staged tree; cross-filesystem acquisition hashes the source tree,
copies, rehashes the source and hashes the copy, with a further source check at
cleanup. Inspection and normalization each verify the owned tree before and after
work; inspection also hashes and probes individual files. Normalization hashes
source audio before and after processing, probes source and output, hashes output,
and fingerprints it. Retained MP3 is decoded and copied; FLAC is encoded and
CUE sources are processed per segment. External artwork has source checks before
and after normalization. Embedded artwork requires an additional post-extraction
source check. Publication hashes while copying and independently verifies or
adopts the destination; retries and recovery reverify.

The current working tree avoids an additional post-processing source hash when
there is no embedded artwork and computes input and output artwork hashes from
the same bounded bytes used for preflight and output validation. Inspection also hashes the complete external artwork stream while collecting its bounded header. `cargo test -p melodee-media --lib --test
normalization_real` passed 56 unit tests (one ignored) and 13 real-media tests,
with `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1`. This is partial RM-11 work, not a
measured reduction or completion of the required integrity and recovery matrix.

The initial Rust-schema fixture smoke at default cardinality created 35,000
artists, 50,000 releases, 600,000 tracks and 600,000 asset references in 98.5 s
on the existing PostgreSQL 18.3 development service; the existing release CLI
`catalog status` reported those counts. The database was invocation-owned and
removed afterwards. This is a fixture construction smoke test, not an import or
representative performance measurement: the CLI binary's build identity was
not established, and the fixture omits physical media and several required
relationship distributions. It is not a baseline/final comparison.

A subsequent fixture construction run with synthetic acoustic evidence and
genre terms completed in 149.1 s. It contained 600,000 acoustic evidence rows,
1,200,000 genre terms and a total of 1,200,000 aggregate track counts across
two surfaces. This is construction and invariant smoke evidence, not a
production-path performance comparison; the missing distributions and workloads
listed in the status ledger remain mandatory.

A release-build in-process OpenSubsonic artist-index experiment on the
600,000-track fixture produced six serial authenticated samples per format.
The raw samples, diagnostic plans, environment, procedure and limitations are
in [the artist index evidence](tests/performance/artist-index-2026-11-26.md).
This was built from an uncommitted tree while other edits were in progress;
the exact build identity is not established. It is not a comparable baseline or
final acceptance run, and excludes network transport and concurrent load.

The detached original revision was also measured with the same cardinalities.
All 24 baseline artist-index samples completed under a 15-second statement
limit; raw output and reproduction are in [the baseline evidence](tests/performance/artist-index-baseline-2026-11-26.md).
The baseline and later observations are directional: the later executable's
exact dirty source identity was not captured, cache conditions were uncontrolled,
and the baseline predates the positive-rating index. They do not satisfy the
required final integrated before/after workload matrix.

A pre-0065 release binary (`c7a1405446d0f568c0497233fce2df0ba0ab941428685d7e3e1c4d33be77257d` SHA-256)
processed 200 synthetic one-track MP3/FLAC releases to `cataloged` in 60.656 s
on the 35,000-artist / 50,000-release / 600,000-track fixture, including
polling and shutdown but excluding corpus generation. This is approximately
197.8 releases/minute and 3.30 tracks/second. The full per-item evidence is
[the ingestion checkpoint summary](design/evidence/performance/ingestion-600k-checkpoint-8a4cf6f.md).
It bypassed search, policy, discovery, enrichment, podcast and concurrent
interactive work and used five-second audio on tmpfs without artwork, CUE, retries or
cross-filesystem staging. It did not collect I/O counts or reimport evidence.
The exact dirty build source identity was not captured; it is neither a baseline
comparison nor final acceptance.

A controlled synthetic codec-stage matrix compared the original revision and
current media source at one, two and four workers across MP3, FLAC, Ogg and WAV;
raw samples, source digests, CPU/RSS observations and limitations are in
[the codec matrix evidence](tests/performance/codec-matrix-2026-11-26.md).
It bypasses database workers and the shared ingestion gate and did not measure
interactive latency, so it does not establish an appropriate production limit.

A single PostgreSQL-backed search rebuild on the 600,000-track fixture indexed
685,000 documents in 30.968 seconds (22,119 documents/second), with 69 snapshot
commits, 30.920 seconds of snapshot age, a 7.24 ms activation call, 218,358,261
logical index bytes and a 248,976 KiB process peak RSS. Catch-up saw zero rows.
This is a dirty-build, uncontrolled-cache, single-run observation with an empty
outbox and no concurrent workload; it is not a comparable baseline or final
acceptance measurement. See [the rebuild evidence](tests/performance/search-rebuild-2026-11-26.md) for raw timings and reproduction.

The original revision's PostgreSQL-backed rebuild of the same document counts
completed in 35.129 seconds, with 219,819,473 logical index bytes and 224,632 KiB
peak process RSS. The current dirty-build sample was 30.968 seconds and
248,976 KiB. These are one sample each with uncontrolled cache and UUID variation;
the later binary's exact source identity was not captured. The baseline lacks
snapshot/commit instrumentation and neither run had concurrent load or catch-up
traffic, so this is directional evidence rather than final acceptance.

The seed-scoped reconciliation diagnostic on the 600,000-track fixture is
recorded in [the identity evidence](tests/performance/identity-scoped-2026-11-26.md).
The observed drainer completed in 170.448 ms with four initial seeds, two Artist
merges and four retained closure seeds; the late lower-ID check passed. The run
unexpectedly rebuilt against a changing dirty tree, and its identity graph was
sparse. Its plans do not establish affected-closure proportionality at a
representative identity distribution; a richer fixture and final comparison
remain required.

A further diagnostic augmented the 600,000-track catalog with 650,000
observations/current decisions, 600,000 Track credits, 11,085 Artist external
identities, 16,666 Release external identities and 349 same-name groups. On one
synthetic case, scoped and global vectors matched for all five methods; the
same-Release, repeated-credit and name-proposal queries took 15.120/2,899.674 ms,
7.029/1,268.991 ms and 5.809/795.096 ms respectively (scoped/global), and the
production drain took 167.904 ms. The strong Release case had no seeded
candidate and was vacuous. The binary's exact dirty source is unestablished;
this is not final equivalence or baseline acceptance evidence.

The later NOWAIT identity/cursor diagnostic on the populated 600,000-track
fixture passed the same narrow scoped/global comparisons, including non-empty
strong Artist and Release cases, and the late lower-ID test. The production
drain took 2,459.191 ms and retained 62 closure seeds. Its cursor SELECT plan
was an index-only scan without Sort or Materialize for 1 and 1,001 credits;
this measures full SELECT execution on one fixture, not incremental FETCH,
locking, a planner guarantee, or full concurrency equivalence.

The later pre-acceptance RM-18/RM-19 fixture rerun is recorded in [the NOWAIT
mutation evidence](tests/performance/recommendation-merge-nowait-2026-11-26.md).
The 10,000-Track merge/split sample took 5.765/8.919 seconds. Observed scrobble
p99 values were higher than the earlier pre-0065 single-run samples, a real
unresolved tail-latency concern; uncontrolled cache and one run per scenario do
not establish the cause. The fixture script changed after launch and the record
identifies the drift. Repeat controlled runs and final source qualification are
required.

A matched disk-backed ingestion pair used 35,000 Artists, 50,000 Releases,
600,000 Tracks and 200 synthetic one-track MP3/FLAC releases, with two workers
per stage. The baseline cataloged all 200 in 96.771 seconds; the current
binary cataloged all 200 in 62.689 seconds. Same-work-item CatalogStore retry
samples (n=50) had baseline p50/p95 3.067/4.121 ms and current 3.081/4.109 ms;
identifiers and import counts were preserved, and the current seed set did not
change. These are single runs with uncontrolled cache and synthetic five-second
media; current binary/source correspondence was not independently established.
Search, policy, interactive traffic, I/O counters, artwork, CUE, media retry
faults and cross-filesystem acquisition were excluded. The [paired evidence](design/evidence/performance/ingestion-600k-paired-disk.md)
links the raw logs and records source, environment and measurement limitations.

A real HTTP/1 ingress diagnostic on the 600,000-Track fixture tested caps
0/4/16/32/64 with 16 Jellyfin stopped events, 16 interactive probes, one idle
keep-alive and one slow MP3 stream while a Track was locked for three seconds.
All 204s corresponded to distinct persisted plays and exact Track popularity;
no play completed while the lock was held. System/Info p95 was 4.05 ms,
3.131 s, 3.044 s, 2.112 ms and 3.809 ms respectively. Caps 4 and 16 delayed all
probes until the lock released. Each cap has only 16 samples, with one synthetic
lock, loopback HTTP/1 and tmpfs media; the dirty server binary's source identity
was not independently established. This does not justify a generally safe
production default or establish HTTP/2, proxy or concurrent-ingestion behavior.

The controlled five-round contention repeat reported zero errors and exact
popularity, but p99 ranges remained 575–1,281 ms for a shared Track,
233–809 ms for separate Tracks sharing an Artist, and 679–852 ms during
concurrent credit changes (160 plays per round). The admission histograms did
not identify permit wait as the dominant measured cost. These results do not
establish a cause; Track and candidate locks, retries and scheduling still need
isolation, and the tail concern is not resolved.

The follow-up real HTTP/1 ingress run measured first media byte separately from
response headers; raw samples are in [the TTFB evidence](tests/performance/ingress-ttfb-600k-2026-11-26.md).
All 80 Jellyfin responses corresponded to persisted plays and exact popularity.
Caps 4 and 16 delayed all interactive probes during the held Track lock. This
single synthetic run does not establish a safe default or cover concurrent
production ingestion.

In a separate disk-backed mixed run on the 600,000-Track fixture, 200/200
synthetic releases reached `cataloged` in 60.901 seconds while a slow stream
remained open. Among active-window samples, first-media-byte p50/p95/p99 were
2.535/4.517/5.241 ms (n=59), and System/Info p50/p95/p99 were
0.549/0.718/0.894 ms (n=236), with no recorded request errors. This single run
used two workers per stage and synthetic media; it has no baseline, real-client
or audible-underrun measurement and did not run concurrent search rebuilding.
See [the mixed-run evidence](tests/performance/ingestion-stream-600k-2026-11-26.md)
for overlap and measurement limitations.

On the 600,000-Track fixture, three current release-build production policy
samples took 8.791–9.595 ms for snapshot plus evaluation and 377.170–404.488 ms
for an eight-Release dry-run load plus evaluation. The harness observed 13/3
SQLx statements and 7/10 returned rows respectively; retained acoustic digest
data was 64 bytes, not a measurement of wire traffic. An isolated scoped/global
query diagnostic is in [the current policy log](tests/performance/policy-current-2026-11-26.log).
These are three samples of one synthetic input with uncontrolled cache; baseline
and final integrated acceptance remain pending.

The corrected original/current policy comparison used identical benchmark and
fixture source on separate 600,000-Track databases (baseline migrations through
0059; current through 0066). Three baseline normal samples took 1,285.6–1,472.1
ms and returned 600,014 SQLx rows; current samples took 7.894–8.804 ms and
returned seven. Eight-Release dry-run samples took 16.305–16.756 s baseline
versus 390.060–418.873 ms current. The baseline retained 4,800,057 digest
entries (307,203,648 digest-character bytes), versus one/64 bytes current.
These are not PostgreSQL wire-byte counts. Process high-water RSS includes setup
and allocator effects. Each side used one fixture, three samples, uncontrolled
cache and eight one-track handoffs, with no mixed load; raw logs are
[baseline](tests/performance/policy-baseline-2026-11-26.log) and
[current](tests/performance/policy-current-2026-11-26.log).

An incremental search diagnostic applied and acknowledged 2,048 synthetic
Artist upserts in eight 256-event production batches in 0.4335 seconds (4,725
per second), with 152 observed SQLx statement events. Writer initialization
was 0.807 ms median and 6.203 ms total; commits had 10.726 ms median. A cold
query took 7.891 ms and ten warm queries 1.468–2.222 ms. This one Artist-only
run did not exercise concurrent load, and statement events are not proven
network round trips. It does not by itself settle writer lifecycle or final
acceptance.

The genre/scheduler diagnostic on the 600,000-Track fixture cataloged all 200
synthetic one-Track Releases in 62.518 seconds. Native and OpenSubsonic genre
aggregates each exactly matched 200 Releases and 200 Tracks. Across 234 sampled
observations, at most three advisory/lock waiters were seen; ready-to-first-claim
p50/p95 were 12.814/23.025 seconds (n=200). The [raw log](tests/performance/genre-scheduler-600k-2026-11-26.log)
records one run, an unverified dirty server binary identity and no interactive
load; sampling does not prove continuous utilization, fairness or lock duration.

Three subsequent mixed Artist/Release/Track outbox trials each applied and
acknowledged 2,048 events in eight batches, draining in 0.5905/0.5032/0.4976
seconds with 164 SQLx statement events per trial. Across 24 batches, writer
initialization was 0.740 ms median and 19.062 ms total; commit median was
15.326 ms. Process high-water RSS rose from 273,856 to 418,168 KiB across the
rebuild and trials; it cannot be attributed to writers alone. These were
sequential warm trials, not concurrent worker/interactive acceptance or a
long-lived-writer comparison.

A concurrent PostgreSQL/Tantivy diagnostic held the second-generation snapshot,
applied and acknowledged 2,048 mixed events on the old generation, replayed
those events into the new generation, then verified Artist/Release/Track
visibility. Four in-process readers completed 2,472 queries without errors;
p50/p95/p99 were 2.175/3.348/4.502 ms. Snapshot age was 32.491 s and the
activation call 6.640 ms. No sampled query spanned the activation completion,
so this does not establish in-flight switch behavior. This was one process, one
run, with an instrumentation barrier, uncontrolled cache and no HTTP or
competing writer process; SQLx events are not network round trips.


## Additional current execution evidence (not final acceptance)

The isolated original-baseline mixed workload on an owned 35K/50K/600K fixture
completed 200 generated one-track imports with concurrent search rebuilding and
HTTP probes at pool sizes 8 and 32. Elapsed times were 127.26 and 127.25 seconds
respectively, with zero HTTP errors. The [baseline record](tests/performance/db-competition-baseline-2026-11-26.md)
contains percentile samples, resource figures, exact binary provenance and the
important omissions: one uncontrolled-cache trial per case, no idle/single-load
controls, and no baseline pool hold metrics. The matching final-source run is
still required.

The [rate-limit contention workload](tests/performance/rate-limit-600k-2026-11-26.md)
exercised the production store on the 600K fixture. At 32 simultaneous workers,
three rounds of shared and separate actor keys produced 259–320 attempts/s,
roughly 95.9–98.2 ms p50 and 206.0–307.9 ms p99, with exact counters and no
errors. The tested pre-instrumentation limiter function matched the original
baseline; this is not a before/after measurement. Normal HTTP authentication,
password hashing, realistic request distributions, multi-process and mixed-load
behavior were not measured. Checkout and hold instrumentation was subsequently
added and passed its focused tests; a post-change representative measurement
and an evidence-backed limiter decision remain open.

On an owned 600K catalog, a [Jellyfin Audio SQL plan comparison](tests/performance/jellyfin-audio-index-plan-2026-11-26.md)
measured four sequential page-query samples at 109.7–118.4 ms before and
1.29–1.68 ms with the candidate ordering index. Exact count and page hashes
matched; the exact count alone remained approximately 68–77 ms. Migration 0069,
the page-scoped Artist count, all 20 focused Jellyfin media API tests, and fresh
and populated upgrade checks passed. This is a warm-order SQL microbenchmark,
not an HTTP before/after or a measurement of deep-page and write costs.

A [post-supervisor codec matrix](tests/performance/codec-matrix-supervised-2026-11-26.md)
measured retained MP3 and FLAC at 1/2/4 workers, two samples each, using eight
60-second synthetic single-track inputs. Mean release rates were
1.731/3.505/6.853 per second for MP3 and 1.419/2.825/5.418 for FLAC. The run
excluded database, scheduling and interactive traffic and predates subsequent
media source changes, so a final-source and mixed-load decision remains open.

An unused ffprobe version lookup was subsequently removed; 14 real-media
normalization tests passed, including wrapper counts confirming two independent
data probes and no version probe for retained MP3. A cold version subprocess is
avoided only when the configured executable path resolves directly. At that measurement the usual bare executable defaults failed metadata
lookup before spawning, leaving inspection version provenance absent. A later
PATH-resolution fix and focused tests restored Unix bare-name provenance; a
final integrated measurement remains pending. This narrow saving does not
complete RM-11. The current custody, ambiguous-path and subprocess
work remains in progress. In particular, the occurrence-bound filesystem
handoff and cleanup, final upgrades/recovery, required full suites and quality
gates have not been verified against the integrated working tree.

The representative fixture's former Release and Track rating expressions
generated only rating 1 for selected rows because their selection predicates
used the same remainder as the rating. The expressions now vary ratings across
1–5. An invocation-owned 50-Artist/100-Release/1,000-Track database verified
that Artist, Release and Track ratings each cover all five values. The Python
harness tests passed (13 passed, one owned-database test skipped outside the
fixture); the database-only test passed inside that fixture. Earlier benchmark
captures retained the old, skewed rating distribution, so their rating-sensitive
results must not be presented as representative; large-scale validation and
comparable final measurements remain pending.

The fixture subsequently gained alternate media-asset references for a subset
of Tracks. A 50-Artist/100-Release/1,000-Track invocation-owned smoke verified
that both playable and alternate associations were present and reran the
rating-distribution check. These are database references, not additional
physical media. Earlier captures did not include these alternates; the updated
fixture still requires representative-scale validation and matched comparisons.

The fixture also creates one generated PNG in its disposable asset root and
content-addressed Artist and Release artwork references. A small owned-database
smoke verified both reference kinds and the file's recorded SHA-256 and length.
This shared image is bounded fixture content; it is not evidence of artwork
route throughput, cache behavior, or a representative large artwork corpus.

The fixture subsequently gained library-group ACLs and synthetic publication
provenance for a subset of Releases. In a small owned database, two non-admin
users each had both allowed and denied Releases according to the production
authorization predicate. The receipt and manifest rows are placeholders, not
evidence of real ingestion; this smoke does not establish representative ACL
distribution, route latency, or end-to-end correctness at 600K Tracks.

The updated fixture was then verified at 35,000 Artists, 50,000 Releases and
600,000 Tracks, including 30,000 alternate links. Four data-shape checks passed;
the 183.2-second seeding duration is fixture setup, not application throughput.
The [sanitized run record](tests/performance/fixture-verification-2026-11-27.md)
states its provenance and remaining limitations. This does not replace matched
baseline and final production-path measurements.

An [intermediate 600K identity closure run](tests/performance/identity-closure-600k-2026-11-27.md)
compared all five scoped and global candidate vectors and found them nonempty
and equal. The initial production drain acknowledged all eight captured seeds
and generated 68 follow-up seeds; a further bounded turn consumed those in
32.029 ms and reached an empty queue. This single synthetic graph and source
snapshot do not replace final integrated, concurrent-import or end-to-end
acceptance measurements.

An intermediate 35K-Artist/50K-Release/600K-Track in-process admin statistics
ABBA comparison recorded 16 warm samples per variant: baseline p50/p95 were
615.140/640.197 ms and the candidate was 528.069/567.805 ms. Each process kept
its ordered normalized responses identical; across processes only the validated
volatile `database.totalBytes` value was masked. The [measurement record](tests/performance/admin-stats-ab-representative-2026-11-27.md)
uses a dirty intermediate snapshot, a quiesced fixture and shared host; it is
not a final-source, TCP or mixed-load measurement. Earlier failed attempts are
retained rather than counted as samples.

The first isolated supported-filesystem sealed-tree test emitted a positive
copy marker but subsequently exited with status 101. It is a failed test, not
evidence of a verified protected stage. The [sanitized failure](tests/performance/stage-verity-sealed-tree-2026-11-27.json)
and [build proof](tests/performance/stage-verity-sealed-tree-build-2026-11-27.json)
are retained. A [separate diagnostic rerun](tests/performance/stage-verity-sealed-tree-diagnostic-2026-11-27.json)
also failed: its named-user ACL setup selected an unmapped UID and returned
EINVAL inside the private user namespace. The sealed-copy and promotion
assertions ran before that failure; the full test still did not pass. A valid
mapped-UID ACL case was added, but a [subsequent rerun](tests/performance/stage-verity-sealed-tree-corrected-2026-11-27.json)
failed earlier because its cross-process lock probe could not execute inside
the private chroot. A [further re-exec run](tests/performance/stage-verity-sealed-tree-reexec-2026-11-27.json)
also failed with ENOENT despite the fixed executable path; the lack of a
`/dev/null` node was a hypothesis. A later [fingerprinted Btrfs run](tests/performance/stage-verity-sealed-tree-2026-11-27.md)
inheriting stdin passed the complete test 1/1, including the real sealed-copy
marker, with unchanged parent namespaces. This is a debug test of an
intermediate snapshot in a one-UID namespace; it excludes newer changes and
does not establish production publication, cleanup or final integrated safety.

One release-build [10,000-Track offline closure capture](tests/performance/admin-offline-release-closure-10k-2026-11-27.md)
completed in 22.999 seconds over 629 bounded turns against an owned database.
It exercises provisional metadata APIs only; filesystem deletion, merge/split,
statement and lock counts, concurrent activity and a 600K catalog were not
measured.

---

# 10. Final integrated measurement round (2026-11-27, `554e9d5` + benchmark fixes)

This section records measurements executed on the final integrated sources. It
does not replace §9 or the ledger; `PERF-REMEDIATION-STATUS.md` remains the
authoritative package state. Full provenance, commands, environment, limitations
and the explicit list of workloads not executed are in
`tests/performance/final-matrix-2026-11-27.md`.

## 10.1 Matched baseline/final at the 560K–600K tier

Two RM-00 workload rows now have matched production-path measurements on the
same declared fixture (35,000 Artists / 50,000 Releases / 600,000 Tracks),
release builds, one workload at a time, uncontrolled post-seed cache, fresh
invocation-owned database per case. The baseline is the verified detached
worktree at `a0979cde…`; only benchmark harness/test files were added there.

- **Full search rebuild (RM-14 / PERF-006).** Baseline 42.32 s, 16,188
  documents/s, 250,007,082 logical index bytes, 217,552 KiB peak RSS. Final
  35.21 s (snapshot 35.169 s), 19,454 documents/s, 231,080,841 bytes,
  279,084 KiB, with 69 commits (median 64.68 ms), 6.60 ms activation and
  17.1 ms / 0-row catch-up now instrumented (the baseline does not emit those
  events). One sample per side.
- **Policy evaluation and dry-run (RM-02 / PERF-002).** Baseline
  single-Release evaluate 1,203.0–1,278.9 ms returning and retaining 600,014
  rows / 600,008 acoustic digests (38,400,512 B); final 8.21–9.66 ms returning
  7 rows / 1 digest (64 B). Baseline bounded eight-Release dry-run
  15,806.8–16,363.8 ms retaining 4,800,057 digests (307,203,648 B); final
  374.7–396.4 ms retaining 1 digest. The scoped query returns the same
  cross-Release match; three samples per case per side.

## 10.2 Final-only production-path measurement

**Identity reconciliation at 600K (RM-09 / PERF-004).** The pre-remediation
revision has no scoped candidate methods, so no baseline pair exists. On the
final source, inside the production drainer with an augmented identity graph:
strong Artist 6.53 ms scoped vs 59.72 ms global; same-Release Track Artist
15.78 vs 2,963.83 ms; repeated credit 6.32 vs 1,365.02 ms; strong Release 2.24
vs 17.90 ms; name proposal 5.68 vs 718.75 ms. One bounded drain turn took
3,188.3 ms over 8 durable seeds (3 Artist merges, 1 Release merge, 68
follow-ups) and closure was quiescent after one further 32.1 ms turn.

## 10.3 Benchmark defects fixed (not production behavior)

The representative policy benchmark assumed its own eight seeded Releases were
the earliest `catalog_imports` rows; after the fixture gained ACL publication
placeholders it panicked at representative size. The benchmark now discovers
the actual bounded scope through the production loader and measures a subject
provably inside it. The fixture gained `--without-acl-placeholders` so a
bounded policy dry-run scope covers workload Releases rather than publication
placeholders; a Python regression covers the option. Production policy and
search code are unchanged by these edits.

## 10.4 Still outstanding in RM-00

Ten of the thirteen required workload rows were not executed in this round:
single-album import, 200-album import, unchanged reimport, media processing,
concurrent genre imports, incremental indexing, concurrent interactive load,
streaming during ingestion, worker scheduling, and merge/split. Their existing
artifacts are intermediate and must not be presented as final-source
before/after evidence. RM-00 therefore remains PARTIAL.
