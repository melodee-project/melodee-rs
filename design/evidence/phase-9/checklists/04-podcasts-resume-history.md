# Checklist 04 — Podcasts, resume, history (WP-02)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Feature toggle + role gating | PRD-POD-001 | ✅ | `native_podcasts_api.rs` |
| Channel CRUD/refresh/episodes/downloads/retention | PRD-POD-002 | ✅ | `native_podcasts_api.rs`, `native_podcasts_phase7.rs` |
| Resume/bookmark/history | PRD-POD-002 | ✅ | OS bookmarks/play-queue (Phase 8); JF PlayedItems/Progress |
| OPML import/export, discovery/trending | PRD-POD-002 | ✅ | `native-podcasts/` goldens + tests |
| Native + OpenSubsonic compatibility | PRD-POD-003 | ✅ | Phase 7 native + Phase 8 OS legs (promoted W0-1) |
| Stuck-download/orphan recovery CLI | PRD-POD-002 | ✅ | `melodee podcasts stuck|recover` in `apps/melodee-cli/src/main.rs` reuses `podcast_worker::list_stuck_downloads`/`recover_stuck_downloads` (expired-lease predicate identical to `claim_download`; live leases never stolen) and `reconcile_owned_files` behind `require_owned_root` (PRD-CLI-002); `apps/melodee-cli/tests/podcasts.rs::podcasts_recovery_requeues_only_expired_leases_and_removes_only_orphans` |
| Real external feed exercise | PRD-POD-002 | ✅ | `apps/melodee-server/tests/native_podcasts_real_feed.rs::real_itunes_feed_drives_refresh_download_and_playback_state_end_to_end` — local self-contained iTunes-namespaced RSS 2.0 fixture (3 episodes, CDATA, `itunes:duration`/`summary`, `atom:link`, `content:encoded`) served over loopback HTTP through the real refresh + leased download pipeline; byte-identical enclosure storage, exact pubDate/duration rows, re-refresh idempotency, and play-history/bookmark progress rows verified |
