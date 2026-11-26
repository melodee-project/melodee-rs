# Melodee-rs — Independent Performance Audit Validation

## A. Executive summary

Gemini-4-Neon's audit is a useful **hot-path locator**, but it is not a trustworthy implementation plan without substantial correction. Its strongest contributions are the catalog-wide acoustic-fingerprint read, the unindexed playable-asset lookup, global post-import reconciliation, discovery starvation, media-pipeline redundancy, and observability gaps. Its weakest areas are categorical claims about PostgreSQL plans, severity based on theoretical rather than measured resource exhaustion, overconfident latency/throughput targets, and remediations that change correctness, durability, or security semantics.

### Verdict totals

| Classification | Count |
|---|---:|
| CONFIRMED | 6 |
| PARTIALLY CONFIRMED | 9 |
| INCORRECT | 0 |
| REQUIRES MEASUREMENT | 0 |
| ALREADY RESOLVED | 0 |
| INSUFFICIENT EVIDENCE | 0 |
| New independently identified findings | 2 |

“Zero already resolved” applies to whole findings. Important **subclaims** were already mitigated: migration 0029 makes the native descending release-date query index-assisted, migration 0043's acoustic partial-index predicate is only `evidence_kind`, and the legacy importer already inserts media assets before tracks.

At least **12 of the 15 remediation packages require correction, scoping, or measurement**. Two are unsafe enough to reject as written in material part: the read-committed rebuild proposal and the metadata-instead-of-content-integrity proposal. The password-verification cache, naive rate-limit upsert, invalid trigger DDL, and blanket index/heartbeat removal are also rejected as written.

### Most significant production risks

1. `PolicyStore::load_context` and dry-run policy loading perform catalog-wide acoustic work per release.
2. `media_assets_native_file_name_refresh` performs a per-row unindexed reverse lookup into `tracks` on the ordinary import path.
3. Five global identity/reconciliation scans follow every successful catalog import.
4. Genre aggregate maintenance is both expensive and **provably incorrect under concurrent inserts**.
5. Search rebuild activation can permanently retain stale data after a late lower-ID outbox event has already been consumed (new PERF-V001).
6. Discovery repeatedly processes only the initial candidate subset and can permanently defer Releases, Artists, and chart relinking.

### Most significant remediation-induced correctness risks

- `max(id)` is not a commit-order watermark. A late-committing lower ID can be skipped by a read-committed rebuild.
- The proposed `AFTER UPDATE OF ... REFERENCING OLD/NEW TABLE` trigger is invalid PostgreSQL 18 syntax.
- Recount-overwrite genre aggregates lose concurrent contributions; naive incremental counters would make this worse unless distinct-release membership is handled correctly.
- Filesystem `(dev, ino, size, mtime)` does not prove content identity and cannot replace the required before/after content checks.
- A naive release-scoped reconciliation algorithm may fail to reach the transitive identity closure.
- Fixed pool splitting does not isolate PostgreSQL CPU, storage, locks, or WAL and cannot guarantee interactive SLOs.
- Password-result caching can weaken revocation and create a credential-equivalent in-memory artifact.
- A per-key rate-limit upsert alone does not preserve the exact retained-key ceiling.
- Removing the media-asset INSERT trigger breaks other valid insertion orders, including existing tests and recovery/future writers.

**Overall decision: accept the audit with modifications as an investigation input; reject it as an implementation plan.** The modular monolith, PostgreSQL authority, durable leases, filesystem content evidence, and rebuildable Tantivy generations are appropriate foundations. No additional broker, cache service, search cluster, or microservice decomposition is justified by this evidence.

Immediate next steps: establish release-mode baselines; add the narrow playable-asset index; scope acoustic novelty lookups with empty-input handling; correct genre aggregate concurrency before optimizing it; implement durable discovery progress; fix the search activation race before any snapshot redesign; and remove the unbounded scheduler result accumulation. Broad reader/writer, reconciliation, media, and pool refactors should follow measurement and explicit correctness designs.

## B. Repository revision and review coverage

### Revision and environment

- Commit: `abda925bf88fc6ebddfb10daece82514ffa2f9ab`
- Branch: `main` (tracking `origin/main` at the same commit at review start)
- Initial working tree: untracked `PERF-CONCERNS-REVIEW.md`; no tracked modifications
- Final intended repository change: new `PERF-CONCERNS-VALIDATION.md` only
- Rust/Cargo: `1.99.0`
- Python: `3.15.2`
- Disposable PostgreSQL: `18.3` on x86_64
- Host used for isolated checks: AMD Ryzen 9 3900X, 24 logical CPUs, approximately 64 GiB RAM
- Locked dependencies: Tokio `1.53.3`, SQLx `0.9.0`, Tantivy `0.26.2`, BLAKE3 `1.8.7`, Lofty `0.25.4`, Axum `0.8.9`
- Tantivy 0.26.2 supports `Index::reader_builder()`, `ReloadPolicy::{Manual, OnCommitWithDelay}`, and `IndexReader::reload()`. `SearchStore` currently hides its reader/writer fields, so a registry requires an intentional API design rather than blindly moving code from another Tantivy release.

### Reviewed architecture and source

- Entry points/configuration: `apps/melodee-server/src/main.rs`, persistence/catalog/ingestion/search configuration, worker-pool controls.
- Persistence: `catalog.rs`, `catalog_browse.rs`, `catalog_identity.rs`, `catalog_mutations.rs`, `catalog_hard_delete.rs`, `policy.rs`, `search.rs`, `search_retention.rs`, `work_items.rs`, `discovery.rs`, `auth.rs`, `manifest_evidence.rs`, `asset_files.rs`, `migrate.rs`.
- Catalog and policy: `melodee-catalog/src/{importer,scheduler,config}.rs`; `melodee-policy/src/{catalog_context,novelty,config}.rs`; `apps/melodee-server/src/policy_worker.rs`.
- Search: `melodee-search/src/{service,store,worker,rebuild,config}.rs`; generation/outbox tests and `phase6_benchmark.rs`.
- Ingestion/media: acquisition, inspection, normalization, validation/publication, reconciliation/scheduling, and media inspection/normalization/subprocess code.
- Interactive paths: admin stats, OpenSubsonic browse/auth/media, native catalog/stream, password execution, blacklist and artwork caching.
- Legacy writes: `apps/melodee-cli/src/legacy/import_tracks.rs` and the release merge/split paths.
- Tests/benchmarks: catalog performance and concurrency, native scale tests, ingestion performance, search smoke/rebuild tests, common database isolation support.
- ADRs: 0105/0107 (authority/Tantivy), 0115 (media hashing), 0117 (catalog identity and set-based import), 0121 (search generations), 0142 (browse indexes), 0145 (durable staging), 0158 (job history), 0165 (genre projection), 0166 (maintenance-fenced migrations), 0186 (fingerprint digest), 0202 (NFS publication), 0209 (lanes/throughput), 0210/0213 (asset and manifest evidence), 0212 (bounded enrichment), 0214 (outbox retention), plus the API/authentication decisions cited by those paths.

All 59 migrations were indexed for relevant tables, indexes, constraints, functions and triggers. Detailed definitions were checked in 0001–0003, 0005–0011, 0018–0019, 0029, 0038–0039, 0043, 0047, 0053, 0058 and 0059, with later mutations and write callers checked against earlier declarations.

### Executed tests, benchmarks and diagnostics

Release-mode tests against newly created databases on a socket-only disposable PostgreSQL instance:

- `melodee-search --test benchmark_smoke`: **1 passed**.
- `melodee-persistence --test catalog_concurrency`: **5 passed**.
- `melodee-search --test rebuild_integration`: **9 passed**.

These are functional checks, not scale measurements. The search smoke used its synthetic 10,000-track corpus. No full 560K/1M/5M/10M production-path benchmark was executed.

A separate disposable database was migrated from empty through all 59 migrations: 158 public tables and 473 public indexes. A synthetic catalog then contained 35,000 artists, 50,000 releases, 600,000 tracks, 600,000 acoustic evidence rows and 1.2 million genre-term rows. Diagnostics included:

- `EXPLAIN (ANALYZE, BUFFERS)` for present and absent playable assets, exact hashes, scoped/unscoped fingerprints, genre hydration, artist ratings, release-date sorting, and two-/three-character substring searches.
- `auto_explain` nested trigger plans before and after a temporary playable-asset index.
- A valid/invalid trigger-DDL experiment proving `UPDATE OF` cannot be combined with transition tables on PostgreSQL 18.3.
- Temporary indexes used only in the disposable database to compare plans.
- A two-session genre-insert test demonstrating aggregate undercount after both transactions committed.
- Controlled outbox experiments for lower-ID late committers and old-generation acknowledgement before rebuilt-generation activation.
- Inspection of installed SQLx 0.9 migration code: transactional by default; `-- no-transaction` is recognized only at the beginning of a migration.

The application-configured database and media roots were not used. Experimental SQL and Rust harnesses were created under `/tmp`, outside the application source tree.

### Limitations and areas not deeply reviewed

- No production-shaped NFS, cold-cache, 5 TB media corpus, sustained client load, 1M/5M/10M catalog, CPU profiler, Tokio runtime instrumentation, `perf`, `iostat`, or `pidstat` measurements were collected.
- Plan timings on the 600K synthetic catalog are single-instance evidence, not production SLO evidence. Buffer hits are not physical reads, and plan selection depends on data distribution and configuration.
- The synthetic UUIDs are UUIDv7 and the genre distribution is intentionally skewed, but they are not a copy of a real library.
- Frontend rendering, MQL parser internals, scripting execution, podcast/provider network behavior, and the complete legacy migration suite were outside the performance-critical follow-up review.
- Absolute estimates below are analytical bounds or conditional calculations unless explicitly identified as measured in this review.

## C. Original finding validation matrix

| ID | Original Severity | Independent Verdict | Revised Severity | Confidence | Remediation Verdict | Implementation Recommendation |
|---|---|---|---|---|---|---|
| PERF-001 | P0 | CONFIRMED | P0 | High, including runtime plan evidence | ACCEPT WITH CHANGES | Add narrow index first; decouple ordering/trigger redesign |
| PERF-002 | P0 | CONFIRMED | P0 | High, including runtime plan evidence | ACCEPT WITH CHANGES | Scope to release digests, preserve `?` predicate, handle empty inputs |
| PERF-003 | P0 | CONFIRMED | P0 | High; concurrent correctness failure reproduced | ACCEPT WITH CHANGES | Correct concurrency first; add indexes; retain valid statement trigger |
| PERF-004 | P0 | CONFIRMED | P0 | High static; cost requires scale measurement | DEFER | Index now; design affected-identity closure before scoping |
| PERF-005 | P1 | PARTIALLY CONFIRMED | P1 | High static; impact partly unmeasured | ACCEPT WITH CHANGES | Drain/gate/batch first; reader/writer lifecycle needs design |
| PERF-006 | P1 | PARTIALLY CONFIRMED | P1 | High static; duration unmeasured | REJECT for snapshot replacement; DEFER safe optimizations | Preserve snapshot; optimize blocking work/commits/hydration first |
| PERF-007 | P1 | PARTIALLY CONFIRMED | P1 | High static; counts and throughput conditional | REJECT as written; ACCEPT safe subset with changes | Reuse only provably identical work; bounded blocking; measure FFmpeg matrix |
| PERF-008 | P1 | PARTIALLY CONFIRMED | P1 | High for topology/transaction; exhaustion unproven | DEFER | Measure pool pressure; redesign manifest revalidation before split |
| PERF-009 | P1 | PARTIALLY CONFIRMED | P2 | High | ACCEPT WITH CHANGES | Bound retained results; carefully implement rolling claims |
| PERF-010 | P1 | PARTIALLY CONFIRMED | P1 | High static plus plan evidence | ACCEPT WITH CHANGES | Optimize measured endpoints; do not use simplistic auth caching |
| PERF-011 | P2 | PARTIALLY CONFIRMED | P2 | High | ACCEPT WITH CHANGES | Drop only proven equivalents; reject HOT/audit claims |
| PERF-012 | P2 | CONFIRMED | P1 | High; functional starvation | ACCEPT WITH CHANGES | Durable per-entity cursors, wraparound and independent chart budget |
| PERF-013 | P2 | PARTIALLY CONFIRMED | P2 | High static; runtime impact unmeasured | REJECT security-sensitive pieces; ACCEPT WITH CHANGES elsewhere | One blocking reader task/stream; no password cache as proposed |
| PERF-014 | P2 | PARTIALLY CONFIRMED | P2 | High static; cache and throughput unmeasured | ACCEPT WITH CHANGES / DEFER recommendation async | Batch selectively; sort actual lock keys; preserve recommendation freshness |
| PERF-015 | P2 | CONFIRMED | P2 | High | ACCEPT WITH CHANGES | Small bounded in-process metrics and production-path benchmarks |

## D. Detailed validation of original findings

### PERF-001 — Media-asset reverse lookup and missing playable-asset index

**Original severity:** P0  
**Revised severity:** P0  
**Independent verdict:** CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

`tracks(playable_media_asset_id)` has no usable leading-column index, so every media-asset insert performs a full track scan, with billions/trillions of logical row examinations during bursts. The audit also recommends reversing insertion order and removing the INSERT half of the media trigger.

#### Independent evidence

