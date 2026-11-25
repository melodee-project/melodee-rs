# Changelog

All notable changes to the Rust rewrite repository will be documented here.

The project is pre-release. Compatibility and migration guarantees apply only when explicitly stated
by an accepted ADR and release gate.

## Unreleased

## 0.2.1 — 2026-11-24

Interrupted Release inspections now recover automatically after their worker lease expires.
The retry verifies the staged original, including a same-filesystem move interrupted before its
receipt was saved. The upgrade uses the existing database schema and staging directories.

## 0.2.0 — 2026-11-24

The Admin UI shows the running server version in the navigation footer. Artist and Release
artwork uploads accept WebP and refresh the primary image after upload. The legacy user-state
importer can match accounts by email and Tracks by exact Artist, Release, and Track titles.
Rerunning the import as the library grows picks up newly matched preferences.

For source installations upgrading from the November 24 `e35ca9f` build, run the usual
updater. This release adds no database migration beyond that revision.

### Fixed

- Artist and Release artwork uploads accept WebP and convert it to JPEG. The detail header
  and Images tab both refresh after an upload, even when a browser has cached the previous image.

- Ingestion stages no longer throttle a bulk import to their poll interval. Every stage loop
  (inspection, normalization, validation, publication, catalog import, policy) now runs another pass
  immediately when the previous pass claimed its full batch, so the poll interval is an idle cadence
  rather than a throughput ceiling. Measured on a 643-Release policy backlog: **4 Releases/minute
  before, entire backlog drained in under 30 seconds after** — the policy stage was claiming four
  Releases per 60-second tick while doing ~80 ms of work per pass.

