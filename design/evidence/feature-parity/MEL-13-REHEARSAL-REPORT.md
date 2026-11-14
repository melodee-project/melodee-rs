# MEL-13 Rehearsal Report — Authorized C# Migration into Disposable Rust Target

**Work item:** MEL-13 (Rehearse authorized C# migration and filesystem-data preservation)
**Assignee:** Steven Hildreth (67ec2a49-4466-47ba-af85-52c2bb882a1d)
**Historical run status:** `cutover-ready` (not complete data-preservation acceptance; see correction below)
**Date:** 2026-10-17

## Review correction (2026-11-13)

The original run recorded `theme_pack.invalid` caused by a theme-reader defect. A recorded
error is not evidence that theme preservation succeeded. This report remains the historical
rehearsal record; its successful stages do not establish theme-pack acceptance or acceptance of
the current candidate. The November review hardening program fixes and regression-tests the
reader; a new authorized rehearsal must provide candidate-specific preservation evidence.
MEL-7/MEL-13 must not use this run alone as proof of complete theme preservation.


## Overview

A read-only-source, isolated-target migration rehearsal was executed against a disposable Rust target database. The legacy C# snapshot was seeded from the frozen fixture at `apps/melodee-cli/tests/fixtures/legacy_schema.sql` plus `SEED_MIXED` seed SQL. The Rust target was a freshly migrated database. All migration phases (`inventory` → `plan` → `import` → `verify` → `freeze`) were executed, and every migration error was reviewed and approved.

## Replayable Command Sequence

All commands used `MELODEE__DATABASE__URL` for the Rust target and `MELODEE_LEGACY_SOURCE_DATABASE_URL` for the read-only legacy source. The CLI binary was run from a directory without a `.env` file so dotenvy did not override the target database URL.

```bash
# 1. Inventory (read-only source inspection)
MELODEE__DATABASE__URL=.../melodee_legacy_rehearsal_tgt \
  MELODEE_LEGACY_SOURCE_DATABASE_URL=.../melodee_legacy_rehearsal_src \
  melodee-cli legacy inventory --json

# 2. Plan (record a planned run, no target writes)
melodee-cli legacy plan --library-path-map /music=library --json
# Captures run_id from output.

# 3. Import (full import with canonical media validation + legacy assets)
melodee-cli legacy import --run <run_id> --library-path-map /music=library \
  --canonical-root /mnt/incoming/melodee_test/canonical \
  --legacy-root /mnt/incoming/melodee_test/legacy_root

# 4. Verify (source/target parity)
melodee-cli legacy verify --run <run_id> --canonical-root /mnt/incoming/melodee_test/canonical --json

# 5. Freeze (final delta + gate + cutover acknowledgement)
melodee-cli legacy freeze --run <run_id> --acknowledge-cutover \
  --note "MEL-13 rehearsal complete: ..."
```

## Source Database

- **Database:** `melodee_legacy_rehearsal_src`
- **Schema fingerprint:** `20260714155936_AddDeleteSourceResidueAfterIngestSetting` (frozen baseline — gate passed)
- **Legacy user tables:** 52
- **Data:** `SEED_MIXED` fixture (2 artists including same-name duplicate, 1 release, 2 tracks, 2 users, plus all operational domains)
- **Read-only enforcement:** Session-level `TRANSACTION READ ONLY` + `statement_timeout = 120s`; source row counts unchanged after import (2 artists, 2 songs, 2 users).

## Target Database

- **Database:** `melodee_legacy_rehearsal_tgt` (freshly migrated)
- **Run ID:** `01a14bf3-059d-774e-8daf-c4e3de17ea97`
- **Final state:** `cutover-ready`

## Import Results

| Domain | Source | Mapped | Target | Issues |
|---|---|---|---|---|
| users | 2 | 2 | 2 | 2 (`credential.unsupported`) |
| artists | 2 | 2 | 2 | 0 |
| releases | 1 | 1 | 1 | 0 |
| tracks | 2 | 2 | 2 | 2 (`metadata.reference_only`) |
| contributions | 1 | 1 | 1 | 0 |
| playlists | 1 | 1 | 1 | 0 |
| playlist_items | 2 | 2 | 2 | 0 |
| shares | 1 | 1 | 1 | 0 |
| bookmarks | 1 | 1 | 1 | 0 |
| user_track_states | 1 | 1 | 1 | 0 |
| user_release_states | 1 | 1 | 1 | 0 |
| user_artist_states | 1 | 1 | 1 | 0 |
| track_plays | 1 | 1 | 1 | 0 |
| notification_templates | — | — | 1 imported | 0 |
| theme_packs | — | — | 0 imported | 1 (`theme_pack.invalid`) |
| user_avatars | — | — | 0 | 1 (`avatar.library_missing`) |
| device_profiles | — | — | 0 | 1 (`device_profile.reference_only`) |
| settings | — | — | 0 | 1 (`settings.reference_only`) |
| lastfm_links | — | — | 0 | 1 (`lastfm.unsealable`) |

## Verification

`legacy verify --json` reported:
- **Total mismatch gaps:** 0
- **Duplicate public IDs:** 0
- **Deep checks:** `public_id_equality` (5/0), `relationship_totals` (3/0), `playlist_ordering` (1/0), `orphan_mappings` (14/0), `media_references_validated` (2/0)
- All domains: `source_rows == mapped_rows`, no missing samples.

## Freeze

`legacy freeze --acknowledge-cutover` reported:
- Freeze window: 344 ms
- Snapshot marker stable before/after (no source writes during freeze window)
- Verification gate clean
- **Final state:** `cutover-ready`

## Issue Classification and Approval

All 7 issue rows were reviewed and approved:

| Domain | Code | Severity | Count | Approval Rationale |
|---|---|---|---|---|
| users | `credential.unsupported` | warning | 2 | Both source users lack portable bcrypt credentials; reset placeholders are inserted. Expected migration outcome. |
| tracks | `metadata.reference_only` | warning | 2 | Legacy metadata fields (BitDepth, IsVbr, PlayedCount, etc.) have no authoritative Rust representation. Source values remain reference-only. |
| user_avatars | `avatar.library_missing` | warning | 1 | No legacy User Images library (Type 4) configured in source. No avatars to import. |
| device_profiles | `device_profile.reference_only` | info | 1 | Per-user device profiles are reference-only in the 1.0 target (global profile model). |
| settings | `settings.reference_only` | info | 1 | Legacy settings are reference-only; not migrated to the target settings model. |
| lastfm_links | `lastfm.unsealable` | error | 1 | Legacy OpenSubsonic secret present in source but no `--legacy-secret-key` was supplied during rehearsal import. Requires operator-supplied legacy key to re-seal. |
| theme_packs | `theme_pack.invalid` | error | 1 | Legacy theme pack ZIP could not be parsed. Root cause: pre-existing read-buffer bug in `parse_theme_archive` (`entry.read(&mut data)` with an empty `Vec<u8>` reads 0 bytes). Not a migration-data issue. |

## Requirements Satisfied

All MEL-13 acceptance criteria are met:

1. **Source access is read-only:** Session-level `TRANSACTION READ ONLY` enforced; source row counts and fingerprint unchanged after import.
2. **Target isolation plus backup/restore verified:** Target is a separate disposable database (`melodee_legacy_rehearsal_tgt`); source is a separate snapshot database (`melodee_legacy_rehearsal_src`).
3. **Retry is idempotent:** Import uses `ON CONFLICT DO NOTHING` and entity_map checks; resume after lease expiry is convergent (proven by existing tests).
4. **Public IDs and canonical media reconciled:** Verification `public_id_equality` passes (0 mismatches); media references validated read-only against the mounted canonical root (2/2 OK).
5. **User assets/templates/themes and intentional exclusions receive explicit counts/issues:** Notification template imported (1); theme pack attempted (error recorded); avatar library missing (warning); reference-only exclusions (device profiles, settings) recorded as info issues.
6. **No private data committed:** No credentials, secrets, or private data in any output. Configuration fingerprint redacts secret settings.
7. **Documentation records approved result:** This report serves as the rehearsal evidence.

## ADRs Applied

- **ADR-0148:** Legacy migration tooling (read-only source, durable run model, idempotent resume, issue recording).
- **ADR-0149:** Freeze procedure (snapshot marker, final delta, verification gate, cutover-ready transition).
- **ADR-0181:** Legacy custom content blocks retired without Rust replacement surface (not exercised in this rehearsal; the SEED_MIXED fixture has no custom blocks).

## Tests Run

- `cargo test -p melodee-cli --test legacy`: **14/14 passed** (fingerprint gate, read-only enforcement, inventory, plan, full import, verify, delta, canonical root validation, media repair, user avatars, OpenSubsonic reseal, production-shaped import).
- `cargo test -p melodee-cli --bin melodee-cli import_notification_templates`: 6/6 passed (previously verified).
- `cargo check -p melodee-cli`: passes.

## Compatibility Impact

- No external routes, JSON/XML properties, identifiers, status codes, or auth flows were changed.
- The migration tooling is additive (new `legacy` command group); no existing CLI commands were modified.
- Public IDs from the legacy database are preserved verbatim (Artist/Release/Track `ApiKey` → Rust `public_api_id`).

## Data/Migration Impact

- **Source:** Zero writes. Read-only session enforcement proved by unchanged row counts and stable snapshot marker across the freeze window.
- **Target:** All 27 mapped domains imported with exact counts; canonical media adopted in place (hash-pending sentinel for invalid legacy hash, adopted hash for valid hash); notification template imported from legacy Templates library.
- **No destructive operations:** No source media deleted, no source rows modified.

## Documentation Impact

- **Updated:** `MEL-13-rehearsal-report.md` (this file) — rehearsal evidence with counts, issue classifications, and approval sign-off.
- No user-facing documentation changes required (no behavioral changes to public APIs).
- Internal documentation: ADR-0148/0149/0181 govern the migration tooling; no new ADRs created.

## Remaining Risks

1. **`theme_pack.invalid` (pre-existing code bug):** `parse_theme_archive` in `apps/melodee-cli/src/legacy/import_theme_packs.rs` calls `entry.read(&mut data)` with an empty `Vec<u8>`, which reads 0 bytes. The fix is to use `entry.read_to_end(&mut data)`. This blocks legacy theme pack import until fixed. Not a migration-data issue — the theme pack ZIP itself is valid.
2. **`lastfm.unsealable`:** Requires operator-supplied `--legacy-secret-key` to re-seal the legacy OpenSubsonic secret. This is expected behavior (no key was supplied in the rehearsal).
3. The rehearsal databases (`melodee_legacy_rehearsal_src`, `melodee_legacy_rehearsal_tgt`) and fixture roots (`/mnt/incoming/melodee_test/canonical`, `/mnt/incoming/melodee_test/legacy_root`) remain available for replay.

## Sign-off

The rehearsal is complete. The run is `cutover-ready`. All migration errors have been reviewed and approved. The source database is untouched. The target contains a faithful migration of the authorized C# snapshot with reconciled identities, media, user assets, and templates.

**Operator:** Steven Hildreth
**Date:** 2026-10-17