- Migration `0007_phase4_catalog.sql:402-535`: `tracks`, deferred circular playable FK, `track_media_assets`, and indexes. No later migration adds a leading playable-asset index.
- Migration `0019_phase7_native_route_bounds.sql:21-69`: row functions and both INSERT/UPDATE triggers.
- Ordinary import: `crates/melodee-persistence/src/catalog.rs:756-834` (`import_manifest_v3`) and `3733-3808` (`persist_prepared_import`): tracks precede media assets and links.
- Policy exact hash: `crates/melodee-persistence/src/policy.rs:2203-2213`.
- Legacy importer: `apps/melodee-cli/src/legacy/import_tracks.rs:405-530` already inserts media assets before tracks.
- Other callers: release merge/split updates at `catalog.rs:2051` and `2392`; admin metadata updates asset hashes/sizes, not canonical paths; test fixtures use both insertion orders; hard deletion removes tracks/links/assets.
- The deferred FK is `tracks_exactly_one_playable_asset_fk`; `track_media_assets` has ordinary nondeferred FKs. Media-before-track therefore works only when the link follows tracks in the same transaction.

Measured on the disposable 600K-track catalog:

- A present/absent playable-asset lookup used a parallel sequential scan.
- One unreferenced media-asset insert fired `media_assets_native_file_name_refresh` once. Its nested plan scanned all 600,000 tracks and attributed about 58 ms to the insert/trigger.
- A temporary **narrow** `tracks(playable_media_asset_id)` index changed that nested lookup to one index search with three shared-buffer hits; the exact-hash policy query fell from about 45 ms to 0.42 ms in that run.

#### Validation analysis

The defect, row frequency, ordinary insertion order, and policy impact are correct. Logical arithmetic `2,500 × 560,000 = 1.4B` and `2,500 × 10M = 25B` is correct **if every insert scans every row**. It is not a measured disk-I/O estimate. Buffer visits can be shared hits, OS-cache reads, or physical storage reads.

The audit overstates coverage: legacy migration already uses the safer order, and not every test or future writer can be assumed to use ordinary import order. Removing the INSERT trigger would leave filenames missing wherever media is inserted after tracks. Track insertion's trigger currently removes a missing filename and the media trigger later fills it; changing this requires a complete write-path proof.

#### Performance impact

At 560K tracks, per-track import work grows linearly with catalog size and is P0 because it compounds with the other import costs. At 1M/5M/10M the same arithmetic scales to 2.5B/12.5B/25B logical row examinations per 2,500-asset burst if the plan remains a sequential scan. These are worst-case logical examinations, not billions of physical reads. Adding the index turns reverse lookup into approximately logarithmic indexed access. `INCLUDE (release_id)` is not needed by the trigger, which returns only `track.id`; it is optional only after proving an index-only policy plan is useful.

#### Proposed remediation review

The narrow index is strongly supported. Reordering ordinary import and making the media trigger UPDATE-only can reduce residual trigger work, but it is an independent semantic change with migration, test and recovery implications. A row-to-statement trigger rewrite also changes filename projection timing and needs all mutation paths covered. None of these is a prerequisite for the high-value index.

#### Correctness and regression risks

- Removing INSERT maintenance can make `native_track_file_names` incomplete/stale.
- Reordering without retaining both trigger paths breaks media-after-track writers.
- A covering index adds write/storage cost without a demonstrated consumer.
- Concurrent index creation cannot be placed in the ordinary transactional SQLx migration.

#### Recommended remediation

First add only `tracks (playable_media_asset_id)` through a forward migration under the ADR-0166 maintenance fence. Keep both triggers and existing insertion order. Separately evaluate a set-based filename projection after enumerating all writers. Do not add `INCLUDE` until `EXPLAIN` establishes a benefit that justifies it.

#### Required regression tests

- Ordinary import populates exact lowercased basenames from `COALESCE(source_path, canonical_relative_path)`.
- Media-before-track and track-before-media fixtures; retries and conflicts; >4096/empty names deletion behavior.
- Canonical path updates refresh every referencing track; source path/playable asset updates do likewise.
- Legacy import, release merge/split, recovery, admin metadata and hard-delete tests.
- Policy exact-hash tests with duplicates, no matches and multiple evidence rows.

#### Required performance validation

On 560K and 10M-shaped isolated catalogs, compare nested `auto_explain` trigger plans and `EXPLAIN (ANALYZE,BUFFERS)` for present and absent IDs. Acceptance: no playable-asset sequential scan; bounded buffers per asset; unchanged native filename rows. Compare shared hits and reads separately and record cold/warm cache state.

#### Implementation readiness

**READY** for the narrow index. **DESIGN REQUIRED** for insertion-order or trigger changes.

### PERF-002 — Catalog-wide acoustic fingerprint retrieval

**Original severity:** P0  
**Revised severity:** P0  
**Independent verdict:** CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Normal and dry-run policy contexts load every catalog acoustic fingerprint, although downstream novelty only tests membership for the release's digests.

#### Independent evidence

- `crates/melodee-persistence/src/policy.rs:2165-2335`: `load_context`; acoustic query at `2266-2277`.
- Dry-run aggregate at `policy.rs:2572-2580`; conversion at `3779-3782`.
- `crates/melodee-policy/src/novelty.rs:104-113,327-343`: only `contains(track_digest)` is used. No global count or uniqueness consumer exists.
- `crates/melodee-policy/src/catalog_context.rs:32-65`: the set is cloned into `NoveltyInput`.
- Migration `0043_acoustic_identity_evidence_indexes.sql:14-21`: expression index predicate is **only** `evidence_kind = 'acoustic_fingerprint'`, not `payload ? 'fingerprint_digest'`.
- The dry-run selection is bounded to 10,000 releases, with separate total track/classification bounds, but its per-release acoustic arrays remain catalog-wide.

Measured on 600K unique acoustic rows:

- Unscoped SQL returned 599,988 distinct values and executed in about 804 ms before Rust allocation/transfer.
- Adding the release digest `ANY` predicate used `audio_identity_evidence_acoustic_digest`; both with and without the JSON-key predicate used that partial index. The selected plan still scanned `tracks` in a hash join, so index usage alone is not a complete plan guarantee.
- The current exact-byte-hash policy query also lacked the PERF-001 reverse index.

#### Validation analysis

The central claim is accurate. The fingerprint result can be restricted to the current release's non-null/nonblank digests while excluding the current release, preserving membership semantics. `DISTINCT` handles duplicate evidence and multiple owners. Missing/null digests are ignored by novelty. Empty digest input should return an empty set without issuing a broad query.

Memory figures are conditional. A 64-byte digest has 64 payload bytes plus `String`, B-tree, SQLx row and clone overhead. 65–80 MB at 560K is plausible but unmeasured; >1.3 GB at 10M is a plausible upper-range allocation scenario, not a demonstrated representative footprint. The dry-run can produce billions of array elements and must be treated as a severe independent risk.

#### Performance impact

At 560K/1M/5M/10M, the current SQL, transfer and Rust sets scale with the entire catalog per release and four default concurrent policy tasks multiply allocation. Dry-run multiplies it again by selected releases. Scoped work becomes roughly proportional to release digest count plus matched evidence, not catalog cardinality.

#### Proposed remediation review

`payload->>'fingerprint_digest' = ANY($2)` is directionally correct and compatible with the existing partial expression index. Preserve `evidence_kind` in every form. Keeping `payload ? 'fingerprint_digest'` is harmless and remains required by the current SQL shape, but is not part of the index predicate.

The dry-run `IN (SELECT track_fact.fingerprint_digest ... wm.id ...)` correctly uses manifest facts and avoids full-evidence reads. Prefer returning only cross-release matches (or an `EXISTS`-derived set) rather than all evidence rows for those digests. Verify the selected plan rather than asserting it will always be an index scan.

#### Correctness and regression risks

- Empty arrays must not degrade into an unbounded query.
- Same-release exclusion must remain by `owner.release_id`, not evidence source identity.
- Multiple evidence rows and duplicate fingerprints must not duplicate decisions.
- Dry-run and normal policy must return equivalent novelty decisions.
- Restricting by manifest track facts must include every whole-file fingerprint; CUE segment behavior remains unchanged.

#### Recommended remediation

Build a deduplicated digest vector from already loaded release tracks. If empty, skip the query. Otherwise add the exact expression `= ANY($2::text[])`, `evidence_kind`, JSON-key predicate and same-release exclusion. Apply the manifest-fact scoped equivalent to dry-run. Add the PERF-001 index independently.

#### Required regression tests

- Empty, null, blank and duplicate digests; multiple evidence rows per track; shared digests across releases.
- Same-release-only fingerprints produce no catalog match; cross-release matches produce known novelty.
- Normal/dry-run equivalence with externalized and legacy inline manifests.
- Large dry-run remains bounded and does not construct catalog-wide arrays.

#### Required performance validation

At 560K/1M/5M/10M evidence rows, measure SQL time, rows transferred, application RSS and dry-run temp files. Acceptance: rows transferred proportional to current-release digests/matches, no unscoped result materialization, equivalent decisions, and no large dry-run temp spill.

#### Implementation readiness

**READY**, with query-shape and regression requirements above.

### PERF-003 — Genre aggregation triggers, indexing and correctness

**Original severity:** P0  
**Revised severity:** P0  
**Independent verdict:** CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Genre terms lack useful indexes, four full recounts occur per import, and release search hydration can become O(Releases × Tracks). The proposed trigger restricts updates with `UPDATE OF status` and transition tables; longer-term delta maintenance is proposed.

#### Independent evidence

- Migration `0038_catalog_genre_serving_projection.sql:14-53,105-237`: table/indexes and three statement triggers.
- Migration `0059_manifest_evidence_files.sql:196-232`: genre projection now reads compact track facts, not manifest JSON.
- Import writes: `catalog.rs:4128-4180` (catalog import then overlay pointer update).
- Search hydration: `crates/melodee-persistence/src/search.rs:1825-1830`.
- Existing indexes lead with `(track_id,...)`, `(surface,match_value,...)`, `(track_id,surface,...)` and `(surface,normalized_value,...)`, not `release_id` or exact `display_value`.

Runtime checks materially qualify the plan claims:

- PostgreSQL 18.3 used skip-scan on `catalog_genre_terms_release_match_idx` for release/native hydration (123 index searches on a present release, 1.279 ms), not a sequential scan. A temporary `(release_id,surface) INCLUDE(display_value)` index used one search and about 0.071 ms.
- The audit's 28B estimate uses 50K releases × 560K tracks even though the synthetic relation would contain roughly 1.12M term rows; more importantly, skip-scan makes the claimed unconditional multiplication false.
- The invalid trigger DDL was executed and PostgreSQL returned: **`transition tables cannot be specified for triggers with column lists`**.
- A two-session same-new-genre experiment committed two tracks on two releases but stored `song_count=1, release_count=1`; authoritative terms contained two/two. This is a reproduced lost-contribution correctness defect.

#### Validation analysis

The index gaps and unnecessary overlay-trigger work are real. Four recount executions on the ordinary path are structurally correct, but “full scan every time” is planner-dependent. Popular-genre recount cost and row-lock serialization remain scale concerns.

The most important result is concurrency correctness. Each statement-level function computes from its own snapshot and overwrites the aggregate. Two transactions can both count one, then both store one. Deterministic sorting alone does not solve this. Naive incremental `song_count += delta` also cannot derive distinct-release counts safely without release-genre membership handling.

#### Performance impact

At 560K tracks, popular-genre recounts can scan large genre groups and serialize two default catalog workers. At 1M/5M/10M this grows with genre cardinality, not just album size. Search hydration is improved by a leading release index; do not use the audit's O(R×T) number as a measured estimate.

#### Proposed remediation review

Both proposed indexes are reasonable. The trigger DDL must not be used. Keep the existing all-UPDATE transition-table trigger and return early when no old/new status actually differs:

```sql
IF NOT EXISTS (
  SELECT 1 FROM old_release_rows o
  JOIN new_release_rows n ON n.id = o.id
  WHERE n.status IS DISTINCT FROM o.status
) THEN
  RETURN NULL;
END IF;
```

The proposed incremental design is under-specified for concurrent inserts/deletes, multiple genres per track, duplicate terms, release merges, rollback and rebuild. A helper relation is only justified if it makes release membership correct and testable; it should not be added merely to reduce counts.

#### Correctness and regression risks

- Lost concurrent aggregate updates (demonstrated).
- Track vs distinct-release count divergence under multi-track/multi-genre imports.
- Deadlocks from inconsistent per-genre lock order.
- Incorrect counts on deletion, merge/deprecation/restoration, retry and rollback.
- Eventual-consistency changes if maintenance becomes asynchronous.

#### Recommended remediation

First add release/surface and surface/display/release indexes. Keep the statement trigger and add an internal actual-status-change guard. For immediate correctness, serialize recounts for the same `(surface,display_value)` using deterministic advisory keys before recounting, or another design proven to make a later transaction see the earlier committed contribution. Add concurrent tests before claiming a fix. Only then benchmark whether asynchronous coalesced maintenance or a membership-aware incremental projection is justified.

#### Required regression tests

- Two concurrent same-genre imports and opposing multi-genre orderings; exact song/release counts.
- Multiple tracks per release and duplicate terms; insert/delete/update of terms.
- Active↔deprecated/merged/deleted transitions; overlay-only updates cause no recount.
- Release merge/split, hard delete, rollback, interrupted maintenance and full projection rebuild equivalence.
- Deterministic lock ordering and no deadlock under concurrent workers.

#### Required performance validation

Use 4+ concurrent imports into a 560K/10M catalog with shared and distinct genres. Record per-genre lock waits, deadlocks, buffers and count correctness. Acceptance: zero incorrect aggregates/deadlocks; release hydration uses a leading release index; no aggregate work for overlay-only updates.

#### Implementation readiness

**READY** for indexes and valid status-transition guard. **DESIGN REQUIRED** for concurrency correction and any incremental/asynchronous replacement.

### PERF-004 — Global artist and release reconciliation

**Original severity:** P0  
**Revised severity:** P0  
**Independent verdict:** CONFIRMED  
**Confidence:** High for execution path; Medium for wall-clock impact  
**Remediation verdict:** DEFER

