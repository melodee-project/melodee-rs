# Checklist 02 — Playlists, smart playlists, shares (WP-01)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Native playlist CRUD/order/visibility/import/image | PRD-PLAYLIST-001/003 | ✅ | `native_playlists.rs`; migration 0016 |
| Smart playlists (MQL) CRUD + evaluate | PRD-PLAYLIST-002 | ✅ | `native_playlists.rs` smart-playlist tests; ADR-0128 |
| OpenSubsonic playlist group | PRD-PLAYLIST-001 | ✅ | Phase 8 matrix + differential replay |
| Jellyfin playlist CRUD/items/move | PRD-PLAYLIST-001 | ✅ | Phase 8 matrix; D4 clean-add deviation (ADR-0128) |
| Cross-API ID stability for shared playlists | PRD-PLAYLIST-001/003 | ✅ | `cross_api_playlist_ids.rs::native_created_playlist_keeps_one_identity_across_all_three_surfaces`: one `native_playlists.public_api_id`, three lossless encodings (native dashed UUID, OS `playlist_{uuid}`, JF N-format); pins the captured OS entry-ordering divergence (creation order vs position order) |
| Shares CRUD + public access | PRD-SHARE-001/002 | ✅ | `native_community_api.rs`; ADR-0129 |
| Share IDs/URLs survive migration | PRD-SHARE-003 | ✅ | `native_community_phase7.rs` replay-safe import |
| OpenSubsonic share group | PRD-SHARE-001 | ✅ | Phase 8 differential replay |
| Public share assembly at scale | PRD-SHARE-002, PRD-PERF-006 | ✅ | `native/community.rs`: per-Track sequential DTO loads replaced by one set-based statement per Release/Playlist (`PublicTrackRow`), response shape byte-identical; `native_community_api.rs::public_share_track_assembly_is_set_based_with_identical_shape` (sequential-reference equality, literal pin, EXPLAIN index pin, 120-item scale leg). Bound unchanged: `MAX_NATIVE_PLAYLIST_ITEMS` (10,000) enforced at write time. Note: no statement-counting harness exists in the repo (`catalog_performance.rs` counts via store-returned fields), so the constant-statement bound is structural + EXPLAIN evidence |
