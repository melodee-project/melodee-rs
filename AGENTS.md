# Melodee Rust Rewrite — AI Agent Guide

Mandatory reading before changing this repository. Ground-up Rust replacement of the C#
`melodee-project/melodee` server; the legacy repo is a behavior oracle, not an architecture
template.

## Commands and environment

- **Pre-commit/PR gate: `python3 scripts/do-prechecks.py`** (or `make prechecks`) checks
  unstaged/staged Git diffs, runs the full repository gate and docs-site gate, and stops on
  failure. Requires pnpm, cargo-deny, and media tools rather than allowing local skips.
  Requires development/test PostgreSQL and network access for dependency installs/audits.
  Add `--include-e2e` for Admin UI work (requires Playwright Chromium).
- **Fast gate: `make check-fast`** (`python3 scripts/check.py fast`) gives broad feedback:
  repository/doc and requirement-index validation, Rust formatting, workspace Clippy, and Rust
  library tests. It intentionally excludes PostgreSQL integration, compatibility captures, UI
  static builds, and dependency auditing.
- **Full local gate: `make check`** (`python3 scripts/check.py full`). Runs, in
  order: repository/doc validation → requirements index check → Python tests → admin-ui gate
  (pnpm install --frozen-lockfile, typecheck, lint, i18n check, unit tests, build; skipped
  locally if pnpm is absent) → `cargo fmt --check` → `clippy -D warnings` →
  `cargo test --workspace --all-features` → `cargo deny check` (skipped locally if cargo-deny is
  absent; full CI installs both pnpm and cargo-deny). Run this before considering work done.
  Admin UI work additionally runs Playwright e2e/visual tests via `make admin-ui-e2e`
  (CI: `.github/workflows/admin-ui.yml`).
- **Run `cargo fmt --all` before `make check`.** `fmt --check` is a hard gate and fails on
  non-canonical formatting.
- **Focused runs:** `cargo test -p melodee-domain` for one crate; `make test-unit` (database-free
  Rust library and binary unit tests), `make test-integration` (PostgreSQL-backed integration
  tests), `make lint`, `make test`, `make docs` (doc validation only), `make deny`.
- **Toolchain:** pinned `1.99.0` in `rust-toolchain.toml`. Workspace dependencies use exact `=`
  versions in `Cargo.toml [workspace.dependencies]`; keep versions centralized there.
  `Cargo.lock` is committed — regenerate and commit it with dependency changes.
- **Clippy lint groups need explicit priority:** a bare `all = "warn"` in
  `[workspace.lints.clippy]` errors with `lint_groups_priority`; use
  `all = { level = "warn", priority = -1 }` so individual `deny` lints override the group.
- **Dev Postgres:** `make postgres-up` (`docker compose -f compose.dev.yaml up -d postgres`).
  Host port is **5433**, not 5432 — `DATABASE_URL` in `.env` must match. The `postgres:18+`
  image requires the volume mounted at `/var/lib/postgresql` (not the legacy
  `/var/lib/postgresql/data`).
- **Legacy compatibility oracle:** `python3 scripts/legacy_oracle.py recreate`
  rebuilds the frozen C# baseline (commit `4028b58d...`, fails closed on mismatch) and
  `python3 scripts/legacy_oracle.py capture` regenerates all golden captures. Details:
  `compatibility/README.md`.
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

## Repository automation language — Python is mandatory

- **All new or replacement repository scripts MUST be Python 3.** Do not add executable Bash,
  POSIX-shell, PowerShell, or other platform-specific orchestration scripts. In particular, do
  not add files under `scripts/` with `.sh`, `.bash`, or `.ps1` extensions.
- **Make and CI definitions MUST remain thin entry points.** A recipe or workflow step may invoke
  one external command directly. Any sequencing, branching, validation, retries, cleanup,
  environment construction, or destructive-operation safety logic belongs in an importable,
  tested Python module under `scripts/`.
- Use `pathlib`, `subprocess` with argument lists and `shell=False`, `shutil.which`, `argparse`,
  and explicit working directories. Never depend on Unix-only helpers such as `awk`, `sed`,
  `grep`, `curl`, `jq`, `ss`, or shell pipelines when Python's standard library can perform the
  operation. Never embed a developer-specific absolute path.
- Invoke child Python processes with `sys.executable`. Make targets use the configurable
  `PYTHON` variable (default `python3`) so environments where Python is named `python` can use
  `make PYTHON=python <target>`.
- Every Python entry point MUST provide safe `--help` behavior without contacting external
  services or performing mutations. Destructive commands MUST validate exact targets, fail
  closed on topology drift, and have unit coverage proving their scope.
