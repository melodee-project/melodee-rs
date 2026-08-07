# ADR-0107: Use Tantivy as a rebuildable search projection

- **Date:** 2026-08-07
- **Status:** Proposed
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SEARCH-001 through PRD-SEARCH-007

## Context

Melodee needs very fast full-text search over current and future multi-million-Track catalogs without
operating Elasticsearch/OpenSearch.

## Decision

Proposed: embed Tantivy as a local index fed by a transactional PostgreSQL outbox. PostgreSQL remains
truth. The index is disposable and fully rebuildable.

## Validation

Run the 10-million-Track benchmark, measure index size/build time/query latency, validate multilingual
tokenization needs, and test corruption/rebuild recovery before acceptance.
