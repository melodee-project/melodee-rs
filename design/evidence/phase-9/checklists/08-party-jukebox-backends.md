# Checklist 08 — Party mode, jukebox, playback backends (WP-12)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Native Party/Queue absence preserved (35 routes, frozen 404) | PRD-PARTY-001, PRD-USER-004 | 🚫 waived | ADR-0133; 35-case negative matrix stays green |
| OpenSubsonic jukeboxControl intentional 410 | PRD-PARTY-002 | 🚫 waived | ADR-0133; `opensubsonic/negative.rs`; legacy ADR-0001 |
| Playback backend management | PRD-PARTY-003 | ✅ | `native_endpoints_api.rs`; migrations 0017/0019 |
| Historical party/jukebox ADR review | PRD-PARTY-004 | ✅ | ADR-0123; Phase 7 matrix |
| Client plan for the waiver | ADR-0123 reactivation clause | ✅ | ADR-0133 §Decision.4 (no client can depend on 404/410 routes) |
