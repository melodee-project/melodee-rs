# ADR-0157: Metadata transformation contract and safe file rewrites

- **Date:** 2026-08-24
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MEDIA-005, PRD-MEDIA-006, PRD-WORK-003, PRD-OBS-001,
  PRD-TEST-006

## Context

C# Melodee ships `MediaEditService.cs` (1113 lines) — the library of **batch, reversible
metadata-transformation operations** applied to an album/release directory:
`RemoveUnwantedTextFromAlbumTitle`, `RemoveUnwantedTextFromSongTitles`,
`RemoveFeaturingArtistsFromSongTitle`, `RemoveFeaturingArtistsFromSongsArtist`,
`PromoteSongArtist`, `SetYearToCurrent`, `ReplaceGivenTextFromSongTitles`,
`RemoveAllSongArtists`, `ReplaceAllSongArtistSeparators`, `SetArtistOnAllSongs`,
`RenumberSongs`, `DeleteAllImagesForAlbums`, `RemoveArtistFromSongArtists`,
`SetAlbumsStatusToReviewed`, plus `SaveImageUrlAsCoverAsync` / `DoMagic` (cover fetch + apply) and
`UpdateArtistForAlbums` / `UpdateArtistsByMapping`. Each op reads tag fields across an album,
mutates them in memory, optionally validates (`AlbumValidator`), and writes tags back to the media
files. A "remove unwanted text" op is regex/pattern-based; "featuring artist" ops split
"feat."/"ft."/"with" segments; "renumber" rewrites track order fields; cover ops fetch an image
from a URL and embed it.

The Rust codebase has the **storage and review seams** (the Phase 7 release review workflow,
`media_assets`, `dispositions`, `catalog_release_review`, the `melodee-storage` crate) but **no
tag-transformation engine**: there is no `MediaEdit`-equivalent that can rewrite tags across a
Release, no "remove featuring artist" / "renumber tracks" / "replace text" / "embed cover"
operation, and no audit trail for such rewrites. PRD-MEDIA-005 (artwork preservation) and
PRD-MEDIA-006 (sidecar files) plus the W4 "metadata transformation engine" workstream require a
durable decision on how these operations are contracted, where they run, and how rewrites stay
safe and auditable.

## Decision

1. **Operation contract.** A `MediaEditOperation` trait in the application layer with a
   **dry-run/apply** two-phase contract:
   - `preview(release: ReleaseId, params) -> EditPreview` — compute the set of (file, field, old
     value, new value) deltas without touching any file or row.
   - `apply(release: ReleaseId, params, preview: EditPreview) -> EditResult` — apply the
     pre-computed deltas (optionally re-validating they still match) and commit the audit record.
   The preview is the unit of audit and the unit of idempotency: `apply` is a no-op if the
   pre-computed deltas are already applied.
2. **In-scope operations (v1).** The first batch preserves the highest-value legacy operations:
   - `remove_featuring_artists` (split "feat."/"ft."/"with" segments from track title and
     artist; the primary artist remains, secondary artists become `TrackArtist`/`Contribution`
     rows with a secondary contribution type).
   - `renumber_tracks` (rewrite track number/disk fields to match catalog order).
   - `replace_text_in_title` / `replace_text_in_artist` (exact-string and optional regex
     substitution across track titles and/or artist fields).
   - `embed_cover_art` (fetch an image from a URL or local path, validate size/type, embed as
     APIC/`cover art` tag).
   - `set_year` (set release year to a given value or the current year).
   - `remove_track_artist` (clear the secondary artist on tracks, demoting to contribution).
   - `set_release_status_to_reviewed` (move Release to the reviewed disposition without
     a full review).
   Additional operations are additive; each new operation is a separate, versioned, auditable
   transformation.
3. **Tag reading/writing.** The `melodee-storage` crate (or a dedicated `melodee-tags` module
   within it) provides `read_tags(file) -> TagMap` and `write_tags(file, TagMap)` for the
   supported containers (FLAC, MP3/ID3, M4A/MP4, OGG/Vorbis, WAV). Writing is **atomic**: write
   to a temp file in the same directory, then `rename(2)` over the original. On failure, the
   original is untouched.
