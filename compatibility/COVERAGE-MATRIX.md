# Compatibility Coverage Matrix (Phase 0)

Legacy baseline: `4028b58dbd3302ed431e2c5b32d9dde5634db218`. Machine-readable form: `coverage-matrix.json`. Regenerate: `python3 scripts/build_coverage_matrix.py`.

| Surface | Known routes/endpoints | Classification | Golden captures | Client usage | Planned Rust phase |
|---|---|---|---|---|---|
| Melodee API v1 | 202 routes | captured: 167, excluded-from-build: 35 | 287 | melodee-player: 13 routes; meloamp: 98 routes | Phase 7 (native API adapter) |
| OpenSubsonic | 81 endpoints | conditional: 15, gone: 6, not-implemented: 4, supported: 56 | 180 | (server-side contract; client smoke in Phase 8) | Phase 8 (OpenSubsonic adapter) |
| Jellyfin | 60 routes | covered: 60/60 | 127 | (server-side contract; client smoke in Phase 8) | Phase 8 (Jellyfin adapter) |
| Media pipeline | 31 corpus cases | extraction+pipeline+validation per case | n/a (baseline.json) | n/a | Phase 2-3 (ingestion/media pipeline) |

## Open discrepancies

- **D1** (opensubsonic) [approved-compatibility-deviation; decision: design/adr/ADR-0113-do-not-preserve-insecure-opensubsonic-unsigned-jwt-authentication.md]: Unsigned-JWT acceptance with precedence over password auth (security-critical; do not preserve) — evidence: `golden/auth/os-auth-jwt-forged-unsigned`
- **D2** (opensubsonic) [do-not-preserve]: Double range-processing corrupts nonzero-offset seeks (416 or wrong bytes) — evidence: `golden/media/os-stream-range-middle*`
- **D3** (all) [needs-explicit-decision]: Unknown-ApiKey detail behavior differs per surface (native 500, OS ok+error or 302, Jellyfin clean 404) — evidence: `golden/*/…-unknown*`
- **D4** (all) [do-not-preserve]: Playlist mutation corruption class (native/JF add-to-nonempty 500; OS cover-dependent 302; zero-GUID SongApiKey rows) — evidence: `golden/*/…playlist*`
- **D5** (melodee-api-v1) [needs-explicit-decision; decision: design/adr/ADR-0114-accept-meloamp-playlist-add-songs-object-body-alongside-canonical-array-form.md (Proposed: accept both shapes in Phase 7)]: MeloAmp add-to-playlist posts object body; server binds bare Guid[] (400) — client feature broken at baseline — evidence: `golden/native-catalog/catalog-playlist-crud-add-songs-meloamp-shape`
- **D6** (melodee-api-v1) [preserve-absence]: Queue + Party controllers are excluded from compilation (404 at runtime despite manifest presence) — evidence: `golden/native-catalog/catalog-queue-*`
- **D7** (melodee-api-v1) [preserve-for-compatibility]: JWT access tokens survive logout (stateless); refresh tokens revoked — evidence: `golden/native-auth/auth-refresh-rotation-access-after-logout`
- **D8** (opensubsonic) [needs-explicit-decision]: Advertised extensions apiKeyAuthentication/transcodeOffset are non-functional — evidence: `golden/system/os-getopensubsonicextensions-*`
- **D9** (media) [do-not-preserve]: F1: metadata-container gate silently skips tagless WAV/AIFF; F2: same-size files deleted without content comparison — evidence: `compatibility/media/cases/{clean-wav,same-size-files}.json`
- **D10** (all) [do-not-preserve]: List sorts lack tiebreaks (recents, playlists, global song lists) — nondeterministic order on equal keys — evidence: `design/evidence/phase-0/native-api-v1-capture.md section 5`

## Gaps (routes without golden coverage)

- Melodee API v1: 0 routes (client-used-uncaptured or undocumented-uncaptured) — the later-phase backlog; see route-inventory.json.
- OpenSubsonic: 0 endpoints without scenarios.
- Jellyfin: 0 routes without scenarios.
