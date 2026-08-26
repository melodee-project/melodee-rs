# Phase 4 Report — Catalog, Artist identity, and contributions

**Status:** Final — Phase 4 complete (2026-08-10)  
**Date:** 2026-08-10  
**Authority:** PRD-ARTIST-001 through PRD-ARTIST-012, PRD-RESOLVE-001 through
PRD-RESOLVE-005, PRD-CREDIT-001 through PRD-CREDIT-008, PRD-CATALOG-001 through
PRD-CATALOG-005, the supporting requirements named by the
[Phase 4 work package](../../ROAD-TO-FEATURE-COMPLETION.md), SPEC sections 7–13, 16–17,
22–25, and 28, ADR-0117/0118/0119, and
`design/agents/DEFINITION-OF-DONE.md`.

## Executive summary

Phase 4 implements the authoritative PostgreSQL semantic catalog and closes the manifest-v3
handoff introduced in Phase 3:

```text
Published -> Cataloging -> Cataloged
```

The catalog distinguishes Artist, Release, Track occurrence, MediaAsset bytes, and AudioIdentity.
It preserves raw observations and display presentation; resolves identities from durable evidence;
biases weak or conflicting evidence toward provisional identities; supports exact negative and
locked manual authority; stores structured billing, contributions, and participation; and records
audited Artist and AudioIdentity corrections without moving media.

Manifest-driven Releases adopt Phase 3's reserved UUIDv7 Release ID and receive an immutable catalog
semantic overlay. Published manifest v3, its plan/hash, sealed manifest hash, receipt, source, and
canonical assets remain unchanged. Legacy catalog rows preserve their public UUIDs verbatim; under
ADR-0119, a correction to a legacy-only Release records audit/outbox evidence without inventing a
manifest-v3 overlay.

## Architecture and durable data

- ADR-0117 defines the PostgreSQL catalog, internal/public ID namespaces, evidence and correction
  schema, participation projection, set-based import, catalog lease/fencing protocol, and Phase 4
  transactional outbox boundary.
- ADR-0118 distinguishes playable Track occurrences, MediaAsset encodings, and underlying
  AudioIdentity. It keeps manifest v3 immutable and places resolved semantic revisions in append-only
  PostgreSQL overlays.
- ADR-0119 makes legacy-only correction provenance explicit: audit/outbox evidence is authoritative,
  while a v3 overlay is forbidden when no truthful catalog-import/publication binding exists.
- Additive migration `0007_phase4_catalog.sql` creates normalized catalog, identity, evidence,
  correction, billing, participation, import, aggregate, audit, overlay, legacy-mapping, and outbox
  relations. UUIDv7, immutable public ID, append-only evidence/overlay, Release scope, contribution
  scope, exactly one playable MediaAsset per committed Track, and leased Cataloged-state invariants
  are enforced in PostgreSQL. A mandatory playable-asset ID, generated purpose, partial uniqueness,
  and deferred composite foreign key let atomic set-based import insert Tracks before their asset
  links without custom row-trigger queries or an invalid commit.
- The framework-independent domain crate contains typed UUIDv7 IDs and pure invariants. SQL,
  filesystem, web, and worker concerns remain outside that boundary.
- Normal import and correction paths rebuild only explicitly affected Tracks/Releases. A separate
  full participation rebuild is maintenance-only.

## Acceptance scenarios

