# Phase 5 Report — Classification, novelty, and policy

**Status:** Final — Phase 5 complete (2026-08-10)  
**Evidence captured:** 2026-08-10T23:21:55Z  
**Base commit:** `68a11a0364a9c57bb72e8f43e9092851c01c275d` plus the shared Phase 5 working tree  
**Authority:** PRD-CLASS-001 through PRD-CLASS-003, PRD-POLICY-001 through
PRD-POLICY-008, the supporting requirements named by the
Phase 5 work package, SPEC invariants 001–009 and
sections 7, 11, 15–17, 22–25, and 28, ADR-0120, and the
[Definition of Done](../../agents/DEFINITION-OF-DONE.md).

## Exit decision

The focused acceptance evidence demonstrates the intended Phase 5 boundary:

```text
Cataloged -> policy classification + Track novelty + versioned rules
          -> Indexing | NeedsReview | Ignored | Quarantined
```

The pure policy engine consumes a validated, complete snapshot of authoritative PostgreSQL facts.
The worker does not open source or canonical media. Classification is multi-valued and retains
scoped evidence; all five Track novelty categories are derived from explicit byte, AudioIdentity,
manual-alternate, and audited no-match facts; and the store revalidates those facts and the sealed
manifest fingerprint in the same leased transaction that writes classification, novelty, decision,
audit, event, and work-state results.

`Delete` is deliberately not a filesystem operation. It remains a pending `NeedsReview` decision
until an explicit administrator authorization and a separate, matching completion record exist.
Phase 5 contains no media-removal implementation. `Accept`, `Ignore`, and `Quarantine` are
non-destructive state outcomes.

> Corrective follow-up (ADR-0145, accepted 2026-08-15): the durable staging lifecycle refined the
> source-custody model this report's retention language relied on. NeedsReview, Quarantined,
> Failed, and Published work retains the durable *staged* source; an acquired inbound candidate is
> drained after its verified staging receipt. The corrective closure evidence, including
> review/quarantine retention-across-restart proofs, lives in
> [R0 — corrective baseline and durable staging closure](../r0/README.md).

Independent persistence review found no residual P0/P1 after verifying existing-disposition
matching, symmetric alternate-recording reachability, independently asserted catalog-context
evidence, canonical decision summaries, and default-Accept dry-run persistence without a synthetic
rule identifier.

## Architecture and durable behavior

- ADR-0120 defines the classification, evidence hierarchy, novelty vocabulary, immutable rule
  sets, dispositions, durable decisions/audit, and dry-run boundary.
- `melodee-policy` is a pure application crate. Its catalog-context adapter maps a bounded snapshot
  into classifier/novelty inputs without filesystem access.
- Migration `0008_phase5_policy.sql` adds immutable classification/evidence and novelty facts,
  explicit AudioIdentity alternate/no-match assertions, versioned rule sets, policy decisions,
  append-only decision audit, and dry-run summaries. Exact composite foreign keys bind each
  decision and audit row to its Release, rule version, classification, novelty evaluation,
  catalog import, and sealed fingerprint.
- The persistence commit rejects caller-spoofed traits, novelty labels, rule outcomes, actors,
  versions, incomplete Track sets, and foreign Release/media/AudioIdentity facts before any
  partial policy write.
- Policy claims are independently bounded and leased. Renewal, cancellation release, expiry,
  theft, direct reclaim, identical retry adoption, and crash-after-decision restart convergence
  are covered in PostgreSQL and server-worker tests.
- Rule sets are immutable by ID/version. Configured changes require a new version; unsafe rule
  outcomes and duplicate identifiers fail closed.

## Required acceptance scenarios

The work package requires at least twenty acceptance tests. The following twenty behavior groups
are independently asserted; most groups have multiple pure, PostgreSQL, worker, or compatibility
cases.

