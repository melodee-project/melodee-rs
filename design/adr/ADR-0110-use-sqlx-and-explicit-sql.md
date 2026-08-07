# ADR-0110: Use SQLx and explicit SQL

- **Date:** 2026-08-07
- **Status:** Proposed
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
