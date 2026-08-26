# Phase 9 Report — Remaining Parity Domains

**Status:** Complete. All twelve per-domain parity checklists are green (every row ✅ with
linked test evidence or 🚫 waived by an accepted ADR), every Phase-9-tagged requirement is in a
terminal state (52 tested, 2 implemented, 7 deferred under accepted ADRs), the feature-parity
matrix carries no unapproved gaps, and the full repository gate passes with the entire change
set applied.

**Evidence captured:** 2026-08-14

**Base commit:** `19d1a8f` (device profile persistence, WP-10) plus the pending Phase 9 change
set (WP-07 Last.fm, WP-08 onboarding/config, WP-09 notifications, WP-11 scripting, WP-14 web
parity residue, status reconciliation, and the pre-existing gate-blocker fix described below)

**Authority:** IMPLEMENTATION §12, PRD §31–37/44–46 + the Phase 9 exit gate (§56),
ADR-0133 (party/jukebox waiver), ADR-0134 (parity-domain schema; §7 amended by WP-07),
ADR-0135 (Boa scripting runtime), ADR-0136 (non-Last.fm provider deferral),
ADR-0109 (accepted with the Phase 10 scope amendment), ADR-0132 (accepted),
and `PRD.md` (four waves, WP-01..WP-14).

## Current state

| Checklist | Rows | ✅ | 🚫 (ADR) | ➡ Phase 10 |
| --- | ---: | ---: | ---: | ---: |
| 01 users/groups/ACLs/avatars | 13 | 11 | 2 (OS user-management 501s stay frozen) | 0 |
| 02 playlists/shares | 9 | 9 | 0 | 0 |
| 03 requests/comments/activity | 4 | 4 | 0 | 0 |
| 04 podcasts/resume/history | 7 | 7 | 0 | 0 |
| 05 Last.fm/scrobbling | 5 | 5 | 0 | 0 |
| 06 charts/analytics/EQ | 7 | 7 | 0 | 0 |
| 07 internet radio | 4 | 4 | 0 | 0 |
| 08 party/jukebox/backends | 5 | 3 | 2 (ADR-0133 waiver) | 0 |
| 09 scripting hooks | 5 | 5 | 0 | 0 |
| 10 onboarding/config/notifications | 8 | 8 | 0 | 0 |
| 11 themes/localization/web | 5 | 3 | 0 | 2 (THEME/I18N stay Phase 10) |
| 12 CLI/maintenance | 8 | 8 | 0 | 0 |

Requirement accounting (61 Phase-9-tagged IDs): **52 tested, 2 implemented, 7 deferred** —
deferred IDs are PRD-PARTY-001/002 (ADR-0133), PRD-EXT-001/003/004 unimplemented-provider legs
(ADR-0136), PRD-DEVICE-003 (profile-driven stream selection, post-parity ADR-gated), and
PRD-WORK-002's Admin-UI leg (Phase 10 entry scope; API/CLI legs tested). Zero Phase-9 IDs remain
`planned`, `captured`, or `in-progress`; `extract_requirements.py --check` is clean.

The frozen legacy baseline remains commit `4028b58dbd3302ed431e2c5b32d9dde5634db218`.

## Wave 0 — unblock and reconcile (landed with the phase start)

- Requirement statuses promoted for the accepted Phase 7/8 evidence; new IDs created
  (PRD-RADIO-001/002, PRD-NOTIF-001..003, PRD-CLI-001/002); index regenerated and validated.
- FEATURE-PARITY-MATRIX.md reconciled against reality; stale artifacts removed (orphan
  `crates/melodee-api-v1/` scaffold, corrected top-level `migrations/README.md`).
- Decision bundle: ADR-0133 (party/jukebox waiver), ADR-0134 (schema), ADR-0135 (Boa) accepted;
  ADR-0132 and ADR-0109 dispositions recorded (both Accepted; 0109's shell validation moved to
  Phase 10 entry criteria).
- Twelve per-domain checklists seeded under `design/evidence/phase-9/checklists/`.

## Wave 1 — low-risk domain completion (WP-01..WP-06)

Landed before or at the phase boundary and evidenced in checklists 01–04, 06, 07, 12:

