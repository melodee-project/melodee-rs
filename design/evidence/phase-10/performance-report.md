# Phase 10 W4-5 — Performance Evidence: Admin API Pagination and SPA Bundle Budget

- **Date (UTC):** 2026-08-15
- **Commit:** `23b5d6ca1c537709dff917b60b0861388b9f09f0` (plus uncommitted Phase 10 working tree)
- **Scope:** SPEC §23 row "API list endpoints — bounded memory and indexed server-side
  pagination" for the Phase 10 admin list endpoints; admin SPA bundle/boot budget
  (no pre-existing budget was defined; one is defined and measured here).
- **Related requirements/ADRs:** PRD-API-ADMIN-002, PRD-UI-005/006/007/008/010, SPEC §23;
  ADR-0134, ADR-0138, ADR-0139.

## 1. Method

1. Read every admin list endpoint and its persistence query (sources:
   `apps/melodee-server/src/admin.rs`, `admin_access.rs`, `admin_catalog.rs`,
   `admin_review.rs`, `admin_identity.rs`;
   `crates/melodee-persistence/src/work_items.rs`, `user_admin.rs`, `group_admin.rs`,
   `catalog_browse.rs`, `identity_review.rs`).
2. Inventoried every `CREATE INDEX` in `crates/melodee-persistence/migrations/`
   (0001–0028) and mapped each sortable/filterable column to a supporting index.
3. Measured with `EXPLAIN (ANALYZE, BUFFERS)` on the dev PostgreSQL 18.4 container
   (port 5433) against a **throwaway scratch database** `melodee_perf_scratch`
   (created, measured, and dropped; no migration files touched). The full schema was
   applied by running every migration file in order with `psql`, then a synthetic
   catalog-scale dataset was loaded (harness: [perf_seed.sql](perf_seed.sql)):
   200k work items, 100k artists (+5k aliases), 100k releases, 500k tracks,
   500k media assets, 200k observed artists (180k unresolved), 200k match decisions,
   200k match evidence rows, 300k audit events, 5k users.
4. EXPLAIN harness: [perf_explain.sql](perf_explain.sql); raw output:
   [perf_explain_output.txt](perf_explain_output.txt) and
   [perf_explain_output_filters.txt](perf_explain_output_filters.txt).
   Each query mirrors the store SQL verbatim with bind parameters inlined as literals
   (caveat in §6).
5. SPA: `pnpm build` in `admin-ui/` before and after route-level code splitting;
   gzip sizes computed with `gzip -c`. Full post-change build log:
   [spa_build_after.txt](spa_build_after.txt).

**Measurement environment:** PostgreSQL 18.4 (Alpine container, dev Compose),
`shared_buffers=128MB`, `work_mem=4MB`, `max_parallel_workers_per_gather=2`, JIT on;
host 24 cores. Timings are single-run `EXPLAIN ANALYZE` values on a quiet dev box —
comparative evidence of plan shape and memory bounding, not SLA benchmarks.

## 2. Endpoint → sort/filter columns → supporting index

