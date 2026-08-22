# Phase 4 Work Package — Catalog, Artist Identity, and Contributions

## Objective

Implement the Phase 4 catalog boundary from immutable Release manifest v3 through one authoritative,
queryable PostgreSQL catalog: Melodee-owned Artist/Release/Track/MediaAsset/AudioIdentity identities,
evidence-based Artist resolution, structured billing/contributions, safe correction history, and a
bounded set-based `Published -> Cataloged` handoff.

This document is the assigned Phase 4 work package required by `AGENTS.md`. ADR-0117, ADR-0118, and
ADR-0119 are the accepted schema, identity, overlay, and legacy-correction provenance decisions for
this package. Implementation must remain in the slices and non-goals below.

## Authority

In precedence order:

1. ADR-0100, ADR-0101, ADR-0102, ADR-0104, ADR-0105, ADR-0106, ADR-0107, ADR-0110,
   ADR-0111, ADR-0115, ADR-0116, ADR-0117, ADR-0118, and ADR-0119.
2. SPEC invariants 001 through 009 and sections 7 through 13, 16, 17, 22 through 25, and 28.
3. PRD-ARTIST-001 through PRD-ARTIST-012, PRD-RESOLVE-001 through PRD-RESOLVE-005,
   PRD-CREDIT-001 through PRD-CREDIT-008, and PRD-CATALOG-001 through PRD-CATALOG-005.
4. Supporting requirements PRD-PRINCIPLE-001/002/005/006/008, PRD-SCALE-001/002,
   PRD-ARCH-002/003/005/006, PRD-INGEST-007/008/009/010, PRD-FP-001/002/004/005,
   PRD-STORAGE-001/002/004, PRD-API-M1-004, PRD-MIG-002/004, PRD-OBS-001/004,
   PRD-PERF-001/002/003/004, and PRD-TEST-001/005/006/007.
5. `IMPLEMENTATION.md` Phase 4 and `design/agents/DEFINITION-OF-DONE.md`.

## Assumptions fixed before coding

- Phase 3 manifest v3, its PostgreSQL payload, canonical `manifest.json`, authorization, plan, and
  receipt are immutable inputs. Phase 4 adds a PostgreSQL `CatalogSemanticOverlay`; it does not
  rewrite v3 or reopen audio.
- A manifest-driven catalog Release adopts the UUIDv7 Release ID reserved by ADR-0116. Work-item,
  manifest, plan, public compatibility, external-provider, and catalog identity namespaces remain
  distinct.
- Artist names, aliases, external IDs, source paths, hashes, fingerprints, and billing strings are
  attributes or evidence, never semantic identity keys.
- `AudioIdentity` is the underlying recording/performance identity. A Track is a Release occurrence;
  a MediaAsset is encoded bytes. Initial imports bias toward distinct provisional identities.
- Exact legacy Artist/Release/Track public UUIDs are preserved in separate immutable columns. Phase 4
  proves catalog/public-ID mapping; Phase 11 retains production cutover and user-state migration.
- Catalog import is leased, fenced, bounded, set-based, and atomic. It writes future-search outbox
  records but does not implement a search index or worker.
- Merge, split, rename, alias, contribution, membership, and AudioIdentity correction never move,
  rewrite, or delete canonical or inbound media.
- Manifest-driven corrections append immutable semantic overlays. Under ADR-0119, legacy-only
  corrections retain audit/outbox evidence without fabricating manifest-v3 overlay provenance.
- Classification, novelty, general disposition policy, and destructive action remain Phase 5.

## Allowed scope

- Framework-independent domain types and invariants for Artist, aliases, external identity,
  relationships, group membership, Release, Track, MediaAsset, AudioIdentity, billing, contributions,
  observations, matches, negative assertions, locks, merge/split, and semantic overlays.
- Additive PostgreSQL migrations, SQLx repositories, application services, and transactional audit /
  outbox behavior governed by ADR-0117/0118/0119.
- A bounded catalog worker and CLI/diagnostic paths needed to claim, retry, inspect, and explain
  catalog import.
- Set-based v3 import, aggregates, direct/inferred participation projection, legacy catalog/public-ID
  fixture import, tests, benchmarks, evidence, traceability, CI, and affected documentation.

## Forbidden scope

- Phase 5 ReleaseTrait classification, novelty conclusions, policy/disposition engines, source
  deletion, or destructive-action execution.
- Tantivy schema/index writer/search/MQL, even though Phase 4 writes the transactional outbox.
- Native Melodee API v1, OpenSubsonic, Jellyfin, streaming/playback, authentication, podcast, party,
  UI, or other later-phase product implementation.
- Production migration/cutover, user/application-state migration, final delta synchronization, or
  removal/relocation/re-encoding of an existing canonical library.
- A broker, Redis, Elasticsearch/OpenSearch, GraphQL, microservices, provider-owned canonical
  identity, global physical byte deduplication, or C# service/class translation.

## Implementation slices

### 4.1 Domain vocabulary and invariants

- Implement IDs, values, statuses, roles, date ranges, direct/inferred participation, observations,
  match evidence/decisions, negative assertions, locks, AudioIdentity, and overlay invariants.
- Preserve exact display strings separately from structured relationships.
- Add unit/property tests for non-unique names, parser non-authority, provisional bias, role filters,
  invalid self-relationships, date boundaries, and immutable IDs.

### 4.2 Catalog and identity migration

- Add one coherent, deterministic migration series for ADR-0117/0118's relations, constraints,
  partial uniqueness, indexes, audit events, semantic overlays, catalog imports, participation
  projection, and outbox.
- Include forward/rollback guidance. Rollback retains identity/catalog/audit evidence unless a
  verified database restore is performed; it never touches media.
