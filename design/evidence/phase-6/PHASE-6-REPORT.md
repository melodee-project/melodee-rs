# Phase 6 Report — Search and MQL

**Status:** Candidate — review remediation and local acceptance complete; independent re-review required

**Evidence captured:** 2026-08-11T23:21:57Z

**Base commit:** `e9cf1b3c9ce81b0d28fbf17cf4f582dba4218422` plus the Phase 6 change set

**Authority:** PRD-SEARCH-001 through PRD-SEARCH-007 and the supporting requirements in the
[Phase 6 work package](../../agents/PHASE-6-WORK-PACKAGE.md), SPEC sections 16–18 and 22–23,
ADR-0105, ADR-0107, ADR-0121, ADR-0122, and the
[Definition of Done](../../agents/DEFINITION-OF-DONE.md).

## Candidate decision

Phase 6 implements a disposable Tantivy projection over the authoritative Phase 4 PostgreSQL
catalog, a pure bounded MQL parser/planner, leased incremental indexing, generational rebuild and
catch-up, atomic activation, corruption diagnostics/recovery, autocomplete, ID-backed Artist
lookup, server lifecycle/readiness, and operator CLI commands.

The implementation never opens source or canonical media while indexing or rebuilding. Search
loss or corruption does not change PostgreSQL or media. Readers validate the PostgreSQL active
pointer against the owned generation manifest, schema, high-water mark, and document counts before
opening the index. Query readers do not acquire the exclusive Tantivy writer lock.

This report deliberately remains **Candidate**. The first independent review blocked on two P1
findings; their remediations now require independent confirmation before Phase 6 can be marked
complete or Phase 7 can be activated.

## Implemented architecture

- `melodee-mql` owns the span-aware lexer, recursive-descent parser, AST, field/operator validator,
  bounds, and framework-free `CatalogQueryPlan`. It has no SQLx, Tantivy, Axum, or filesystem
  dependency.
- `melodee-search` owns the exact Tantivy 0.26.1 schema (version 2), Unicode/ASCII-folding analyzers,
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
| A2 aliases/corrections searchable | Satisfied | alias test plus catalog-correction commit-before-ack integration path |
| A3 role participation is ID-backed | Satisfied | `primary_featured_performer_participation` and snapshot SQL |
| A4 stale/unordered rows cannot regress | Satisfied | revision and pending-revision fencing tests |
| A5 tombstones affect only projection | Satisfied | delete and stale-resurrection tests |
| A6 bounded deterministic pagination | Satisfied | MQL bounds and deterministic pagination tests |
| A7 Unicode/diacritics/multilingual | Satisfied | Björk/Sigur Rós and Japanese token tests |
| A8 claim/expiry/fencing | Satisfied | `outbox_claim_expiry_retry_and_ack_are_exclusive_and_fenced` |
| A9 commit-before-ack replay | Satisfied | real catalog correction committed to Tantivy, DB ack omitted, exact replay skipped by revision |
| A10 retry/permanent failure | Satisfied | configured max-attempt escalation plus deterministic poison-row isolation/healthy-sibling acknowledgement |
| A11 snapshot plus concurrent writes | Satisfied | repeatable-read exclusion followed by outbox catch-up visibility |
| A12 rebuild boundary recovery | Satisfied | cancellation lease release/direct retry, activation and retired-generation recovery |
| A13 corrupt/missing index | Satisfied | active `meta.json` corruption, failed diagnostic, PostgreSQL rebuild recovery |
| A14 zero media reads | Satisfied | published/cataloged fixture has nonexistent source/canonical paths before and after rebuild |
| A15 equivalent rebuilt results | Satisfied | exact document counts and query hits before/after corruption recovery |
| A16 filesystem path safety | Satisfied | symlink, root overlap, unowned root, UUIDv7/manifest, direct-child removal tests |
| A17 one writer per generation | Satisfied | PostgreSQL global lease plus Tantivy second-writer rejection/read-only concurrency test |
| A18 bounds/backpressure/cancellation | Satisfied | bounded claims/pages and deterministic rebuild cancellation/reclaim |
| A19 readiness/liveness/diagnostics | Satisfied | server healthy/missing/corrupt-generation readiness, live-count, and permanent failure diagnostic tests |
| A20 MQL syntax/precedence | Satisfied | 34 parser/planner tests |
| A21 exact errors and resource bounds | Satisfied | span/depth/token/clause/regex/date/timeout tests |
| A22 compatibility plus four rejected bugs | Satisfied | executable `compatibility/mql/phase6-corpus.json` tied to frozen legacy commit |
| A23 autocomplete/admin lookup | Satisfied | deterministic bounded autocomplete and active Artist lookup integration |
| A24 10M warm p95 below 50 ms | Satisfied for the declared interactive corpus | default-gate benchmark contract/latency smoke plus exact 10M evidence below; broad stress results disclosed separately |
| A25 complete gates | Satisfied | post-review repository and documentation gates passed on the final tree |