| Scenario | Result | Reproducible evidence |
| --- | --- | --- |
| Exact same name | Satisfied | Domain and PostgreSQL identity tests create distinct Artists with identical canonical/normalized names and independent evidence/public IDs. |
| Alias resolution | Satisfied | Resolver and persistence tests require alias plus contextual or locked evidence; the observed/display strings remain unchanged. |
| Negative/manual authority | Satisfied | Scoped negatives suppress only the rejected candidate, revocation appends history, and locked manual decisions survive provider refresh/conflict. |
| Colin Hay participation | Satisfied | PostgreSQL participation tests separate solo/direct/guest/date-valid group inference from composition/production all-credit queries and assert the default-query index plan. |
| Artist merge/split | Satisfied | Transactional tests retain redirect/public identity, retarget normalized relationships, move only selected split facts, append audit/outbox/overlay evidence, reject cycles/conflicts, and leave media/publication facts unchanged. |
| AudioIdentity separation | Satisfied | Domain and PostgreSQL tests keep Track, MediaAsset, and AudioIdentity separate; fingerprints are evidence rather than keys; confirmed merge and selected split append decisions/overlays without moving assets. |
| Reserved Release identity and retry | Satisfied | Import adopts manifest v3's Release ID; identical retry succeeds; conflicts, lease expiry/theft, cancellation, and retryable failure expose no partial catalog and remain fenced/reclaimable. |
| Public-ID preservation | Satisfied | The frozen Phase 0 fixture round-trips all Artist/Release/Track UUIDs and reconciles parent/relationship ownership set-wise, idempotently, and fail-closed on conflict. |
| 200 Releases | Satisfied | The bounded PostgreSQL acceptance workload reaches 200 Cataloged exactly once with zero lost, duplicate, partial, or media-read result. |
| Immutable media contract | Satisfied | Import/correction tests compare source/canonical paths, manifest v3, plan and sealed hashes, and publication receipt before/after identity changes. The catalog crate has no filesystem or media-decoder dependency. |

## Resolution, correction, and participation

The deterministic local resolver evaluates normalized-name, alias, external-ID, context, and
conflicting evidence. Name equality alone is never authoritative. External provider access is behind
a provider-neutral authority trait; provider failure retains a provisional result, exact active
negatives are filtered, locked manual state short-circuits automated refresh, and provider-only
evidence cannot confirm a Melodee identity.

Artist match evidence and decisions are append-only. A proposed decision is audit-only; only a
confirmed or locked decision advances authoritative billing/projections. Merge locks identities in
deterministic UUID order, preserves the source tombstone/public ID, retargets supported normalized
facts, and fails closed on self/cycle/uniqueness conflicts. Split rejects a merged source and moves
only explicitly selected facts whose current decision belongs to that source.

Stable observation/identity rows serialize a first decision before a current pointer exists, while
database indexes permit exactly one history root and one successor per predecessor. Negative
revocation uses the same single-successor rule. Redirect resolution follows every merge hop and fails
closed on a cycle; deprecated identities cannot become correction targets.

AudioIdentity uses the same conservative evidence/correction principle. Exact byte hashes and
fingerprints do not auto-merge identities. Merge/split changes Track-to-AudioIdentity references and
semantic overlay/audit/outbox evidence only; it does not change Track identity or MediaAsset paths.

The participation projection distinguishes direct billing/contribution rows from date-valid inferred
group membership. Explicit membership exclusions win, an unknown Track/Release date cannot satisfy a
bounded membership, and the default performance query excludes composition and production roles.
The explicit all-credits query retains them.

Correction work is set-based and bounded before semantic writes: at most 1,000 affected Releases,
10,000 Tracks, 10,000 selected split assignments, 50,000 source-owned identity rows, and 100,000
overlay-reference rows per transaction. A larger request returns a typed dimension/limit error and
rolls back without an event or partial correction. Overlay source/reference loading uses a fixed
query set; immutable per-Release payload hashes are then constructed in bounded memory and their
rows, pointers, audit facts, and outbox work are written set-wise.

## Bounded import, failure recovery, and observability

Catalog claims use an independent configured concurrency bound and `min(batch_size, concurrency)`
claim cap. Each started task renews its lease immediately before the atomic import transaction;
unstarted cancellation releases its claim. Import validates the authoritative v3/plan/receipt
binding, uses set-based semantic mutations, writes catalog/import/aggregate/audit/outbox rows, and
fences `Cataloging -> Cataloged` in one transaction. An identical completed binding is idempotent;
any different work-item/Release/manifest binding fails closed.

Retryable errors retain `Cataloging`, clear the lease, and record backoff/evidence; exhausted or
permanent errors become `Failed`. A concrete short-lease PostgreSQL test proves renewal prevents
theft, cancellation leaves zero partial rows, a second worker can reclaim immediately, and the retry
creates one authoritative import.

The server runs the catalog loop independently of inspection, normalization, validation, and
publication. Configuration uses `MELODEE__CATALOG__ENABLED`, `CONCURRENCY`, `BATCH_SIZE`, and
`POLL_INTERVAL_SECONDS`. The CLI exposes bounded import, status, and per-work-item inspection;
durable work events, import records, audit facts, overlays, retry errors, and released-item counts are
available for operator diagnosis.

