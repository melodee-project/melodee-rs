# ADR-0110: Use SQLx and explicit SQL

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-CATALOG-001 through PRD-CATALOG-005

## Context

The rewrite needs PostgreSQL transactions, streaming rows, migrations, compile-time query checks, and
set-based performance without recreating Entity Framework in Rust.

## Decision

Proposed: use SQLx with PostgreSQL and explicit SQL. Domain/application code depends on repository
interfaces, not SQLx types.

## Validation

Prototype work-item claiming, batch manifest import, migration/offline query metadata, and integration
test ergonomics before acceptance.

## Phase 0 ratification (2026-08-09)

- **Status change:** Proposed -> Accepted (Issue 10 ratification authority).
- **SPEC support:** SPEC.md selects "SQL/data access | SQLx | 0.9.0 current stable line |
  MIT/Apache-2.0" and assigns `melodee-persistence` SQLx repositories, migrations, and
  transactional work claims. (PRD.md's reference link pointed at the retired transact-rs org and was
  corrected to https://github.com/launchbadge/sqlx in the same change.)
- **Phase 0 evidence:** benchmark dataset generation proved set-based PostgreSQL inserts at
  24.6-30.6k rows/s through 5M rows (`compatibility/benchmarks/datasets/tier-*.json`); the
  legacy schema's unique constraints and leasing needs are documented (Issue 8/9 evidence).
- **Technology revalidation (2026-08-09):** crates.io — sqlx 0.9.0 (2026-05-21),
  MIT OR Apache-2.0, active. No commercial boundary.
- The ADR's prototype validation (work-item claiming, offline query metadata) is Phase 2
  scope and remains the implementation gate.