#### Original claim

Five unscoped global reconciliation queries run after every successful import, causing 1,000 analytical scans per 200 albums. Scope them to the imported release and retain a global safety sweep.

#### Independent evidence

- `crates/melodee-catalog/src/importer.rs:176-180,221-226,503`: all four artist queries plus strong-release query execute on every `Ok(result)`, including already-completed imports.
- Candidate SQL: `catalog.rs:857-970`, `977-1050`, `1402-1580`, `1589-1660`, `1666-1715`.
- Startup: `crates/melodee-catalog/src/scheduler.rs:48` invokes artist reconciliation only; strong-release reconciliation is not included there.
- `artist_match_decisions` has no leading `artist_id` index (migration `0007:680-722`, later 0029 adds observed/outcome only).
- Default catalog claim limit is `min(batch_size, concurrency) = 2`, so two workers can execute overlapping global scans and merge attempts.

#### Validation analysis

The execution path and missing index are correct. Queries are bounded to 64 candidates, but candidate discovery is global and potentially expensive. Complexity is not uniformly five full sequential scans: indexes, hash joins and CTEs alter plans. No timing was collected for these five SQL bodies, so “seconds per album” and “tens of seconds” remain projections.

The claim that direct-release scoping necessarily preserves convergence is unsupported. An import can make a prior external-ID/name group resolvable; merges change redirect and credit evidence; same-release rules depend on complete name groups; a release merge changes subsequent candidates. The startup artist sweep helps legacy state but does not prove a permanent global-release safety net.

#### Performance impact

At 560K tracks and 200 albums, exactly 1,000 global candidate evaluations occur if every import returns `Ok`. At 1M/5M/10M, per-album work can grow with total identity/evidence cardinality, producing quadratic catalog-build behavior. Actual severity needs `pg_stat_statements` and plans at representative evidence distributions.

#### Proposed remediation review

Adding `artist_match_decisions (artist_id, observed_artist_id)` is safe and immediately useful. A release-only filter is too narrow for cross-release/transitive identity effects. A robust design needs durable affected identity seeds, iterative closure, bounded work, retry and a periodic global backstop. Coalescing imports and avoiding reconciliation for idempotent no-op results are simpler complementary reductions.

#### Correctness and regression risks

- Missed historical or transitive merges/proposals.
- Concurrent workers choosing conflicting merge candidates.
- Scoped merges changing evidence needed by later identity decisions.
- Mistaking candidate limits for bounded database cost.

#### Recommended remediation

Add the index now. Return structured affected IDs/names/external IDs from imports and design an affected-identity work queue with iterative closure and a low-frequency global reconciliation backstop. Prove equivalence against the current global algorithm on historical ambiguous fixtures before replacing it. Do not infer correctness merely because unchanged entities “should” have converged earlier.

#### Required regression tests

- New external ID resolves an old duplicate; multi-hop merges; same-name distinct artists remain distinct.
- Previously ambiguous same-release and cross-release groups; locked/negative decisions.
- Release merge changes later candidates; concurrent imports and idempotent retries.
- Crash/retry of affected closure and periodic global equivalence comparison.

#### Required performance validation

Capture all five statements in `pg_stat_statements` over 200/1,000 albums at 560K and 10M. Acceptance for a redesign: per-import work proportional to affected closure, no missed candidates versus global oracle, and bounded queue depth/RSS.

#### Implementation readiness

**READY** for the index. **DESIGN REQUIRED** for scoped reconciliation. **MEASUREMENT REQUIRED** before claiming a 10–50× improvement.

### PERF-005 — Tantivy reader/writer lifecycle and outbox processing

**Original severity:** P1  
**Revised severity:** P1  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Every search opens a new index/reader; outbox opens a writer per batch, sleeps after every batch, hydrates N+1, and performs maintenance aggregation at 1 Hz.

#### Independent evidence

- `crates/melodee-search/src/service.rs:36-140,214-247`: active generation DB lookup and manifest read/hash precede per-call `open_readonly` in `spawn_blocking`; the store is dropped after each operation.
- `store.rs:58-127`: writer allocation budget 50,000,000 bytes and default reader. `IndexReader` is held only by `SearchStore`; a short-lived `Searcher` is created per method.
- `worker.rs:243-284`: fixed sleep every loop, even after a full batch; maintenance at `292-350`; sequential hydration at `487-515`; blocking batch at `765-815`.
- `search_retention.rs:220-370`: one SQL with **eight** aggregate scalar subqueries, followed by a separate blocking-rebuild query; settings resolution adds work. The “six-subquery” description is inaccurate.
- Existing snapshot page SQL at `search.rs:2029-2110` does use `ANY($1)` for IDs, but its shape is not a drop-in replacement for ordered arbitrary outbox hydration.
- Tantivy 0.26.2's default `index.reader()` uses `OnCommitWithDelay`; OS page cache and Tantivy internal caches reduce repeated physical reads but do not remove open/mmap/setup cost.

#### Validation analysis

The structural claims are substantially correct. The 256/s number is a configured **upper bound from one-second sleeps with zero work time**, not measured throughput; actual throughput is lower with work and configurable poll interval can change it. The 86,400 maintenance-status invocations/day is mathematically correct for the default 1 Hz loop, but there are additional SQL statements, and empty outbox costs are not equivalent to full-table physical scans.

Long-lived readers are conventional and ADR-0121 explicitly calls for opening active generations once, so current behavior also diverges from intended architecture. Long-lived writers need generation and lease correctness, not just resource reuse.

#### Performance impact

At 560K–10M documents, per-request setup may hurt tail latency, but no p95/p99 measurement supports a specific number. Outbox throughput is bounded by sleep, N+1 SQL and writer churn. Idle maintenance is unnecessary repeated DB work, though its actual cost depends on retained outbox cardinality and indexes.

#### Proposed remediation review

Drain consecutive full batches and gate maintenance locally with a monotonic `Instant`; these are narrow improvements. Batched hydration must preserve outbox ID order, duplicate entity revisions, deletes/no-document behavior, poison-row replay and commit-before-ack. It cannot blindly reorder by entity type.

A reader registry must validate manifest/generation, reload safely, and handle activation by another process. A long-lived writer must reopen on generation/error and respect the single projection lease. Retired-generation deletion needs consideration for cached readers and multi-process lifetime.

#### Correctness and regression risks

- Stale readers after activation or deleted generation directories.
- Multiple writers on network filesystems or after lease loss.
- Batching changing revision/deletion semantics or acknowledging before durable commit.
- A document failure losing previously accepted writes during replay.
- Unbounded memory if writer heap is increased without admission limits.

#### Recommended remediation

Implement drain and hourly in-memory maintenance gating first; use the existing small `last_prune_at` read for that gate if needed. Add ordered batched hydration with explicit per-event mapping. Design reader/writer registries around the projection lease and generation state, using Tantivy 0.26.2 APIs. Keep commit-before-ack and failure replay unchanged.

#### Required regression tests

- Full/partial batch drain and idle backoff; maintenance once/hour across restart.
- Ordered mixed artist/release/track upserts/deletes and duplicate entities/revisions.
- Crash after Tantivy commit before SQL ack; poison row replay; lease expiry.
- Local/remote generation activation, old-reader retirement, writer lock loss and corrupt manifest recovery.

#### Required performance validation

At 560K and 10M documents, compare cold/warm query p50/p95/p99, open count, outbox drain events/s, SQL count, segment count, RSS and maintenance queries/day. The <50 ms and >1,500 docs/s figures are proposed targets, not established acceptance evidence.

#### Implementation readiness

**READY** for drain/maintenance gating. **DESIGN REQUIRED** for batched hydration and long-lived handles.

### PERF-006 — Search rebuild consistency and blocking work

**Original severity:** P1  
**Revised severity:** P1  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** REJECT for snapshot replacement; DEFER safe optimizations

#### Original claim

Rebuild performs Tantivy work on Tokio async threads, does pointless revision lookups on an empty index, commits every 1,000 rows, and holds a multi-hour repeatable-read snapshot that “blocks autovacuum globally.” Replace it with short read-committed pages plus outbox replay.

#### Independent evidence

- `crates/melodee-search/src/rebuild.rs:129-215`: `SearchStore::create`, snapshot, per-page `upsert_*` and `commit` directly in an async function.
- `store.rs:127-205`: every upsert does a term query/stored-document revision lookup; a fresh generation is empty.
- `search.rs:1998-2125`: one `REPEATABLE READ READ ONLY` connection and bounded pages.
- `rebuild.rs:248-347`: catch-up and activation; `search.rs:753-842`: activation takes outbox `SHARE` lock and rechecks max ID.
- `rebuild.rs:344-380`: immediate old-generation retirement; activation/generation leases prevent concurrent live rebuilds per root.
- ADR-0121 requires snapshot build, catch-up and atomic activation. ADR-0214 pauses pruning during a rebuild and preserves pruned high-water/tombstones.
- Current rebuild tests passed (9/9), but they do not cover the lower-ID late-committer/old-generation-consumption sequence.

The new PERF-V001 experiment proves that sequence-ID order is not commit order. A long `REPEATABLE READ` does not stop the autovacuum **process**; it advances the vacuum xmin horizon and prevents relevant dead tuples from being reclaimed across the database, allowing bloat.

#### Validation analysis

The synchronous work, per-page commits, unnecessary revision lookups and snapshot lifetime are confirmed. “Hours” is unmeasured. Commit counts (about 645 at 560K and 10,085 at 10M with the stated entity totals) are mathematically derived. No evidence supports <2 minutes or <30 minutes.

The proposed read-committed algorithm is not safe. Outbox IDs are assigned before commit. `max(id)` cannot prove that all lower IDs were visible when read. Revision fencing cannot help when a newer revision is never read. Outbox pruning protection addresses retention, not invisibility. Activation's `SHARE` lock also does not solve it, as PERF-V001 demonstrates.

#### Performance impact

At 560K–10M, thousands of synchronous commits and unnecessary lookups can extend snapshot duration and delay dead-tuple cleanup. The actual wall time, vacuum impact and Tokio stalls require measurement. The safer optimizations can shorten the snapshot without changing consistency.

#### Proposed remediation review

**Reject** replacing the snapshot with only max-ID replay. Safe initial items: an empty-generation bulk insert path without revision lookup; move indexing/commit to bounded blocking execution; reduce commit frequency with bounded writer memory; improve genre hydration; and measure snapshot duration.

A future short-transaction design needs a commit-order/catch-up proof, not merely UUID keyset pagination and max ID. Options require an accepted ADR and explicit crash/concurrency tests.

#### Correctness and regression risks

- Late lower-ID commits omitted.
- Upserts/deletes before/after row scan yielding stale or resurrected documents.
- New UUIDv7 rows behind cursor; outbox retention and crash during catch-up.
- Old-generation counts and activation validation becoming inconsistent.
- `spawn_blocking` merely moving work into an unbounded queue.

#### Recommended remediation

Retain the consistent snapshot for now. Implement bounded blocking bulk insert, remove revision lookups only on the proven-empty generation, reduce commits with a memory/time bound, and optimize hydration. Fix PERF-V001 independently before considering snapshot changes.

#### Required regression tests

- Late lower-ID upsert and delete, normal outbox consumed before activation (PERF-V001).
- Concurrent inserts/updates/deletes before and after each keyset page; crash before/during/after catch-up and activation.
- Pruned outbox history, duplicate revisions, stale revision replay and concurrent rebuild claims.
- Vacuum/transaction-age diagnostic test; bounded blocking queue and cancellation.

#### Required performance validation

At 560K/1M/5M/10M, measure rebuild time, docs/s, commits, writer RSS, PostgreSQL oldest-xmin age and dead tuples, and Tokio poll latency. Acceptance must first require identical authoritative results under concurrent writes; performance targets remain to be baselined.

#### Implementation readiness

**READY** for safe bulk/commit/hydration optimizations. **DO NOT IMPLEMENT** the proposed max-ID read-committed replacement. **DESIGN REQUIRED** for any alternative consistency model.

### PERF-007 — Media I/O, hashing and subprocesses

**Original severity:** P1  
**Revised severity:** P1  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High for static paths; Medium for operation counts  
**Remediation verdict:** REJECT as written; ACCEPT safe subset with changes

#### Original claim

Albums undergo 6–12 full reads, 13.5K–16K subprocesses per 200 albums, synchronous hashing blocks Tokio, inspection ignores concurrency, and FFmpeg needs `-threads 1`.

#### Independent evidence

- Same/cross-device acquisition: `crates/melodee-ingestion/src/acquisition.rs:455-493,647-704,746-936`.
- Inspection: `inspect.rs:39-125` serially processes the claimed batch; immediate and post-inspection verification at `189-204,344-365`.
- Normalization: `normalize.rs:162-204` performs receipt verification then immediate duplicate verification; blocking media work is already in `spawn_blocking` at `320-323`.
- Audio: `melodee-media/src/normalize.rs:1021-1125` source hash, source ffprobe, retained decode/copy or encode, output hash/probe, fpcalc, source hash; outer source verification at `318`.
- Tool versions: `normalize.rs:184-186` three processes per album; inspection also probes ffprobe version once.
- Subprocess polling and pipe threads: `melodee-media/src/lib.rs:1266-1310`, `normalize.rs:1518-1572,1583-1668`.
- ADR-0145 explicitly accepts repeated cross-filesystem verification; ADR-0202 requires publication verification. Staging is Melodee-owned, but ownership is not an OS-enforced immutable-file guarantee.

For an ordinary non-CUE, no-artwork album:

