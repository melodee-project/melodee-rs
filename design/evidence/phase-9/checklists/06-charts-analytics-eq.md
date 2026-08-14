# Checklist 06 — Charts, recommendations, analytics, audio features, EQ (WP-04)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Chart ingestion/linkage/browse | PRD-DISCOVERY-001 | ✅ | `native_discovery_phase7.rs`, CLI `charts import` |
| Recommendations (materialized, bounded) | PRD-DISCOVERY-002 | ✅ | ADR-0128; `native_discovery_api.rs` |
| Listening analytics/top content | PRD-DISCOVERY-003, PRD-USER-005 | ✅ | indexed/materialized facts; `native_discovery_api.rs` |
| Audio features/BPM read path | §7 inventory row | ✅ | `native_track_audio_features`; `native_discovery_api.rs` |
| EQ presets + playback settings | §7 inventory row | ✅ | `native/preferences.rs`; community lifecycle goldens |
| Scheduled chart refresh + aggregate maintenance | PRD-WORK-003 | ✅ | `NativeDiscoveryStore::refresh_discovery_aggregates` (`crates/melodee-persistence/src/discovery.rs`); `apps/melodee-server/src/discovery_worker.rs` (`MELODEE__DISCOVERY__WORKER__*`, default 300 s interval, 500-row batches, 100 batches/pass); wired into `main.rs` startup/shutdown. Tests: `native_discovery_phase7.rs::discovery_maintenance_relinks_chart_items_only_on_unique_catalog_identity`; `apps/melodee-server/tests/discovery_worker.rs` (drift/missing-row reconciliation from `native_track_plays`, idempotency, bounded-batch convergence + truncation accounting, periodic run, graceful stop, config parsers) |
| Intentional BPM/feature extraction limits documented | — | ✅ | Limits section below; `docs/src/content/docs/features/discovery-and-playback.md` + `docs/src/content/docs/installation/configuration.md` |

## Intentional limits (WP-04 documentation deliverable)

**Chart-refresh semantics:**

- `native_recommendation_candidates` is trigger-maintained (migration 0019); the scheduled
  maintenance pass is the convergence backstop for drift introduced when triggers could not run
  (backup restore, binary downgrade window). It re-adds missing catalog candidates and recomputes
  popularity from durable `native_track_plays` facts only where the stored value differs.
- Chart-item linkage is retried for **unlinked** rows only, under the same unique-exact-match rule
  as CSV import (exactly one active Release matched on normalized Artist + Release title + optional
  year). Existing links are never rewritten or cleared; ambiguous observations stay unlinked across
  passes until the catalog makes them unique. Read paths additionally filter inactive Releases at
  query time, so a later-deactivated Release needs no unlink pass.
- Every reconciliation statement touches at most `batch_size` rows and commits independently; a
  pass capped by `max_batches_per_pass` reports `truncated` and later passes resume the work.
  Passes are idempotent: a second pass over unchanged facts is a no-op (proven by test).

**BPM / audio-feature extraction limits:**

- Melodee performs **no** media analysis for audio features; `native_track_audio_features` rows are
  populated out-of-band and served as stored facts. Tracks without a stored row return `404` from
  the audio-feature detail route and are absent from tempo-search results.
- Stored tempo is bounded to 0–1000 BPM by `native_track_audio_features_finite`; the other feature
  dimensions carry their own storage bounds (0..1, loudness ±1000, time signature 1..32).
- Tempo search (`audio_by_bpm`) requires a finite non-negative `min`/`max` with `min <= max`
  (`400` otherwise) and is paged with the shared native page bound (default size 50).