| Endpoint (all `GET /api/admin/v1/…`) | Table | Filter columns | Sort keys (`:asc/:desc`) | Supporting index |
|---|---|---|---|---|
| `ingest/work-items` | `release_work_items` | `state` | `created_at` (default), `updated_at`, `state`, `attempt_count` (+`id` tiebreak) | Filter: `release_work_items_state_idx (state)` ✅. Sorts: **no index on `created_at`/`updated_at`/`attempt_count`**; `(state, …)` composite absent — seq scan + top-N sort ⚠️ |
| `ingest/work-items/counts` | `release_work_items` | — (GROUP BY `state`) | `state` | `release_work_items_state_idx` used for filtered count; GROUP BY is a seq scan (bounded) ⚠️ |
| `ingest/leases` | `release_work_items` | `lease_expires_at > now()` | `lease_expires_at DESC, id` | `release_work_items_lease_expiry_idx (lease_expires_at) WHERE lease_expires_at IS NOT NULL` ✅ index scan |
| `review/queue` | `release_work_items` (+`catalog_imports`) | `state = ANY(review states)` | same as `ingest/work-items` | Same as above for the page; bindings via `catalog_imports.work_item_id UNIQUE` ✅ |
| `users` | `native_users` | ILIKE on `username`, `email` | `normalized_username` (default), `created_at` | `native_users_normalized_username_key` UNIQUE ✅ (index scan + incremental sort); `created_at` unindexed — table is not catalog-scale, acceptable |
| `groups` | `native_library_access_groups` | — | `name, id` | none — tiny table, seq scan acceptable |
| `catalog/artists` | `artists` ⋈ `artist_catalog_aggregates` | ILIKE on `canonical_name`, `sort_name`, alias EXISTS | `sort_name` (default), `created_at`, `updated_at` | ⚠️ **Gap:** the 0018 browse indexes (`artists_native_browse_name_idx (sort_name, public_api_id)`, `…_created_idx`) and the trigram GIN `artists_native_search_name_idx` are **partial** (`WHERE status NOT IN ('merged','deprecated')`) and cannot serve the admin listing, which deliberately includes merged/deprecated rows. `sort_name` has no trigram index at all. Alias EXISTS uses full GIN `artist_aliases_native_search_alias_idx` ✅. Aggregates join on PK ✅ |
| `catalog/releases` | `releases` ⋈ `release_catalog_aggregates` | ILIKE `title`; `artistId` EXISTS | `sort_title` (default), `release_date`, `created_at`, `updated_at` | ⚠️ Same partial-index gap: `releases_native_browse_*` and `releases_native_search_title_idx` are `WHERE status = 'active'`. `artistId` filter has `release_artists_artist_idx (artist_id, release_id)` ✅ but the EXISTS-via-`artists`-join query shape does not drive from it (§4 finding R3) |
| `catalog/tracks` | `tracks` ⋈ `releases` | ILIKE `title`; `releaseId` equality | `sort_title` (default), `duration_ms`, `created_at` | Title ✅ `tracks_native_browse_title_idx (sort_title, public_api_id)` + full trigram `tracks_native_search_title_idx`; createdAt ✅ `tracks_native_browse_created_idx`; releaseId ✅ `tracks_release_order_idx (release_id, ordinal, id)`; **`duration_ms` unindexed** ⚠️ |
| `identity/unresolved` | `observed_artists` ⋈ anti-join `artist_match_current` | NOT EXISTS (no authoritative decision) | `created_at` (default), `display_value` | ⚠️ **Gap:** no index on `observed_artists (created_at)` or `(display_value)`; anti-join probe uses `artist_match_current` PK ✅; per-row count subplans use `artist_match_decisions (id, observed_artist_id)` UNIQUE (skip-scan, ~4 ms/row) and `artist_match_evidence_observation_idx` ✅ |
| `identity/decisions` | `catalog_audit_events` | `entity_type IN ('observed_artist','artist')` | `created_at DESC, id DESC` | `catalog_audit_events_entity_idx (entity_type, entity_id, created_at, id)` — leading `entity_type` serves the filter but not global `created_at` order; seq scan + top-N (bounded) ⚠️ |

## 3. EXPLAIN (ANALYZE, BUFFERS) results at synthetic catalog scale

Single-run timings; all plans shown in the raw output files. "top-N" = top-N heapsort,
whose memory is bounded by `LIMIT`, not table size.