## MQL compatibility boundary

The committed compatibility corpus records the frozen legacy commit
`4028b58d...`, preserved implicit AND, field/range/comparison/boolean/quoted/regex syntax, and four
intentional corrections: no silent truncation, case-insensitive boolean keywords, consistent
default-field expansion, and regex over indexed Unicode-folded tokens rather than raw bytes. It is
parsed with unknown-field rejection and executed through the real parser/planner.

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

Environment: Linux `7.1.5-201.fc44.x86_64` x86-64, Rust/Cargo 1.97.1, 24 logical CPUs. The harness
uses corpus identity marker `5567940417645004038`, 35,000 Artists, 50,000 Releases, commit interval
100,000, and records whole-process Linux `VmHWM`. Record generation is a pure function of ordinal;
the marker versions the corpus and is not a pseudo-random seed. Synthetic records stream directly to the disposable index;
therefore PostgreSQL query count/time and committed generated dataset bytes are truthfully zero.
Source and canonical media reads are independently zero.

| Tracks | Documents | Build | Docs/s | Index bytes | Peak RSS KiB | Interactive warm p50/p95/p99 | Target |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| 10,000 | 95,000 | 584 ms | 162,631.34 | 17,719,072 | 107,880 | 0.176 / 1.886 / 1.922 ms | pass |
| 560,000 | 645,000 | 5,416 ms | 119,082.04 | 138,085,487 | 214,896 | 0.978 / 4.754 / 4.984 ms | pass |
| 10,000,000 | 10,085,000 | 112,572 ms | 89,587.06 | 1,837,039,122 | 732,260 | 5.951 / 27.258 / 31.842 ms | pass |

The 10M interactive corpus is: selective Track token, genre+exact year, primary Artist, exact ID,
and narrow duration range. Result counts were 25/25/25/1/25; individual warm p95 values were
7.409/27.565/1.915/0.640/31.842 ms.

The pre-review schema-v1 diagnostic run included broad analytical scans in the target aggregate and
did **not** pass: broad duration p95 was about 624–626 ms. The independent review identified that
ADR-declared numeric/date fast fields were not actually fast. Schema v2 fixes that contract and the
entire benchmark was rerun rather than retaining stale evidence. The harness still reports broad
scans separately as stress queries outside the declared interactive corpus. On the final 10M run,
`genre:rock AND year>=2000` was 72.775 ms p95 and
`duration>=60000 AND duration<=70000` was 36.752 ms p95. The genre scan still requires later
optimization before broad scans can be advertised as interactive. This qualification prevents a
selective-query result from becoming a false claim about arbitrary full-range analytics.

No generated catalog dataset or Tantivy index is committed. A 10,000-Track benchmark smoke runs in
the default Rust test gate to detect generator/count/result/media-read contract failures and
catastrophic interactive-latency regressions; the release-scale 10M measurement remains the recorded
acceptance artifact above.

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
repository gate passed on the final tree:

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
- Phase 7 owns public search routes and compatibility DTOs. Phase 10 owns the graphical diagnostics
  and search-management UI. Neither is activated by this Candidate.

Documentation impact:
- Updated — public search/MQL reference, installation configuration, system diagnostics,
  troubleshooting, `.env.example`, local migration/restore runbook, changelog, README/phase status,
  technology baseline, feature matrix, requirement traceability, ADR/evidence registries, and this
  Candidate report.