- Inspection: one version probe plus one ffprobe per track.
- Normalization: three version probes plus, per track, source ffprobe, retained decode **or** encode ffmpeg, output ffprobe and fpcalc = four processes per track.
- 200 albums/2,500 tracks: **10,800 processes**, plus one per embedded/external artwork invocation. One embedded artwork per track gives 13,300; the audit's 13.5K–16K range is conditional, not representative proof.
- Same-filesystem acquisition has one tree hash and no source cleanup hash after successful rename. Cross-filesystem has source before/copy/source after/copied/cleanup verification passes.
- Inspection adds three tree/audio verification passes; normalization adds two more before media processing. Per-track source before/after and final verification add more. The “6–12” count is therefore not a universal total; an ordinary same-filesystem audio path can have roughly 13 source/tree/audio passes plus output reads, and cross-filesystem adds five. Output and decode/copy/encode reads must not be conflated with source reads.

#### Validation analysis

Duplicate adjacent hashes, serial inspection, serial per-track normalization, batch barriers and subprocess polling are real. Some hashes are deliberate custody/integrity boundaries, not waste. The audit underestimates some paths while overstating a single universal count.

`-threads 1` is not universally optimal. Placement matters: decoder/input threading, output codec threading and filter threading are different (`-threads`, `-filter_threads`, `-filter_complex_threads`). Benchmark codec/worker matrices and preserve quality. `spawn_blocking` has a default bounded thread count but an unbounded waiting queue; moving all hashing there without admission control relocates the bottleneck.

#### Performance impact

At 200 albums, CPU/decode/storage overhead can dominate after DB fixes. At 1M–10M total tracks, repeated full-content verification is an enormous cumulative I/O cost. Exact amplification depends on acquisition filesystem, codec, artwork, CUE, retry and interruption path.

#### Proposed remediation review

Safe candidates: remove adjacent duplicate verification when no mutation can occur; hash inline during required copies; cache tool version by canonical binary identity/version; deduplicate identical embedded art before output creation; use one bounded blocking reader/task design; add independent CPU/FS admission. Unsafe candidates: replacing content hashes with metadata, blindly reusing inspection ffprobe facts without checking completeness, and universal single-threading.

#### Correctness and regression risks

- Source changes between checks, retries, lease loss, forged evidence and crash recovery.
- Different source/output codec facts required for validation; retained MP3 still needs structural verification.
- Parallel tracks changing resource use, temp storage and error ordering.
- Async process conversion changing cancellation/timeout and pipe backpressure.

#### Recommended remediation

First remove only proven adjacent duplicates and compute hashes during mandatory reads/copies where the same opened byte stream can be verified. Keep content checks at custody and failure boundaries. Add bounded dedicated CPU/FS admission. Measure FFmpeg threading and only then configure per-codec limits. Parallelize inspection and tracks with shared semaphores, not unbounded `spawn_blocking`.

#### Required regression tests

- Same/cross-filesystem; retained MP3/FLAC; embedded/external art; CUE.
- Concurrent source modifications during every read/copy/hash boundary; lease loss and cancellation.
- Interrupted acquisition/normalization/publication and exact retry/adoption hashes.
- Corrupt retained MP3 and output mismatch; temp-storage bounds and concurrent error behavior.

#### Required performance validation

Release-build 1/200 album FLAC and MP3 cases on local disk and NFS. Record source/output bytes read/written, process count, CPU, wall time, Tokio poll latency and peak temp/RSS. Acceptance based on relative reduction with identical verified outputs, not a fabricated universal 2.5× or 65% target.

#### Implementation readiness

**READY** for narrow duplicate elimination with integrity tests. **MEASUREMENT REQUIRED** for threading/parallelism. **DO NOT IMPLEMENT** metadata-only content equivalence.

### PERF-008 — Database pools and manifest transaction duration

**Original severity:** P1  
**Revised severity:** P1  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** DEFER

#### Original claim

One 32-connection pool and Tokio runtime serve all HTTP/workers, allowing exhaustion; manifest disk reads occur inside a locked transaction. Fixed pool splitting will guarantee responsiveness.

#### Independent evidence

- `crates/melodee-persistence/src/config.rs:27-109`: default max 32/min 2/acquire 30s/idle 600s, no explicit max lifetime.
- `apps/melodee-server/src/main.rs:221-415,651-655`: one default `#[tokio::main]` runtime and cloned pool across monitor, ingestion, catalog, policy, search, podcast, discovery, enrichment and all API surfaces.
- Worker concurrency is bounded and catalog/policy claim only `min(batch, concurrency)`; inspection is serial. Potential contention exists but not demonstrated exhaustion.
- `catalog.rs:756-834`: transaction locks the work item before `load_manifest_binding`; `3347-3385` performs `FOR SHARE` and evidence loading.
- `manifest_evidence.rs:633-660`: external evidence is read/verified with blocking asset I/O while the catalog transaction and locks remain open.

#### Validation analysis

The topology and transaction-duration defect are real. Pool exhaustion, CPU saturation, lock contention, storage saturation and Tokio starvation are different failure modes; the audit does not establish which occurs. A shared pool is not inherently defective. Splitting pools can reserve acquisition capacity but cannot guarantee CPU/lock/storage fairness and can idle connections in one class.

#### Performance impact

At 200 albums plus rebuild/enrichment/discovery, 32 acquisition slots can contend. At 10M, slow global queries and long snapshot transactions increase hold time. The actual risk must be measured with acquisition wait histograms and `pg_stat_activity`.

#### Proposed remediation review

Manifest preparation outside a transaction is directionally good but requires a complete revalidation design: work-item state, lease owner/expiry, manifest ID/schema/hash/reference, plan hash/payload, receipt Release ID/sealed hash, existing idempotent result and revision. Otherwise TOCTOU can import changed or stale evidence.

A fixed 16–20/12–16 split is not evidence-based. Max lifetime is operational hygiene, not a latency solution. First instrument acquisition and bound background DB admissions; then evaluate configurable reservation or process roles.

#### Correctness and regression risks

- Changed manifest/reference between unlock and relock.
- Lease expiry while reading; incorrect adoption of existing result.
- Separate pools increasing total connections beyond small PostgreSQL installations.
- Lease heartbeat starvation and shutdown/recovery differences.

#### Recommended remediation

Instrument pool acquisition first. Move evidence read/parse outside the transaction only with a two-phase binding snapshot and full fenced revalidation. Evaluate configurable worker admission and, if measurements justify it, pool reservation with an explicit total-connection budget and admin classification.

#### Required regression tests

- Manifest/reference/plan/receipt mutation between phases; lease loss/expiry; retry/adoption.
- Cancellation before/during read and write; stale workers fenced.
- Small max-connections configuration; heartbeat under load; graceful shutdown.
- HTTP, admin and stream authorization behavior under saturation.

#### Required performance validation

Concurrent 200-album import, search rebuild and 50 interactive VUs at 560K. Measure per-class acquisition wait, in-use connections, DB CPU/locks/storage, HTTP/stream p99 and errors. No unsupported “p99 <5 ms” guarantee.

#### Implementation readiness

**MEASUREMENT REQUIRED** for pool split. **DESIGN REQUIRED** for two-phase manifest loading.

### PERF-009 — Worker scheduling and retained result vectors

**Original severity:** P1  
**Revised severity:** P2  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Batch barriers idle workers and `Vec<ReconcileResult>` grows without bound for the process lifetime.

#### Independent evidence

- `normalize.rs:71-140`, `publication_worker.rs:77-140,325-390`: complete `JoinSet` drained before next claim.
- `inspect.rs:487-545`, `normalize.rs:513-580`, `publication_worker.rs:813-996`: each stage pushes one result per attempted pass, including empty passes, until shutdown.
- Reconciliation uses replacement (`results = recent`), not unbounded append.
- Results are returned only from `SchedulerHandle::stop` and discarded by `stop_worker` in `main.rs:57-60`; callers do not need full history.
- `ReconcileResult` is a 24-byte `String` plus 80-byte stats; with string allocation/vector overhead, roughly 104–120 bytes per retained item.

At default 30-second idle cadence: 2,880 items/day per enabled stage; four stages produce about 11,520/day. At 104–120 bytes, roughly **1.2–1.4 MB/day**, **36–42 MB/30 days**, **0.44–0.50 GB/year**, not an immediate multi-GB leak. Actual cadence under full batches is higher.

#### Validation analysis

The retained collection is reachable and unbounded, but it is not an unreachable-memory leak. P1 overstates the near-term impact. Batch head-of-line blocking can waste up to half of two slots in a constructed two-item case, not a general 50% measured loss.

#### Performance impact

Long-running enabled servers accumulate memory linearly. At 10M tracks the scheduling barrier is much more important than the small daily vector growth. Idle and busy cadence must be distinguished.

#### Proposed remediation review

Retain only the latest result or a bounded ring. A sliding-window scheduler is appropriate only if it claims work that can begin immediately, preserves weighted 3:1 lanes, lease duration/heartbeat/fencing, retry backoff, cancellation, fair stage scheduling and no hot empty-claim polling.

#### Correctness and regression risks

- Claims expiring while queued behind resource limits.
- Busy-loop DB polling and starvation of lower-priority work.
- Changed shutdown result semantics; tests may expect accumulated results even though production discards them.
- Intra-album parallelism overloading CPU/FS.

#### Recommended remediation

Use latest/bounded result storage now. Refactor one stage at a time to rolling refill with explicit admission and idle backoff. Preserve claim-at-start and heartbeat semantics.

#### Required regression tests

- 10,000 idle ticks with bounded retained memory.
- Mixed long/short releases; full/partial queue; ordinary/bulk 3:1 fairness.
- Lease renewal/expiry, cancellation/shutdown, retry backoff and empty-claim sleep.

#### Required performance validation

Measure slot utilization, albums/min, claim polls/s and RSS over 24h/30d soak. Acceptance: bounded retained result memory and improved mixed-size utilization without more DB polling or lease failures.

#### Implementation readiness

**READY** for bounded results. **DESIGN REQUIRED** for rolling scheduling.

### PERF-010 — Interactive API queries

**Original severity:** P1  
**Revised severity:** P1  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Admin stats, OpenSubsonic artist indexes, native browse/suggest/ACL and admin artist search have major unindexed work and need new indexes/caching/concurrency.

#### Independent evidence

- `apps/melodee-server/src/admin_stats.rs:167-177,208-327,330-484`: serial library/listening/database queries, aggregate scans and largest-release join.
- `opensubsonic/browse.rs:269-301`: all artist rows, correlated `AVG(rating)>0` excluding current user, `COLLATE "C"` ordering; typed row conversion precedes generic `Value` construction.
- Native ordering at `native/catalog.rs:669-677`; suggestions at `2123-2204`; ACL checks and counts at `1628-1750`; `has_library_bindings` at `1100-1106`.
- Admin artist filter at `catalog_browse.rs:583-589`.
- Existing rating index at `0011:25` and full admin release-date index at `0029:33-35`; normalized trigram indexes at 0039.

Runtime findings:

- OpenSubsonic's correlated rating aggregate used the existing `native_artist_user_states_lists_idx` via 35,000 index searches, not a sequential rating-table scan. A leading partial rating index is more appropriate but not proof of an “unindexed full scan.”
- Descending native release-date used `releases_admin_browse_date_idx` plus incremental sort (0.084 ms in this run), so that subclaim is already mitigated.
- Two-character substring examples used sequential scans; three-character track search used the normalized trigram index. Broad matches can still be expensive after bitmap retrieval.
- Admin OR/EXISTS used ordered index scanning with filtering and early limit, not a full sort.

#### Validation analysis

Heavy admin statistics and all-artist response construction are genuine scale concerns. Suggested indexes need exact predicate, sort, collation and partial predicate review. `WHERE rating > 0` matches the AVG predicate, but preaggregation must preserve exclusion of the current user.

`release_catalog_aggregates` is not automatically equivalent to the current largest-release query: it sums distinct assets across `track_media_assets`, whereas the current query sums playable assets per track; alternate assets and post-import asset edits can differ.

Authorization caching is security-sensitive. A global `has_library_bindings` boolean does not cache per-user access by itself, but a TTL delays first-binding activation and removal. Set-based ACL predicates are safer than broad caching. Running independent heavy queries concurrently may reduce one request's latency while increasing database contention.

#### Performance impact

At 560K tracks and 35K artists, all-artist payload/aggregation and admin scans can be significant; at 1M–10M tracks and hundreds of thousands of artists they grow linearly. The actual p95/p99 and benefit of each change require route-specific measurements.

#### Proposed remediation review

Accept selective query/index work after plans. Use typed serialization where wire bytes remain identical. Consider release aggregate only after semantic reconciliation. Do not add a second native date index without proving benefit over 0029. Do not cache authorization state without revocation/invalidations and fail-closed behavior. Benchmark `try_join!` before using it.

#### Correctness and regression risks

- User isolation, permission changes, library revocation and stale ACL caches.
- Rating averages changing due current-user exclusion.
- Largest-release byte counts changing due alternate assets/stale aggregates.
- Compatibility ordering/collation, deep `OFFSET` behavior and response shapes.

#### Recommended remediation

Baseline the actual endpoints. Add the partial artist rating index if selected by real plans; preserve average semantics in a prejoined aggregate. Fold compatible stats scans and use existing aggregate facts only when equivalent. Optimize ACLs set-wise. Treat caching and concurrency as measured second steps.

#### Required regression tests

- Golden JSON/XML for `getArtists`/`getIndexes`, rating exclusion, ordering and empty states.
- Native/OpenSubsonic/Admin API compatibility, two-/three-character searches and deep pages.
- ACL creation/removal/revocation and fail-closed DB errors; no cross-user cache entries.
- Largest releases with playable/alternate assets and asset byte-size edits.

#### Required performance validation

