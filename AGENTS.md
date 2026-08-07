# Melodee Rust Rewrite — AI Agent Guide

This file is mandatory reading for every coding agent before changing the repository.

## 1. Authority and instruction precedence

When instructions conflict, follow this order:

1. Accepted ADRs in `design/adr/`.
2. `SPEC.md` technical invariants and interface contracts.
3. `PRD.md` product requirements.
4. `IMPLEMENTATION.md` phase scope and exit criteria.
5. The assigned issue/work package.
6. Existing implementation patterns in this repository.
7. Legacy C# code, which is evidence of behavior but **not** an architecture template.

Do not silently resolve a material conflict. Record it in the PR and propose an ADR when needed.

## 2. Current phase boundary

The repository starts in **Phase 0 — Baseline freeze and compatibility capture**.

During Phase 0, agents MAY:

- Capture OpenAPI documents, routes, request/response fixtures, and behavior snapshots from the
  existing C# server.
- Build compatibility harness infrastructure.
- Add synthetic baseline generators and benchmarks.
- Improve repository validation, CI, and documentation.
- Implement the minimal server/CLI foundation already represented by the scaffold.
- Ratify or revise proposed ADRs with evidence.

During Phase 0, agents MUST NOT:

- Implement broad catalog, ingestion, search, podcast, party, or UI features.
- redesign Melodee API v1, OpenSubsonic, or Jellyfin contracts;
- introduce a database schema beyond a separately accepted schema ADR;
- initialize the React admin UI unless explicitly assigned;
- introduce a message broker, Redis, Elasticsearch/OpenSearch, GraphQL, or microservices;
- copy C# service/class boundaries into Rust;
- perform destructive operations against an existing Melodee library.

## 3. Product language

The Rust core uses:

- `Artist`
- `Release`
- `Track`
- `ReleaseArtist`
- `TrackArtist`
- `Contribution`
- `MediaAsset`
- `ObservedArtist`
- `ArtistMatch`
- `ReleaseTrait`
- `Disposition`

Do not introduce `Album` or `Song` into core domain modules. Those words are valid only inside
legacy compatibility adapters, captured contracts, migration code, and documentation that describes
legacy behavior.

A string is never an Artist identity. A string is an observation or display value that resolves to
zero, one, or multiple Melodee-owned Artist identities.

## 4. Compatibility rules

The native Melodee API v1, OpenSubsonic, and Jellyfin compatibility layers are separate products.
They must not share wire DTOs merely because the underlying data overlaps.

Never change any existing external route, JSON/XML property, identifier, status code, error shape,
authentication flow, pagination behavior, range behavior, or intentional `410`/`501` behavior without:

1. a compatibility test showing current behavior;
2. a product decision;
3. an accepted ADR; and
4. an explicit migration/client plan.

Public IDs from the C# database are migration data and must not be regenerated casually.

## 5. Ground-up rewrite rule

Do not translate legacy classes line-for-line.

Bad assignment framing:

> Convert `DirectoryProcessorToStagingService.cs` to Rust.

Correct assignment framing:

> Implement `PRD-INGEST-004` settling detection according to `SPEC.md`, using the legacy service and
> test corpus only as behavioral evidence.

When consulting legacy code, document:

- the user-visible behavior being preserved;
- the existing tests or routes that prove it;
- any known bug that should **not** be preserved; and
- the new requirement/ADR that governs the Rust implementation.

## 6. Architecture and dependency rules

- Prefer a modular monolith and explicit compile-time boundaries.
- Keep the domain crate free of web, SQL, filesystem, and framework dependencies.
- PostgreSQL is the proposed authoritative state store; Tantivy is a rebuildable projection.
- Filesystem paths never determine semantic identity.
- Watchers are accelerators, never the source of truth.
- All required dependencies must pass the FOSS gate in `PRD.md` and
  `design/requirements/TECHNOLOGY-BASELINE.md`.
