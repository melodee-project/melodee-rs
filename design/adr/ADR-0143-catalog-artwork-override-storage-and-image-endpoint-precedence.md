# ADR-0143: catalog artwork override storage and image endpoint precedence

- **Date:** 2026-08-15
- **Status:** Accepted
- **Decision owners:** Phase 10 track owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-UI-006, parity matrix artwork row (Phase 10 checklist 07)

## Context

Phase 10 WP-15 delivered the read side of catalog artwork: admin catalog DTOs carry `imageId`
values consumed by the frozen native `GET /images/{id}/{size}` endpoint
(`apps/melodee-server/src/native/images.rs`). For the `album_`, `artist_`, and `song_` kinds
that endpoint resolves a Release and serves the first `artwork`-kind asset recorded in the
**immutable publication plan** (`canonical_artwork_bytes`), read from the canonical storage
root through the no-follow `DirectPlayService` boundary; otherwise it serves the captured
placeholder bytes.

Closing the parity-matrix artwork row additionally requires *management* operations:
operator upload and removal of custom Artist/Release artwork from the admin UI
(`PUT/DELETE /api/admin/v1/catalog/artists/{id}/image` and
`.../releases/{id}/image`, validated via the existing `validate_uploaded_image` bounds).

A Phase 10 gap-closure survey (2026-08-15) established that this cannot be implemented on the
existing schema:

- No table stores per-Artist/per-Release custom images. `native_user_avatars` is keyed by
  user (FK to `native_users`), `native_playlists.image_bytes` is playlist-scoped, and the
  catalog schema (migration `0007_phase4_catalog.sql`) links `media_assets` to Tracks only
  (`track_media_assets`, purposes `playable`/`alternate`); there is no Artist/Release media
  link table and no image-override column.
- The frozen `/images/{id}/{size}` handler has no override hook: it always prefers
  publication-plan artwork, and the plan is deliberately immutable (sealed hashes,
  `catalog_semantic_overlays` chains, journal evidence).

Therefore the feature requires (1) a schema change (new storage for operator-uploaded artwork
associations and bytes) and (2) a behavior change to a frozen compatibility endpoint (custom
artwork must take precedence over publication-plan artwork for `album_`/`artist_`/`song_`
ids). Both are forbidden without an accepted ADR (repository rules: no schema without an
accepted schema ADR; no frozen-contract behavior change without compatibility evidence, a
product decision, and an accepted ADR). This ADR records the decision so the work can be
scheduled.

## Decision

1. **Storage.** Add a migration creating a catalog artwork override table, e.g.
   `catalog_entity_images (entity_type text CHECK (entity_type IN ('artist','release')),
   entity_id uuid NOT NULL, image_mime_type text NOT NULL, image_bytes bytea NOT NULL with a
   1–10 MiB bound, uploaded_by text NOT NULL, created_at/updated_at timestamptz NOT NULL,
   UNIQUE (entity_type, entity_id))`, with `entity_id` referencing `artists(id)` /
   `releases(id)` via separate typed columns or documented soft keys, plus rollback/forward
   notes and data-integrity tests. Upload bytes are normalized through the existing image
   pipeline bounds (`MAX_IMAGE_INPUT_BYTES`, dimension/pixel caps, magic-byte detection —
   the `avatars.rs` upload pattern).
2. **Endpoint precedence.** In `native/images.rs`, `load_image` for the `Album`, `Artist`,
   and `Song` kinds checks the override table first; a stored override is processed and served
   through the same bounded pipeline. Absence of an override preserves today's behavior
   exactly (publication-plan artwork, then placeholder). The compatibility evidence package
   must capture both branches.
3. **Admin surface.** `PUT /api/admin/v1/catalog/artists/{id}/image` and
   `PUT /api/admin/v1/catalog/releases/{id}/image` accept raw-body `image/jpeg`/`image/png`
   uploads (magic-byte validated, `DefaultBodyLimit`-bounded), and the paired `DELETE`s remove
   only the override row — canonical media files and publication plans are never touched
   (removal restores the plan/placeholder fallback). Every mutation is audit-logged with the
   acting administrator id (`catalog_audit_events`, entity types `artist`/`release`).
4. **Removal semantics.** `DELETE` is idempotent: `204` when an override existed, `404
   not_found` for unknown entity ids; deleting a nonexistent override returns `204` (absent
   state reached) or `404` per the endpoint contract decided in the implementing WP.

## Alternatives considered

### Reuse `media_assets` + new link tables

Store uploads as `media_assets` rows with new `artist_media_assets`/`release_media_assets`
link tables (purpose `artwork`). Rejected: `media_assets` rows are canonical-library facts
with `byte_hash` integrity semantics tied to publication; operator uploads are not canonical
library content, and entangling them with the playable-asset machinery (including the
deferred exactly-one-playable invariant) risks breaking ingestion invariants for a UI
convenience.

### Write uploads into the canonical storage root and reference them from the plan

Rejected outright: publication plans are immutable sealed evidence; mutating or extending
them for operator artwork would invalidate plan hashes and break the Phase 3–4 audit chain.

### Store overrides in `server_settings` / `catalog_audit_events` payloads

Rejected: settings and audit payloads are not blob stores; this would abuse append-only
evidence tables and bypass byte-bounds enforcement.

## Rationale

A dedicated override table keeps operator curation data clearly separated from canonical
publication evidence, gives DELETE a clean restore-fallback semantic (row removal only), and
keeps the frozen endpoint's precedence change to a single, test-covered lookup. The
placeholder/fallback branch of `/images/{id}/{size}` is untouched for every entity without an
override, preserving captured behavior by default.

