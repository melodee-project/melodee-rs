# Phase 6 Report — Search and MQL

**Status:** Complete — independently reviewed and accepted

**Evidence captured:** 2026-08-12T03:11:06Z

**Base commit:** `e9cf1b3c9ce81b0d28fbf17cf4f582dba4218422` plus the Phase 6 change set

**Authority:** PRD-SEARCH-001 through PRD-SEARCH-007 and the supporting requirements in the
[Phase 6 work package](../../agents/PHASE-6-WORK-PACKAGE.md), SPEC sections 16–18 and 22–23,
ADR-0105, ADR-0107, ADR-0121, ADR-0122, and the
[Definition of Done](../../agents/DEFINITION-OF-DONE.md).

## Final decision

Phase 6 implements a disposable Tantivy projection over the authoritative Phase 4 PostgreSQL
catalog, a pure bounded MQL parser/planner, leased incremental indexing, generational rebuild and
catch-up, atomic activation, corruption diagnostics/recovery, autocomplete, ID-backed Artist
lookup, server lifecycle/readiness, and operator CLI commands.

The implementation never opens source or canonical media while indexing or rebuilding. Search
loss or corruption does not change PostgreSQL or media. Readers validate the PostgreSQL active
pointer against the owned generation manifest, schema, high-water mark, and document counts before
opening the index. Query readers do not acquire the exclusive Tantivy writer lock.

The first independent review blocked on two P1 findings. Both findings and the broader follow-up
audit were remediated, the complete benchmark and repository gates passed on the frozen tree, and
an independent re-review returned **ACCEPT** with no remaining P0 or P1 findings. Phase 6 is
complete and Phase 7 may begin.

## Implemented architecture

- `melodee-mql` owns the span-aware lexer, recursive-descent parser, AST, field/operator validator,
  bounds, and framework-free `CatalogQueryPlan`. It has no SQLx, Tantivy, Axum, or filesystem
  dependency.
- `melodee-search` owns the exact Tantivy 0.26.1 schema (version 3), Unicode/ASCII-folding analyzers,
  deterministic queries and pagination, revision-fenced writes/tombstones, active-generation
  services, autocomplete/lookup, outbox worker, rebuild, filesystem ownership, and benchmark.
- Migration `0009_phase6_search.sql` and `melodee-persistence::SearchProjectionStore` own the global
  writer checkpoint, bounded SKIP-LOCKED outbox leases, configured retry escalation, per-row
  permanent failures, sealed snapshot plus current live counts, repeatable-read snapshots,
  generation/rebuild jobs, and the atomically fenced active pointer.
- The server starts one bounded search scheduler only when explicitly enabled, drains it on graceful
  shutdown, and makes readiness depend on a healthy active generation while `/healthz` remains
  process liveness.
- The CLI exposes `search status|rebuild|query|autocomplete|parse|explain`. Status includes backlog,
  leases, queue age source data, checkpoint, active generation, latest rebuild, and last permanent
  failure evidence.

## Acceptance matrix