- **WP-01/02/03/04/06:** cross-API ID stability, podcast remainder, group CRUD + avatar
  upload (`native_user_avatars`, migration 0022), scheduled chart/discovery maintenance
  (`discovery_worker.rs`, PRD-WORK-003), native internet-radio surface over the shared
  ADR-0134 §1 store (migration 0020; immutable public-id trigger), all with persistence,
  server, and golden/differential tests.
- **WP-05 CLI breadth:** user bootstrap/recovery (fail-closed, env-var password), group
  management, worker-pool pause/resume (migration 0023 `worker_pool_state`, lease-coordinated),
  plus — landed this session — `config list/validate/export/import` and
  `notifications test-send` (WP-08/WP-09 legs below).

## Wave 2 — new subsystems (WP-07..WP-11)

### WP-07 External providers + Last.fm

- Evidence: `design/evidence/phase-9/lastfm-provider-evidence.md` (legacy ADR-0003/0004
  behavior, auth/scrobble wire mapping, provider inventory; configured-state capture gap
  recorded per the plan's risk-6 fallback — no real Last.fm credentials exist for capture).
- Migration 0028: `external_provider_cache` (nine-provider enumerated vocabulary, hashed
  request keys, bounded payloads, expiry) and `native_lastfm_links`.
- **ADR-0134 §7 amendment:** the original "digest only, never the raw key" wording is
  unimplementable because Last.fm request signing needs the raw session key at runtime; the
  table now stores a SHA-256 digest for lookup/audit plus a ChaCha20-Poly1305 ciphertext
  (`ring`, already in the dependency tree) sealed under a domain-separated key derived from
  `security.secretKey`. The refined invariant — the raw key is never at rest in plaintext,
  never logged, never exposed by any API — is stronger than the legacy plaintext `varchar(64)`.
- FOSS client over the existing reqwest dependency (MD5 `api_sig` pinned to a hand-computed
  Last.fm-spec vector): auth-url/session-exchange/disconnect routes with the captured
  unconfigured shapes verified byte-identical; scrobble/now-playing dispatch wired into the
  native, OpenSubsonic, and Jellyfin scrobble paths behind `scrobbling.lastFm.Enabled` +
  per-user link, fail-degraded (Last.fm errors 4/9/10 clear the link per legacy ADR-0004).
  Preserved legacy quirks documented: processing-time timestamp (client timestamp ignored),
  no batching/retry, dead `scrobbling.enabled` setting recognized but not enforced.
- PRD-EXT-002 invariant tested: catalog entry never requires a provider.

### WP-08 Onboarding + config import/export

- Evidence: `design/evidence/phase-9/configuration-inventory.md` — the missing Phase 0
  deliverable, generated from the frozen oracle: **190 `SettingRegistry.cs` constants = 190
  unique key strings** (157 DB-seeded, 33 code-default-only), with secrets classification
  (12 hard secrets + 2 credential-adjacent), the consumption pattern, and the parity quirks
  (casing inconsistencies, the `dontDeleteExisiting...` typo, malformed transcoding seeds,
  compromised shipped defaults deliberately not carried).
- Migration 0025: `server_settings` (jsonb values, `is_secret`, bounded) and
  `onboarding_state` (singleton-enforced) per ADR-0134 §3.
- Settings layering (PRD-CONFIG-003): env → config file → DB → inventory defaults, with the
  WP-09 email settings reading through the layer unchanged at the struct level.
- Export/import (PRD-CONFIG-002, PRD-AUTH-007): secrets always redacted to a fixed marker;
  import is preview-first with per-key validation, rejects unknown keys without an explicit
  flag, and fail-closed rejects a redacted marker submitted as a value; apply is atomic and
  audited. Admin API (`/api/admin/v1/config/export|import`) + CLI (`config list/validate/
  export/import`).
- Onboarding (PRD-SETUP-001/002): six named preflight checks (database/admin required;
  storage/media-tools/search/email advisory), `GET /onboarding/status`, fail-closed
  `POST /onboarding/complete`, and the migration bypass heuristic (completed marker, non-empty
  catalog, or known settings rows ⇒ `bypassed: true`).

### WP-09 Notifications/templates

- Migration 0024: `notification_templates` (PK `(template_key, locale)`, seeded `en` templates
  for password-reset/share-created/request-fulfilled) and append-only `notification_deliveries`
  with 30-day code-enforced retention, per ADR-0134 §4.
- `{{placeholder}}` rendering with HTML escaping; SMTP transport via **lettre 0.11.23**
  (MIT OR Apache-2.0, exact pin, rustls-only, no broker/pool — documented dependency decision;
  `0BSD` allowance added for its transitive `quoted_printable`), NoopTransport when disabled.
- Fail-degraded wiring (PRD-NOTIF-002): password-reset, share-created, and request-fulfilled
  events dispatch post-response fire-and-record sends; delivery outcomes are audit rows, never
  rolled back into the triggering operation. Admin template CRUD + deliveries + test-send;
  CLI `notifications templates list` / `test-send`.

### WP-11 Scripting hooks

- Evidence: `design/evidence/phase-9/script-events-legacy-inventory.md` (PRD-SCRIPT-002: both
  legacy subsystems, all hooks with file:line citations, the fail-open/fail-closed asymmetry,
  and ten gaps/dead-code items that must NOT be preserved) and
  `design/evidence/phase-9/script-hook-migration-guide.md` (PRD-SCRIPT-005 per-hook
  dispositions + Jint→Boa translation rules).
- Migration 0027: `script_registry` / append-only `script_runs` per ADR-0134 §6.
- Executor per ADR-0135: **boa_engine 0.21.1** (Unlicense OR MIT, exact pin, pure Rust),
  capability-less sandbox (zero host objects registered; only `ctx` + `scriptConfig` as plain
  data), legacy auto-wrap/`check(ctx, scriptConfig)` contract, wall-clock timeout on a
  dedicated thread with detached-leak bounding, payload/return size caps, and a circuit
  breaker that transactionally disables repeat offenders with a `tripped` audit row.
  Fail-degraded everywhere (PRD-SCRIPT-004).
- Wired hook: `directoryProcessingStart` at the ingestion gate (`melodee-ingestion`
  `DirectoryProcessingGate` boundary trait keeps ingestion Boa-free), with the legacy
  context built field-for-field and deny ⇒ return-to-settling skip parity (the legacy
  `delete`→`skip` safety rewrite preserved; physical quarantine moves deferred and documented).
  The five legacy UI-gate events are registry-accepted but honestly marked inert; the two
  unsandboxed shell hooks are deliberately not re-created (migration guide documents
  re-expression paths).
- Admin API (`/api/admin/v1/scripts` CRUD + test-run) and CLI (`scripts list/test`). The
  never-implemented legacy `/api/scripts/validate` route is not recreated.

### WP-10 Device profiles (pre-existing at the phase boundary)

Migration 0026 + `DeviceProfileStore` + admin API landed with the phase-opening commit;
metadata-only rows, no transcoding-control claim (PRD-DEVICE-002), single-default invariant
fail-closed at the database.

## Wave 3 — contract-sensitive items (WP-12..WP-14)

- **WP-12 party/jukebox:** resolved by the accepted **ADR-0133 waiver** — frozen 404/410
  absence preserved and differentially evidenced; reactivation requires a superseding ADR +
  client compatibility plan. PRD-PARTY-001/002 deferred under it.
- **WP-13 OS user management:** the intentional 501s stay frozen; user administration lives on
  the Admin API/CLI surfaces (checklist 01 🚫 rows). No contract change was made.
- **WP-14 web parity residue:** `design/evidence/phase-9/blazor-page-inventory.md` (63 pages /
  68 routes, KEEP 54 / REPLACE 6 / DROP-candidate 3) satisfies PRD-WEB-001; the classification
  outcome + the rule that every DROP requires its own ADR satisfies PRD-WEB-002. ADR-0109
  accepted with its shell-validation moved to Phase 10 entry criteria; ADR-0132 accepted.

## Pre-existing gate blocker found and fixed

The full gate was red on arrival and reproducibly so on the clean base commit — not introduced
by any Phase 9 change:

1. **Root cause:** the workspace serializes `serde_json::Value` with `preserve_order`
   (insertion order, a captured-contract requirement), while manifests persist through
   PostgreSQL `jsonb`, which reorders object keys. `hash_manifest` hashed the ordered
   serialization, so any manifest whose free-form subdocuments (raw tags, parsed sidecars)
   inserted keys in non-jsonb order failed validation with
   `input semantic manifest hash does not match the normalized manifest` after the database
   round trip. This made `real_mp3_flac_ogg_wav_unicode_artwork_and_sidecars_reach_published`
   fail 4/4 whenever real media tools are present (CI without ffmpeg/fpcalc silently skips the
   test, which is how it survived). The Phase 8 report recorded the symptom as an
   environment-dependent exclusion; it is now root-caused and fixed.
2. **Fix:** `hash_manifest` canonicalizes the semantic value (recursive key sort) before
   hashing, so the hash depends only on semantic content. Regression test:
   `manifest_hash_is_independent_of_value_key_order` (simulated jsonb reordering). No captured
   contract or golden vector pins the hash function (verified: the domain golden vector pins
   only the canonical path layout).
3. **Hygiene fix:** the normalization `Scenario::drop` joined a cleanup thread whose
   `PgPool::close()` can wait forever on connections leaked by a panicking test, parking the
   whole test binary (and the gate) instead of reporting the failure. Cleanup is now bounded
   (20 s timeout; leaks one `melodee_test_*` database rather than stalling the suite).

Both fixes are test-visible and the previously hanging suite now completes green.

## Compatibility decisions and deviations

- ADR-0133 (waiver), ADR-0134 (schema; §7 amended as quoted in its text), ADR-0135 (Boa),
  ADR-0136 (provider deferral) accepted; ADR-0109/ADR-0132 dispositions recorded.
- No existing external route, DTO, identifier, status code, error shape, auth flow, pagination,
  or intentional 404/410/501 behavior changed. The frozen native-excluded and OS negative
  matrices still pass; the Last.fm unconfigured goldens are byte-identical after wiring.
- New routes added by Phase 9 are Rust-native admin/operator surfaces only, not frozen
  contracts.

## Data and migration impact

Phase 9 adds migrations 0022, 0023, 0024, 0025, 0026, 0027, 0028 — all additive-only, each with
rollback notes (binary downgrade leaves the tables intact; schema rollback restores a verified
pre-phase backup into a new database) and integrity tests. No migration rewrites media, public
IDs, or existing rows. Secrets are never stored in plaintext (notifications SMTP, Last.fm
session keys) and are redacted on every read surface.

## Test evidence

```text
./scripts/check.sh                    # full gate green: repository validation, requirements
                                      # index, 140 Python capture tests, cargo fmt --check,
                                      # clippy -D warnings, workspace tests, cargo deny check
cargo test --workspace --all-features # 834 passed, 0 failed, 4 ignored (evidence harnesses)
cargo deny check                      # advisories ok, bans ok, licenses ok, sources ok
python3 scripts/extract_requirements.py --check   # index current (239 requirements)
python3 scripts/validate_repository.py            # passed (37 ADRs, 8 mockups, 239 requirements)
cd docs && pnpm check && pnpm check:links && pnpm build   # green (run per WP)
```

Per-WP suites are enumerated in the checklist evidence links; highlights: 9 mock-wire Last.fm
tests, 11 scripting executor unit tests + service/admin/CLI suites, 7 settings + 4 notification
persistence suites, the jsonb-ordering hash regression test, and the scripting ingestion-gate
test.

## Requirement and matrix closeout (Gate G)

- 61/61 Phase-9-tagged requirements terminal: 52 tested, 2 implemented, 7 deferred under
  ADR-0133/0136 or the Phase 10 boundary (each deferred entry carries its ADR/phase citation).
- FEATURE-PARITY-MATRIX.md: no unapproved gaps. Remaining `Planned` rows are all explicitly
  Phase 10/11-tagged (appearance, localization, RTL, container images, API explorer); the
  party/jukebox rows are waived by ADR-0133; the artwork row names its remaining Phase 10
  work.
- All twelve checklists green per `checklists/README.md`'s rule (✅ with linked evidence or 🚫
  waived by an accepted ADR).