| # | Scenario | Result | Executable evidence |
| ---: | --- | --- | --- |
| 1 | Multi-valued ReleaseTraits | Satisfied | `classification_is_multivalued_and_every_trait_has_scoped_evidence` |
| 2 | Evidence hierarchy and explicit administrator authority | Satisfied | `administrator_classification_is_explicit_highest_tier_evidence`; configured threshold tests |
| 3 | Exact title/phrase classification without loose substring matches | Satisfied | title boundary, remix, and repeated-signal tests |
| 4 | Exact duplicate is byte identity, not guessed AudioIdentity identity | Satisfied | `exact_duplicate_is_derived_from_bytes_not_audio_identity_membership` |
| 5 | Known AudioIdentity is redundant without conflating bytes | Satisfied | pure exact/known test plus supported PostgreSQL-to-engine matrix |
| 6 | Alternate recording requires exact audited positive evidence | Satisfied | public alternate writer plus symmetric two-endpoint PostgreSQL commit, five-category store matrix, and server engine matrix |
| 7 | New Track requires an explicit audited catalog no-match | Satisfied | no-match assertion, exact retry/conflict, server engine matrix |
| 8 | Missing/ambiguous facts remain Uncertain | Satisfied | pure novelty tests and PostgreSQL/server five-category matrices |
| 9 | Fully redundant Greatest Hits defaults to safe Ignore | Satisfied | pure default-policy and executable compatibility corpus |
| 10 | New or alternate Greatest Hits material cannot take redundant outcome | Satisfied | pure policy matrix and compatibility corpus |
| 11 | Uncertain Greatest Hits material routes Review | Satisfied | pure policy matrix and compatibility corpus |
| 12 | Directory-only Greatest Hits evidence cannot authorize destruction | Satisfied | pure hierarchy test and `directory-only-exact-safe-accept` corpus case |
| 13 | Immutable media read-error evidence routes Quarantine atomically | Satisfied | persistence and server corrupt-context tests; no media path exists to reopen |
| 14 | Versioned/configurable rule sets are ordered, immutable, and safe | Satisfied | pure configured-rule tests and PostgreSQL rule-set adoption/conflict tests |
| 15 | Facts, decision, audit, event, and state commit atomically | Satisfied | `valid_evaluation_commits_facts_decision_audit_and_state_atomically` |
| 16 | Operator override is explicit, fingerprint-bound, and audited | Satisfied | pure actor test, PostgreSQL pending-review override, CLI integration |
| 17 | Destructive authorization and completion are separate and exact | Satisfied | Delete override/completion test rejects missing or mismatched actor/reason/fingerprint |
| 18 | Dry-run is bounded and writes no decisions | Satisfied | pure nested cap+1 tests, persistence zero-bucket/no-decision test, CLI PostgreSQL test |
| 19 | Lease theft, cancellation, and process restart converge once | Satisfied | persistence fencing/crash tests and server cancellation/adoption tests |
| 20 | 200 Releases drain with bounded concurrency and no media reads | Satisfied | default server worker load test and exact JSON below |

## Greatest Hits compatibility and safety evidence

`compatibility/phase5/greatest-hits-safety.json` is committed, schema-versioned input rather than a
prose-only claim. `phase5_compatibility.rs` parses every case with unknown-field rejection and sends
its complete classification and novelty facts through the real pure policy pipeline. It asserts the
expected and forbidden disposition and every derived Track category for:

- exact-byte, known-AudioIdentity, and mixed redundant Greatest Hits;
- new, confirmed alternate, and uncertain Greatest Hits;
- a directory-only Greatest Hits hint;
- title-supported Greatest Hits; and
- an ordinary new Release.

The legacy C# evidence is intentionally limited. It proves that a directory keyword could hold or
stage a Release and that the legacy flow had unsafe same-size deletion behavior. It does **not**
provide an AudioIdentity/novelty oracle. The Rust implementation preserves the user-visible safety
intent—uncertainty and weak path evidence are non-destructive—while explicitly refusing to preserve
same-size deletion or title/path strings as identity.

No native Melodee API v1, OpenSubsonic, Jellyfin route, payload, identifier, authentication flow,
pagination behavior, or streaming behavior changes in Phase 5.

## PostgreSQL, fault, and adversarial evidence

The focused persistence suite proves:

- complete Cataloged handoff and sealed-fingerprint binding without readable media paths;
- claim exclusivity, bounded expired-lease recovery, renewal, release, fencing, and direct reclaim;
- exact-fact commit plus fail-closed forged novelty, classification, actor, rule, scope, and source
  binding cases with zero partial materialization;
