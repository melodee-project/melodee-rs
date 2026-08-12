# Phase 7 Work Package — Native Melodee API v1

## Objective

Deliver a production-quality Rust-native `melodee-api-v1` adapter that is behaviorally compatible with the frozen legacy contract, with explicit, phase-gated ownership for any deviations, and complete traceable evidence for every route in scope.

## Required authority

In precedence order:

1. `design/agents/DEFINITION-OF-DONE.md`
2. `AGENTS.md` (root)
3. `SPEC.md` sections: external API surfaces, native API v1, authentication/security, compatibility testing, migration, and performance budgets
4. `IMPLEMENTATION.md` (Phase 7)
5. `PRD.md` requirements (exactly, unless explicitly removed by accepted ADR):
   - `PRD-API-M1-001` — `PRD-API-M1-007`
   - `PRD-STREAM-001` — `PRD-STREAM-003`
   - `PRD-AUTH-001` — `PRD-AUTH-007`
   - `PRD-PLAYLIST-001` — `PRD-PLAYLIST-003`
   - `PRD-USER-001` — `PRD-USER-005`
   - `PRD-REQUEST-001` — `PRD-REQUEST-002`
   - `PRD-SHARE-001` — `PRD-SHARE-003`
   - `PRD-POD-001` — `PRD-POD-003`
   - `PRD-DISCOVERY-001` — `PRD-DISCOVERY-003`
   - `PRD-PARTY-001` — `PRD-PARTY-004`
6. Accepted ADRs: `ADR-0100, ADR-0101, ADR-0102, ADR-0103, ADR-0104, ADR-0105, ADR-0107, ADR-0108, ADR-0110, ADR-0111, ADR-0114, ADR-0123, ADR-0124, ADR-0125, ADR-0126, ADR-0127, ADR-0128, ADR-0129`

No route, DTO, persistence schema, or compatibility change is implemented without a corresponding requirement and/or accepted ADR trace entry.

## Activation and boundary assertions

- Root/phase checks completed: Phase 7 is independently accepted and Phase 8 is active.
- Phase 6 fixed findings confirmed:
  - bounded max-attempt escalation for persistent outbox failures
  - per-row isolation of malformed/poison outbox rows

These are currently closed in:

- `design/evidence/phase-6/PHASE-6-REPORT.md`

## Scope status and explicit route matrix

Source artifacts:

- `compatibility/melodee-api-v1/contracts/route-manifest.json`
- `compatibility/melodee-api-v1/contracts/openapi-v1.json`
- `compatibility/melodee-api-v1/contracts/native-client-usage.json`
- `compatibility/scenarios/*.json`
- `compatibility/melodee-api-v1/contracts/route-inventory.json`

Route totals from the canonical inventory at the Candidate freeze:

- `routeCount`: 202
- Captured with promoted golden evidence: 167
- Excluded-from-build with promoted negative evidence: 35

Rust evidence registry:
`compatibility/melodee-api-v1/contracts/phase7-route-evidence.json`.

Generated machine-readable matrix: `design/agents/PHASE-7-ROUTE-MATRIX.json`. Regenerate with
`python3 scripts/build_phase7_route_matrix.py`; check drift with `--check`. Do not edit the generated
matrix directly.

Phase 7 status rules (exactly one per route):

1. `implementation-pending`
2. `negative-compatibility-pending`
3. `implemented-and-differentially-tested`
4. `preserved-intentional-legacy-absence`
5. `approved-compatibility-normalization` (accepted ADR required)
6. `explicitly-deferred-by-accepted-scope-decision` (accepted ADR required)

Final matrix classification status (derived from the canonical inventory and evidence
registry):

- implementation pending: 0
- negative compatibility pending: 0
- implemented and differentially tested: 150
- preserved intentional legacy absence: 35
- approved compatibility normalization: 17
- explicitly deferred by accepted scope decision: 0

Every row has promoted legacy evidence and route-specific Rust evidence. The registry validator
continues to require promoted scenarios, Rust tests, requirement IDs, and accepted ADR authority
where applicable; the completed count is not inferred from route registration alone.

## Hard boundary decisions for this package

### 1) Overlap with Phase 9 domains

ADR-0123 resolves the Phase 7/Phase 9 overlap:

- Every route compiled into the frozen native-v1 server is a Phase 7 implementation obligation,
  including analytics, requests, shares, and podcasts.
- Phase 9 owns completion of those domains outside the captured native-v1 contract.
- Lack of a legacy scenario is capture work, not authority to defer a live route.

### 2) Queue routes

- All 32 Party and three Queue routes were excluded from the frozen legacy build.
- They remain Phase 7 negative-compatibility obligations under ADR-0123.
- Queue has legacy runtime scenarios; Party currently does not. Every route needs both a legacy
  negative capture and a Rust route-level negative test before the matrix may say its absence is
  preserved.

### 3) `ADR-0114`

- `ADR-0114` is **Accepted**.
- `POST /api/v1/playlists/{apiKey}/songs` must accept both the canonical bare array and MeloAmp's
  object body, with differential tests for both forms and the approved normalization.