## Follow-up ownership confirmation (plan W4-4)

- `Album.OriginalReleaseDate` column absence: remains a **deferred schema ADR** item from the
  Phase 8 report's known-differences list; owning track is the Phase 11 migration/import work
  (it is migration data, not a parity gap). Recorded here so it is not lost.
- ~~Durable artwork wiring~~ — completed in Phase 10 under ADR-0143. Native, OpenSubsonic, and
  Jellyfin now read catalog overrides; Jellyfin additionally reads immutable publication-plan
  artwork, while their captured no-artwork fallbacks remain intact.
- Legacy scheduled jobs with no Rust equivalent (recorded in PRD-WORK-001's note):
  `nowPlayingCleanup` (superseded by one-row-per-user upsert/delete),
  `stagingAutoMove`/`stagingAlbumRevalidation` (non-destructive model, by design),
  `artistHousekeeping`/`artistSearchEngineHousekeeping` (Tantivy rebuild/catch-up instead of a
  legacy search-engine DB), `musicbrainzUpdateDatabase` (ADR-0136 deferral). None is a parity
  obligation; each is a design divergence with an authority trail.

## Risks and limitations

- **Configured-state Last.fm capture gap:** the `{url}` / `{"message":"Last.fm linked"}`
  success shapes are source-evidenced only (no real Last.fm application credentials exist for
  capture). If credentials become available, re-capture per the Phase 0 runbook and amend the
  evidence doc. The disabled-state goldens are frozen and verified.
- **SMTP happy path** (`sent` status) is trait-isolated but not exercised against a live relay;
  skip/fail paths are evidenced.
- **Scripting:** no deterministic mid-execution interrupt (accepted ADR-0135 trade-off; bounded
  by wall-clock + leaked-thread counter + circuit breaker). No real-world operator script
  corpus exists to validate Jint↔Boa equivalence beyond the contract and test corpus. The five
  legacy UI-gate events are inert registrations (server-side enforcement points are future
  work, documented in the migration guide). Physical quarantine moves are deferred.
- **Env-var naming** for settings without an existing convention folds camelCase via the
  documented mechanical `MELODEE__` mapping (recorded in rustdoc + user docs).
- The WP-14 web classification is **preliminary pending Phase 10 ratification**; the three
  DROP candidates each still require their own ADR before any route removal.
- One pre-existing timing-sensitive test
  (`discovery_worker::scheduler_runs_periodically_and_stops_gracefully`) failed once under
  full-suite parallel load during WP-09 development and passes in isolation and in both full
  gate runs since; recorded here rather than silently.

## Independent review decision

Phase 9 executed per `PRD.md` under accepted ADRs 0133/0134/0135/0136.
The exit gate (PRD §56: parity matrix without unapproved gaps; IMPLEMENTATION §12: per-domain
checklists and tests) is satisfied by the evidence above. Recommended decision: **accept Phase
9 and open Phase 10 (admin UI) scope**; Phase 11 (migration/import) owns the
`Album.OriginalReleaseDate` deferred schema ADR and the C# data import CLI leg of
PRD-CLI-001.

Documentation impact:
- Updated (public, `/docs`, in the same changes as the code): `installation/configuration.md`
  (email, settings layering, import/export, Last.fm), `api/admin-api.md` (notifications,
  config, onboarding, scripts), `admin/users-and-access.md`, `admin/system-diagnostics.md`,
  `admin/scripting-hooks.md` (new), `features/lastfm.md` (new), `features/sharing.md`,
  `features/requests.md`, `features/discovery-and-playback.md`, `getting-started/quick-start.md`
  (first-run onboarding). Docs gates (`pnpm check`, `check:links`, `build`) ran green per WP.
- Updated (internal): this report; the twelve checklists; evidence documents
  (`configuration-inventory.md`, `script-events-legacy-inventory.md`,
  `script-hook-migration-guide.md`, `blazor-page-inventory.md`, `lastfm-provider-evidence.md`,
  `device-profiles-legacy-inventory.md`, `request-002-surface-inventory.md`);
  ADRs 0109/0132/0134 (dispositions/amendment) and new ADR-0136;
  `design/requirements/status-overrides.json`, regenerated `REQUIREMENTS-INDEX.md` /
  `requirements.json`, and `FEATURE-PARITY-MATRIX.md`; `AGENTS.md` phase boundary.
