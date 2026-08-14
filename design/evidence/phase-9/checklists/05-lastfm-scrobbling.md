# Checklist 05 — Last.fm and scrobbling (WP-07)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Native/OS/JF scrobble + now-playing writes | PRD-USER-003 | ✅ | Phase 7/8 evidence; ADR-0128 transactional visibility |
| Last.fm unconfigured-state contract | PRD-AUTH-002 | ✅ | `native-scrobble/` goldens (stub shapes preserved) |
| Last.fm account linking/session lifecycle | PRD-AUTH-002, PRD-EXT-001; legacy ADR-0004 | ⬜ | WP-07: `native_lastfm_links` (ADR-0134 §7), auth-url/session flows |
| Real Last.fm scrobble submission behind config flag | PRD-EXT-001/003/004 | ⬜ | WP-07: FOSS client, cache/rate-limit/fail-degraded |
| Provider provenance + no-provider-required invariant | PRD-EXT-002/003 | ⬜ | WP-07: `external_provider_cache`; catalog entry never requires a provider |