- A new production dependency requires an ADR or a documented dependency decision in the PR.
- Never add a dependency solely to save a few lines of standard-library code.
- Do not add proprietary SDKs, hosted-only services, or “Pro” UI features.
- Commit lockfiles and keep workspace dependencies centralized.

## 7. Rust coding standards

- Rust Edition 2024.
- `unsafe` is forbidden unless an accepted ADR identifies the exact boundary and audit strategy.
- Do not use `unwrap`, `expect`, `panic!`, `todo!`, or `unimplemented!` in production code.
- Use typed errors at domain/application boundaries; preserve root causes for diagnostics.
- Use `tracing` spans/events for operations that cross async or worker boundaries.
- Use bounded concurrency and explicit backpressure.
- Cancellation must be propagated through long-running operations.
- Keep functions small enough to make state transitions and failure behavior obvious.
- Comments explain **why**, invariants, and non-obvious safety constraints—not what readable code does.
- Public modules and types need concise rustdoc when their contract is not self-evident.

## 8. Database and data-safety rules

- No schema change without a migration, rollback/forward plan, and data-integrity tests.
- No destructive migration without explicit backup/restore documentation and an accepted ADR.
- Work-item claiming must be lease-based and safe across processes.
- Database writes and outbox/index events must commit atomically where required by the specification.
- Prefer set-based SQL and bounded batches over per-row query loops.
- Never use normalized names as unique identity keys.
- Never delete source media because classification is uncertain.

## 9. Testing requirements

Every behavioral change requires tests at the lowest useful layer.

Required test categories include:

- unit tests for pure domain invariants;
- property tests where state transitions or parsers benefit from generated inputs;
- integration tests for PostgreSQL behavior;
- differential compatibility tests against the C# server;
- golden request/response fixtures for public APIs;
- media-corpus tests for real file/tag pathologies;
- fault-injection tests for crash/retry/idempotency behavior;
- performance tests for documented budgets; and
- Playwright/visual tests when the admin UI phase begins.

A test that only proves the implementation calls itself is not useful.

## 10. Documentation and ADR rules

Write an ADR when a decision is:

- difficult to reverse;
- likely to affect multiple modules;
- security-, storage-, compatibility-, or migration-sensitive;
- a new runtime dependency or infrastructure service; or
- likely to be debated later.

Use `python3 scripts/new-adr.py "short title"` to create the next ADR.
Never edit history to make an old decision appear never to have existed; supersede it.

Update requirement traceability whenever a requirement becomes implemented, tested, deferred, or
superseded.

## 11. UI rules for the future admin phase

- Carbon Design System is the preferred candidate, pending final ADR acceptance.
- Use Carbon components and Melodee shared components before introducing another UI library.
- TanStack Table is optional and must solve a proven gap in Carbon DataTable.
- All visible text must be localized.
- Light, dark, system appearance, custom themes, and existing RTL behavior must be preserved.
- Use `Release` and `Track` in new admin UI language.
- Do not treat generated mockup text as factual data; mockups communicate layout and hierarchy.
- Destructive actions must show impact, reason, and confirmation.
- Catalog-scale sorting/filtering/paging is server-side.

## 12. Required workflow for every task

1. Read the assigned requirement IDs and linked ADRs.
2. Inspect existing tests and legacy behavior where relevant.
3. State assumptions in the issue/PR before coding when they affect behavior.
4. Add or update tests first when practical.
5. Implement the smallest coherent change.
6. Run `./scripts/check.sh` or the documented smallest equivalent.
7. Update documentation and traceability.
8. Report:
   - files changed;
   - requirements satisfied;
   - ADRs applied or proposed;
   - tests run and results;
   - compatibility impact;
   - data/migration impact; and
   - remaining risks.

## 13. Definition of done

The shared Definition of Done is in `design/agents/DEFINITION-OF-DONE.md`. A task is not done because
code compiles. It is done when requirements, tests, compatibility, observability, documentation, and
operational behavior are addressed at the level appropriate to the phase.