| ID | Result | Executable evidence |
| --- | --- | --- |
| A1 same-name Artists remain distinct | Satisfied | `same_name_artists_remain_distinct` |
| A2 aliases/corrections searchable | Satisfied | `aliases_are_searchable`, `public_identity_correction_merges_aliases_into_real_search_workers`, and `search_artist_snapshot_collects_redirect_chains_for_merged_sources` |
| A3 role participation is ID-backed | Satisfied | `primary_featured_performer_participation`, `search_release_snapshot_splits_roles_for_primary_featured_performer_and_other`, and `postgres_snapshots_preserve_release_and_track_artist_roles_in_index` |
| A4 stale/unordered rows cannot regress | Satisfied | revision and pending-revision fencing tests |
| A5 tombstones affect only projection | Satisfied | delete and stale-resurrection tests |
| A6 bounded deterministic pagination | Satisfied | MQL bounds and deterministic pagination tests |
| A7 Unicode/diacritics/multilingual | Satisfied | Björk/Sigur Rós and Japanese token tests |
| A8 claim/expiry/fencing | Satisfied | `outbox_claim_expiry_retry_and_ack_are_exclusive_and_fenced` |
| A9 commit-before-ack replay | Satisfied | real catalog correction committed to Tantivy, DB ack omitted, exact replay skipped by revision |
| A10 retry/permanent failure | Satisfied | configured max-attempt escalation plus deterministic poison-row isolation/healthy-sibling acknowledgement |
| A11 snapshot plus concurrent writes | Satisfied | `repeatable_read_snapshot_excludes_concurrent_write_that_catchup_observes` and `outbox_insert_waits_and_is_catchup_after_activate_generation_share_lock` |
| A12 rebuild boundary recovery | Satisfied | cancellation lease release/direct retry, activation and retired-generation recovery |
| A13 corrupt/missing index | Satisfied | active `meta.json` corruption, failed diagnostic, PostgreSQL rebuild recovery |
| A14 zero media reads | Satisfied | published/cataloged fixture has nonexistent source/canonical paths before and after rebuild |
| A15 equivalent rebuilt results | Satisfied | exact document counts and query hits before/after corruption recovery |
| A16 filesystem path safety | Satisfied | `owned_generation_removal_rejects_symlinks_and_root_overlap`, `generation_removal_rejects_a_leaf_replaced_by_a_symlink_after_root_open`, `nonempty_unowned_root_is_rejected`, and `generation_identity_and_manifest_facts_are_fail_closed` |
| A17 one writer per generation | Satisfied | PostgreSQL global lease plus Tantivy second-writer rejection/read-only concurrency test |
| A18 bounds/backpressure/cancellation | Satisfied | `outbox_claim_expiry_retry_and_ack_are_exclusive_and_fenced`, `outbox_max_attempts_escalate_exactly_at_configured_limit`, `lease_theft_fences_commit_and_scheduler_converges_on_next_pass`, `scheduler_stop_interrupts_long_idle_poll`, and `outbox_insert_waits_and_is_catchup_after_activate_generation_share_lock` |
| A19 readiness/liveness/diagnostics | Satisfied | server healthy/missing/corrupt-generation readiness, live-count, and permanent failure diagnostic tests |
| A20 MQL syntax/precedence | Satisfied | 37 parser/planner tests |
| A21 exact errors and resource bounds | Satisfied | span/depth/token/clause/regex/date/timeout tests |
| A22 compatibility plus four rejected bugs | Satisfied | executable `compatibility/mql/phase6-corpus.json` tied to frozen legacy commit |
| A23 autocomplete/admin lookup | Satisfied | deterministic bounded autocomplete and active Artist lookup integration |
| A24 10M warm p95 below 50 ms | Satisfied | frozen schema-v3 10M run below |
| A25 complete gates | Satisfied | the complete repository gate passed twice, including once after the final traceability edits; the documentation gate and independent re-review also passed |

## MQL compatibility boundary

The committed compatibility corpus records the frozen legacy commit
`4028b58d...`, preserved implicit AND, field/range/comparison/boolean/quoted/regex syntax, and four
intentional corrections: no silent truncation, case-insensitive boolean keywords, consistent
default-field expansion, and regex over indexed Unicode-folded tokens rather than raw bytes. It is
parsed with unknown-field rejection and executed through the real parser/planner.

Current executable acceptance coverage is 37 MQL parser/planner tests, 24 search unit tests, 6
real rebuild integration tests, and 12 PostgreSQL search-phase tests.

No native Melodee API v1, OpenSubsonic, Jellyfin route, payload, identifier, authentication,
pagination, streaming, or error contract changes in Phase 6.

## Rebuild, failure, and data safety

Rebuild reads a repeatable PostgreSQL snapshot in keyset pages, records its outbox high-water mark,
commits the new generation outside the active path, validates counts/schema, catches up later outbox
rows, and changes the active pointer only in one fenced transaction. A short table-share lock closes
the catch-up/activation race. The prior generation is retired and removed only after ownership,
manifest, direct-child, and symlink checks.

An idle outbox poll releases the global writer lease immediately. Retryable database/index errors
retain durable backoff evidence and become operator-visible permanent failures at the configured
attempt limit. A document-level Tantivy error discards its uncommitted replacement, replays proven
siblings, acknowledges those siblings, and records only the poison row; it cannot strand the batch.
PostgreSQL acknowledgment occurs only after a durable Tantivy commit and advances current live
document counts atomically. Lease loss leaves replay as the recovery path, with per-document
revision fencing. Sealed snapshot counts remain unchanged for manifest validation.

Migration 0009 is additive. Binary downgrade retains its evidence tables and disposable index. A
schema rollback drains search workers, restores a verified pre-0009 PostgreSQL backup into a new
database, and rebuilds search after forward migration; it never modifies media. Exact commands are
in `design/runbooks/LOCAL-DEVELOPMENT.md`.

## Performance evidence

Command (the single streaming binary is both deterministic generator and runner):

```text
cargo run --release -p melodee-search --bin phase6_benchmark -- \
  --tracks <COUNT> --root /absolute/empty-or-owned/search/root
```

