# ADR-0105: Use PostgreSQL as the authoritative mutable state store

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARCH-003, PRD-INGEST-002, PRD-CATALOG-001

## Context

Catalog, identity, user state, policy decisions, and ingestion progress need transactions, durable
leases, indexed queries, and migration from the existing PostgreSQL-backed product.

## Decision

PostgreSQL is authoritative for mutable application state. Filesystems store media/manifests;
Tantivy/caches/generated views are projections. Work-item claiming uses PostgreSQL row locking and
leases. `LISTEN/NOTIFY` may wake workers but never carries the only copy of work.

## Consequences

- PostgreSQL is a required service.
- Schema/migration quality is critical.
- A separate queue broker is unnecessary initially.