4. **Safety invariants.**
   - A transformation never deletes a media file (repository invariant: "never delete source
     media because classification is uncertain"); it only rewrites tag fields.
   - A transformation never changes a file's path, filename, or extension.
   - A transformation never changes the catalog identity (Release ID, Track ID, MediaAsset ID);
     it changes *display/credit fields* only.
   - If a file's fingerprint (SHA-256) changes after a write, the write is treated as a failure
     (the file was corrupted); the transaction is rolled back (original file restored from the
     pre-write backup) and the audit record is marked failed.
5. **Audit trail.** Every `apply` writes a `media_edit_audit` row (migration) recording:
   `release_id`, `operation_name`, `operation_params` (JSONB), `affected_files` (count),
   `field_deltas` (JSONB array of `{file, field, old, new}`), `applied_by` (user),
   `applied_at`, `result` (success/failed/rolled-back), `fingerprint_before` /
   `fingerprint_after` for each file. The audit row is the recovery record: a failed apply can be
   inspected and reversed.
6. **Reversibility.** `EditPreview` is reversible: `reverse(preview) -> EditPreview` produces the
   inverse preview (swap old/new) that can be `apply`-ed to undo the operation. A
   `POST /api/v1/releases/{id}/edits/{audit_id}/revert` route (additive) applies the inverse.
7. **Concurrency.** A Release being edited is soft-locked (an advisory `pg_advisory_xact_lock` on
   the release ID) for the duration of the `apply` transaction; concurrent edits to the same
   Release are serialized. A dry-run/preview does not take the lock.
8. **Wiring.** The operations are exposed as additive native-v1 routes:
   - `POST /api/v1/releases/{id}/edits/preview` — dry-run a named operation with params.
   - `POST /api/v1/releases/{id}/edits/apply` — apply a previewed operation.
   - `GET /api/v1/releases/{id}/edits` — list audit records.
   - `POST /api/v1/releases/{id}/edits/{audit_id}/revert` — apply the inverse.
   The admin SPA (Feature Completion Phase 2, DATA workstream) surfaces these as the metadata
   edit forms (the single deliberate Phase 10 deferral now resolved).
9. **Batch scope.** An operation applies to **one Release** (album) at a time. Cross-release
   batch operations (e.g., "renumber all releases") are out of scope for v1; they can be
   composed by repeated single-release calls.

## Alternatives considered

### Port the C# `MediaEditService` class methods

Rejected per the ground-up rewrite rule: the C# methods mix tag I/O, validation, save, and
configuration in a single `sealed class` with `OperationResult` return types; the Rust contract
is a trait + atomic file writes + audit row, not a line-for-line port.

### Operations write directly to the database (no file rewrite)

Rejected: the media files *are* the source of truth for tag fields in the user's library;
rewriting only the DB would diverge from the files on the next scan. File rewrites are the
intended behavior.

### A general "tag editor" (arbitrary field set)

Rejected: arbitrary field writes are unsafe (a user could corrupt identity fields, change the
container, or break the file). The operation contract enumerates named, validated
transformations.

## Rationale

The dry-run/apply + audit + revert contract makes every transformation inspectable, idempotent,
and reversible — matching the "safe and auditable" requirement and the existing release-review
posture. The atomic write + fingerprint-check invariant prevents file corruption. The scope
limit (tag fields only, no path/filename/identity changes) keeps the operations safe against
the "never delete source media" and "never change identity" invariants.

## Consequences

### Positive

- Closes the W4 metadata-transformation workstream with a durable, testable contract.
- The admin SPA metadata edit forms (Phase 10 deferral) have a defined API.
- Audit + revert gives operators a safety net for batch tag rewrites.

### Negative / trade-offs

- Atomic file writes (temp + rename) require free disk space for the file's size during the
  write; on a near-full disk this can fail (surfaced as a failed apply, not corruption).
- A new migration (`media_edit_audit`) and new native routes add surface.

### Follow-up work

- Feature Completion Phase 2 (DATA / MEDIA): `melodee-tags` read/write for each supported
  container; each named operation with preview/apply/revert; the migration; API routes;
  admin SPA edit forms; fault-injection tests (disk full, permission denied, fingerprint
  mismatch).

## Validation

- Tag read/write round-trip tests for each supported container (FLAC, MP3, M4A, OGG, WAV):
  read → write → read produces the same values.
- Each named operation: preview produces the expected deltas; apply rewrites the file; the
  file's fingerprint changes; the audit row is written; a second apply is a no-op.
- Revert: apply then revert restores the original tag values and file fingerprint.
- Fault injection: a write that fails mid-way (simulated by a `rename` failure) leaves the
  original file untouched and the audit record marked failed.
- Concurrency: two concurrent applies to the same Release are serialized (one waits).
- No existing native route/DTO/status changes (golden diff).

## References

- [PRD.md](../../PRD.md) §11 (PRD-MEDIA-005/006), §45 (PRD-WORK-003), §47 (PRD-OBS-001),
  §50 (PRD-TEST-006)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
  (identity/contribution schema — transformations write to it)
- `src/Melodee.Common/Services/Scanning/MediaEditService.cs` (C# legacy, behavioral oracle)
- historical artifact removed (metadata edit forms deferral)
