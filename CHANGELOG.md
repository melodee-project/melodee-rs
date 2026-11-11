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
  verified no-clobber atomic publication, exact-final retry adoption, and retained durable staged
  sources (ADR-0145: inbound candidates are drained after their verified staging receipts).
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
  negative decisions. The complete Phase 4 acceptance decision and exact 200-Release metrics were
  recorded in `design/evidence/phase-4/PHASE-4-REPORT.md` (now a withdrawn evidence report).
- Phase 5 explainable multi-valued Release classification, complete Track novelty across exact,
  known, alternate, new, and uncertain outcomes, immutable versioned policy rules, bounded catalog
  dry-run, independently leased policy workers, durable facts/decision/audit state, explicit
  administrator overrides, and a separate destructive-completion authorization boundary. The
  worker and CLI consume PostgreSQL facts without reopening media; no deletion implementation was
  added. The complete acceptance decision and exact 200-Release metrics were recorded in
  `design/evidence/phase-5/PHASE-5-REPORT.md` (now a withdrawn evidence report).
- Phase 6 pure bounded MQL parsing/planning and rebuildable Tantivy catalog search for Artists,
  Releases, Tracks, aliases, genres, display credits, and ID-backed participant roles. PostgreSQL
  outbox leases fence multi-process writers; generation rebuilds stream bounded catalog pages,
  validate/catch up, and activate atomically without reading media. Added search worker readiness,
  status/rebuild/query/autocomplete/parse/explain CLI operations, compatibility/fault/path tests,
  bounded retry escalation and poison-row isolation, sealed snapshot plus live diagnostic counts,
  and a reproducible 10-million-Track benchmark harness with a default-gate smoke test.
- Phase 7 native Melodee API v1 compatibility: identity-preserving authentication and user state,
  catalog browse/search/images/direct streaming, playlists and bounded smart playlists, scrobbling,
  requests, shares, podcasts, discovery/analytics/recommendations, playback endpoints, and exact
  negative behavior for the frozen Party/Queue exclusions. The 202-route evidence matrix contains
  150 exact implementations, 17 accepted compatibility normalizations, and 35 preserved absences,
  with no pending or deferred route. Additive migrations `0010` through `0019`, bounded workers,
  library ACL/rate/secret/path safety, current-catalog performance evidence, and Android,
  Android Auto-mode, and MeloAmp real-client evidence accompany the adapter.
- Canonical Carbon admin UI reference mockups.
- Astro Starlight public documentation platform under `/docs`, with GitHub Pages deployment,
  PR validation, and documentation-impact rules in the Definition of Done.

### Settings, diagnostics, and curation parity (pre-v1.0.0 correction set)

- Registered-but-unhonored settings now have runtime consumers: password-reset token lifetime
  and the `security.blacklistedEmails`/`.blacklistedIPs` deny lists (native auth + authenticated
  native requests, `403 BLACKLISTED`), `streaming.maxConcurrentStreams.global`/`.perUser`
  (settings-driven admission on native/OpenSubsonic/Jellyfin, legacy `429` rejection for the
  per-user cap), `scripting.enabled` (ingestion-gate master switch, default disabled),
  the three legacy podcast retention policies plus enclosure/download-concurrency/HTTP-timeout
  bounds, and `system.maxUploadSize` for playlist artwork uploads.
- New administrator-gated troubleshooting API: `GET /api/admin/v1/diagnostics/{selftest,settings,
  events,bundle}` with pass/warn/fail checks, per-setting consumer/inert classification, a bounded
  recent WARN/ERROR ring buffer with correlation ids, and a redacted support bundle.
- New admin chart curation API (`/api/admin/v1/charts*`): create/update/delete charts, replace
  ranked items, resolve items to catalog Artists/Releases, and ranked/missing reports.
- `settings_inventory.rs` classifies all 209 registry keys as consumed or deliberately inert, with a
  test preventing drift from the registry.