- A non-Python script is permitted only when Python cannot provide the required platform
  integration. The agent MUST stop, explain the concrete technical limitation, and obtain
  explicit user approval before adding it. Familiarity, brevity, or convenience is not a valid
  exception.

## Authority and instruction precedence

1. Accepted ADRs in `design/adr/`
2. `SPEC.md` technical invariants and interface contracts
3. `PRD.md` product requirements and the active roadmap
4. The assigned phase, issue, or work package
5. Existing implementation patterns in this repository
6. Legacy C# code — evidence of behavior, **not** an architecture template

Do not silently resolve a material conflict; record it in the PR and propose an ADR when needed.

## Coding-agent assignment granularity

The assignment unit and implementation units are different:

- Requests for the next substantive body of work produce a complete work-program assignment. Do not
  reinterpret them as permission to return only the first prerequisite, bug fix, or workstream.
- A program-level prompt delegates the end-to-end outcome: resolve or surface entry decisions,
  decompose and order internal workstreams, implement every in-scope requirement, run applicable
  compatibility and validation gates, update public documentation and traceability, and produce
  closeout evidence.
- Issues and small coherent changes are execution and review units inside a work program. They do
  not reduce the scope of the assigned outcome.
- Return a single-issue prompt only when the user explicitly requests that granularity or an external
  decision prevents the rest of the work from proceeding safely.
- If several work programs are available, recommend the one that best closes current requirement or
  release-readiness gaps and explain its dependencies.

Use [the substantive work prompt](design/prompts/IMPLEMENT-WORK.md) for program-level assignments.
The issue template is reserved for explicitly assigned narrow issues.

## Current project state

The Rust server and Admin UI are functionally feature complete relative to the current C# product,
subject to accepted differences recorded in `PRD.md`. MeloAmp owns
listener-facing web workflows under ADR-0161. The active roadmap is
`PRD.md`.

Non-terminal requirements remaining: **15** (status `in-progress` in
`design/requirements/REQUIREMENTS-INDEX.md`; `implemented`, `tested`, and `deferred` are the
terminal statuses defined by `scripts/audit_requirement_overrides.py`, with `deferred` rows
recorded under accepted ADR-level waivers). The count in this paragraph is enforced by
`tests/python/test_requirements_index.py` so it cannot drift. They concern immutable release
artifacts, authorized migration verification, public-ID and media preservation, performance
and scale acceptance, differential validation, fault recovery, cutover, and rollback. They are
release-readiness and operator-acceptance obligations, not missing runtime workflows.

Allowed without new user direction: corrections to existing evidence and documentation. An explicit
request to implement the next substantive work program authorizes all safe in-repository work needed
for that outcome.

Production rehearsal, private-data access, external credentials, publication, cutover, rollback,
cleanup, and other destructive or external actions still require their documented operator approvals.

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

## UI rules

Carbon Design System is the preferred candidate, pending ADR acceptance. Use Carbon and Melodee
shared components before another UI library; TanStack Table only for a proven Carbon DataTable
gap. All visible text localized; light/dark/system/custom themes and RTL preserved. Use
`Release`/`Track` in admin UI language. Generated mockup text is layout guidance, not factual
data. Destructive actions show impact, reason, and confirmation. Catalog-scale
sorting/filtering/paging is server-side.

## Required workflow for every assigned unit

For a program-level assignment, apply this workflow across every internal workstream and continue
until the complete outcome satisfies its acceptance criteria. Do not stop after the first passing
issue and ask the user to assign the next issue.

1. Read the assigned requirement IDs and linked ADRs.
2. Inspect existing tests and legacy behavior where relevant.
3. State assumptions in the issue/PR before coding when they affect behavior.
4. Add or update tests first when practical.
5. Implement each smallest coherent change needed to complete the assigned unit. This constrains
   implementation and commit boundaries, not the breadth of a phase-level assignment.
6. Run `make check-fast` during iteration and `make check` before merge or
   release (or the documented smallest equivalent for the change).
7. Evaluate user-facing documentation impact and update `/docs` in the same change when
   affected; update internal documentation and traceability.
8. Report: files changed; requirements satisfied; ADRs applied or proposed; tests run and
   results; compatibility impact; data/migration impact; documentation impact (explicit);
   remaining risks.

## Definition of done

`design/agents/DEFINITION-OF-DONE.md`. An issue or phase is not done because one slice compiles —
it is done when all applicable requirements, tests, compatibility, observability, documentation,
operational behavior, and phase exit criteria are addressed.