At 560K/5M/10M and 35K/100K+ artists, record query plans, p50/p95/p99, RSS and DB time under background load. Acceptance: equivalent wire bytes/ACLs and improved whole-system throughput, not just one endpoint.

#### Implementation readiness

**MEASUREMENT REQUIRED** for most changes. **READY** for a validated partial rating index and compatible stats reductions. **DO NOT IMPLEMENT** simplistic authorization caching.

### PERF-011 — Index redundancy, lease renewal and retention

**Original severity:** P2  
**Revised severity:** P2  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

About 12 indexes are duplicate/superseded, heartbeat updates are non-HOT, and permanent lease events/settling history are unbounded. Drop indexes, make a HOT-friendly renewal without events, and add retention.

#### Independent evidence

Exact structural duplicates:

- `tracks_native_random_candidate_idx (id)` duplicates `tracks_pkey (id)`.
- `artist_track_associations_native_track_idx (track_id,artist_id)` duplicates `artist_track_associations_track_idx`.
- `search_outbox_hourly_totals_age_idx` duplicates its PK `(projection_name,hour)`.

Overlaps are not automatically redundant:

- Native release GIN is partial on `title`; admin GIN is full and native now queries normalized title through 0039. Different predicate/use cases.
- Outbox pending indexes differ by `failed_at` predicate, affecting plans and size.
- Track audio identity indexes differ in second column (`id` vs `release_id`).
- Claim indexes differ by state, lease predicate, retry/order and current ingest expression queue; production recovery and diagnostics can use older paths.
- `idx_scan=0` is not sufficient evidence after stats resets or rare recovery/admin paths.

`renew_lease` at `work_items.rs:2787-2825` updates `updated_at` and `lease_expires_at`, then appends `LeaseRenewed`. `release_work_items_lease_expiry_idx` directly indexes `lease_expires_at`; `ingest_priority_queue` and older claim indexes include `updated_at`. Therefore the proposed “HOT-friendly” claim is false even if `updated_at` is preserved.

ADR-0158 deliberately keeps per-item lease outcomes in `release_work_item_events`, while tick rollups live in `job_run_history`. Existing tests assert lease-renewed events. Retention requirements are currently unimplemented for these histories.

#### Validation analysis

Three exact duplicates and unbounded history are confirmed; the broader 12-index list requires plan/constraint equivalence. Heartbeat write amplification exists, but HOT is impossible while `lease_expires_at` remains indexed. Removing events reduces auditability and can break tests/diagnostics.

#### Performance impact

Index write/WAL cost grows with tracks and imports. Heartbeats every up-to-30s per active item generate events and index maintenance. At 10M, rare-index storage can be significant, but 1–1.5 GB savings is unsupported without actual index sizes.

#### Proposed remediation review

Drop only the three exact duplicates after `pg_get_indexdef`, FK/constraint and representative plan checks. Use concurrent drops only in an explicit out-of-transaction operator phase. Do not drop based solely on zero scans.

A single fenced `UPDATE ... WHERE id/lease_owner/lease_expires_at` reduces round trips but cannot be advertised HOT. Keeping a separate queue-age timestamp would be needed to stop heartbeats changing `updated_at`; changing its meaning silently would distort queues. Prefer bounded retention for high-frequency lease events over deleting current behavior without an ADR/product decision.

#### Correctness and regression risks

- Loss of recovery/admin access paths and planner changes.
- Stale/invalid indexes after failed concurrent DDL.
- Queue-age semantics, operator history and diagnostics regression.
- Retention deleting legally/product-required evidence or violating chart/history semantics.

#### Recommended remediation

Create a forward-only, measured duplicate-index package for the three exact duplicates. Keep lease event semantics until a documented retention/observability decision. Optimize the fenced UPDATE separately, with truthful non-HOT expectations. Design bounded per-class history retention and backfill aggregates before deletion.

#### Required regression tests

- `pg_get_indexdef` equivalence and representative `EXPLAIN` for all affected claims/recovery/admin paths.
- Lease owner/expiry fencing, concurrent renewals, queue age/order and event assertions.
- Retention idempotency, crash recovery, boundary ages and aggregate equivalence.

#### Required performance validation

Measure actual index sizes, WAL, `n_tup_hot_upd`, write latency and plan time before/after. Acceptance: no required plan regression and equivalent audit/recovery behavior; no assumed storage total.

#### Implementation readiness

**READY** for the three exact duplicates after operational checks. **DESIGN REQUIRED** for retention and heartbeat changes. **DO NOT IMPLEMENT** blanket drops or the HOT claim.

### PERF-012 — Discovery cursor starvation

**Original severity:** P2  
**Revised severity:** P1  
**Independent verdict:** CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Discovery resets its cursor to nil each pass, so with 100 batches it processes only the first 48,500 tracks and never reaches releases/artists/chart relinking.

#### Independent evidence

- `crates/melodee-persistence/src/discovery.rs:190-245`: three missing-candidate passes then three popularity passes then chart relinking, all sharing `stats.batches`.
- `discovery.rs:316-375`: each `correct_candidate_popularity` call initializes `cursor = Uuid::nil()` and walks forward.
- Default `batch_size=500`, `max_batches=100`, interval 300s in `apps/melodee-server/src/discovery_worker.rs:29-31`.
- Three initial missing-candidate batches plus 97 track batches = 48,500 tracks, leaving no budget for the other entity types or charts.
- The stats comment says the next pass “resumes the same work,” but no durable cursor exists: implementation contradicts its own stated convergence contract.

#### Validation analysis

The starvation mechanism is exact under default configuration when there are enough candidates. It is a functional eventual-coverage defect, not just a performance issue, hence P1. Trigger-maintained play counts reduce the impact for current plays but do not fix backstop correction, drift recovery or chart relinking.

#### Performance impact

At 560K and 10M, the same prefix is repeatedly scanned while most entities never receive backstop correction. This wastes DB work every five minutes and leaves functionality incomplete.

#### Proposed remediation review

A durable per-entity keyset cursor and independent chart budget are directionally correct. Running missing-candidate insertion only on wraparound is not enough by itself: triggers cover normal inserts, but backup restore/binary downgrade can create missing rows, so periodic full anti-join passes remain a useful backstop.

#### Correctness and regression risks

- New rows behind the cursor must be visited on the next wrap.
- Deleted cursor IDs must not stall the keyset.
- Concurrent passes must not regress cursors or duplicate work.
- Per-type budgets must prevent one large type from starving others and guarantee wraparound.
- Cursor persistence and crash recovery must preserve idempotency.

#### Recommended remediation

Persist `(entity_type,last_entity_id)` with wrap completion, separate per-type and chart budgets, and deterministic full-pass scheduling. Keep periodic missing-candidate backfill on wrap/startup. Bound concurrent passes and advance cursors transactionally with batch work where practical.

#### Required regression tests

- >100K tracks plus releases/artists/charts proving every type is eventually visited.
- New inserts behind/ahead of cursor, deleted cursor entity, wraparound and restart.
- Concurrent passes, empty candidates, partial failure and chart-only backlog.

#### Required performance validation

Run 100K+ synthetic candidates and record per-type visitation, batches, wall time and repeated prefix work. Acceptance: eventual full coverage and bounded per-pass work with no permanent truncation.

#### Implementation readiness

**READY** with the cursor/fairness tests above.

### PERF-013 — Streaming, authentication and caching

**Original severity:** P2  
**Revised severity:** P2  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** REJECT security-sensitive pieces; ACCEPT WITH CHANGES elsewhere

#### Original claim

Per-chunk blocking tasks, sync open/seek, repeated OpenSubsonic auth, blacklist stampedes, singleton rate limiting and artwork hashing are inefficient. Replace them with async files, password cache, singleflight, per-key upsert and shared artwork cache.

#### Independent evidence

- `native_stream.rs:700-725,775-810,875-920`: no-follow `openat` and seek on async thread; each 64 KiB chunk uses `spawn_blocking` and a fresh vector. Range and permit lifetimes are explicit.
- `opensubsonic/auth.rs:205-267`: user lookup each request; token+salt adds secret query; `u+p` verifies bcrypt through bounded 4-active/64-admitted executor.
- `security.rs:183-260`: no singleflight around blacklist refresh; expired-policy refresh retains old policy on failure.
- `auth.rs:684-755`: singleton `FOR UPDATE`, expiry delete, counter update, retained count.
- `opensubsonic/media.rs:640-725`: SHA-256 of bytes and a 256-entry FIFO; asset keys already exist upstream but are not carried into the cache key.
- Tokio `fs` itself uses blocking operations; changing API alone does not remove filesystem scheduling costs.

#### Validation analysis

The structural observations are correct, but their significance is workload-dependent. A 10 MiB stream has about 160 chunk tasks, but no latency or CPU measurement is provided. Bcrypt cost is intentionally bounded; client tile bursts can amplify it.

Password verification caching is unsafe as proposed: it can retain credential-equivalent HMACs, complicate timing/rate-limit behavior, and delay disable/password revocation. A cache keyed by current password hash and enabled/locked state still needs authoritative revalidation.

The singleton replacement is not semantics-preserving: atomic per-key increment does not enforce the global retained-key ceiling, expiry cleanup or exact multi-process admission by itself.

#### Performance impact

At 32 streams and 20-tile artwork bursts, task scheduling, bcrypt and DB lookups can contend. At 10M tracks these are mostly per-request rather than catalog-size costs; actual throughput requires measurements.

#### Proposed remediation review

Use one bounded blocking reader loop per stream with a reused per-stream buffer, preserving range, no-follow path, cancellation and backpressure. This reduces task count rather than pretending I/O becomes nonblocking.

Combining user and secret lookup is safe if result semantics remain indistinguishable. Reject the password cache as written. Singleflight blacklist refresh is reasonable and retains fail-closed previous policy. Rate limiting needs a new exact multi-process design, not a simplistic upsert. Shared artwork cache should be keyed by `(asset_key,size,format,no_upscale)` and byte-bounded.

#### Correctness and regression risks

- Password changes/disabling/revocation, secret memory retention, timing and rate-limit bypass.
- Rate-limit atomicity and retained-key cap across processes.
- Range correctness, file descriptor/permit lifetime, client disconnect and symlink/path protections.
- Artwork cache invalidation and unbounded bytes.

#### Recommended remediation

Implement one blocking reader task per stream and benchmark it. Combine auth queries only. Add blacklist singleflight. Reject password cache and naive rate-limit rewrite. Implement byte-bounded asset-key artwork caching after confirming key lifecycle and format compatibility.

#### Required regression tests

- Full/HEAD/single and invalid ranges, early EOF, disconnect/cancel, path/symlink attacks, stream/per-user caps.
- Password change/disable, token and `u+p`, failed attempts/concurrency and rate-limit exactness across processes.
- Blacklist refresh failure and revocation; artwork bytes, cache bounds and key changes.

#### Required performance validation

32 direct streams plus 20-tile artwork bursts and 50-VU auth/rate-limit tests. Measure blocking queue depth, CPU, DB queries, p95/p99, underruns and memory. Acceptance includes identical security/range behavior and atomic limits.

#### Implementation readiness

**READY** for singleflight, combined auth lookup and bounded stream reader with tests. **DESIGN REQUIRED** for rate limiting. **DO NOT IMPLEMENT** proposed password cache.

### PERF-014 — Catalog writes and recommendation updates

**Original severity:** P2  
**Revised severity:** P2  
**Independent verdict:** PARTIALLY CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES / DEFER recommendation async

#### Original claim

Import uses per-item SQL loops and variable SQL causing statement-cache churn; recommendation triggers lock hot rows. Use `UNNEST`, batch locks, and async recommendation updates.

#### Independent evidence

- `catalog.rs:4345-4420`: per-track acoustic/embedded evidence; `3838-3920`: external identity loops; `3575-3600`: advisory lock per name.
- Names are collected in a `BTreeSet`, so current advisory acquisition is already deterministic by name. The proposed `ORDER BY ord` preserves input order, not actual hash-key order.
- `QueryBuilder` shape varies with value count. SQLx 0.9's default per-connection statement cache is 100, but actual evictions/prepare overhead were not measured.
- `0019_phase7_native_route_bounds.sql:148-215`: play trigger locks track and adjusts track/release/primary-artist counters synchronously.

#### Validation analysis

The loops and trigger work are real. “30–60 extra round trips” is codec/tag dependent. Variable SQL alone does not prove constant eviction or a meaningful share of album time. `UNNEST` is a useful fixed-shape option, not an automatic optimization; arrays need consistent lengths/types, null and duplicate semantics, and constraint behavior.

Current lock order is deterministic, but a generic batched implementation should sort by the **computed 64-bit lock keys**, because lexical name order and hash order differ. Input `WITH ORDINALITY` alone does not guarantee deadlock-free ordering for arbitrary callers.

#### Performance impact

At ordinary 12-track albums, round trips matter; at large boxsets and high scrobble concurrency they become more important. No evidence supports a universal fixed statement target or need to make recommendations asynchronous.

#### Proposed remediation review

Batch only the proven loops with typed `UNNEST` or `QueryBuilder` where PostgreSQL parameter limits and JSON construction make sense. Preserve source ordering where required, duplicates and constraint errors. Keep advisory locks deterministic by actual key. Benchmark recommendation trigger contention before changing freshness.

#### Correctness and regression risks

- Array length/type mismatch, null/duplicate handling and changed constraint/idempotency behavior.
- Deadlocks from hash-key ordering or changed lock scope.
- Async recommendation updates creating stale rankings, lost plays after crashes, or changing recovery semantics.

#### Recommended remediation

Use fixed-shape batched inserts for acoustic/embedded evidence and external IDs after equivalence tests. Keep sorted transaction-scoped locks. Leave recommendation updates synchronous until measurements and an explicit freshness/recovery design justify otherwise.