- Add PostgreSQL integrity and representative-snapshot migration tests before repository behavior.

### 4.3 Evidence-based Artist resolution

- Persist raw `ObservedArtist` values and context before resolution.
- Implement deterministic candidate evaluation over local/external/alias/context evidence and
  conflicting evidence, with provider calls behind an authority interface.
- Persist the evidence and decision; honor locks and negative assertions; create provisional Artists
  rather than merge on weak/name-only evidence.
- No external HTTP provider is required for Phase 4 acceptance; deterministic fake authorities prove
  the boundary and failure semantics.

### 4.4 Bounded set-based manifest v3 import

- Add dedicated claim/reclaim/index behavior for `Published` and `Cataloging` with lease heartbeat,
  fencing, cancellation, retry history, and an independent catalog-write concurrency bound.
- Validate the PostgreSQL v3/plan/receipt contract and adopt the reserved Release ID.
- Insert Release/Track/asset/observation/provisional identity/billing/AudioIdentity/overlay/import /
  aggregate/audit/outbox rows with set-based SQL in one completion transaction.
- Treat an identical completed import as success and any work-item/Release/manifest conflict as a
  durable fail-closed error. Never read source or canonical audio bytes.

### 4.5 Contributions, correction, and participation

- Implement ReleaseArtist, TrackArtist, Contribution, DisplayArtist/join ordering, group membership,
  and direct/inferred association maintenance.
- Implement locked manual resolution, negative assertions, merge tombstones/redirects, selected
  audited split reassignment, and append-only semantic overlay revisions. Legacy-only corrections
  follow ADR-0119's audit/outbox path and never synthesize manifest-v3 facts.
- Prove the default performance participation query and explicit all-credits query separately.

### 4.6 Legacy public-ID and scale proof

- Import the frozen Phase 0 catalog fixture or an equivalent representative snapshot through the
  Phase 4 mapping, preserving Artist/Release/Track public UUIDs and reconciling counts/relationships.
- Add the exact-same-name case the legacy unique constraint could not represent.
- Run a 200-manifest bounded catalog scenario and record wall time, peak catalog concurrency,
  database query count/time, rows written, zero duplicate/lost/partial imports, and no file reads.

### 4.7 Phase 4 closure

- Produce `design/evidence/phase-4/PHASE-4-REPORT.md` with reproducible commands/results,
  requirement disposition, schema/rollback impact, compatibility/public-ID impact, known limits, and
  a clear exit decision.
- Update requirements status/evidence, architecture/runbook/status documentation, the living feature
  parity matrix, changelog, and affected `/docs` content in the same change.
- Run `cargo fmt --all`, `make check`, and the documented docs gates.

## Required acceptance scenarios

1. **Exact same name:** two distinct Artists have identical canonical and normalized names, retain
   different IDs/disambiguation/evidence, and are never merged by string equality.
2. **Alias resolution:** a different observed name resolves to an existing Artist only through
   sufficient/locked evidence; the raw display remains unchanged.
3. **Negative/manual authority:** a rejected match is not repeatedly proposed and a locked manual
   decision survives provider refresh/conflict.
4. **Colin Hay participation:** the default query returns solo, explicit guest/feature/performance,
   and only date-valid inferred Men at Work participation; unrelated composition/production credit is
   available only through all credits.
5. **Merge/split:** merge retains the old public identity as a redirect; split moves only selected
   relationships with an audit record; neither changes a canonical asset path or bytes.
   Manifest-driven Releases append overlays, while legacy-only Releases retain audit/outbox evidence
   without a fabricated overlay or catalog import.
6. **AudioIdentity separation:** Track occurrence, byte-identical/different MediaAsset, and
   underlying AudioIdentity remain distinct; fingerprint similarity alone does not merge.
7. **Reserved Release identity:** Cataloged Release ID equals manifest v3's Release ID. Duplicate
   submission, crash, lease theft/expiry, or PostgreSQL failure creates no duplicate/partial catalog.
8. **Public-ID preservation:** frozen legacy Artist/Release/Track UUIDs round-trip verbatim with
   reconciled catalog relationships.
9. **200 Releases:** every valid Published item reaches Cataloged exactly once under bounded catalog
   concurrency; every failure retains durable state, evidence, and a next legal action.
10. **Immutable media contract:** catalog import and all identity corrections leave source media,
    canonical media, manifest v3, plan hash, sealed-manifest hash, and publication receipt unchanged.

## Phase exit criteria

Phase 4 is complete only when all required acceptance scenarios pass and:

- the schema and implementation satisfy every Phase 4 Artist/Resolve/Credit/Catalog requirement with
  traceability to implementation, tests, and evidence;
- `PRD-INGEST-007` is proven end-to-end from v3 through catalog insertion without audio reread;
- catalog writes/aggregates are set-based and independently bounded, with no N+1 or whole-catalog
  processing in normal import/correction paths;
- identity resolution is explainable, manually lockable, negative-aware, and biased against false
  merge;
- direct and date-valid inferred participation is queryable at the lowest useful PostgreSQL layer;
- migration integrity, public-ID reconciliation, rollback/restore guidance, observability, operator
  errors, compatibility impact, and documentation impact are explicit;
- no forbidden Phase 5+ feature or destructive media behavior entered the change; and
- the clean repository and documentation gates pass with reproducible Phase 4 evidence.

## Required report

Report files changed; exact requirements satisfied; ADRs applied or proposed; test/benchmark commands
and results; compatibility and real-client impact; schema/data/migration/rollback impact;
observability/operations impact; documentation impact in the mandated format; and remaining risks.
Do not call Phase 4 complete while any criterion above lacks reproducible evidence.
