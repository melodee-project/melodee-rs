-- native user avatar store (ADR-0134 §2).
--
-- One avatar row per user. `updated_at` drives avatarLastChanged semantics for
-- the OpenSubsonic user model. Group/ACL state is unchanged: migration 0010
-- already models it; group CRUD is an API gap, not a schema gap.
--
-- This migration is additive. A binary downgrade leaves the table intact;
-- schema rollback restores a verified pre-migration backup into a new database.

CREATE TABLE native_user_avatars (
    user_id uuid PRIMARY KEY REFERENCES native_users (id) ON DELETE CASCADE,
    content_type text NOT NULL,
    image bytea NOT NULL,
    updated_at timestamptz NOT NULL,

    CONSTRAINT native_user_avatars_content_type_check CHECK (
        content_type IN ('image/jpeg', 'image/png')
    ),
    CONSTRAINT native_user_avatars_image_check CHECK (
        octet_length(image) BETWEEN 1 AND 1048576
    )
);