#### Required regression tests

- Empty/multiple/duplicate/null values, >ordinary album sizes, constraint violations and retry idempotency.
- Multi-name opposing import lock order and no deadlock.
- Concurrent plays, relationship changes, rollback and recommendation count equivalence.

#### Required performance validation

Measure SQL statements, prepares, round trips and transaction time per 200 albums; concurrent scrobbles measuring lock waits/WAL. Acceptance: same rows/counts and materially less time without stale recommendation guarantees beyond the documented contract.

#### Implementation readiness

**READY** for selected evidence batching. **MEASUREMENT REQUIRED** for statement-cache claims and asynchronous recommendations.

### PERF-015 — Observability and benchmark validity

**Original severity:** P2  
**Revised severity:** P2  
**Independent verdict:** CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Original claim

Metrics lack latency distributions/pool waits/throughput, and benchmarks bypass production bottlenecks.

#### Independent evidence

- `diagnostics.rs:39-235`: 10-second slow-request threshold and per-request debug/warn duration logs, no histogram.
- `resource_bounds.rs`: in-flight/waiting/rejected/admitted gauges with oldest wait, useful but not duration distributions.
- `admin_system.rs:216-296`: pool size and a representative `SELECT 1` timing, not acquisition-wait timing.
- `melodee-media/src/lib.rs:45-89`: `ToolInvocation` already records per-process duration/outcome at debug level, contrary to a claim of no subprocess timing at all.
- `phase6_benchmark.rs:17,74-96`: 100K commit interval, direct documents, zero PostgreSQL queries and one open store.
- `perf_catalog_queries.rs`: direct SQL seed and admin browse only; no catalog triggers/policy/media/search hydration/concurrent workload.
- `melodee-ingestion/tests/performance.rs`: 32/64-byte files and metadata settling only.
- `native_phase7_performance.rs`: real native routes but fixture-driven, no media I/O and synthetic authentication/signing scope.

#### Validation analysis

The central observability/benchmark-representativeness finding is correct, with the subprocess-timing qualification. Structured spans, job history, resource gauges and debug durations are useful foundations; percentiles, pool acquisition and stage rates are missing. A 1-second slow threshold may be a useful default but should be configurable rather than universally mandated.

#### Performance impact

Without these measurements, regressions at 560K–10M can remain invisible and small synthetic tests can create false confidence. This is an engineering-control risk, not a direct runtime bottleneck.

#### Proposed remediation review

Small bounded in-process histograms are appropriate; avoid unbounded route/entity label cardinality and an unnecessary external stack. SQLx acquisition timing needs to surround actual acquisition/checkout boundaries, not merely SQL execution. Benchmarks must drive production entry points and trigger paths.

#### Correctness and regression risks

- Metrics overhead/cardinality and sensitive path/query leakage.
- Benchmark fixtures bypassing leases, triggers and recovery while being mislabeled as production evidence.
- Absolute debug or hardware-specific targets becoming inappropriate gates.

#### Recommended remediation

Add fixed-bucket histograms for route class, pool acquisition, stage duration/rates, subprocess and search open/query/commit. Keep labels bounded and IDs out of values. Add production-path release benchmarks with documented fixture limitations and cold/warm runs.

#### Required regression tests

- Histogram bucket/overflow and bounded labels; no credential/path leakage.
- Pool wait includes queue time; gauges drop on cancellation.
- Bench correctness/count assertions, actual trigger/policy participation and recovery.

#### Required performance validation

Run release baselines at representative scales and compare before/after medians and distributions. Acceptance: metrics overhead is bounded and benchmark reports explicitly enumerate bypassed subsystems.

#### Implementation readiness

**READY**, with bounded cardinality and release-build requirements.

## E. Quantitative claims validation

| Original Claim | Evidence Type | Assumptions | Independent Assessment | Corrected Estimate |
|---|---|---|---|---|
| 1.4B track-row examinations per 2,500 assets at 560K | Mathematically derived | Every media insert sequentially scans all tracks | Arithmetic correct; not measured I/O | Same logical upper bound; actual plan depends on indexes/concurrency |
| 25B at 10M | Mathematically derived | Same | Correct upper bound | Same; distinguish buffer visits from storage reads |
| 375–625 GB track pages per burst | Conditional estimate | 150–250 MB heap and 2,500 full scans | Plausible range, not measured physical I/O | One 600K probe read/hit 18,567 buffers (~145 MiB logical); scale only by measured heap |
| 65–80 MB fingerprint heap/task at 560K | Conditional estimate | 64-char strings plus Rust/tree/clone overhead | Plausible but unverified | 35.84 MB payload minimum; total depends on rows/clones/allocator |
| >1.3 GB/task at 10M | Conditional estimate | Same overhead amplified | Plausible upper scenario, not measured | 640 MB payload + headers/tree/rows/clones; measure RSS |
| 5.6B dry-run array elements | Mathematically derived | 10K releases × 560K distinct per release | Correct worst-shape arithmetic | Scope query; never materialize this shape |
| 86,400 maintenance queries/day | Mathematically derived | Default 1-second loop | Frequency correct for status call; not complete SQL count | One status SQL + blocking-rebuild query + settings work per poll |
| 13.5K–16K subprocesses per 200 albums | Conditional estimate | Artwork per track/album assumed | Not a representative established count | 10,800 without artwork; 13,300 with one embedded artwork/track |
| 6–12 full-album reads | Conditional estimate | Acquisition/codec/art/retry paths combined inconsistently | Not reliable as a universal count | Ordinary same-FS path can have ~13 source/tree/audio passes plus output; cross-FS adds five |
| 10–50× catalog import improvement | Unsupported | No before/after production benchmark | Unsupported | Measure; only analytical work reduction can be bounded |
| ~65% subprocess reduction | Unsupported | Codec/artwork/version reuse assumptions | Unsupported | Compute per workload; base process count above |
| Search p95 <50 ms | Product target from ADR/PRD | Warm suitable hardware and corpus | Not measured by audit | Keep as target requiring 10M release evidence |
| >1,500 documents/s incremental indexing | Proposed target | Batched hydration/writer changes | Unverified | Establish baseline before accepting |
| Rebuild <2 min (560K), <30 min (10M) | Proposed target | Bulk optimizations and hardware | Unverified | Measure; do not gate on these initially |
| Multi-hour snapshot | Plausible but unverified | Slow rebuild at large scale | No measured duration | Record actual oldest-xmin age and vacuum effects |
| 28B release genre checks at baseline | Conditional estimate | Assumes per-release full scan and only 560K terms | Formula and plan premise unreliable | Terms may be ~1.12M; PG18 used skip-scan, not full scan |
| 1,000 global reconciliation queries per 200 albums | Mathematically derived | Every import returns `Ok` | Confirmed by caller path | Exact invocation count, not exact rows/time |
| 256 entities/s outbox ceiling | Analytical upper bound | 256 rows + 1s sleep, zero work | Upper bound, not measured rate | Actual rate ≤ configured bound and hardware/work dependent |
| 50% worker slot waste | Conditional estimate | Two tasks, one long/one short, backlog | Possible worst case, not measured | Measure mixed-size slot utilization |
| 1–1.5 GB saved by index drops | Unsupported | 10M index sizes unknown | Unsupported | Query `pg_relation_size` per candidate |
| Rebuild 645 commits at 560K, 10,085 at 10M | Mathematically derived | 35K artists + 50K releases + track count, 1K pages | Approximately correct | Same derivation; actual page count varies with status filters |
| “Autovacuum blocked globally” | Imprecise terminology | Long-lived repeatable-read snapshot | Snapshot does not stop process; it delays dead-tuple reclamation | Describe xmin horizon and bloat precisely |

## F. Newly identified findings

### PERF-V001 — Rebuild activation can permanently miss a late lower-ID committed outbox event

**Severity:** P1  
**Verdict:** CONFIRMED  
**Confidence:** High, reproduced with the real search worker and disposable PostgreSQL  
**Remediation verdict:** DESIGN REQUIRED

#### Evidence and reproduction

`catalog_projection_outbox.id` is `GENERATED ALWAYS AS IDENTITY`. `claim_rebuild` records the current maximum at `search.rs:400-485`; catch-up loads `id > cursor AND id <= latest` at `887-915`; activation compares `caught_up_outbox_id` to current `max(id)` at `753-842`.

Controlled sequence using the actual `run_rebuild_once` and `apply_outbox_once` implementations:

1. An active generation existed. Transaction T1 changed Artist “Before” to “After” and inserted outbox ID 1 without committing.
2. A rebuild began. Thousands of later events (IDs 2–5001) committed. T1 remained invisible.
3. Catch-up advanced beyond ID 1.
4. T1 committed. The ordinary outbox worker consumed and acknowledged 256 events, including ID 1, against the old active generation.
5. The rebuilt generation activated with `caught_up_outbox_id = 5001`. Its lookup returned “Before,” while authoritative PostgreSQL revision was 2/“After.” The event was processed and therefore could not repair the new generation.

A simpler run without old-generation consumption initially activated stale and the next ordinary outbox pass repaired it. This distinguishes a **permanent-loss sequence** from a transient catch-up delay. It also explains why existing green rebuild tests are insufficient.

#### Impact

At any scale, a committed catalog mutation can be absent indefinitely from the active projection until another mutation/rebuild. This violates ADR-0121's catch-up/activation guarantee. It is not authoritative data loss, but it is a search correctness defect. The same algorithmic risk applies to upserts/deletes and to any proposed short read-committed rebuild.

#### Recommended remediation

Introduce a commit-safe catch-up mechanism and generation activation invariant, with an accepted ADR. Possible directions include a commit-order log/sequence allocated at commit, a snapshot exported/shared with catch-up, or a final authoritative revision reconciliation that detects and repairs all differences before activation. Simply checking pending rows or `max(id)` is insufficient. Until designed, preserve the consistent snapshot and consider blocking activation when unresolved early-ID writers exist only as an operational mitigation, not a proof.

#### Required tests and acceptance

Automate the exact two-writer/old-consumer sequence, plus late deletes, new entities, retries/crashes and pruned history. Acceptance: after activation, every entity equals authoritative PostgreSQL and no consumed event can be omitted. Run this before and after every rebuild performance change.

### PERF-V002 — Release merge/split performs per-track and per-observation SQL loops

**Severity:** P2  
**Verdict:** CONFIRMED  
**Confidence:** High  
**Remediation verdict:** ACCEPT WITH CHANGES

#### Evidence

- `crates/melodee-persistence/src/catalog.rs:1990-2110` (`split_release_merge`): per-observation updates and one `UPDATE tracks` per moved track.
- `catalog.rs:2340-2450` (`merge_release` core): another per-track update and per-observation loop.
- `catalog.rs:2392` and `2051`: each update also touches revision/outbox-relevant catalog state; surrounding operations retain locks and a large transaction.
- `0019` recommendation triggers can add per-row play-count queries and aggregate adjustments when relationships change.

#### Impact and recommendation

Large releases/boxsets create N+1 round trips, long write transactions, WAL and recommendation locks. This is distinct from PERF-014's import loops. Replace with set-based ordinal mapping (for example `unnest` with ordinality) while preserving immutable merge maps, observation order, reversible split data, aggregates and outbox events. Benchmark 1K/10K-track merges and require byte-for-byte merge/split evidence equivalence and identical final catalog/recommendation counts.

## G. Rejected or unsafe recommendations

