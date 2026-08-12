# Feature Parity Matrix

This is the living checklist for the Rust replacement. No row may be silently removed.

| Capability | Parity class | Target phase | Status | Legacy evidence | Rust evidence | Test evidence | Notes |
|---|---|---:|---|---|---|---|---|
| Onboarding/setup wizard | Capability | 9 | Planned | — | — | — | |
| Multiple media libraries | Capability | 4,9 | Planned | — | — | — | |
| Inbound processing | Redesigned capability | 2,3 | In progress | `design/evidence/phase-0/media-baseline.md` (F1 metadata-gate silent skips; F2 destructive same-size dedupe; non-transactional restarts) | `crates/melodee-persistence/src/work_items.rs`; `crates/melodee-ingestion/`; `crates/melodee-catalog/`; `crates/melodee-policy/`; Phase 2–5 reports | Ingestion reconciliation/inspection/normalization/publication, catalog import/recovery/performance, and policy worker/fault/load tests | Durable discovery through authoritative policy disposition is complete; later operator/UI breadth remains, and broken legacy behavior intentionally differs (D9) |
| Staging/manual review | Redesigned capability | 2,5,10 | In progress | Phase 0 media findings and review-state inventory | Durable `NeedsReview`/quarantine/reprocess state, history, explainable policy decisions/audit, CLI, and diagnostics API in domain/persistence/server | `crates/melodee-persistence/tests/transitions.rs`; `history.rs`; `phase5_override.rs`; server policy-worker tests | The durable review and policy/operator boundary is implemented; the Phase 10 graphical review UI remains |
| Media conversion/normalization | Capability | 3 | Complete | `compatibility/media/baseline.json` (MP3 384k VBR-q4 48kHz ingest-only; no streaming transcode exists) | `crates/melodee-media/src/normalize.rs`; `crates/melodee-ingestion/src/normalize.rs`; Phase 3 report | `crates/melodee-media/tests/normalization_real.rs`; `crates/melodee-ingestion/tests/normalization.rs` | Canonical ingestion policy is implemented under PRD-MEDIA-002; streaming transcoding remains separate |
| Artwork management | Capability | 3,10 | In progress | Phase 0 representative media/artwork corpus | Phase 3 embedded/external artwork extraction, validation, JPEG normalization, manifest mapping, and publication | `crates/melodee-media/tests/normalization_real.rs`; Phase 3 publication/pathology tests | Media-pipeline artwork handling is complete; operator management UI remains Phase 10 |
| Metadata editing | Capability | 4,10 | In progress | — | Phase 4 audited Artist/AudioIdentity decision, negative, merge/split, contribution, membership, alias, and external-ID mutations | `crates/melodee-persistence/tests/catalog_identity.rs`; `catalog_merge_split.rs`; `catalog_audio_identity.rs` | Safe persistence/application correction boundaries are tested; remote API and admin UI remain Phase 10 |
| Production storage/catalog indexing | Redesigned capability | 3,4,6 | Complete | Phase 0 media/storage coupling and scale baselines | Phase 3 immutable-ID publication/manifest v3, Phase 4 authoritative PostgreSQL catalog/outbox, and Phase 6 rebuildable Tantivy consumer | Phase 3 publication, Phase 4 catalog, and final Phase 6 search/rebuild evidence | PostgreSQL remains authoritative; search is disposable and never rereads media |
| Artist browsing/editing/merge | Capability | 4,10 | In progress | — | Phase 4 Artist model, indexed participation reads, evidence/decision history, negative assertions, and audited merge/split | `crates/melodee-persistence/tests/catalog_identity.rs`; `catalog_participation.rs`; `catalog_merge_split.rs` | Domain/persistence capability is complete; user-facing browsing/editing API and admin UI remain Phase 10 |
| Release browsing/editing | Capability | 4,10 | In progress | — | Phase 4 Release catalog, structured billing/contributions, immutable overlays, and audited relationship correction | `crates/melodee-persistence/tests/catalog_import.rs`; `catalog_merge_split.rs` | Domain/persistence capability is complete; user-facing browsing/editing surfaces remain Phase 10 |
| Track browsing/detail | Capability | 4,10 | In progress | — | Phase 4 Track occurrence, MediaAsset, AudioIdentity, billing/contribution, and participation model | `crates/melodee-persistence/tests/catalog_import.rs`; `catalog_audio_identity.rs`; `catalog_participation.rs` | Domain/persistence capability is complete; user-facing browsing/detail surfaces remain Phase 10 |
| Search | Improved capability | 6 | Complete | `design/evidence/phase-0/scale-baseline.md` (~1s floor at 5M tracks; PRD-SEARCH-006 gap ~20x) | `crates/melodee-search`, migration `0009_phase6_search.sql` | Phase 6 correctness/outbox/rebuild/path/10M evidence | ADR-0107 and ADR-0121 |
| MQL advanced search | Contract/capability | 6 | Complete | frozen legacy MQL source/tests at `4028b58d...` | `crates/melodee-mql` | executable compatibility corpus + parser/translator tests | ADR-0122 |
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
| Admin diagnostics/Doctor | Capability | 1,10 | In progress | Phase 0 operational failure inventory | Rust-native health/readiness, Admin diagnostics API, ingestion history, and CLI Doctor/status commands | `apps/melodee-server/tests/admin_diagnostics_api.rs`; `system_api.rs`; persistence history/publication tests | Operational API/CLI foundation is implemented; Phase 10 diagnostics UI remains |
| Scheduled maintenance | Capability | 1,9 | Planned | — | — | — | |
| Job history/manual trigger/pause/resume | Redesigned capability | 1,2,10 | In progress | Legacy scheduled-work inventory | Durable work/event history plus CLI reconcile/retry/reprocess and diagnostics API | `crates/melodee-persistence/tests/history.rs`; `transitions.rs`; ingestion reconciliation scheduler tests | History and safe manual ingestion actions exist; general pause/resume UI remains Phase 10 |
| Themes/custom theme packs | Capability | 10 | Planned | — | — | — | |
| Light/dark/system appearance | Capability | 10 | Planned | — | — | — | |
| Localization | Capability | 10 | Planned | — | — | — | |
| RTL supported cultures | Capability | 10 | Planned | — | — | — | |
| OpenSubsonic API | Exact compatibility | 8 | Planned | `design/evidence/phase-0/opensubsonic-capture.md` (81 endpoints; 179 golden; 410/501 matrix automated) | — | — | D1 unsigned-JWT and D2 seek corruption are do-not-preserve |
| Melodee API v1 | Exact compatibility | 7 | Planned | `design/evidence/phase-0/native-api-v1-capture.md` (118 golden; unified inventory; client usage inventoried) | — | — | 88-route later-phase backlog tracked in route-inventory.json |
| Jellyfin music compatibility | Exact current subset | 8 | Planned | `design/evidence/phase-0/jellyfin-capture.md` (60/60 routes; 127 golden; no transcoding; images 404 without art) | — | — | Music-only subset boundary frozen |
| Container images amd64/arm64 | Capability | 11 | Planned | — | — | — | |
| CLI administrative operations | Capability/replacement | 1,9 | In progress | — | Rust-native migrate/doctor/work/ingest/catalog commands with bounded one-shot processing and durable diagnostics | `apps/melodee-cli/src/main.rs`; `apps/melodee-cli/tests/plan.rs`; workspace integration tests | Foundation and Phase 2–4 operator commands are implemented; broader user/admin recovery and migration commands remain Phase 9 |
