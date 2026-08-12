# Changelog

All notable changes to the Rust rewrite repository will be documented here.

The project is pre-release. Compatibility and migration guarantees apply only when explicitly stated
by an accepted ADR and release gate.

## Unreleased

### Added

- Initial repository scaffold.
- Product requirements, technical specification, implementation plan, ADR program, and agent guide.
- Minimal Rust server, CLI, and domain workspace.
- Phase 0 compatibility-capture and repository-validation scaffolding.
- Phase 1 Rust foundation: the Axum/Tokio composition root, PostgreSQL pool and migration runner,
  structured tracing and graceful shutdown, health/system information routes, administrative CLI,
  and dependency-policy gates.
- Phase 2 durable ingestion engine: PostgreSQL-backed per-Release work items, leases, retries,
  transition history, reconciliation and settling, failure isolation, operator CLI commands, and
  the Rust-native Admin diagnostics API.
- Phase 3.1 media inspection with Lofty/ffprobe observations, streaming byte hashes, and durable,
  schema-versioned Release manifests.
- Phase 3.2 bounded FFmpeg normalization, output verification, artwork normalization, semantic
  sidecar parsing, Chromaprint observations, and the leased `Normalizing` to `Validating` handoff.
- Phase 3.3/3.4 structural validation and manifest v3, durable publication authorization/plan/
  journal/receipt evidence, immutable UUIDv7 Release IDs, exact hash-derived canonical layout,
  verified no-clobber atomic publication, exact-final retry adoption, and retained inbound sources.
- Independently bounded validation/publication workers, canonical-publication CLI diagnostics, and
  an additive PostgreSQL-backed `/readyz` readiness probe.
- Additive canonical-publication migration `0006` and the ADR-0116 `rustix` dependency decision for
  safe Unix no-follow/no-replace filesystem operations.
- Phase 4 PostgreSQL catalog schema and bounded manifest-v3 catalog importer, including independent
  worker configuration and graceful shutdown plus `catalog import`, `catalog status`, and
  `catalog inspect` operator commands. Import reuses the reserved Release ID and does not reread
  media. A bounded deterministic local Artist resolver distinguishes provisional, proposed, and
  confirmable evidence without string-only confirmation. A provider-neutral Artist evidence
  interface retains provisional identities on provider failure and cannot outrank locked manual or
  negative decisions. The complete Phase 4 acceptance decision and exact 200-Release metrics are in
  `design/evidence/phase-4/PHASE-4-REPORT.md`.
- Phase 5 explainable multi-valued Release classification, complete Track novelty across exact,
  known, alternate, new, and uncertain outcomes, immutable versioned policy rules, bounded catalog
  dry-run, independently leased policy workers, durable facts/decision/audit state, explicit
  administrator overrides, and a separate destructive-completion authorization boundary. The
  worker and CLI consume PostgreSQL facts without reopening media; no deletion implementation was
  added. The complete acceptance decision and exact 200-Release metrics are in
  `design/evidence/phase-5/PHASE-5-REPORT.md`.
- Phase 6 pure bounded MQL parsing/planning and rebuildable Tantivy catalog search for Artists,
  Releases, Tracks, aliases, genres, display credits, and ID-backed participant roles. PostgreSQL
  outbox leases fence multi-process writers; generation rebuilds stream bounded catalog pages,
  validate/catch up, and activate atomically without reading media. Added search worker readiness,
  status/rebuild/query/autocomplete/parse/explain CLI operations, compatibility/fault/path tests,
  bounded retry escalation and poison-row isolation, sealed snapshot plus live diagnostic counts,
  and a reproducible 10-million-Track benchmark harness with a default-gate smoke test.
- Canonical Carbon admin UI reference mockups.
- Astro Starlight public documentation platform under `/docs`, with GitHub Pages deployment,
  PR validation, and documentation-impact rules in the Definition of Done.