- immutable rule-set exact adoption and conflicting/unsafe rule rejection;
- all five novelty categories using supported durable APIs and exact store revalidation;
- explicit no-match and alternate assertions, including exact retry and conflict behavior;
- pending Review override and separately authorized destructive completion;
- a stolen stale worker that cannot write authoritatively, followed by one successful winner; and
- a simulated crash after durable facts/decision but before the work-state route, followed by
  lease recovery and exact adoption with one classification, novelty evaluation, Track fact,
  decision, and audit chain.

The schema integration gate additionally covers UUIDv7 and scope constraints, append-only policy
facts/audit/rules, exact cross-Release foreign keys, legal leased/unleased Cataloged state, and
rejection of partial lease tuples and generic work-state bypasses.

## 200-Release bounded runtime evidence

The default server integration test creates 200 valid Cataloged Releases, configures concurrency
eight, drains them in repeated bounded passes, and exercises the actual worker path:

```text
PolicyStore claim -> load_context -> pure evaluate_catalog_context -> atomic commit
```

Exact observed output from the latest local focused run:

```json
{"absent_media_paths_verified":400,"canonical_media_reads":0,"classifications":200,"decision_audit_rows":200,"decisions_adopted":0,"decisions_inserted":200,"duplicate_decisions":0,"errors":0,"evaluated":200,"failed_items":0,"failures":0,"final_states":{"needs_review":200},"lost_claims":0,"max_in_flight":8,"novelty_evaluations":200,"partial_releases":0,"pass_count":25,"peak_test_process_rss_kib":17140,"peak_test_process_rss_supported":true,"policy_transition_events":200,"policy_window_wall_ms":1631,"set_based_mutation_statement_count":1400,"source_media_reads":0,"track_novelty_rows":200}
```

`set_based_mutation_statement_count` is the store's seven-statement semantic mutation count for
each inserted evaluation (1,400 total). It is **not** labeled as total PostgreSQL queries or total
application SQL statements. The 1,631 ms policy window and 17,140 KiB Linux `VmHWM` are one
development-host observation, not product latency or importer-only memory budgets. `VmHWM`
measures the whole Rust test process. The no-read assertion is separate and concrete: all 400
source/canonical paths are deliberately absent while all 200 evaluations complete.

Environment for this observation:

- Linux `7.1.5-201.fc44.x86_64`, x86-64;
- Rust `1.97.1`, Cargo `1.97.1`;
- PostgreSQL client/server test environment `18.4`; and
- deterministic single-Track synthetic Releases, batch/concurrency limited to eight.

## Migration, downgrade, rollback, and restore

Migration 0008 is additive and does not touch media. The forward procedure is:

1. stop or exclude policy workers;
2. create a PostgreSQL custom-format backup outside the repository;
3. verify the backup with `pg_restore --list`;
4. run the normal migration command;
5. verify migration version 8 and the policy relations; and
6. start one bounded worker and inspect policy claims/decisions before normal rollout.

An application downgrade retains 0008 and every classification, novelty, rule, decision, dry-run,
and audit row. An older binary must first be validated against an isolated post-0008 copy. There is
no destructive down migration. If the older build cannot tolerate the additive schema or policy
work states, the only schema rollback is restoration of the verified pre-0008 backup into a new,
empty database followed by migration-history and catalog-integrity checks. The current database is
retained until cutover verification completes. PostgreSQL restore does not modify media bytes.
Exact commands are documented in `design/runbooks/LOCAL-DEVELOPMENT.md`.

## Commands and results

Focused pure-policy and compatibility gate:

```text
cargo test -p melodee-policy --all-features
PASS — 37 tests: 2 unit, 34 policy acceptance, 1 executable compatibility corpus
```

Focused PostgreSQL gate:

```text
cargo test -p melodee-persistence \
  --test phase5_claims --test phase5_commit --test phase5_faults \
  --test phase5_handoff --test phase5_novelty --test phase5_override \
  --test phase5_rule_sets --test policy_phase5 --test catalog_schema \
  --all-features -- --nocapture
PASS — 21 tests: 20 Phase 5 cross-slice tests plus 1 catalog/schema integrity test
```

Focused worker/runtime gate:

```text
cargo test -p melodee-server --test policy_worker --all-features -- --nocapture
PASS — 6 tests, including supported five-category and independent catalog-trait engine/commit
flows, cancellation, restart/adoption, corrupt-media quarantine, and the default 200-Release load
test
```

Focused CLI PostgreSQL gate:

