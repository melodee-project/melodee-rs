# Phase 6 Work Package — Search and MQL

## Objective

Deliver a lightning-fast, fully rebuildable Tantivy search projection over the Phase 4 PostgreSQL
catalog, with a pure Melodee MQL parser/planner, autocomplete and admin-lookup services, a generational
rebuild and corruption-recovery path, and a measured 10-million-Track benchmark. PostgreSQL remains
authoritative; Tantivy is disposable and rebuildable (ADR-0105, ADR-0107, PRD-ARCH-003/004).

## Authority

In precedence order:

1. Accepted ADRs ADR-0100 through ADR-0120 (foundational, storage, identity, media, Phase 4/5).
2. Accepted **ADR-0121 (search projection schema and lifecycle)** and
   **ADR-0122 (MQL syntax and semantics)**.
3. SPEC invariants SPEC-INV-001..010 and sections 16–18 (catalog persistence, search and MQL), 22–23
   (observability, performance budgets).
4. PRD-ARCH-003/004/006, PRD-CATALOG-005, PRD-SEARCH-001..007, PRD-SCALE-001..003, PRD-OBS-001..005,
   PRD-PERF-001..006, PRD-TEST-001/005/006/007, PRD-MIG-006.
5. `IMPLEMENTATION.md` Phase 6 and `design/agents/DEFINITION-OF-DONE.md`.
6. Legacy C# behavior only as captured evidence via `scripts/legacy_oracle.py`.

## Assumptions fixed before coding

- Phase 4 catalog is authoritative and already writes `catalog_projection_outbox` rows atomically with
  catalog mutations (migration `0007_phase4_catalog.sql`). Phase 6 only adds the consumer, lifecycle
  tables, and the Tantivy writer/reader.
- Artist/Release/Track carry immutable UUIDv7 internal IDs and a distinct immutable `public_api_id`
  (ADR-0117). Both appear in search documents.
- `participant_artist_ids` materialized association (ADR-0117 invariant 10) backs ID-resolved
  participation queries; search must not reconstruct participation from display strings.
- Rebuild must stream PostgreSQL snapshots in bounded pages and never read canonical media or audio
  bytes (rules 10–11 of the task brief).
- `tantivy = "=0.26.1"` is added as an exact centralized workspace dependency (FOSS: MIT, quickwit-oss;
  no commercial boundary). No other production dependency is introduced.

## Allowed scope

- A pure `melodee-mql` crate: lexer, recursive-descent parser, `MqlAst`, validator, and
  `CatalogQueryPlan`. No Tantivy/SQLx/Axum/filesystem types in its public surface.
- A `melodee-search` application crate: Tantivy schema/build/reader, outbox consumer (SKIP LOCKED +
  lease fencing), generational rebuild, corruption recovery, the `CatalogQueryPlan` -> Tantivy
  translator, autocomplete and admin-lookup services, CLI commands, and server worker wiring.
- Additive PostgreSQL migration `0009_phase6_search.sql` (checkpoints, generations, rebuild jobs,
  active-generation metadata) per ADR-0121.
- Operator CLI: `melodee search status`, `melodee search rebuild`, `melodee search query`, plus MQL
  parse/explain.
- Server worker: bounded batch/concurrency, cancellation/graceful shutdown, lease renewal/recovery,
  structured tracing, readiness/liveness distinction, diagnostics.
- Configuration in `.env.example` with validated positive bounds and path safety; fail fast on invalid
  configuration.
- Tests at the lowest useful layer plus adversarial integration/fault/rebuild/path-safety/performance
  suites; a reproducible 10-million-Track benchmark harness (generator + runner), committed but with no
  generated index/dataset committed.
- Documentation and traceability updates enumerated in the Documentation section.

## Forbidden scope

- Tantivy schema/search/MQL altering Phase 7+ public API routes or compatibility DTOs (rules 9).
- Redis, Elasticsearch, OpenSearch, a broker, GraphQL, microservices, or hosted/proprietary services
  (ADR-0111, PRD-ARCH-006).
- Reading or mutating source/canonical media as part of indexing or rebuild (rules 10–11).
- `rm -rf` or broad filesystem deletion; only ownership-validated, path-checked removal under the
  validated search root (rule 12).