## 200-Release closure

The normal PostgreSQL performance test creates 200 valid Published manifest-v3 inputs and asserts:

- 200 Cataloged items and catalog-import bindings;
- zero lost, duplicate-authoritative, partial, source-read, or canonical-media-read result;
- catalog execution reaches but never exceeds the independent configured concurrency of four;
- 18 set-based semantic mutation statements per Release (3,600 total);
- 5,450 application SQL statements in the measured window, excluding transaction control: 24 per
  successful import, three per claimed item, and one claim-candidate query per 50 batches;
- 2,600 reconciled core authoritative rows and 6,000 total catalog/audit/outbox rows; and
- bounded whole-test-process peak RSS, reported as host evidence rather than an importer-only budget.

Exact observed output from the latest completed run:

```text
phase4-catalog-acceptance={"total":200,"cataloged":200,"lost":0,"duplicate_authoritative_result":0,"partial":0,"source_or_media_reads":0,"claim_batches":50,"configured_catalog_concurrency":4,"measured_catalog_concurrency":4,"set_based_catalog_mutation_statements":3600,"set_based_mutation_statements_per_release":18,"application_sql_statements_in_window_excluding_transaction_control":5450,"core_authoritative_rows_reconciled":2600,"catalog_rows_written":6000,"catalog_store_wall_ms":1191,"claim_store_call_ms":260,"import_store_call_sum_ms":3451,"peak_test_process_rss_kib":13020}
```

The process high-water mark includes the Rust test harness and is not attributed solely to the
importer. The full focused test finished in 9.70 seconds. The timings are one development-host
observation, not a product latency threshold.

## Compatibility and migration impact

No native Melodee API v1, OpenSubsonic, or Jellyfin route, wire DTO, error envelope,
authentication flow, pagination, streaming/range behavior, or intentional negative behavior
changed. The server/CLI catalog controls are additive Rust-native operator surfaces. No real client
cutover occurs in Phase 4.

The frozen compatibility fixture's 10 Artists, 10 Releases, 19 Tracks, two Artist relationship keys,
and all 39 public UUIDs are reconciled into separate immutable `public_api_id` columns. Equal public
and internal IDs for new rows do not collapse the namespaces. Phase 11 still owns production backup,
user/application-state migration, delta synchronization, traffic cutover, and rollback to the C#
server.

Migration 0007 is additive and never touches source or canonical media. A binary downgrade leaves
catalog/evidence tables intact. Full schema rollback requires a verified pre-migration PostgreSQL
backup and restore; dropping catalog, audit, or overlay tables in place is not a supported rollback.
The local-development runbook records forward, operational-downgrade, and full-restore procedures.

## Requirements disposition

| Requirement group | Closure status | Evidence |
| --- | --- | --- |
| PRD-ARTIST-001–012 | Tested | Typed domain/schema invariants; same-name, alias/external/relationship/membership, manual/negative, conservative resolution, merge/split, and provider-neutral tests. |
| PRD-RESOLVE-001–005 | Tested | Deterministic resolver plus durable exact evidence/decision, negative, proposal, lock, correction, and overlay PostgreSQL tests. |
| PRD-CREDIT-001–008 | Tested | Ordered ReleaseArtist/TrackArtist/Contribution/display model and Colin Hay direct/inferred/all-credit projection/query tests. |
| PRD-CATALOG-001–005 | Tested | Additive schema, reserved-ID set-based atomic import, aggregates/outbox, retry/fencing, legacy reconciliation, and 200-Release test. |
| PRD-INGEST-007 | Tested | End-to-end immutable manifest-v3 consumption through Cataloged without source/canonical audio reread. |
| PRD-INGEST-008/010 | Tested | Unresolved identity remains provisional and operator-facing state/evidence is inspectable. |
| PRD-INGEST-009 | In progress | Catalog writes now have an independent bounded pool; later enrichment and Phase 6 indexing resource classes remain unimplemented. |
| PRD-FP-001/002/004/005 | Tested for Phase 4 | Fingerprints/hashes remain evidence, AudioIdentity is explicit, uncertainty stays provisional, and corrections do not move media. Phase 5 owns novelty policy. |
| PRD-MIG-002/004 and PRD-API-M1-004 | Tested for Phase 4 mapping | Frozen public IDs/relationships round-trip; Phase 11 still owns production migration/cutover and user-state validation. |
| PRD-STORAGE-001/002 | Tested | Catalog identity and correction never determine or change canonical paths. |
| PRD-STORAGE-004 | In progress | Phase 4 preserves legacy mappings/media location, but Phase 11 registration of a real existing library remains unimplemented. |

