# Melodee Rust Rewrite — AI Agent Guide

Mandatory reading before changing this repository. Ground-up Rust replacement of the C#
`melodee-project/melodee` server; the legacy repo is a behavior oracle, not an architecture
template.

## Commands and environment

- **Local gate: `./scripts/check.sh`** (also `make check`). Runs, in order: repository/doc
  validation → requirements index check → Python tests → `cargo fmt --check` →
  `clippy -D warnings` → `cargo test --workspace --all-features` → `cargo deny check` (skipped
  locally if cargo-deny is absent; CI installs it). Run this before considering work done.
- **Run `cargo fmt --all` before `check.sh`.** `fmt --check` is a hard gate and fails on
  non-canonical formatting.
- **Focused runs:** `cargo test -p melodee-domain` for one crate; `make lint`, `make test`,
  `make docs` (doc validation only), `make deny`.
- **Toolchain:** pinned `1.97.1` in `rust-toolchain.toml`. Workspace dependencies use exact `=`
  versions in `Cargo.toml [workspace.dependencies]`; keep versions centralized there.
  `Cargo.lock` is committed — regenerate and commit it with dependency changes.
- **Clippy lint groups need explicit priority:** a bare `all = "warn"` in
  `[workspace.lints.clippy]` errors with `lint_groups_priority`; use
  `all = { level = "warn", priority = -1 }` so individual `deny` lints override the group.
- **Dev Postgres:** `make postgres-up` (`docker compose -f compose.dev.yaml up -d postgres`).
  Host port is **5433**, not 5432 — `DATABASE_URL` in `.env` must match. The `postgres:18+`
  image requires the volume mounted at `/var/lib/postgresql` (not the legacy
  `/var/lib/postgresql/data`).
- **Legacy oracle (Phase 0 compatibility):** `python3 scripts/legacy_oracle.py recreate`
  rebuilds the frozen C# baseline (commit `4028b58d...`, fails closed on mismatch) and
  `python3 scripts/legacy_oracle.py capture` regenerates all golden captures. Details:
  `design/evidence/phase-0/legacy-baseline.md` and `compatibility/README.md`.
- **cargo-deny** warnings like `license-not-encountered` (allowed licenses with no current
  dependent crate) are benign; errors are not.
- **Doc validation is executable.** `python3 scripts/validate_repository.py` enforces required
  files, markdown link integrity, ADR status lines, and mockup PNG dimensions — doc edits can
  fail CI. It skips vendored/generated trees and the Starlight content routes under
  `docs/src/content/`; docs-site links are enforced by the docs CI (`pnpm check:links` plus
  build-time link validation in `.github/workflows/docs.yml`). Docs validation commands:
  `cd docs && pnpm install --frozen-lockfile && pnpm check && pnpm check:links && pnpm build`.
  `python3 scripts/extract_requirements.py --check` fails when PRD.md requirement IDs
  drift from `design/requirements/`; regenerate by running it without `--check` and committing
  the updated index.

## Authority and instruction precedence

1. Accepted ADRs in `design/adr/`
2. `SPEC.md` technical invariants and interface contracts
3. `PRD.md` product requirements
4. `IMPLEMENTATION.md` phase scope and exit criteria
5. The assigned issue/work package
6. Existing implementation patterns in this repository
7. Legacy C# code — evidence of behavior, **not** an architecture template

Do not silently resolve a material conflict; record it in the PR and propose an ADR when needed.

## Current phase boundary: Phase 6 — Search and MQL

Phases 0–5 are complete. The authoritative Phase 5 exit decision is in
`design/evidence/phase-5/PHASE-5-REPORT.md`. Phase 6 is the next implementation boundary, but agents
must not create its search schema, projection consumer, or MQL behavior until an assigned work
package and accepted schema/behavior ADR authorize it.

Allowed when explicitly assigned: Phase 6 Tantivy schema and lifecycle, transactional-outbox
consumer/checkpoints, full rebuild and incremental catch-up, Artist/Release/Track search documents,
MQL parser/AST/translators, autocomplete/admin lookup, scale/performance evidence,
tests/evidence/traceability, CI, and documentation.

Forbidden: Phase 7+ podcast, party, native API,
OpenSubsonic, Jellyfin, or UI implementation; redesigning captured external contracts; any database
schema without an accepted schema ADR; message brokers, Redis, Elasticsearch/OpenSearch, GraphQL,
microservices; copying C# service/class boundaries into Rust; destructive operations against an
existing Melodee library outside the durable Phase 5 decision/audit/authorization contract.

## Product language

Core domain uses `Artist`, `Release`, `Track`, `ReleaseArtist`, `TrackArtist`, `Contribution`,
`MediaAsset`, `ObservedArtist`, `ArtistMatch`, `ReleaseTrait`, `Disposition`. Never introduce
`Album` or `Song` into core domain modules — those words are valid only in legacy compatibility
adapters, captured contracts, migration code, and docs describing legacy behavior. A string is
never an Artist identity; it is an observation or display value resolving to zero, one, or
multiple Melodee-owned Artist identities.

## Compatibility rules

Native Melodee API v1, OpenSubsonic, and Jellyfin compatibility layers are separate products and
must not share wire DTOs merely because underlying data overlaps. Never change any existing
external route, JSON/XML property, identifier, status code, error shape, auth flow, pagination,
range behavior, or intentional `410`/`501` behavior without: (1) a compatibility test showing
current behavior, (2) a product decision, (3) an accepted ADR, (4) an explicit migration/client
plan. Public IDs from the C# database are migration data — never regenerate them casually.