Environment: Rust/Cargo 1.97.1, 24 logical CPUs, Linux. The recorded harness
uses corpus identity `ordinal-v1`, 35,000 Artists, 50,000 Releases, 10,000,000 Tracks,
10,085,000 documents, commit interval 100,000, and records whole-process Linux `VmHWM`. Record
generation is a pure function of ordinal; that identity identifies the corpus and is not a
pseudo-random seed. Synthetic records stream directly to the disposable index, so PostgreSQL query
count/time, source reads, canonical reads, and committed generated bytes are all zero.

| Tracks | Documents | Build wall ms | Docs/s | Index bytes | Peak RSS KiB | Interactive warm p50/p95/p99 | Target |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| 10,000,000 | 10,085,000 | 110,667 | 91,128.88357453649 | 1,880,384,451 | 759,440 | 6.042969 / 26.507272 / 31.871234 ms | pass |

The interactive corpus is: selective Track token, genre+exact year, primary Artist, exact ID, and narrow
duration range. Result counts were 25/25/25/1/25, in ordered query order, and the search results
self-validated. The exact per-query warm p95 values were 6.706675 ms, 26.912343 ms, 1.898736 ms,
0.577815 ms, and 31.871234 ms. Broad stress queries remain explicitly outside the interactive SLO:
`genre:rock AND year>=2000` was 72.578955 ms p95 and `duration>=60000 AND duration<=70000` was
42.982031 ms p95.

No generated catalog dataset or Tantivy index is committed. A 10,000-Track benchmark smoke still
runs in the default Rust test gate to detect generator/count/result/media-read contract failures and
catastrophic interactive-latency regressions.

## Independent review remediation

The first independent review returned `BLOCK` on two P1 outbox liveness findings. Both are closed:

- retryable rows now have `MELODEE__SEARCH__MAX_OUTBOX_ATTEMPTS` (default 5, maximum 100); the
  terminal attempt atomically records `failed_at`, `failure_kind`, error evidence, and clears leases;
- document-level application failures are isolated from healthy siblings, and the writer is dropped
  before replay so a failed upsert cannot commit its scheduled deletion.

The same remediation pass aligned all ADR-declared identity/sort/numeric/date fast fields, changed
Release/Track dates to Tantivy date fields, separated immutable snapshot from live diagnostic counts,
added corrupt-generation HTTP readiness coverage, corrected MQL leading-operator/signed-number/span/
Unicode-regex/empty-regex cases, clarified the ordinal corpus marker, and removed the report's
whitespace defect. Executable regressions are in `rebuild_integration.rs`, `search_phase6.rs`,
`search_tests.rs`, `mql_tests.rs`, `benchmark_smoke.rs`, and `system_api.rs`.

## Commands and results

Focused MQL/search/persistence/runtime gates:

```text
cargo test -p melodee-mql --all-features
cargo test -p melodee-search --all-features --tests
cargo test -p melodee-persistence --all-features --test search_phase6
cargo test -p melodee-server --all-features --test system_api -- --test-threads=1
cargo test -p melodee-cli --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

All focused commands above passed before review and again after remediation. The fresh post-review
repository gate passed on the current tree:

```text
./scripts/check.sh
```

The gate validated 23 ADRs, 8 mockups, and 232 requirements; confirmed the generated requirement
index; passed 111 Python tests; passed formatting, warnings-denied workspace Clippy, and every
workspace all-feature Rust test and doc-test; and completed `cargo deny` with advisories, bans,
licenses, and sources all accepted. Its only messages were the documented benign duplicate-version
dependency and `license-not-encountered` warnings.

The final public-documentation gate also passed:

```text
cd docs && pnpm check && pnpm check:links && pnpm build
```

Astro reported 0 errors, 0 warnings, and 0 hints; the relative-link and build-time link validators
passed; and the static build produced 35 pages.

## Remaining risk and later work

- Broad, low-selectivity numeric/range scans are documented above and are not claimed to meet the
  interactive SLO.
- Production-scale PostgreSQL snapshot contention, index disk sizing on the real catalog, and
  non-Linux filesystem retirement behavior require operational observation.
- Phase 7 owns public search routes and compatibility DTOs and is now the active phase. Phase 10
  owns the graphical diagnostics and search-management UI.

Documentation impact:
- Updated — public search/MQL reference, installation configuration, system diagnostics,
  troubleshooting, `.env.example`, local migration/restore runbook, changelog, README/phase status,
  technology baseline, feature matrix, requirement traceability, ADR/evidence registries, and this
  final Phase 6 report.