| Finding | Proposed recommendation | Reason for rejection | Potential consequences | Safer alternative / validation |
|---|---|---|---|---|
| PERF-001 | Remove media INSERT trigger and require media-first everywhere | Existing and future writers use both orders; track trigger does not populate before media exists | Missing/stale native filenames | Keep triggers; narrow index first; validate every writer before separate redesign |
| PERF-001 | `INCLUDE (release_id)` by default | Trigger needs only `id`; no demonstrated index-only consumer | Extra storage/write cost | Narrow index; add cover only after plan proof |
| PERF-003 | `AFTER UPDATE OF status ... REFERENCING OLD/NEW TABLE` | PostgreSQL 18.3 rejects transition tables with column lists | Migration fails | Retain statement trigger and filter status changes in function |
| PERF-003 | Naive incremental genre deltas | Concurrent recount already loses data; distinct releases require membership logic | Incorrect counts/deadlocks/stale projection | Prove per-key serialization first; then membership-aware design if needed |
| PERF-004 | Only directly imported release identity scope | Transitive/historical effects not covered | Missed merges/proposals | Durable affected-identity closure plus global backstop and equivalence tests |
| PERF-006 | Read-committed pages + `max(id)` replay | ID allocation precedes commit; late lower IDs invisible and skippable | Permanent stale/deleted search documents (PERF-V001) | Keep snapshot; commit-safe design + ADR and concurrent tests |
| PERF-007 | Metadata instead of content verification | Metadata does not prove bytes and staging is not cryptographically immutable | Corrupt media accepted; provenance/recovery weakened | Reuse hashes only for the same verified byte stream; preserve boundary hashes |
| PERF-007 | Universal `-threads 1` | Codec/input/output/filter threading differs; throughput hardware dependent | Lower throughput or changed processing behavior | Benchmark matrix, explicit per-codec settings and admission bounds |
| PERF-007/006 | Move all work to `spawn_blocking` | Waiting queue is unbounded | Memory growth and resource oversubscription | Dedicated bounded semaphore/admission and cancellation |
| PERF-008 | Fixed pool split guarantees interactive latency | Shared CPU/locks/storage/WAL remain; sizing unmeasured | Idle pools, false SLO claims, small-install harm | Instrument first; configurable reservation/admission and full-system tests |
| PERF-010 | Cache library/ACL state simplistically | Revocation and first-binding changes become stale | Authorization bypass or denial | Set-based ACLs; if cached, versioned invalidation and fail-closed tests |
| PERF-010 | Use release aggregates for largest releases unconditionally | Current stats count playable assets; aggregate includes distinct linked assets and can differ | Incorrect admin statistics | Reconcile semantics and freshness before substitution |
| PERF-011 | Drop all listed “redundant” indexes | Different predicates, columns, collations, constraints and rare paths | Slower recovery/admin/query paths | Drop only exact equivalents after definitions/plans/production stats |
| PERF-011 | Call lease UPDATE HOT-friendly | `lease_expires_at` and `updated_at` are indexed | No promised HOT benefit | Measure WAL/HOT honestly; separate queue timestamp if redesigning |
| PERF-011 | Remove `LeaseRenewed` history by default | Per-item audit/operator visibility and tests rely on it | Lost diagnostics | Bounded documented retention, not silent removal |
| PERF-013 | Short-TTL keyed-HMAC password-success cache | Revocation, secret artifact, timing and rate-limit concerns | Credential/security regression | Keep bounded bcrypt; combine DB lookup only |
| PERF-013 | Naive per-key rate-limit upsert | Does not preserve global retained-key ceiling/expiry/admission | Rate-limit bypass/unbounded keys | Exact multi-process design and concurrency proof |
| PERF-013 | Assume `tokio::fs` removes blocking costs | Tokio filesystem also uses blocking pool | No fundamental improvement | One bounded blocking reader per stream with reusable buffer |
| PERF-014 | `ORDER BY ord` as universal advisory-lock ordering | Does not sort actual lock keys | Potential deadlock with other callers | Sort computed 64-bit keys deterministically |
| PERF-014 | Async recommendations without freshness contract | Crash/recovery/freshness semantics unexamined | Stale/lost popularity | Measure first; documented consistency and replay design |
| PERF-015 | Hardware-independent absolute benchmark gates | No representative baseline | False failures or false confidence | Relative before/after plus separately ratified SLOs |
| Migration strategy | Put `CREATE/DROP INDEX CONCURRENTLY` in normal SQLx migration | SQLx wraps ordinary migrations in a transaction; CIC forbids it | Failed migration and invalid index | ADR-0166 fenced ordinary build, or explicit resumable no-transaction phase |
| Migration strategy | Retry concurrent build without inspecting `pg_index.indisvalid` | Failed CIC leaves INVALID index | Wrong assumptions and continued failed upgrades | Inspect exact definition/validity; drop invalid index concurrently then rebuild |

### PostgreSQL migration safety review

The migration strategy is safe only if it distinguishes the repository's normal maintenance-fenced DDL from a deliberately engineered online phase:

- **SQLx 0.9 behavior:** ordinary migrations run each script and its bookkeeping in one transaction. `-- no-transaction` is recognized at the beginning of a script. `CREATE INDEX CONCURRENTLY` and `DROP INDEX CONCURRENTLY` cannot run inside that transaction, in a function, or in a DO block.
- **Fresh installations:** a normal forward `CREATE INDEX` works and is simplest. **Existing large installations:** the same operation permits reads but blocks writes for the build duration; ADR-0166 requires backup, all Melodee writers stopped, one migration invocation, and readiness/integrity verification before reopening traffic.
- **Online construction:** if availability requirements justify it, design a separate resumable operator phase with exact expected index definitions. SQLx no-transaction support alone is not a recovery protocol. Do not manually create migration-owned indexes under their final names and then pretend SQLx history is equivalent.
- **Failure recovery:** a failed concurrent build can leave an `INVALID` index. Inspect `pg_get_indexdef` and `pg_index.indisvalid`; drop that exact invalid index concurrently and rebuild. A retry must not assume the object is absent or valid. Concurrent drops have their own lock/wait behavior and cannot be forced safely by killing unrelated production sessions.
- **Rollback:** there is no automatic down migration. Forward-only fixes and verified restore procedures are required. Never edit a released migration; use a new migration and document the forward path.
- **Triggers/constraints:** replacement requires dependency ordering, preserved constraint validation and tests for in-flight writes. Backfill and projection rebuild requirements must be explicit; changing a trigger without repairing affected rows is not sufficient.
- **Operational proof:** rehearse both fresh and populated upgrades, record lock waits/build time/disk growth, prove application readiness and test recovery from failed concurrent DDL. Performance evidence from a fresh empty database does not establish upgrade safety.

## H. Reconciled performance remediation roadmap

### Phase 0 — Baseline and validation

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| Bounded metrics and production-path harness | 015, all | P0 | None | Low | M | Trustworthy before/after evidence | Release builds, documented hardware/data, route/stage/DB/RSS/I/O metrics |
| Representative isolated datasets | all | P0 | Storage budget | Low | M | Reproducible 560K/1M/5M/10M tests | Existing+missing values, skewed genres/ratings, real media corpus |
| Correctness race tests | 003, 006, V001 | P0 | Harness | Low | M | Prevent optimization regressions | Concurrent genre count and late-outbox activation tests fail before fixes |

### Phase 1 — Low-risk, high-impact corrections

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| Narrow playable-asset index | 001 | P0 | Migration plan | Low | S | Removes per-asset sequential scans | Nested trigger uses index; filenames unchanged |
| Scope acoustic novelty queries | 002 | P0 | Regression fixtures | Medium | S | Removes catalog-wide transfer/allocation | Equivalent decisions; empty inputs; bounded dry-run |
| Genre release index + status guard | 003 | P0 | None | Medium | S | Faster hydration, no overlay recount | Valid DDL; release index; no non-status recount |
| Genre concurrency correction | 003 | P0 | Race test/design | High | M | Correct counts | Concurrent imports produce exact counts/no deadlock |
| Durable discovery progress | 012 | P1 | Cursor schema/tests | Medium | M | Eventual full coverage | All entity types/charts visited across wraps/restarts |
| Bound scheduler results | 009 | P2 | None | Low | S | Bounded long-run RSS | 10K idle ticks, O(1) result retention |
| Three exact duplicate indexes | 011 | P2 | Production plan/size evidence | Medium | S | Lower write/storage overhead | Definitions/plans equivalent; safe forward migration |

These items can mostly proceed independently. The genre concurrency correction must not be bundled naively with asynchronous or incremental maintenance. Index migrations should not be run concurrently with trigger replacement in one unvalidated upgrade.

### Phase 2 — Incremental database processing

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| Affected-identity reconciliation closure | 004 | P0 | Durable seed/closure design | High | L | Album-proportional identity work | Global-oracle equivalence, retry/global backstop |
| Manifest two-phase binding | 008 | P1 | Revalidation design | High | M | Shorter locks/connections | TOCTOU and lease tests, identical imports |
| Ordered batched outbox hydration | 005 | P1 | Ordered revision mapping | High | M | Fewer DB round trips | Revision/delete/failure/commit-before-ack equivalence |
| Evidence/external-ID batch inserts | 014, V002 | P2 | Typed array semantics | Medium | M | Fewer import/merge round trips | Identical constraints, retry and merge evidence |
| History retention | 011 | P2 | Product retention/ADR | High | M | Bounded operational tables | Exact required aggregates/audit after pruning |
| Genre structural redesign (if justified) | 003 | P2 | Phase 1 measurements | High | L | Less popular-genre contention | Concurrent counts and full rebuild equivalence |

### Phase 3 — Media processing and worker efficiency

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| Remove adjacent duplicate hashes; hash during required copy | 007 | P1 | Integrity tests | High | M | Less source I/O | Identical verified evidence under modification/failure injection |
| Tool-version/artwork reuse | 007 | P1 | Binary/art identity checks | Medium | M | Fewer processes | Same outputs, fewer invocations |
| Bounded FS/CPU admission | 006, 007, 013 | P1 | Resource design | High | M | No async stalls/unbounded queue | Bounded queue/RSS, cancellation, throughput maintained |
| Rolling worker refill | 009 | P2 | Lease/fairness tests | High | M | Better slot utilization | No lease/poll/fairness regression |
| FFmpeg tuning and intra-album parallelism | 007 | P2 | Codec matrix | High | L | Better CPU utilization | Bitstream/quality/validation preserved and measured faster |
| One blocking reader per stream | 013 | P2 | Range/cancel tests | Medium | M | Fewer tasks/allocations | Identical ranges, no underruns/security regressions |

Do not concurrently change hashing semantics and parallelism without separate evidence; otherwise diagnosing integrity failures becomes difficult.

### Phase 4 — Search optimization

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| Fix late-ID activation race | V001, 006 | P0 | Commit-safe design/ADR | Critical | L | Correct activated projection | Late upsert/delete/consumer tests converge before activation |
| Empty-generation bulk inserts | 006 | P1 | No consistency change | Medium | M | Remove useless revision lookups | Counts/revisions identical, faster rebuild |
| Bounded blocking rebuild and fewer commits | 006 | P1 | Memory/admission limits | High | M | Shorter snapshot, fewer Tokio stalls | Same documents, bounded RSS/xmin age improved |
| Shared reader registry | 005 | P1 | Generation lifecycle design | High | L | Lower search tail latency | Remote/local activation, corrupt manifest and retirement tests |
| Long-lived writer | 005 | P1 | Lease/error/reopen design | High | L | Less writer/segment churn | Lock loss, replay, multi-process and crash tests |

The activation race fix is a prerequisite for any change to snapshot duration/consistency. Reader/writer lifecycle work should not be concurrent with a consistency-model rewrite.

### Phase 5 — Interactive responsiveness and resource isolation

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| Measured endpoint query/index fixes | 010 | P1 | Phase 0 plans | Medium | M | Lower DB time/RSS | Golden APIs and whole-system throughput improved |
| Set-based ACL predicates | 010 | P1 | Authorization tests | High | L | Less per-row PL/pgSQL work | Revocation/user isolation/fail-closed proof |
| Worker DB admission or configurable pool reservation | 008 | P1 | Pool measurements | High | M | Better interactive queueing | No total-connection regression; heartbeat/stream p99 improved |
| Blacklist singleflight and asset-key artwork cache | 013 | P2 | Cache lifecycle tests | Medium | M | Less duplicate work | Revocation/correct bytes/bounded memory |
| Exact rate-limit redesign | 013 | P2 | Distributed correctness design | High | L | Lower login contention without bypass | Exact counts/cap under multi-process concurrency |

### Phase 6 — Scalability validation

| Item | Related Findings | Priority | Dependencies | Risk | Effort | Expected Benefit | Acceptance Criteria |
|---|---|---|---|---|---|---|---|
| 560K/1M/5M/10M import and reimport | all | P1 | Phases 1–4 | Medium | L | Verify album-proportional work | No unbounded catalog-wide path; stable throughput |
| Concurrent search rebuild and writes | 005, 006, V001 | P0 | Activation fix | High | L | Search convergence and bounded snapshot | No missed mutations, bounded xmin age/RSS |
| Interactive + streaming during ingestion | 007–010, 013 | P1 | Phases 1–5 | Medium | L | Whole-system responsiveness | Measured p99/errors/underruns within ratified targets |
| Upgrade populated database | migrations/all | P1 | Forward migration package | High | L | Fresh and upgrade safety | No invalid indexes; integrity/readiness checks; recovery tested |
| Long-running soak | 009, 011, 015 | P2 | All critical fixes | Medium | L | Bounded memory/maintenance | 24h/30d trends, no starvation/unbounded retained collections |

## I. Performance benchmark and regression test plan

### Common protocol

1. Use only disposable databases/media roots. Verify `current_database()` has an owned random test name before any mutation. Never point the harness at the configured application database.
2. Build with `cargo build --release`; record commit, CPU, RAM, kernel, storage/NFS options, PostgreSQL 18 configuration, exact `MELODEE__*` values, codec/artwork mix and cold/warm state.
3. Seed production-shaped distributions, including missing and present values, duplicates, nulls and skewed genres. Do not rely on all-zero UUIDs.
4. Record p50/p95/p99, throughput, errors, CPU/RSS, read/write bytes, SQL count/time, pool acquisition, locks and WAL. Use `EXPLAIN (ANALYZE,BUFFERS)` only on disposable data and remember it executes the statement.
5. Functional correctness is a prerequisite. Relative before/after comparisons are the initial performance gate. Absolute targets below are **proposed**, not measured.

Example isolated administration (names are illustrative, not instructions to drop any non-owned database):

```bash
cargo build --release
export MELODEE_TEST_DATABASE_URL='postgres://test-admin@127.0.0.1:5433/postgres'
export MELODEE__STORAGE__ASSET_ROOT=/tmp/melodee-perf-assets
cargo test --release -p melodee-search --test rebuild_integration -- --nocapture
cargo test --release -p melodee-persistence --test catalog_concurrency -- --nocapture
```

Useful diagnostics:

```sql
SELECT pid,state,wait_event_type,wait_event,
       now()-xact_start AS xact_age,left(query,160) AS query
FROM pg_stat_activity WHERE datname=current_database()
ORDER BY xact_start NULLS LAST;

SELECT relname,idx_scan,seq_scan,seq_tup_read,n_live_tup,n_dead_tup,
       n_tup_upd,n_tup_hot_upd
FROM pg_stat_user_tables ORDER BY seq_tup_read DESC LIMIT 25;

SELECT indexrelname,pg_size_pretty(pg_relation_size(indexrelid)),idx_scan
FROM pg_stat_user_indexes
WHERE schemaname=current_schema() ORDER BY pg_relation_size(indexrelid) DESC;

SELECT calls,total_exec_time,mean_exec_time,rows,left(query,160)
FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 30;
```

### Scenario matrix