## Ground-up rewrite rule

Do not translate legacy classes line-for-line. Frame work as "implement `PRD-XXX-NNN` according
to `SPEC.md`, using the legacy service and test corpus as behavioral evidence." When consulting
legacy code, document: the user-visible behavior preserved, the tests/routes proving it, any
known bug that must **not** be preserved, and the new requirement/ADR governing the Rust
implementation.

## Architecture and dependency rules

- Modular monolith with explicit compile-time boundaries; the domain crate
  (`crates/melodee-domain`) stays free of web, SQL, filesystem, and framework dependencies.
- PostgreSQL is the proposed authoritative store; Tantivy is a rebuildable projection.
- Filesystem paths never determine semantic identity; watchers are accelerators, never truth.
- Dependencies must pass the FOSS gate (`PRD.md`, `design/requirements/TECHNOLOGY-BASELINE.md`);
  a new production dependency requires an ADR or documented dependency decision in the PR.
  No proprietary SDKs, hosted-only services, or "Pro" UI features. Commit lockfiles.

## Rust coding standards

- Edition 2024; `unsafe` forbidden unless an accepted ADR names the exact boundary and audit
  strategy.
- No `unwrap`, `expect`, `panic!`, `todo!`, or `unimplemented!` in production code (clippy
  enforces most of these at `deny`).
- Typed errors at domain/application boundaries, preserving root causes; `tracing` spans/events
  across async or worker boundaries; bounded concurrency with explicit backpressure;
  cancellation propagated through long-running operations.
- Comments explain **why**, invariants, and non-obvious safety constraints. Public modules/types
  get concise rustdoc when the contract is not self-evident.

## Database and data-safety rules

No schema change without a migration, rollback/forward plan, and data-integrity tests. No
destructive migration without backup/restore docs and an accepted ADR. Work-item claiming is
lease-based and safe across processes. DB writes and outbox/index events commit atomically where
the spec requires. Prefer set-based SQL and bounded batches. Never use normalized names as
unique identity keys. Never delete source media because classification is uncertain.

## Testing requirements

Every behavioral change needs tests at the lowest useful layer: unit tests for pure domain
invariants; property tests for state machines/parsers; PostgreSQL integration tests;
differential compatibility tests against the C# server; golden request/response fixtures for
public APIs; media-corpus tests for real file/tag pathologies; fault-injection tests for
crash/retry/idempotency; performance tests for documented budgets; Playwright/visual tests in
the admin UI phase. A test that only proves the implementation calls itself is not useful.

## Documentation and ADR rules

Write an ADR when a decision is hard to reverse, spans multiple modules, is security/storage/
compatibility/migration-sensitive, adds a runtime dependency or infrastructure service, or is
likely to be debated later. Create it with `python3 scripts/new_adr.py "short title"`. Never
edit history to hide an old decision — supersede it. Update requirement traceability whenever a
requirement becomes implemented, tested, deferred, or superseded (and re-run
`extract_requirements.py` when PRD requirement IDs change).

## User-facing documentation is part of the product

Public documentation lives in `/docs` (Astro Starlight, deployed to GitHub Pages). Internal
engineering documentation lives in `/design`; compatibility evidence in `/compatibility`. Keep
these boundaries strict.

For every behavioral or user-visible change, evaluate documentation impact. If documentation is
affected, update `/docs` **in the same change** — do not file a documentation TODO to claim a
feature is complete. A feature is not complete merely because code and tests pass when its
public documentation is missing or incorrect.

Every implementation report must explicitly state:

```text
Documentation impact:
- None — with reason
or
- Updated — list files
```

A bare `None` without a reason is not sufficient for user-visible work. Read
`/docs/AGENTS.md` whenever you modify files under `/docs` or introduce user-visible behavior.
Public documentation must only describe implemented behavior; planned capabilities are marked
as planned.

## UI rules (future admin phase)

Carbon Design System is the preferred candidate, pending ADR acceptance. Use Carbon and Melodee
shared components before another UI library; TanStack Table only for a proven Carbon DataTable
gap. All visible text localized; light/dark/system/custom themes and RTL preserved. Use
`Release`/`Track` in admin UI language. Generated mockup text is layout guidance, not factual
data. Destructive actions show impact, reason, and confirmation. Catalog-scale
sorting/filtering/paging is server-side.

## Required workflow for every task

1. Read the assigned requirement IDs and linked ADRs.
2. Inspect existing tests and legacy behavior where relevant.
3. State assumptions in the issue/PR before coding when they affect behavior.
4. Add or update tests first when practical.
5. Implement the smallest coherent change.
6. Run `./scripts/check.sh` (or the documented smallest equivalent).
7. Evaluate user-facing documentation impact and update `/docs` in the same change when
   affected; update internal documentation and traceability.
8. Report: files changed; requirements satisfied; ADRs applied or proposed; tests run and
   results; compatibility impact; data/migration impact; documentation impact (explicit);
   remaining risks.

## Definition of done

`design/agents/DEFINITION-OF-DONE.md`. A task is not done because code compiles — it is done
when requirements, tests, compatibility, observability, documentation, and operational behavior
are addressed at the level appropriate to the phase.