- Artist status means something again. Every imported Artist started `provisional`
  (ADR-0117's rule for identities created from observed credit text) and **nothing
  ever promoted one**, so a healthy catalogue showed 100 % provisional — a per-row
  "pending" chip on every Artist, which reads as hundreds of problems and is why the
  Artists list looked alarming. Confirming a match to an existing Artist in Identity
  Review now promotes that Artist to `verified` in the same transaction (the
  `create-new` resolution deliberately does not: it asserts a distinct Artist, not a
  known identity). In the UI, `provisional` renders as muted text rather than a status
  chip on the list, and the Artist detail explains the lifecycle.
- `completed_hourly` tests now assert the zero-filled hour spine introduced with the
  activity chart, instead of an empty series for an idle window.

- The Admin UI visual gate is stable again. The density fix (compact rows now use
  Carbon's `xs` size) changed the rendered rows of the queue and worker pools, so the
  `system-workers` baselines were regenerated; and the narrow mobile baselines — which
  are almost entirely text — now allow up to 3 % pixel drift for font rasterisation
  differences between CI and a developer machine, which is what made
  `processing-queue [mobile-light]` fail on CI while passing locally. Desktop baselines
  keep the 1 % tolerance.

- Admin API timestamps are RFC 3339 strings. `OffsetDateTime`'s default serialization is a
  nine-element array, so every `createdAt`/`updatedAt`/`hour` field the Admin UI rendered was an
  `Invalid Date` (blank cells) and the Overview activity chart sorted its axis by `NaN`. All 40
  admin response timestamps now serialize as `2026-11-13T03:23:10.304Z`.
- The Overview "Processing activity" chart is readable. It plotted only the hours that had activity
  (two distant samples joined by one long diagonal), had no scale and no time axis, and its x values
  came from those array timestamps. The trailing window is now a zero-filled hourly spine (~24
  points per series), and the chart draws a value axis (peak, midpoint, zero) and hour ticks.

- The Processing Queue density control now changes the table. It switched Carbon's
  `sm`/`md` sizes, which render identically for a plain table (both use the same
  cell padding and row height), so `?density=compact` and `?density=default` looked
  the same. Compact now uses Carbon's dense (`xs`) size, matching the 2rem
  `--melodee-density-compact` token and UI-SPEC §4: rows drop from 50px to 41px with
  2px cell padding.

- MeloAmp parity defects found by driving the client against a populated instance
  (`design/evidence/meloamp-parity/MELOAMP-VERIFY.md`): a known Artist without
  artwork served an empty `200` instead of the captured default image;
  `/user/last-played` reported a play count with an empty `data` array, so a real
  listening history looked empty; chart lists lacked the `name`/`trackCount` keys a
  chart-list client reads; the equalizer list lacked the `data` alias envelope
  readers expect; `analytics/top/{period}` rejected plural `type` values; a scrobble
  with a future client timestamp failed a database check and surfaced as `500`; and
  `/system/stats` answered `404` where the frozen capability filter answers `403`.
  Every native `5xx` now logs its cause instead of only the status.
- A tagged release date now reaches the Release. The media observer hard-coded the release
  observation's date to `None`, so `date`/`year` tags were recorded on every Track and then dropped:
  imported Releases had neither `release_date` nor `release_date_text` and only a sidecar could set
  the text. The observer now picks the date tag carried by the most Tracks (ties break deterministically,
  so identical media seals identical manifests) and the importer stores the typed date plus the source
  text. Found by the known-answer corpus, which had been recording it as a known gap.
- Cover art that was published is now actually served. `/images/{id}/original` — the untransformed
  artwork URL an operator or client naturally asks for — was not a recognized size, so the endpoint
  answered `200` with an empty body and never performed a lookup. `original` (and `source`) now serve
  the stored bytes, and the resolver logs which source produced a response.
- Provider artwork no longer outranks the published cover. The image endpoint consulted
  `catalog_provider_images` before the Release's publication plan (contrary to ADR-0143 §2) and logged
  it as an operator override, so an enrichment guess could replace a cover that was imported with the
  Release. Precedence is now override → published plan → provider → placeholder. ADR-0143 carries a
  dated clarification; both behaviours are asserted by the known-answer corpus.

- Artwork reads no longer fail under a normal catalogue browse. The image pipeline refused
  (`503`) every request beyond four in flight instead of queueing, so a release list requesting one
  cover per row rendered mostly initials (measured: 20 of 24 concurrent reads refused). Reads now
  wait for a bounded moment for a decode permit, `original`/`source` requests need no decode permit
  at all, and uploads keep the immediate refusal they can retry.
- Acoustic fingerprints are no longer computed and thrown away. Normalization has always run `fpcalc`
  and retained the Chromaprint observation in the sealed manifest, but nothing promoted it to durable
  identity evidence, so every Track's novelty stayed `uncertain` and a copy whose bytes differed only
  by tags could not be recognised. Imports now record one `acoustic_fingerprint` evidence row per
  Track (BLAKE3 digest as the comparison key, raw fingerprint untouched in the manifest), and novelty
  reports a Track whose digest already exists in another Release as a known recording. Identity
  confirmation semantics are unchanged: a locally computed fingerprint is evidence, not a verdict
  (ADR-0186, migration 0043).

### Added

- A **Stats** page (`/admin/system/stats`) with library, playback, and database statistics:
  catalog totals, media bytes, library duration, lossless share, storage by format, releases by
  decade, largest releases, play counts, total play time, the two-week plays-per-day chart, top
  artists and tracks, database size and connections, the dominant tables, and freshness counters.
  Backed by a new bounded `GET /api/admin/v1/stats` aggregate (fixed `GROUP BY`/`LIMIT` buckets, a
  zero-filled day spine, sizes for a fixed relation list) and charts that follow the existing
  library-free, screen-reader-accessible pattern.

- `scripts/verify_melodee_player_api.py` (`make verify-android-client`) verifies the
  native-v1 contract the Melodee Android / Android Auto client depends on: the 13
  requests its Retrofit interfaces declare, the non-null fields of its Kotlin models,
  the access-token refresh lifecycle, paging/ordering/artist filtering, `Range`
  playback and the app's exact scrobble payload. 46 checks, run against both an
  administrator and a listener-only account.
- `scripts/seed_meloamp_parity_data.py` populates a local instance with the data a
  client parity pass needs (favourites, play history, playlists and a smart playlist,
  a chart, a request, a share, an equalizer preset) through the public API, idempotently.
- Artwork derivatives are cached in-process, keyed by the immutable publication hash,
  so repeat views and ETag revalidations no longer decode and resize the source image;
  the cache is bounded by bytes and entries. The dev stack accepts
  `MELODEE_DEV_PROFILE=release`, because an unoptimised build makes a 110 ms cover take
  3.4 s and reads as an application defect during client testing.

- Known-answer corpus and the four-phase audit that goes with it
  (`design/testing/UI-AND-WORKFLOW-AUDIT.md`, `design/evidence/known-corpus/`):
  `tests/fixtures/media-test-metadata.json` declares eleven fabricated Releases (tags, durations,
  artwork, sidecars, fault cases), `scripts/generate_known_corpus.py` materialises the media plus an
  expected-outcome oracle with ffmpeg, `scripts/verify_known_corpus.py` ingests them and asserts the
  oracle across PostgreSQL, the filesystem, the search projection and OpenSubsonic, and two live
  Playwright suites assert every view and every edit control against the same oracle. `make corpus`,
  `make corpus-verify`.
- `History` now defaults to `indexed` (the successful terminal state) instead of alphabetically
  landing on `cataloged`, which made a full history look empty.

- Runtime audit tooling that closes the seam the mocked e2e suite could not see
  (`design/testing/UI-AND-WORKFLOW-AUDIT.md`): `make audit` runs read-only invariants against a live
  database (workflow completion, `indexed` requiring a durable projection, client visibility,
  statistic drill-down, closed dictionaries, projection freshness), and `make audit-live` drives a
  real instance and administrator session with no API mocks to prove every canonical screen loads
  cleanly and every edit form round-trips. Both encode the defect classes found in this cycle —
  unreachable terminal states, dead form fields, and counts without rows.

- Provider identity candidates are visible in the Admin UI. The enrichment worker has always written
  provider matches (`provider_identity_candidates`) and provider artwork, but nothing read them from
  an operator surface, so MusicBrainz/Deezer/iTunes IDs were invisible. Artist detail now returns a
  bounded `providerCandidates` section and the Identity tab lists provider, external ID, the query it
  matched, confidence, and observation time — clearly labelled as evidence that does not create
  external identities or authorize a merge.

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

### Added

- Artists can be marked **Verified** by an administrator (`PUT …/catalog/artists/{id}/metadata` with
  `status`, exposed as *Edit → Identity status*). Import always creates `provisional` and no code
  promoted it, so in practice every Artist stayed Provisional forever; the chip in the UI promised a
  state nothing could reach. The edit is revision-bumped and audited, `merged`/`deprecated` remain
  system-managed and are rejected, and provider-confirmed promotion remains future work.

### Fixed

- Catalog edit forms no longer offer free text where a dictionary exists, and no longer offer fields
  the API does not accept. Artist **Type** is now a select over the eight `ArtistType` values (the
  store already rejected anything else with `400`, so typing produced a failed save). Release edit
  dropped **Country** and **Type**, which the W0 contract removed when the corresponding columns were
  never added — the form sent values the server silently discarded. **Release date** is validated as
  `YYYY-MM-DD` inline, and Track edit gained the **Display artist** field the patch already supported.

- Artist statistics are now drillable. The summary numbers (track credits, appearances) are counts
  over `artist_track_associations`, while the detail tabs read `contributions` — so an Artist could
  show "1 Appearances" with an empty tab. Artist detail now exposes a `trackCredits` section from the
  same association rows the counts aggregate over, the Admin UI adds a **Tracks** tab listing Track,
  Release, billing role and credit kind, and both stat cards link into it. The "Billed Tracks" label
  became "Track credits", because the underlying count includes every role, not only billing.
- The Artist Identity tab now flags a name that contains a collaboration separator (`&`, `,`, `;`,
  `feat.`, `ft.`, ` x `) and links to Identity Review. Melodee still does not auto-split those
  separators — they occur inside real band names — so the combined credit stays one provisional
  identity until an operator resolves it.

### Changed

- Operator-facing phases over the ingestion state machine (ADR-0185). The fourteen lifecycle states
  are unchanged (each is a claim partition with its own fencing and evidence), but the Admin UI now
  filters and labels by eight phases — `inbound`, `processing`, `publishing`, `catalog`, **`active`**,
  `review`, `disposition`, `failed` — with the precise state kept as the row detail. `active` is the
  operator-facing completion signal: it is set by the policy `accept` commit, which is exactly when
  client surfaces can see and play the Release; the search projection runs inside that phase and
  never gates playback. The work-item listing accepts an additive `phase` filter and returns a
  derived `phase`, `clientVisible`, and `releaseStatus` on every item.
- Documented that a Release is *playable* at policy `Accept` and *searchable* at `indexed`, and added
  throughput-tuning guidance (concurrency is the throughput lever, not the number of stages).

### Fixed

- Releases no longer park forever in `indexing`. `Indexing -> Indexed` is the terminal hop of the
  Release lifecycle (`SPEC.md` §11.2, `PRD.md`), and no code performed it: accepted Releases were
  neither claimable nor terminal, so they never completed, never appeared in the terminal-state
  History view, and kept counting as active processing. The search worker — which owns the only
  durable evidence that indexing finished — now completes a Release once the projection outbox has
  no pending row for its Release, Tracks, or billed Artists, recording the same `transitioned` and
  `completed` events a worker transition would (`WorkItemStore::complete_indexed`).
- `activity.indexedPerHour` now measures Tracks indexed per hour from the search projection outbox
  (rows are marked processed only after the Tantivy commit that makes the document queryable)
  instead of counting transitions into a state nothing reached.

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