```text
cargo test -p melodee-cli --test policy --all-features -- --nocapture
PASS — 1 test
```

Strict test lint gates:

```text
cargo clippy -p melodee-persistence --tests --all-features -- -D warnings
PASS

cargo clippy -p melodee-policy --tests --all-features -- -D warnings
PASS
```

Complete repository gate:

```text
./scripts/check.sh
PASS — default test concurrency against isolated PostgreSQL 18.4-alpine with 512 MiB shared
memory: repository validation (21 ADRs, 8 mockups, 232 requirements), requirements index,
111 Python tests, rustfmt, workspace strict Clippy, workspace all-feature tests and doc-tests,
and cargo-deny
```

Two earlier attempts against the pre-existing 64 MiB dev PostgreSQL container failed outside the
Phase 5 slice: one transient media-process assertion and concurrent Phase 3 test-database migrations
that exhausted PostgreSQL dynamic shared memory. A serialized diagnostic run then exposed Phase 3
worker-stat reporting after per-item errors. The follow-up work raised the documented dev-container
shared-memory allocation to 512 MiB and added Phase 3 lease-release/fail-fast regressions. The
default-concurrency gate above passed against the corrected isolated database; the failed attempts
are not represented as successful gates.

Public documentation gate:

```text
cd docs && pnpm check && pnpm check:links && pnpm build
PASS — Astro diagnostics: 0 errors/warnings/hints; relative and build-time links valid; 35 pages
built
```

## Requirement traceability

| Requirement | Evidence |
| --- | --- |
| PRD-CLASS-001 | Multi-valued domain invariants, title/structured evidence classification, immutable PostgreSQL traits/evidence |
| PRD-CLASS-002 | Per-evidence source/kind/confidence/detail, exact manifest provenance validation, inspectable durable rows |
| PRD-CLASS-003 | Explicit hierarchy and directory-only non-destructive pure/compatibility cases |
| PRD-POLICY-001/002 | Typed dispositions and traits; worker state routing and operator CLI output |
| PRD-POLICY-003 | Greatest Hits uses classification evidence plus complete Track novelty, never title alone |
| PRD-POLICY-004/005 | Exact/Known/Alternate/New/Uncertain derived through supported durable facts and protected in the compatibility matrix |
| PRD-POLICY-006 | Immutable ordered rule sets, safe defaults, valid bounded configuration, corrupt-media quarantine |
| PRD-POLICY-007 | Atomic decision/audit/facts/state, explicit override, separate destructive completion, restart adoption |
| PRD-POLICY-008 | Phase 5 backend/CLI tested: bounded dry-run with per-rule/existing/proposed buckets, samples and impact, with zero decision writes. The broad requirement remains `in-progress` because Phase 10 owns the Admin UI surface. |
| PRD-PRINCIPLE-005/006 | IDs and durable evidence, not strings, determine identity/novelty; uncertainty never becomes destruction |
| PRD-FP-001/002/004/005 | Existing hashes and AudioIdentities are reused as evidence; byte identity remains distinct; no physical-byte dedupe is introduced |
| PRD-CATALOG-001–005 | Policy consumes the authoritative bounded catalog and does not rewrite its IDs, media, or semantic history |
| PRD-STORAGE-001–004, PRD-INGEST-006–010 | Sealed source fingerprint is checked; policy never derives paths or opens/moves/deletes media; durable state/history remains visible |
| PRD-ARCH-006 | Modular monolith with PostgreSQL only; no broker, cache, search service, or new infrastructure |
| PRD-SCALE-001/002 | Bounded claims/snapshots/dry-runs; measured 200-Release convergence; indexed PostgreSQL reads |

## Remaining later-phase work

- Phase 6 owns Tantivy search/indexing, transactional-outbox consumption, rebuild/catch-up, and MQL.
- Phase 10 owns graphical policy editing and Release review.
- Phase 11 owns production migration/cutover and restore rehearsal against real library data.
- Phase 5 intentionally does not execute deletion. Any future media-removal executor must remain a
  separate fingerprint-bound authorization/completion boundary and receive its own fault evidence.

Documentation impact:
- Updated — public policy, configuration, inbound-state, release-review, diagnostics, and
  troubleshooting guidance; the local migration/restore runbook; compatibility safety corpus; ADR
  index; phase/status documents; changelog; requirement traceability; and this evidence report.