- Structured JSON logging (`MELODEE__LOGGING__FORMAT=json`), request completion logs (normalized
  route, status, latency, correlation id; WARN for 5xx/429/slow), and a client-contract route guard
  covering every route the shipped Android Auto and MeloAmp clients call.
- ADR-0182 (library administration is configured roots plus ACL scopes) and ADR-0183 (bounded
  direct-play admission replaces the legacy unlimited streaming default).
- Parity-matrix corrections for metadata transforms, template/theme import, custom-block
  retirement, library administration, and chart curation, with a 2026-11-09 reconciliation
  addendum.

### Admin UI reference alignment (ADR-0184)

- The Admin UI now implements the visual language of the eight canonical screens in
  `design/ui/reference/`: a monogram-tile brand block, one inset header search entry, locale and
  username labels in the header, a dark rail with the reference palette, flat carded surfaces,
  tinted status chips, filter pills, stage progress bars, entity hero cards, stat strips,
  underlined tabs, evidence tiles, and a timeline primitive.
- A Melodee surface and primitive layer (`Surface`, `HeroCard`, `StatStrip`, `MetricCard`,
  `StatusChip`, `ProgressBar`, `FilterPills`, `KeyValueList`, `EvidenceTileGrid`, `TimelineList`,
  `DecisionActionList`, `TabNav`, `EntityTable` skin) replaces per-screen styling. UI-SPEC §2 now
  permits component CSS only through that layer, and the theme is split into token, surface,
  shell, and layout stylesheets.
- Screen composition follows the references: Overview gains KPI delta lines, a two-series activity
  chart, severity attention rows, and a Release-level queue preview; Processing gains filter pills,
  server-side queue search, and Release/Artist/Progress/Size columns with a carded inspector;
  Release Review, Identity Review, Artist Detail, Release Detail, Policies, and System → Workers
  are recomposed with the reference card order, including measured server uptime.
- Additive admin-API fields: `DashboardDto.deltas` and `activity.indexedPerHour`; work-item
  `releaseTitle`, `artistName`, `releaseId`, and `mediaBytes` from the import binding; and a
  `query` filter on the work-item listing. No existing field is removed or retyped and no
  compatibility route changes. Mockup affordances with no data source (queue priority, worker
  mean processing time, catalog week-deltas) are documented as intentionally absent.
- `design/adr/ADR-0184-admin-ui-reference-alignment-and-surface-system.md`,
  `design/ui/REDESIGN-PLAN.md`, and amended `UI-SPEC.md` / `DESIGN-SYSTEM.md` / `MOCKUPS.md`.

### Security

- `rustix` 1.1.6 (was 1.1.5) for RUSTSEC-2026-0486: the extended-attribute functions marked
  unwritten bytes as initialized for a zero-length buffer. Melodee does not call the xattr
  helpers, but the workspace pins exact versions and the advisory fails the dependency gate.

### Operational (R6: Hardening & Closure)

- `melodee doctor --deep`: extended health probe with PostgreSQL connectivity + SELECT 1
  verification, migration currency check (applied vs. available), catalog entity counts sanity
  check, Tantivy search index generation diagnostic, and bounded readiness probe. The original
  `melodee doctor` (basic mode) is unchanged — media tools and configuration checks only.
- `melodee rollback status`: inspects the current cutover state — whether a migration run exists
  in a terminal state, its freeze evidence, PostgreSQL reachability, and migration currency. Outputs
  machine-readable JSON plus human-readable readiness assessment. Does not modify system state.
- `melodee rollback runbook --confirm`: generates a five-step rollback procedure as JSON: verify
  prerequisites, stop v1.0 server, revert reverse-proxy routing, verify legacy responsiveness,
  record audit evidence. The tool documents infrastructure steps; the operator executes them
  manually. Idempotent and never destructive.
- `design/runbooks/CUTOVER-ROLLBACK.md`: cutover and rollback runbook with pre/post checks, full
  rollback procedure with manual infrastructure steps, and design decisions documenting why the
  rollback tool is guidance-only (never modifies proxy config or reverses imports).