| # | Query (endpoint / shape) | Plan summary | Exec time | Sort memory |
|---|---|---|---|---|
| W1 | work-items, `state='needs_review'`, `created_at DESC`, page 1 | Bitmap on `state_idx` → top-N | 14.7 ms | 45 kB |
| W1b | same, `OFFSET 10000` (deep page) | Parallel bitmap → sort, offset skip | 17.1 ms | 2.7 MB (full filtered set) |
| W2 | work-items, no filter, `updated_at DESC` | Parallel **seq scan** → top-N | 27.5 ms | 45 kB |
| W3 | work-items, no filter, `attempt_count DESC` | Parallel **seq scan** → top-N | 26.7 ms | 45 kB |
| W4 | work-items, `state='failed'`, `state ASC, created_at DESC` | Bitmap on `state_idx` → top-N | 7.3 ms | 45 kB |
| W5 | review queue, 3-state ANY, `created_at DESC` | Bitmap on `state_idx` → top-N | 16.0 ms | 37 kB |
| W6 | work-items filtered `COUNT(*)` | Bitmap on `state_idx` | 6.7 ms | — |
| W7 | `counts_by_state` GROUP BY | Parallel seq scan → sort of 18 groups | 32.2 ms | 25 kB |
| W8 | active leases | **Index scan backward** on lease-expiry partial index | 0.09 ms | 25 kB |
| W9 | users, ILIKE filter, `normalized_username ASC` | **Index scan** on UNIQUE + incremental sort | 0.15 ms | 33 kB |
| W10 | users, no filter, `created_at DESC` | Seq scan (5k rows) → top-N | 1.5 ms | 37 kB |
| A1 | artists, no filter, `sort_name ASC` | Parallel **seq scan** + hash join aggregates → top-N | 63.2 ms | 40 kB |
| A2 | artists, ILIKE `%artist 99%`, `sort_name ASC` | PK scan + ILIKE filter (partial trigram unusable), hashed alias subplan | **342.3 ms** | 41 kB |
| A3 | artists, no filter, `updated_at DESC` | Parallel **seq scan** + hash join → top-N | 77.1 ms | 40 kB |
| R1 | releases, no filter, `sort_title ASC` | Parallel **seq scan** + hash join → top-N | 81.7 ms | 39 kB |
| R2 | releases, ILIKE filter, `sort_title ASC` | Parallel **seq scan** (partial trigram unusable) → quicksort of matches | 49.2 ms | 105 kB |
| R3 | releases, `artistId` EXISTS, `sort_title ASC` | Releases PK scan with per-row EXISTS probes (planner does not drive from `release_artists_artist_idx`) | 201.5 ms | 25 kB |
| R4 | releases, no filter, `release_date DESC NULLS LAST` | Parallel **seq scan** + hash join → top-N | 67.9 ms | 37 kB |
| T1 | tracks, no filter, `sort_title ASC` | **Index scan** on `tracks_native_browse_title_idx` + incremental sort (tiebreak `id` differs from index column `public_api_id`; ties resolved incrementally) | 1.2 ms | 33 kB |
| T2 | tracks, no filter, `duration_ms DESC NULLS LAST` | Parallel **seq scan** (500k rows) + hash join → top-N | 153.3 ms | 45 kB |
| T3 | tracks, no filter, `created_at DESC` | **Index scan backward** on `tracks_native_browse_created_idx` + incremental sort | 3.2 ms | 41 kB |
| T4 | tracks, ILIKE `%track 5000%`, `sort_title ASC` | **Bitmap on trigram GIN** `tracks_native_search_title_idx` → nested-loop release join | 15.9 ms | 39 kB |
| T5 | tracks, `releaseId` filter, `sort_title ASC` | `releases_public_api_id_key` → **`tracks_release_order_idx`** | 0.13 ms | 26 kB |
| I1 | identity unresolved, `created_at ASC` | Parallel **seq scan** of 200k + disk-spilling external merge + anti-join + per-row count subplans | **325.4 ms** | external merge, 12.6 MB disk (bounded by `work_mem`) |
| I2 | identity unresolved, `display_value DESC` | same shape | 364.9 ms | 10.6 MB disk |
| I3 | identity unresolved `COUNT(*)` | Parallel hash anti-join | 45.5 ms | — |
| I4 | identity decisions feed | Parallel **seq scan** of 300k audit rows → top-N | 47.3 ms | 37 kB |
| G1 | groups list | Seq scan, subplans never executed (0 rows) | 0.08 ms | 25 kB |

### SPEC §23 verdict