## Consequences

### Positive

- Admin UI gains durable custom artwork without touching the publication pipeline.
- Native/OpenSubsonic/Jellyfin image consumers automatically see overrides through the one
  shared endpoint.
- Removal is non-destructive by construction: only the override row is deleted.

### Negative / trade-offs

- New schema (migration, rollback plan, data-integrity tests) and a deliberate, evidenced
  behavior change to a frozen endpoint — both require this ADR to be accepted first.
- Uploaded bytes live in PostgreSQL (bounded to 10 MiB per row), consistent with
  `native_user_avatars`/`native_playlists` precedent.
- ETag/caching behavior for overridden images must be covered by the compatibility capture
  (content-derived ETags already handle byte changes).

### Follow-up work

- On acceptance: migration + store methods, the four admin endpoints with audit logging,
  precedence change in `native/images.rs` with compatibility captures, admin UI
  upload/remove (DestructiveActionDialog-consistent confirm), docs update
  (`docs/src/content/docs/api/admin-api.md`), and the checklist-07 artwork row.

### Implementation evidence (2026-08-15)

Implemented as decided above; no deviations from the Decision section.

- Storage: `crates/melodee-persistence/migrations/0030_phase10_catalog_entity_images.sql`
  (typed artist/release FK columns per the §1 typed-column option; pair-uniqueness via
  partial unique indexes; jpeg/png + 1 B–10 MiB CHECK backstops; rollback `DROP TABLE
  catalog_entity_images;` documented in the migration header) and
  `crates/melodee-persistence/src/catalog_images.rs` (`CatalogImageStore`: bounded upsert,
  read, idempotent delete; each mutation writes its `catalog_audit_events` row in the same
  transaction with the acting administrator id). Data integrity:
  `catalog_entity_images_schema_enforces_bounds_and_store_round_trip` in
  `crates/melodee-persistence/tests/migrations.rs`.
- Endpoints: `PUT/DELETE /api/admin/v1/catalog/artists/{id}/image` and
  `.../releases/{id}/image` in `apps/melodee-server/src/admin_catalog.rs`. DELETE returns
  `204` whether or not an override existed and `404 not_found` for unknown entity ids
  (§4 first option). The route body limit admits 1 MiB of slack above the 10 MiB pipeline
  bound so oversized uploads earn the contract `400 invalid_image` instead of the
  limiter's bare `413`.
- Precedence: `load_image` in `apps/melodee-server/src/native/images.rs` checks
  `catalog_entity_images` first for the `Album`/`Artist`/`Song` kinds (`song_` resolves to
  its Release's override); the no-override branch is byte-identical to the captured
  behavior — the frozen image golden suite
  (`apps/melodee-server/tests/native_catalog_api.rs`) is unchanged and green.
- Validation evidence: `apps/melodee-server/tests/admin_artwork_api.rs` (5 tests —
  upload→serve round trip, delete→fallback restore with canonical-file/plan survival
  assertions, 401/403, 404, 400 invalid/truncated/mismatched/dimension-bomb/oversized,
  audit attribution); admin UI in `admin-ui/src/screens/catalog/ArtworkManager.tsx` (unit
  tests 7, e2e 3 in `catalog.spec.ts`); docs in
  `docs/src/content/docs/api/admin-api.md`; checklist row:
  withdrawn `design/evidence/phase-10/checklists/07-artist-release-detail.md` evidence
  (artwork row ✅).

### Post-acceptance adapter completion (2026-08-15)

The phrase "one shared endpoint" in the Consequences section was imprecise: ADR-0102 requires
Native, OpenSubsonic, and Jellyfin to remain separate adapters, so they cannot share an HTTP
endpoint or wire DTO. They share the bounded artwork sources and precedence rule instead. The
initial implementation evidence proved the native endpoint branch; a subsequent integration
review completed the corresponding adapter reads without changing any no-artwork captured branch:

- `opensubsonic/media.rs` resolves `artist_` and `album_` IDs to their override and maps a
  `song_` ID to its Release override before retaining the captured default-artwork fallback.
- `jellyfin/images.rs` resolves an override, then immutable publication-plan artwork through
  `DirectPlayService`, and processes PNG/JPEG output through the shared bounded image pipeline
  for its existing size buckets.
- `opensubsonic_media_api.rs`
  `get_cover_art_catalog_overrides_are_served_for_artist_release_and_song` and
  `jellyfin_media_api.rs`
  `images_serve_artist_and_release_overrides_on_all_catalog_routes` cover the additional
  routes, inheritance, cache behavior, HEAD, and size processing;
  `images_fall_back_to_published_canonical_artwork` covers the immutable-plan branch through
  the no-follow canonical root.

## Validation

- Upload → `GET /images/{kind}_{id}/{size}` serves the uploaded bytes (resized pipeline
  output); invalid/truncated/oversized/dimension-bomb uploads → `400`.
- Delete → subsequent detail and `/images/...` restore plan artwork or the placeholder; no
  canonical file is deleted (filesystem assertion).
- Golden/capture diff for `/images/{id}/{size}` proving unchanged behavior for entities
  without overrides.
- 401/403 gating and `404 not_found` for unknown ids on all four endpoints.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- `apps/melodee-server/src/native/images.rs` (frozen image endpoint, upload validation bounds)
- `apps/melodee-server/src/native/avatars.rs` (raw-body upload pattern)
- `crates/melodee-persistence/migrations/0007_phase4_catalog.sql` (catalog schema survey)