## Validation record

Focused checks completed while implementing the phase:

```text
cargo test -p melodee-domain
PASS — 38 total unit/manifest/Phase 4 tests

cargo test -p melodee-catalog --all-features
PASS — 18 tests

cargo test -p melodee-persistence --test catalog_import --test catalog_identity \
  --test catalog_participation --test catalog_legacy --test catalog_schema \
  --test catalog_merge_split --test catalog_audio_identity -- --nocapture
PASS — 14 tests (5 import/recovery, 3 identity, 2 merge/split, and one each for
AudioIdentity, participation, legacy reconciliation, and schema integrity)

cargo test -p melodee-persistence --test catalog_performance -- --nocapture
PASS — 1 test; finished in 9.70 seconds

cargo test -p melodee-persistence --test catalog_concurrency \
  --test catalog_correction_regressions --test catalog_schema \
  --test catalog_identity --test catalog_audio_identity --test catalog_legacy \
  --test catalog_participation -- --nocapture
PASS — 15 tests, including deterministic concurrent decision/revocation chains and direct-SQL
redirect-cycle races; declarative exactly-one playable-asset integrity; typed pre-materialization cap
rollback; unique local/provider negative roots; multi-hop redirects/negatives; deprecated
source/target rejection; bounded Audio Track-before-Release merge preflight; observation-only overlay
refresh; legacy IDs; and participation

cargo clippy -p melodee-persistence --all-targets --all-features -- -D warnings
PASS

cargo check --workspace --all-features
PASS

cargo clippy --workspace --all-targets --all-features -- -D warnings
PASS

cargo test -p melodee-persistence --all-features --no-fail-fast
PASS — 65 tests; 0 failed
```

Final implementation gates:

```text
cargo fmt --all && ./scripts/check.sh
PASS (exit 0) — 20 ADRs, 8 mockups, and 232 requirements validated; requirements index current;
111 Python tests passed; rustfmt and workspace all-target/all-feature Clippy passed; all workspace
all-feature tests and doc-tests passed, including 65 persistence tests, the 200-manifest catalog
workload, the 10,001-Track bounded correction regression, and all earlier ingestion/media fault and
performance suites; cargo-deny advisories, bans, licenses, and sources passed. Only the documented
benign duplicate-version and license-not-encountered warnings were emitted.

cd docs && pnpm check
PASS — Astro checked 4 files with 0 errors, warnings, or hints

cd docs && pnpm check:links
PASS — all relative content links and assets are valid

cd docs && pnpm build
PASS — 35 pages built and all internal links validated (exit 0)
```

## Remaining later-phase work

- Phase 5 ReleaseTrait classification, novelty, policy/disposition, quarantine/ignore/delete
  authorization, and destructive-action audit/execution.
- Phase 6 Tantivy schema, outbox consumer/checkpoint, rebuild, search, and MQL.
- Native/compatibility API catalog browsing and client cutover in their assigned phases.
- Phase 11 production catalog/user-state migration, legacy semantic baseline, final synchronization,
  cutover, and rollback exercise.
- Optional global physical-byte deduplication and any future on-disk manifest version remain separate
  accepted decisions.

Documentation impact:
- Updated — `.env.example`, `README.md`, `CHANGELOG.md`,
  `docs/src/content/docs/getting-started/concepts.md`,
  `docs/src/content/docs/installation/configuration.md`,
  `docs/src/content/docs/library-management/artists.md`,
  `docs/src/content/docs/library-management/inbound-processing.md`,
  `docs/src/content/docs/library-management/tracks.md`,
  `docs/src/content/docs/admin/system-diagnostics.md`, and
  `docs/src/content/docs/troubleshooting/index.md`; internal ADRs, work package, migration/runbook,
  phase/status documentation, changelog, evidence, and requirement traceability are updated in the
  same change.