| Scenario | Workload and procedure | Functional acceptance | Performance regression / proposed targets |
|---|---|---|---|
| Single-album import | 12-track FLAC and retained MP3, with/without art, same/cross FS | Identical verified manifest/catalog/media; filenames and policy decisions correct | Baseline stage times, processes, source/output bytes, RSS; compare relative improvement |
| 200-album import | ~2,500 tracks at 560K existing tracks; mixed codecs | No lost/partial releases; exact aggregate/search state | Albums/min, DB time, I/O and p99; no unscoped acoustic/track scans |
| 1,000-album import | 12,500 tracks, sustained drain | Idempotent final state and bounded queues | No album-1-to-1000 scale cliff beyond hardware saturation; WAL/dead tuples bounded by policy |
| Unchanged reimport | 200 identical albums | Idempotent adoption, no duplicate rows | Avoid reconciliation/media rework; measure query count and RSS |
| Policy evaluation | New, duplicate, null and shared fingerprints | Normal policy decisions equivalent | SQL/transfer/RSS proportional to release digests |
| Policy dry-run | 10/1K/10K releases with bounds | Same decisions and no huge arrays | Bounded temp/RSS and wall time; no catalog-wide array per release |
| Concurrent same-genre imports | 4+ workers, Rock/Pop and multi-genre opposing orders | Exact song/release counts and no deadlock | Lock wait/throughput measured before/after |
| Artist reconciliation | Historical duplicates, external IDs, transitive cases | Same candidates/final identities as global oracle | Affected-closure size, DB time and queue depth bounded |
| Full search rebuild | 560K/1M/5M/10M; concurrent writes | PERF-V001 and all revisions converge before activation | Duration/docs/s/commits/RSS/xmin age; proposed 50 ms query target separately |
| Incremental indexing | 3K mixed outbox events incl deletes/poison | Ordered revision/failure/commit-before-ack semantics | Drain rate, SQL count, segments; proposed >1,500 docs/s only after baseline |
| Concurrent interactive | 50 VUs browse/search/getArtists/stats during import | Golden bytes and ACL correctness | p50/p95/p99/RSS/DB contention; proposed p95 <50 ms for suitable search routes only |
| Streaming during ingestion | 32 direct + 4 transcode + 200 albums | Range/security/cancel correct; no underruns | TTFB/p99, blocking queue, errors; proposed targets require ratification |
| Long-running workers | Full and idle batches for 24h+ | Leases/fairness/shutdown correct | Bounded results/RSS/poll rate; 30d projection separately |
| Discovery fairness | >100K tracks + other types + chart backlog | Every entity/chart eventually visited | No repeated prefix-only passes; bounded batches |
| Interrupted processing | Kill at acquisition/normalization/publication/catalog/rebuild boundaries | Recovery/adoption exact, no corrupt/partial authoritative state | Recovery time/repeated work; no integrity loss |
| Concurrent rebuild updates | Late lower-ID upsert/delete and old-generation consumption | No committed mutation omitted at activation | This is a hard gate, independent of speed |
| Large-database upgrade | Populated existing schema, forced concurrent-DDL failure/recovery | Exact schema/data/public IDs and valid indexes | Migration time/lock waits/availability recorded; rollback/forward procedure proven |

For every scenario, classify each number as measured, mathematically derived, conditional estimate or proposed target. A green synthetic benchmark must list bypassed triggers, policy, media, PostgreSQL hydration, concurrency and recovery just as explicitly as exercised paths.

## J. Proposed permanent performance engineering standards

### Recommended `AGENTS.md` addition (do not apply in this review)

Add a “Performance correctness and scale” section requiring:

- Import cost should be primarily proportional to the affected release and its identity closure, not total catalog size. Exceptions need documented global-maintenance justification.
- No per-track/per-release unbounded catalog operation without an accepted requirement and measured plan.
- Every retained queue/cache/buffer/collection has an explicit bound and idle behavior; distinguish retained growth from leaks.
- No substantial synchronous CPU, filesystem, subprocess or Tantivy work on Tokio async workers. Blocking work requires admission, queue and cancellation limits; `spawn_blocking` alone is not a complete resource design.
- Transactions must not include filesystem/external process work unless a correctness exception is documented. Revalidation is mandatory when moving reads outside a transaction.
- High-frequency SQL needs predicate-aligned indexes and actual production-shaped plans; `idx_scan=0` alone never justifies a drop.
- Background work must be incremental, bounded, resumable and starvation-free with eventual-coverage tests.
- Reuse verified work only when identity, ownership, mutation fencing and recovery are preserved; metadata is not content proof.
- Search changes must preserve revision/delete/generation/commit-before-ack semantics and pass late-committer/concurrent-consumer tests.
- Security, authorization, password hashing and content integrity must not be weakened for throughput.
- Benchmarks must identify bypassed production subsystems and run release builds at declared scales.
- Performance changes require before/after evidence and explicit data/migration/API/documentation impacts; architectural complexity requires evidence.
- Forward migrations must respect ADR-0166 and SQLx transaction restrictions; no editing released migrations.

### Proposed permanent `PERFORMANCE.md`

Include:

1. **Workload model:** representative 35K/50K/560K/5 TB and 1M/5M/10M/25M horizons; codec, artwork, NFS, ACL and concurrency distributions.
2. **Invariants:** PostgreSQL authority, durable staging, lease fencing, idempotency, content verification, search commit/revision/generation rules, authentication/authorization.
3. **Resource model:** pool, Tokio async/blocking, FS/CPU/media/Tantivy admissions, FFmpeg threading, temp/disk and memory limits.
4. **Scale-cost rules:** album/affected-closure proportionality, bounded background scans, periodic global backstops and no hidden N+1.
5. **Benchmark protocol:** disposable isolation, release build, hardware/config record, cold/warm, existing/missing values, relative gates and evidence classification.
6. **Required scenario suite:** all Section I scenarios, including failure injection, fairness, upgrades and concurrent rebuild mutations.
7. **Instrumentation:** bounded route/stage histograms, pool acquisition, SQL counts/plans, worker throughput, subprocess, search and RSS/I/O; no secrets/high-cardinality labels.
8. **Migration operations:** fresh/upgrade paths, maintenance fence, concurrent DDL recovery, invalid index cleanup and backup/forward plan.
9. **Evidence lifecycle:** measured vs derived vs estimate vs target; no unsupported multipliers or physical-I/O claims.
10. **Change authorization:** who approves SLOs, ADRs for consistency/storage/security changes, and mandatory regression suites.

Useful automated guards:

- Static SQL/index checks for known hot paths (not assumptions that every sequential scan is bad).
- Tests rejecting catalog-wide fingerprint result shapes and nil-reset discovery cursors.
- Concurrent genre count tests and late-outbox activation tests as mandatory gates.
- Long-idle scheduler memory test and worker fairness test.
- Benchmark artifact schema requiring commit, scale, release profile, environment, measured units and bypass list.
- Migration test that applies fresh and populated upgrades and validates every index with `pg_index.indisvalid`.

Principle qualification: “transactions must not encompass filesystem/external operations” needs a narrow exception for genuinely atomic local metadata operations where no safer protocol exists; the general rule is to minimize and explicitly justify such scopes. “Interactive requests must remain responsive” needs ratified workload-specific SLOs, not an absolute guarantee under arbitrary hardware saturation.

## K. Final architectural assessment

1. **Is the architecture appropriate for 560K tracks?** Yes, the underlying architecture is appropriate. Current query, projection, concurrency and worker implementation defects—not the modular monolith itself—make the present revision unreliable under concurrent ingestion at that scale.
2. **Appropriate for 10M after targeted remediation?** Plausibly yes, but unproven until scale acceptance passes. PostgreSQL and Tantivy can serve this size on appropriately sized storage/RAM; no evidence mandates distributed infrastructure.
3. **Implementation vs architectural defects:** Missing/mis-scoped queries and indexes, global per-release work, trigger concurrency, cursor reset, per-request handles, redundant I/O, unbounded retained vectors and watermark handling are implementation defects. The durable queue, authority boundaries and generation model are sound foundations.
4. **Additional infrastructure?** Not justified. Redis/Kafka/RabbitMQ/Elasticsearch/microservices would add complexity without addressing the proven root causes.
5. **Decisions to preserve:** PostgreSQL authority; content-addressed filesystem evidence; immutable manifests/public IDs; set-based atomic import; lease fencing; weighted lanes; no-clobber publication; Tantivy as rebuildable projection; commit-before-ack; generational activation; bounded retention; separate compatibility DTOs.
6. **Highest careless-remediation correctness risks:** search snapshot/catch-up (006/V001), genre deltas (003), reconciliation scoping (004), media integrity (007), authorization/rate limiting (010/013), manifest TOCTOU (008), trigger filename maintenance (001).
7. **First five changes:** (a) narrow playable-asset index; (b) scoped acoustic queries; (c) genre indexes/status guard plus concurrency correction; (d) durable discovery progress; (e) bounded scheduler results and outbox maintenance/drain. In parallel, design PERF-V001 because it blocks snapshot redesign.
8. **Explicitly do not implement as written:** max-ID read-committed rebuild; metadata-only integrity; invalid transition-table DDL; naive release-only reconciliation; universal password cache; naive rate-limit upsert; blanket index/heartbeat removal; universal `-threads 1`; unbounded `spawn_blocking` migration.
9. **Measure before broader refactoring:** pool acquisition/DB time/locks, per-stage wall time and resource saturation, search cold/warm p95/p99, outbox drain, rebuild snapshot age, exact I/O/process counts, endpoint plans and long-run memory.
10. **Remaining uncertainties:** actual production data distributions, NFS behavior, real codec/artwork mix, ACL prevalence, PostgreSQL configuration, 1M–10M plan stability, cold-cache tails, multi-process Tantivy behavior and the best commit-safe activation design.

## L. Final implementation authorization recommendation

### Approved for implementation

1. **Narrow `tracks(playable_media_asset_id)` index only (PERF-001).** Mandatory tests: both insertion orders, filename refresh/update/delete, legacy/merge/recovery/policy compatibility. Acceptance: indexed present/absent lookups and unchanged rows.
2. **Release-scoped acoustic novelty queries with empty handling (PERF-002).** Mandatory tests: empty/null/duplicate/shared fingerprints, normal/dry-run equivalence and externalized manifests. Acceptance: equivalent decisions and no catalog-wide result arrays.
3. **Genre leading release/surface index and valid status-change guard (PERF-003).** Mandatory tests: overlay-only update does no aggregate work; all actual status transitions preserve counts. Acceptance: correct counts and leading-index hydration. This approval excludes unproven delta maintenance.
4. **Durable per-entity discovery cursors with wraparound and separate chart budget (PERF-012).** Mandatory tests: >100K entities, insert/delete/restart/concurrency/wrap. Acceptance: every eligible entity/chart eventually visited.
5. **Bounded scheduler result retention and maintenance drain/gating (PERF-005/009).** Mandatory tests: 10K idle ticks, full/partial batches, shutdown and once/hour maintenance. Acceptance: bounded retained memory and no unnecessary fixed sleep after full batches.
6. **Three exact duplicate indexes only, after live plan/size checks (PERF-011).** Mandatory tests: exact `pg_get_indexdef`, constraint/FK review and representative plans. Acceptance: no required query/recovery regression and valid upgrade.
7. **Safe narrow instrumentation (PERF-015).** Mandatory tests: bounded labels, no sensitive data, cancellation and acquisition timing. Acceptance: production-path release benchmark reports with explicit bypasses.
8. **Small safe auth/cache changes (PERF-013):** combined user+secret query, blacklist singleflight, and byte-bounded asset-key artwork cache. Mandatory security/invalidation tests. This excludes password caching and rate-limit redesign.
9. **Selected typed batch inserts (PERF-014)** after null/duplicate/constraint/idempotency tests. This excludes async recommendations.

Every approved schema change needs its forward migration, fresh and populated-upgrade tests, integrity checks and ADR-0166-compliant operational plan.

### Requires further design or measurement

- Genre aggregate concurrency correction and any incremental/asynchronous replacement (PERF-003).
- Affected-identity reconciliation closure (PERF-004).
- Long-lived Tantivy readers/writers and ordered batched hydration (PERF-005).
- Rebuild bulk/commit/off-async optimizations and any snapshot alternative (PERF-006); PERF-V001 is a prerequisite.
- Media hash/copy reuse beyond adjacent provable duplicates, FFmpeg threading and parallel media work (PERF-007).
- Pool reservation/split and two-phase manifest loading (PERF-008).
- Rolling worker scheduling (PERF-009).
- Most interactive stats/ACL changes and any cache/concurrency changes (PERF-010).
- History retention and lease-renewal redesign (PERF-011).
- Exact multi-process rate-limit replacement and stream benchmark-led changes (PERF-013).
- Release merge/split set-based writes (PERF-V002).
- Search activation commit-order/catch-up redesign (PERF-V001).

### Rejected

- Read-committed rebuild using only max outbox ID and catch-up (PERF-006).
- Metadata-only staged-content verification (PERF-007).
- Invalid `UPDATE OF ...` transition-table trigger and naive genre deltas (PERF-003).
- Imported-release-only reconciliation without affected closure/backstop (PERF-004).
- Password-success cache and naive per-key rate-limit upsert (PERF-013).
- Blanket index drops and removal of lease history based on zero scans (PERF-011).
- Universal `-threads 1` and unbounded `spawn_blocking` conversions (PERF-006/007).
- Unconditional release aggregate substitution for current largest-release statistics (PERF-010).
- Imported trigger removal without all-writer proof (PERF-001).

Documentation impact:
- Updated — added `PERF-CONCERNS-VALIDATION.md` with independent findings, runtime evidence and a corrected remediation program. No runtime behavior or public user documentation was changed because no remediation was implemented.