- **Bounded memory: holds.** Every unindexed sort is a top-N heapsort bounded by
  `LIMIT` (≤ 500 rows, clamped server-side), an incremental sort over an index scan,
  or an external merge capped by `work_mem` and spilled to disk (I1/I2). No plan
  materializes an unbounded result in memory; page sizes are clamped at the API layer
  (`MAX_LIST_LIMIT = 500`) and row DTOs are explicit and bounded.
- **Indexed server-side pagination: partially holds.** The default work-item/review
  sorts, track title/createdAt/releaseId paths, user listing, and leases are
  index-backed. The gaps below are not.

## 4. Index gaps and findings (no schema changed — reported for review)

Per the W4-5 constraint, **no migration was added**. Each gap below is a schema-change
candidate requiring review (and an ADR per the database rules); they are listed in
priority order with measured impact at synthetic scale.

1. **Artist ILIKE filter on the admin listing (A2, 342 ms @ 100k artists; scales
   linearly).** Both trigram GIN indexes on `artists` are partial
   (`status NOT IN ('merged','deprecated')`) and the admin listing has no status
   predicate; `sort_name` has no trigram index at all. Candidate: full (non-partial)
   trigram GIN indexes on `artists (canonical_name)` and `artists (sort_name)`, plus
   full btree `artists (sort_name, id)` and `artists (updated_at, id)` for the
   unfiltered sorts (A1 63 ms, A3 77 ms). Trade-off needing review: GIN trigram
   indexes carry real write amplification on an ingest-written table.
2. **Identity unresolved queue sorts (I1/I2, 325–365 ms @ 200k observations, disk
   spill).** No index on `observed_artists (created_at, id)` or `(display_value, id)`.
   Candidate: `observed_artists (created_at, id)` — cheap, single-purpose, and this
   queue is the primary identity-review hot path. Also consider a plain
   `artist_match_decisions (observed_artist_id, outcome)` index for the
   `pending_match_count` subplan (currently a ~4 ms skip-scan per returned row).
3. **Track `duration_ms` sort (T2, 153 ms @ 500k tracks; ~3 s at the SPEC §23 10M
   track scale).** Candidate: `tracks (duration_ms, id)`. Note `tracks_native_random_candidate_idx ON tracks (id)` exists but serves a different path.
4. **Release listing sorts (R1/R2/R4, 49–82 ms @ 100k releases).** Same partial-index
   gap as artists: full `releases (sort_title, id)`, `(release_date, id)`,
   `(created_at, id)`, `(updated_at, id)` btrees, and a full trigram GIN on
   `releases (title)` if ILIKE browse must stay index-backed for deprecated rows.
5. **Work-item unfiltered sorts (W2/W3, 27 ms @ 200k items) and `counts_by_state`
   (W7, 32 ms).** Memory-bounded top-N today; a `release_work_items (created_at, id)`
   index would also make the default sort fully index-ordered and deep paging cheaper
   (W1b already acceptable at 17 ms). Lowest priority: work-item volume is bounded by
   ingest throughput, not catalog size.
6. **Release `artistId` filter query shape (R3, 201 ms; rewrite measured at 156 ms
   incl. 124 ms JIT).** Not an index gap — `release_artists_artist_idx` exists — but
   the `EXISTS (… JOIN artists billed ON … AND billed.public_api_id = $2)` shape
   prevents the planner from driving off it. A scalar subselect
   (`billing.artist_id = (SELECT id FROM artists WHERE public_api_id = $2)`) lets
   PostgreSQL hash the indexed id set once instead of probing per release row.
   Recommendation only; a code change to `catalog_browse.rs` was out of W4-5 scope.

## 5. Admin SPA bundle budget

No bundle budget existed; this report defines and measures one:

| Asset class | Budget | Before | After code splitting | Verdict |
|---|---|---|---|---|
| Initial JS (entry + statically preloaded chunks), gzip | ≤ 200 kB | 240.9 kB single bundle (889.6 kB raw) | **172.3 kB** total (entry chunk 71.4 kB + eager shared chunks: i18n 33.4 kB, PageHeader/shared Carbon 51.7 kB, shell pieces ≤ 7 kB each) | ✅ (was over budget) |
| Initial CSS, gzip | ≤ 150 kB | 100.8 kB (1,002.8 kB raw) | 100.8 kB (unchanged) | ✅ |
| Locale chunks (lazy), gzip each | ≤ 10 kB | 1.07–1.45 kB | 1.07–1.45 kB | ✅ |
| Lazy screen chunks, gzip each | (no budget; observed) | — | 1.5–21.5 kB (largest: `UsersScreen` 21.5 kB, `SettingsScreen` 13.9 kB) | informational |

**Change applied (in scope per W4-5):** the single statically-imported 889.6 kB bundle
exceeded a defensible budget, so route-level code splitting was applied in
`admin-ui/src/App.tsx` — every authenticated screen is now `React.lazy`; `LoginScreen`
(the public entry route) and the shell stay eager. The `Suspense` boundary sits around
`<Outlet/>` inside `AppShell` (`admin-ui/src/shell/AppShell.tsx`) with the localized
`common.loading` fallback, so a chunk fetch never unmounts the shell chrome. Vite
(rolldown) automatically split shared Carbon component code into per-screen and shared
chunks. No route, DTO, or behavior changed; this is invisible to users except faster
first paint.