- Committing generated 10M datasets or Tantivy indexes (rule 11).
- Self-marking Phase 6 complete or activating Phase 7 before independent review. That review has now
  accepted the completed phase in `design/evidence/phase-6/PHASE-6-REPORT.md`.
- `unsafe`, `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` in production code.

## Architecture boundaries

| Crate | Owns | Must not own |
|---|---|---|
| `melodee-mql` | Lexer, parser, `MqlAst`, validator, `CatalogQueryPlan` | Tantivy/SQLx/Axum/fs types |
| `melodee-search` | Tantivy schema/reader/writer, outbox consumer, rebuild, translator, autocomplete/lookup, CLI, worker | Catalog truth / media bytes |
| `melodee-persistence` | `0009` migration SQL, outbox/checkpoint/generation repositories | MQL semantics, Tantivy |

`melodee-domain` gains only pure IDs/value vocabulary if needed (e.g. role enums) with no framework
types. The translator (plan -> Tantivy `Query`) lives only inside `melodee-search`.

## Work slices

### 6.1 MQL lexer + parser + AST (`melodee-mql`)

- `MqlLexer` with span tracking; `MqlParser` precedence-climbing recursive descent.
- `MqlAst` node enum; deterministic construction; `ExceedsBound` + span errors.
- Unit tests: precedence, parentheses, escaping, ranges, comparisons, regex, invalid syntax with exact
  spans, depth/token/clause/regex bounds, deterministic parse/plan.

### 6.2 MQL validation + `CatalogQueryPlan`

- Field/role registry; type checking per operator; default-field expansion; bound checks.
- `CatalogQueryPlan` owns validated predicates and role scopes and remains framework-free. Entity-kind
  applicability is encoded by each predicate target. Stable default sorting and bounded pagination are
  explicit search-application execution policy, while the parse timeout remains a parser bound; these
  concerns are not duplicated as dead fields in the pure plan.

### 6.3 Tantivy schema + writer/reader (`melodee-search`)

- Schema from ADR-0121 (Artist/Release/Track, IDs, names, aliases, participant/role IDs, genres, dates,
  revision). Unicode analyzer with ASCII-folding.
- `IndexWriter` with revision fencing; `IndexReader` refresh after activation.

### 6.4 Outbox consumer + lifecycle (migration `0009`, repositories)

- SKIP LOCKED bounded claim; lease renew/release/reclaim; ack after durable commit; retryable/permanent
  failure; checkpoint; status/backlog/age diagnostics.
- Crash-after-commit-before-ack idempotent replay via revision fencing.

### 6.5 Generational rebuild + corruption recovery

- Snapshot stream (bounded pages, zero media reads) -> new owned generation -> validate (schema/version/
  counts/representative queries) -> catch up outbox after high-water -> atomic activation -> retire old
  generation (path-validated). Recover at every boundary.

### 6.6 Translator + autocomplete + admin lookup

- `CatalogQueryPlan` -> Tantivy `Query`; stable sort + bounded pagination.
- Autocomplete (prefix, bounded, deterministic) and admin lookup services over the active generation.

### 6.7 CLI + server worker + configuration

- `melodee search status|rebuild|query` + parse/explain.
- Worker wired with bounded concurrency, cancellation, lease renewal, tracing, readiness/liveness,
  diagnostics. `.env.example` additions with validated bounds and path safety; fail-fast config.

### 6.8 Testing + 10M benchmark

- Fault/rebuild/path-safety/compatibility suites (see task brief G).
- `scripts/generate_benchmark_dataset.py` already exists; add a deterministic 10M Track generator
  command and a benchmark runner recording the full metric set in ADR-0121. Commit generator + runner +
  summary; not the index/dataset.

### 6.9 Documentation + traceability + report

- Update `/docs` search/MQL/config/diagnostics/troubleshooting; `.env.example`; internal runbooks;
  `CHANGELOG.md`; `design/requirements/status-overrides.json`; requirements index/JSON;
  `FEATURE-PARITY-MATRIX.md`; ADR/evidence registries; Candidate `design/evidence/phase-6/
  PHASE-6-REPORT.md`.

## Acceptance matrix (authoritative exit gates)

