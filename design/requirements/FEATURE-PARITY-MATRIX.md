# Feature Parity Matrix

This is the living checklist for the Rust replacement. No row may be silently removed.

| Capability | Parity class | Target phase | Status | Legacy evidence | Rust evidence | Test evidence | Notes |
|---|---|---:|---|---|---|---|---|
| Onboarding/setup wizard | Capability | 9 | Planned | — | — | — | |
| Multiple media libraries | Capability | 4,9 | Planned | — | — | — | |
| Inbound processing | Redesigned capability | 2,3 | Planned | `design/evidence/phase-0/media-baseline.md` (F1 metadata-gate silent skips; F2 destructive same-size dedupe; non-transactional restarts) | — | — | Broken behaviors measured; Rust intentionally deviates (D9) |
| Staging/manual review | Redesigned capability | 2,5,10 | Planned | — | — | — | |
| Media conversion/normalization | Capability | 3 | Planned | `compatibility/media/baseline.json` (MP3 384k VBR-q4 48kHz ingest-only; no streaming transcode exists) | — | — | Rust canonical policy is new (PRD-MEDIA-002) |
| Artwork management | Capability | 3,10 | Planned | — | — | — | |
| Metadata editing | Capability | 4,10 | Planned | — | — | — | |
| Production storage/catalog indexing | Redesigned capability | 3,4 | Planned | — | — | — | |
| Artist browsing/editing/merge | Capability | 4,10 | Planned | — | — | — | |
| Release browsing/editing | Capability | 4,10 | Planned | — | — | — | |
| Track browsing/detail | Capability | 4,10 | Planned | — | — | — | |
| Search | Improved capability | 6 | Planned | `design/evidence/phase-0/scale-baseline.md` (~1s floor at 5M tracks; PRD-SEARCH-006 gap ~20x) | — | — | Tantivy justification (ADR-0107) |
| MQL advanced search | Contract/capability | 6 | Planned | — | — | — | |
| Ratings/star/favorite/hate/pin | Contract/capability | 7,9 | Planned | — | — | — | |
| Playlists | Contract/capability | 7,8,9 | Planned | `compatibility/COVERAGE-MATRIX.md` D4 (mutation corruption class: add-to-nonempty 500, OS cover-dependent 302, zero-GUID rows) | — | — | Browse/detail/reorder/delete captured; corruption is do-not-preserve |
| Smart/dynamic playlists | Capability | 9 | Planned | `compatibility/melodee-api-v1/contracts/route-inventory.json` (MeloAmp uses smart-playlist CRUD+evaluate; server-side capture deferred to Phase 7) | — | — | client-used-uncaptured backlog |
| Play queue persistence | Contract/capability | 7,8,9 | Planned | `compatibility/melodee-api-v1/golden/native-catalog/catalog-queue-*` (404 at runtime; controllers not compiled) | — | — | Not built at baseline (D6); clients keep queues client-side |
| Scrobbling/now-playing | Contract/capability | 7,8,9 | Planned | `design/evidence/phase-0/native-api-v1-capture.md` section 5 (round-trips captured; playCount eventually-consistent via cache) | — | — | Cache staleness is do-not-preserve |
| Last.fm linking/scrobbling | Capability | 9 | Planned | — | — | — | |
| Charts | Capability | 9 | Planned | — | — | — | |
| Recommendations | Capability | 9 | Planned | — | — | — | |
| Listening analytics/statistics | Capability | 9 | Planned | — | — | — | |
| Audio features/BPM | Capability where exposed | 9 | Planned | — | — | — | |
| Equalizer presets/settings | Capability where exposed | 9 | Planned | — | — | — | |
| Internet radio | Capability | 8,9 | Planned | `compatibility/opensubsonic/golden/community/` (CRUD captured; admin-gated) | — | — |  |
| Public/private shares | Contract/capability | 7,8,9 | Planned | — | — | — | |
| User requests/comments/activity | Contract/capability | 7,9 | Planned | — | — | — | |
| Podcasts/subscriptions/downloads/resume/history | Contract/capability | 7,8,9 | Planned | — | — | — | |
| Users/roles | Contract/capability | 7,8,9 | Planned | — | — | — | |
| User groups/library ACL | Capability | 9 | Planned | — | — | — | |
| User avatars/profile/settings | Contract/capability | 7,8,9 | Planned | — | — | — | |
| Device profiles | Preserve preview | 9 | Planned | — | — | — | |
| Party Mode | Contract/capability | 7,9 | Planned | — | — | — | |
| Jukebox/server playback | Conditional contract/capability | 8,9 | Planned | — | — | — | |
| Playback backend management | Capability where exposed | 7,9 | Planned | — | — | — | |
| JavaScript event scripting hooks | Compatibility/migration | 9 | Planned | — | — | — | |
| Config import/export | Capability | 9 | Planned | — | — | — | |
| Email/notification templates | Capability where used | 9 | Planned | — | — | — | |
| API documentation/explorer | Capability | 7,8,10 | Planned | — | — | — | |
| Admin diagnostics/Doctor | Capability | 1,10 | Planned | — | — | — | |
| Scheduled maintenance | Capability | 1,9 | Planned | — | — | — | |
| Job history/manual trigger/pause/resume | Redesigned capability | 1,2,10 | Planned | — | — | — | |
| Themes/custom theme packs | Capability | 10 | Planned | — | — | — | |
| Light/dark/system appearance | Capability | 10 | Planned | — | — | — | |
| Localization | Capability | 10 | Planned | — | — | — | |
| RTL supported cultures | Capability | 10 | Planned | — | — | — | |
| OpenSubsonic API | Exact compatibility | 8 | Planned | `design/evidence/phase-0/opensubsonic-capture.md` (81 endpoints; 179 golden; 410/501 matrix automated) | — | — | D1 unsigned-JWT and D2 seek corruption are do-not-preserve |
| Melodee API v1 | Exact compatibility | 7 | Planned | `design/evidence/phase-0/native-api-v1-capture.md` (118 golden; unified inventory; client usage inventoried) | — | — | 88-route later-phase backlog tracked in route-inventory.json |
| Jellyfin music compatibility | Exact current subset | 8 | Planned | `design/evidence/phase-0/jellyfin-capture.md` (60/60 routes; 127 golden; no transcoding; images 404 without art) | — | — | Music-only subset boundary frozen |
| Container images amd64/arm64 | Capability | 11 | Planned | — | — | — | |
| CLI administrative operations | Capability/replacement | 1,9 | Planned | — | — | — | |
