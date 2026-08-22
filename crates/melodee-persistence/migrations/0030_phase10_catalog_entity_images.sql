-- catalog artwork overrides (accepted ADR-0143; parity matrix
-- artwork row, checklist 07). Stores operator-uploaded custom
-- Artist/Release artwork that the frozen native GET /images/{id}/{size}
-- endpoint serves with precedence over publication-plan artwork (ADR-0143
-- §2). Rows are operator curation data, not canonical library facts: no
-- media_assets entanglement, no publication-plan mutation.
--
-- ADR-0143 §1 typed-column option: exactly one of artist_id / release_id is
-- set and must match entity_type; the FKs keep overrides from dangling and
-- ON DELETE CASCADE retires an override when its entity disappears. The
-- pair-uniqueness guarantee (ADR-0143 §1 UNIQUE(entity_type, entity_id)) is
-- expressed as two partial unique indexes over the typed columns.
--
-- Byte bounds mirror the upload pipeline: 1 byte..10 MiB
-- (MAX_IMAGE_INPUT_BYTES in apps/melodee-server/src/native/images.rs) and
-- the jpeg/png magic-byte gate; the CHECK constraints are the fail-closed
-- backstop behind application-side validation.
--
-- Additive and safe for binary downgrade: no existing table, constraint, or
-- identity is touched. Rollback (ADR-0143 §1): DROP TABLE
-- catalog_entity_images; — every override disappears and image serving falls
-- back to publication-plan artwork or the placeholder. Rollback never
-- touches canonical media files or publication plans.

CREATE TABLE catalog_entity_images (
    entity_type text NOT NULL CHECK (entity_type IN ('artist', 'release')),
    artist_id uuid REFERENCES artists (id) ON DELETE CASCADE,
    release_id uuid REFERENCES releases (id) ON DELETE CASCADE,
    image_mime_type text NOT NULL
        CHECK (image_mime_type IN ('image/jpeg', 'image/png')),
    image_bytes bytea NOT NULL
        CHECK (octet_length(image_bytes) BETWEEN 1 AND 10485760),
    uploaded_by text NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK (
        (entity_type = 'artist' AND artist_id IS NOT NULL AND release_id IS NULL)
        OR (entity_type = 'release' AND release_id IS NOT NULL AND artist_id IS NULL)
    )
);

CREATE UNIQUE INDEX catalog_entity_images_artist_uq
    ON catalog_entity_images (artist_id) WHERE artist_id IS NOT NULL;
CREATE UNIQUE INDEX catalog_entity_images_release_uq
    ON catalog_entity_images (release_id) WHERE release_id IS NOT NULL;
