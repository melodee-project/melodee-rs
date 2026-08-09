# ADR-0107: Use Tantivy as a rebuildable search projection

- **Date:** 2026-08-07
- **Status:** Accepted
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

## Phase 0 ratification (2026-08-09)

- **Status change:** Proposed -> Accepted (Issue 10 ratification authority).
- **PRD support:** PRD-SEARCH-001: "Use a rebuildable embedded full-text search projection.
  Tantivy is the preferred FOSS candidate." PRD-SEARCH-006 sets the warm p95 < 50 ms target on
  a 10M-Track catalog.
- **Phase 0 evidence:** legacy search measured ~1000 ms warm for broad/no-result query classes
  at ALL tiers and ~1020-1080 ms across all classes at 5M Tracks
  (`compatibility/benchmarks/results/tier-c-*.json`), ~20x over the PRD target — the embedded
  rebuildable projection is justified by measurement, not preference.
- **Technology revalidation (2026-08-09):** crates.io — tantivy 0.26.1 (2026-04-21), MIT,
  actively maintained (quickwit-oss). No commercial feature boundary.
- The ADR's 10M-Track Tantivy benchmark remains the Phase 5 exit criterion (validates the
  implementation, not the decision).