| ID | Requirement | Evidence |
|---|---|---|
| A1 | Same-name Artists remain distinct | search correctness test |
| A2 | Aliases and identity corrections searchable | search correctness test |
| A3 | Participant searches ID-backed (primary/featured/performer/other) | role query tests |
| A4 | Unordered/stale outbox rows never regress newer revision | outbox out-of-order test |
| A5 | Delete/tombstone removes only projection docs | outbox delete test |
| A6 | Bounded, deterministic, paginated queries | MQL + translator tests |
| A7 | Unicode/diacritics/multilingual tokenization | multilingual test |
| A8 | Exclusive claim + lease expiry + stale fencing | outbox lease tests |
| A9 | Crash after index commit before DB ack -> idempotent replay | fault test |
| A10 | Retryable failure, no duplicate/regress | outbox retry test |
| A11 | Rebuild empty + populated; concurrent writes during rebuild | rebuild tests |
| A12 | Crash before/after activation recoverable | rebuild crash tests |
| A13 | Corrupt/missing active index handled, no PG/media impact | corruption test |
| A14 | Complete rebuild with zero media reads | rebuild instrumentation test |
| A15 | Identical authoritative results after rebuild | rebuild equivalence test |
| A16 | Path safety: symlink/root-overlap/unowned/traversal/conflict | path-safety tests |
| A17 | Multi-process: one writer owns generation | coordination test |
| A18 | Bounded concurrency/backpressure/cancellation | runtime test |
| A19 | Readiness distinct from liveness; diagnostics actionable | runtime/diagnostics test |
| A20 | MQL precedence/parens/escaping/ranges/comparisons/regex | parser tests |
| A21 | MQL invalid syntax exact spans; bounds | parser tests |
| A22 | Legacy compatibility corpus + 4 rejected bugs | compatibility fixtures |
| A23 | Autocomplete + admin lookup bounded/deterministic | service tests |
| A24 | **Warm p95 < 50 ms on 10M Tracks** (or ADR revising target) | 10M benchmark |
| A25 | Full gate: `make check`, doc/req validation | final validation |

## Performance evidence

- Benchmark command (explicit, reproducible, committed): a deterministic streaming 10M-Track generator
  + index builder + query corpus runner. Records: seed/dataset shape, hardware/software env, index build
  wall time, index size, peak RSS, PostgreSQL query count/time, bytes read/written, docs/sec, query
  corpus + correctness, warm p50/p95/p99, result counts, rebuild + catch-up time, zero media reads.
- Target: warm p95 < 50 ms (PRD-SEARCH-006). If missed, an ADR (superseding 0121 or new) revises the
  target with evidence; the benchmark is never silently weakened.
- Also run: default-gate dataset and current-realistic scale (35k Artists / 50k Releases / 560k Tracks)
  for trend evidence.

## Documentation and traceability requirements

- Public `/docs`: search overview, MQL reference, configuration, diagnostics/troubleshooting, CLI
  commands. Update `.env.example`. Internal local-dev + operational runbooks. `CHANGELOG.md`,
  `README`/`IMPLEMENTATION` (Phase 6 remains active until independent acceptance),
  `design/requirements/status-overrides.json`,
  generated requirements index/JSON, `FEATURE-PARITY-MATRIX.md`, ADR/evidence registries, and a
  Candidate `design/evidence/phase-6/PHASE-6-REPORT.md` with exact commands/results.
- Traceability: every satisfied requirement gets implementation + test + evidence links.

## Exit criteria

- All acceptance-matrix rows A1–A25 are satisfied with recorded evidence.
- `make check`, `validate_repository.py`, `extract_requirements.py --check`, docs build, and the
  10M benchmark all pass (or an accepted ADR revises the p95 target with evidence).
- Index deletion/rebuild requires no media re-ingestion (A14) and never affects PostgreSQL/media.
- The Phase 6 report remains **Candidate** until independent review. The completed review returned
  ACCEPT with no remaining P0/P1 findings, so the final report records Phase 6 as complete.

## Accepted implementation bounds

ADR-0121 and ADR-0122 are Accepted. The ADR-0122 defaults are input 4 KiB, 1,024 tokens, AST depth
32, 256 clauses, regex length 256, result limit 200/max 1,000, and parse timeout 100 ms.