## Explicitly forbidden in Phase 7

- OpenSubsonic or Jellyfin surfaces in shared DTOs/routes.
- Redis/Broker/RabbitMQ/Elasticsearch/Elasticsearch/OpenSearch or microservices.
- Name-based identity for persisted Artist equality.
- `unwrap/expect/panic/todo/unimplemented` in production code.
- Destructive database changes without accepted schema ADR + migration + rollback plan.

## Module/file ownership

### Router + compatibility layer

- Route ownership: `apps/melodee-server/src/native/` (new)
- Route registration point: `apps/melodee-server/src/main.rs` and `apps/melodee-server/src/lib.rs` (if new state is introduced)
- Wire DTOs/errors/pagination: `apps/melodee-server/src/native/dto.rs`, `apps/melodee-server/src/native/error.rs`

### Authentication/session/security

- `apps/melodee-server/src/native/auth.rs` and `crates/melodee-auth` (if required)
- Shared auth config and secret validation: `crates/melodee-server`/`crates/melodee-persistence`

### Catalog and media read paths

- Domain model remains `melodee-domain` vocabulary (`Artist`, `Release`, `Track`, etc.).
- Persisted route-facing mapping and query adapters live in `apps/melodee-server/src/native/catalog.rs` and `crates/melodee-persistence` repositories/services.

### Streaming

- Stream authorization tokens and URL lifecycle in `apps/melodee-server/src/native/stream.rs`.
- File IO and path safety checks in the same ownership area.

### Compatibility evidence and tests

- Route matrix/canonical evidence: `design/agents/PHASE-7-ROUTE-MATRIX.json`,
  `compatibility/melodee-api-v1/contracts/route-inventory.json`, and
  `compatibility/melodee-api-v1/contracts/phase7-route-evidence.json`
- Golden/differential tests: `tests/python/test_native_v1_capture.py`, `compatibility/scenarios/*.json`, and route-specific fixtures in `compatibility/melodee-api-v1/golden/*`

## Migration boundaries

- All new persistence migrations are owned by `crates/melodee-persistence` with tests in the same crate.
- No migration is added without:
  - exact SQL in an additive versioned file,
  - up/down evidence in tests,
  - public-ID and identity preservation validation,
  - duplicate/replay failure assertions.

## Compatibility policy and normalization registry

Any final route outcome must be one of:

1. exact implementation with differential evidence,
2. preserved intentional absence with legacy and Rust negative evidence,
3. approved normalization linked to an accepted ADR and differential evidence, or
4. explicit deferral linked to an accepted scope ADR.

ADR-0114 and ADR-0128 authorize only their enumerated compatibility normalizations. ADR-0129
governs secure migrated public-share bearers and resolvable links. No generated matrix status
changes until the route-specific implementation evidence is registered.

## Acceptance matrix (execution)

### Required slice contracts

- Route matrix produced and complete.
- No route leaves scope without explicit disposition.
- Captured native contracts continue to be preserved and regression-gated.
- All native-auth/scrobble/stream fixtures remain reproducible via sanitize-and-compare pipeline.

### Mandatory checks

- Rust: `cargo fmt --all`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo check --workspace --all-features`, `cargo test --workspace --all-features`.
- Repo/doc gates: `python3 scripts/validate_repository.py`, `python3 scripts/extract_requirements.py --check`, `python3 scripts/build_native_inventory.py --check`, `python3 scripts/build_phase7_route_matrix.py --check`.
- `./scripts/check.sh` and `git diff --check`.
- Docs (if docs changed): `cd docs && pnpm install --frozen-lockfile && pnpm check && pnpm check:links && pnpm build`.

### Client validation gates

- Native legacy capture and Rust differential suites for all routes classified as implemented,
  normalized, or preserved-negative.
- Real-client smoke for Android, Android Auto, MeloAmp paths in PRD-STREAM-003 scope once slice is complete.
- Legacy-baseline oracle pinned at `4028b58dbd3302ed431e2c5b32d9dde5634db218`.

### Evidence required before `Ready for independent review`

- Route disposition evidence table per route category and disposition.
- Auth/session failure matrix: credential, replay, family revocation, expired/malformed tokens.
- Stream matrix: range variants, malformed token behavior, HEAD/full GET/content-range/header semantics.
- Migration and ID preservation checks for users/artists/releases/tracks/playlists/share-related public IDs.
- Public docs impact statement.

## Performance/security targets

- Preserve bounded server-side pagination and search-through-Tantivy/query projection patterns (no unbounded scans for listing/search endpoints).
- Stream startup shall avoid catalog-wide scans and large media reads.
- p95 latency/error budgets are recorded per PRD-defined harness and linked to the phase report.
- Auth/session failure/error observability: typed error codes only, secret-safe message content, correlation support.

## Exit criteria for this package

Phase 7 is incomplete while any route is pending. Every compiled legacy route must be implemented
with differential evidence or carry an accepted explicit deferral. Every legacy-excluded route must
have its negative behavior captured and proven against Rust. The Android/Android Auto and MeloAmp
release gates, documentation, traceability, migration checks, and full repository gate must also pass.