**Notes:** CSS ships unminified by deliberate `vite.config.ts` choice (lightningcss
rejects Carbon's `@position-try`); it is still within budget. The 90 IBM Plex woff2
assets are referenced via `@font-face` with unicode-range and are fetched lazily by
the browser, so they do not count against initial JS/CSS; a font-subset strategy is a
possible future optimization, not a current breach.

## 6. Limitations

- **Synthetic data distribution is uniform.** Real catalogs are skewed (popular
  artists on many releases, bursty work-item states). Plan shapes that depend on
  selectivity (R3, bitmap vs seq choices) may differ; the seq-scan findings are
  distribution-independent because the measured queries have no usable index at all.
- **Literal-inlined binds.** sqlx uses the extended protocol, so the app sees
  parameter-driven plans; where it mattered (R3/T5) filters were re-measured with
  literal UUIDs standing in for bound parameters.
- **Single-run timings** on a quiet dev container; treated as plan-shape and
  memory-bound evidence, not latency SLAs. No p95 was computed.
- **Scale ceiling:** 500k tracks vs the SPEC §23 10M-track reference point. Unindexed
  sorts scale linearly (top-N heapsort over a full scan), so T2 ≈ 153 ms implies ≈ 3 s
  at 10M — recorded as reasoning, not measurement. Index-backed paths (T1/T3/T5)
  scale logarithmically in the leading column.
- **SPA budget** measures transfer size, not runtime boot timing (no TTI/paint
  measurement; the Playwright rig exists but visual/boot timing was out of W4-5
  scope).
- The scratch database `melodee_perf_scratch` is disposable; the harness scripts
  above recreate it end to end.

## 7. Files changed

- `design/evidence/phase-10/performance-report.md` (this report, new)
- `design/evidence/phase-10/perf_seed.sql` (new — scratch-DB seed harness)
- `design/evidence/phase-10/perf_explain.sql` (new — EXPLAIN harness)
- `design/evidence/phase-10/perf_explain_output.txt`,
  `perf_explain_output_filters.txt` (new — raw plan output)
- `design/evidence/phase-10/spa_build_after.txt` (new — post-split build log)
- `admin-ui/src/App.tsx` (route-level `React.lazy` code splitting)
- `admin-ui/src/shell/AppShell.tsx` (`Suspense` boundary around `<Outlet/>`)
- `admin-ui/e2e/mockApi.ts` (removed pre-existing unused `SUBSYSTEM` const that failed
  `pnpm check`; unrelated to the budget change)
- `admin-ui/e2e/debug-locale.spec.ts` (typed a pre-existing `any` that failed
  `pnpm lint`; unrelated to the budget change)

No Rust files, no migrations, no frozen API surfaces, no `docs/` (public), and no
checklist files were modified.

Addendum (§11, ADR-0142 remediation package):

- `crates/melodee-persistence/migrations/0029_phase10_admin_browse_indexes.sql` (new)
- `design/adr/ADR-0142-admin-browse-index-remediation.md` (new)
- `crates/melodee-persistence/tests/migrations.rs` (index data-integrity test)
- `design/evidence/phase-10/perf_explain_output_after_indexes.txt`,
  `perf_explain_output_filters_after_indexes.txt` (new — raw after-run plan output)
- this report (§10/§11)

Still no changes to frozen API surfaces, `admin-ui/`, or `docs/` (public docs do not
document indexes).

## 8. Validation

- `admin-ui`: `pnpm check` ✅, `pnpm lint` ✅ (0 errors; 2 pre-existing fast-refresh
  warnings in `AuthContext.tsx`/`ThemeProvider.tsx`), `pnpm test` ✅ (36 files,
  196 tests), `pnpm i18n:check` ✅, `pnpm build` ✅.
- Rust: untouched — cargo gates not applicable.
- `python3 scripts/validate_repository.py` ✅ (run after this report was added).

## 9. Documentation impact

- None for `/docs` (public): the code splitting is user-invisible (identical routes
  and behavior; only chunk loading changed), and no public documented behavior
  changed. Public API documentation already describes the server-side
  pagination/sort parameters measured here; this report adds performance evidence,
  not new behavior. Internal evidence added under `design/evidence/phase-10/`.

## 10. Remaining risks

- The §4 index gaps will degrade linearly with catalog growth; the identity queue
  (I1/I2) and artist ILIKE browse (A2) are the first paths likely to breach
  interactive latency at 10× scale. A schema ADR/work package covering the §4
  candidate indexes is recommended before Phase 10 exit. **Resolved in the §11
  addendum (ADR-0142) for every gap except the artist ILIKE query shape.**
- JIT overhead (~124 ms) dominated the rewritten R3 variant measurement; the app
  does not disable JIT, so both shapes were reported with JIT on.

## 11. Addendum (2026-08-15): ADR-0142 admin browse index remediation — before/after

The §4 index remediation landed as migration `0029_phase10_admin_browse_indexes.sql`
under ADR-0142. Same scratch database, dataset, hardware, and harness as §1/§3
(schema rebuilt from migrations 0001–0028, seeded with `perf_seed.sql`, then 0029
applied and `ANALYZE`d; full harness re-run). Raw after-run output:
[perf_explain_output_after_indexes.txt](perf_explain_output_after_indexes.txt) and
[perf_explain_output_filters_after_indexes.txt](perf_explain_output_filters_after_indexes.txt).

| # | Query | Before (§3) | After | Change |
|---|---|---|---|---|
| W1 | work-items, state filter, `created_at DESC` | 14.7 ms | 2.8 ms | bitmap → index-ordered |
| W1b | same, deep page `OFFSET 10000` | 17.1 ms | 21.3 ms | unchanged shape (bitmap + sort); single-run noise |
| W2/W3 | work-items, unfiltered `updated_at`/`attempt_count` | 27.5 / 26.7 ms | 30.3 / 27.9 ms | unchanged (no index added; bounded top-N, ADR-0142 follow-up) |
| W4 | work-items, state filter + state sort | 7.3 ms | 0.7 ms | index-ordered |
| W5 | review queue, 3-state ANY, `created_at DESC` | 16.0 ms | 0.2 ms | index-ordered |
| A1 | artists, `sort_name ASC` | 63.2 ms | 0.3 ms | index scan on `artists_admin_browse_name_idx` |
| A2 | artists, ILIKE + `sort_name ASC` | 342.3 ms | 115.5 ms | improved (ordered index scan + early-out after 50 matches); **ILIKE filter still per-row — not trigram-servable, see finding 1** |
| A3 | artists, `updated_at DESC` | 77.1 ms | 9.8 ms | index scan on `artists_admin_browse_updated_idx` |
| R1 | releases, `sort_title ASC` | 81.7 ms | 0.3 ms | index scan on `releases_admin_browse_title_idx` |
| R2 | releases, ILIKE + `sort_title ASC` | 49.2 ms | 11.1 ms | bitmap on full `releases_admin_search_title_idx` GIN |
| R3 | releases, `artistId` EXISTS | 201.5 ms (literal: 201.5 ms) | 27.0 ms (literal: 30.0 ms) | improved via `releases_admin_browse_title_idx` ordering; EXISTS-rewrite stays a follow-up |
| R4 | releases, `release_date DESC NULLS LAST` | 67.9 ms | 0.3 ms | index-only scan on `releases_admin_browse_date_idx` |
| T1 | tracks, `sort_title ASC` | 1.2 ms | 1.2 ms | unchanged |
| T2 | tracks, `duration_ms DESC NULLS LAST` | 153.3 ms | 1.1 ms | index-only scan on `tracks_admin_browse_duration_idx` |
| T3/T4 | tracks, `created_at DESC` / ILIKE | 3.2 / 15.9 ms | 4.9 / 18.6 ms | unchanged within noise |
| T5 | tracks, `releaseId` filter | 0.13 ms (literal-uuid) | 0.11 ms (literal-uuid) | unchanged; the parameterized InitPlan variant was ~215–227 ms **in both runs** — pre-existing planner mis-estimate of the inline-literal InitPlan shape, not caused by this migration |
| I1 | identity queue, `created_at ASC` | 325.4 ms (disk spill) | 3.5 ms | index-driven anti-join + `artist_match_decisions_observed_outcome_idx` subplan |
| I2 | identity queue, `display_value DESC` | 364.9 ms (disk spill) | 1.9 ms | same |
| I3/I4 | identity count / decisions feed | 45.5 / 47.3 ms | 37.0 / 36.0 ms | unchanged within noise |
| W6–W10, G1 | counts, leases, users, groups | — | — | unchanged within noise |

Two measurements during remediation narrowed the §4 candidate set (recorded in ADR-0142):

1. **Full artist trigram GINs are never selected for the artist listing's filter.**
   The store filter is `canonical_name ILIKE $1 OR sort_name ILIKE $1 OR EXISTS
   (artist_aliases …)`; an OR whose arms are not all index scans cannot become a
   BitmapOr, so with both full GINs present the planner still seq-scanned `artists`
   (207 ms, unchanged). Those indexes were dropped from the migration; A2's full
   remediation requires flattening the OR/EXISTS shape (a `catalog_browse.rs` change,
   grouped with the R3 EXISTS-rewrite follow-up).
2. **`NULLS LAST` needs a matching index ordering.** A plain `tracks (duration_ms, id)`
   btree did not serve `ORDER BY duration_ms DESC NULLS LAST` (seq scan + top-N
   remained at 122 ms); `(duration_ms DESC NULLS LAST, id)` serves it as a 0.1 ms
   index-only scan. The two nullable sort columns (`tracks.duration_ms`,
   `releases.release_date`) are indexed `DESC NULLS LAST` (the measured and
   operator-common direction); their ASC NULLS LAST variants keep the previous
   LIMIT-bounded top-N fallback. One btree cannot serve both NULLS LAST directions,
   and dual-direction indexes were rejected for size/write cost at the 10M-track
   SPEC §23 reference point.

Additionally, §4 item 2's "also consider" index `artist_match_decisions
(observed_artist_id, outcome)` was promoted into the migration: once I1/I2's sorts
became index-driven, the `pending_match_count` subplan dominated at 4.07 ms × 50
rows (~204 ms of the 242 ms page); the dedicated index drops it to 0.027 ms/row.

**SPEC §23 verdict after remediation:** bounded memory holds (unchanged — no sort
above grew; the I1/I2 disk spills are eliminated) and indexed server-side
pagination now holds for every admin list endpoint except the artist ILIKE filter
shape (finding 1) and the deliberately-deferred W2/W3 sorts, both documented
follow-ups in ADR-0142.
