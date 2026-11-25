-- ADR-0210. Each converted reference must have a verified filesystem receipt.
-- The rewrites release the old TOAST data when this transaction commits.
CREATE FUNCTION melodee_migrated_asset_key(payload bytea) RETURNS text
LANGUAGE sql IMMUTABLE STRICT AS $$
    SELECT encode(sha256(payload), 'hex') || '-' || octet_length(payload)::text
$$;

ALTER TABLE catalog_provider_images DROP CONSTRAINT catalog_provider_image_size_check;
ALTER TABLE catalog_provider_images ALTER COLUMN image_bytes TYPE text USING melodee_migrated_asset_key(image_bytes);
ALTER TABLE catalog_provider_images RENAME COLUMN image_bytes TO asset_key;
ALTER TABLE catalog_provider_images ADD FOREIGN KEY (asset_key) REFERENCES asset_files(asset_key);

ALTER TABLE catalog_entity_images DROP CONSTRAINT catalog_entity_images_image_bytes_check;
ALTER TABLE catalog_entity_images ALTER COLUMN image_bytes TYPE text USING melodee_migrated_asset_key(image_bytes);
ALTER TABLE catalog_entity_images RENAME COLUMN image_bytes TO asset_key;
ALTER TABLE catalog_entity_images ADD FOREIGN KEY (asset_key) REFERENCES asset_files(asset_key);

ALTER TABLE native_user_avatars DROP CONSTRAINT native_user_avatars_image_check;
ALTER TABLE native_user_avatars ALTER COLUMN image TYPE text USING melodee_migrated_asset_key(image);
ALTER TABLE native_user_avatars RENAME COLUMN image TO asset_key;
ALTER TABLE native_user_avatars ADD FOREIGN KEY (asset_key) REFERENCES asset_files(asset_key);

ALTER TABLE native_theme_assets DROP CONSTRAINT native_theme_assets_size;
ALTER TABLE native_theme_assets ALTER COLUMN bytes TYPE text USING melodee_migrated_asset_key(bytes);
ALTER TABLE native_theme_assets RENAME COLUMN bytes TO asset_key;
ALTER TABLE native_theme_assets ADD FOREIGN KEY (asset_key) REFERENCES asset_files(asset_key);

ALTER TABLE native_playlists DROP CONSTRAINT native_playlists_image_check;
ALTER TABLE native_playlists ALTER COLUMN image_bytes TYPE text USING melodee_migrated_asset_key(image_bytes);
ALTER TABLE native_playlists RENAME COLUMN image_bytes TO asset_key;
ALTER TABLE native_playlists ADD FOREIGN KEY (asset_key) REFERENCES asset_files(asset_key);

ALTER TABLE native_users DROP CONSTRAINT native_users_image_check;
ALTER TABLE native_users ALTER COLUMN image_bytes TYPE text USING melodee_migrated_asset_key(image_bytes);
ALTER TABLE native_users RENAME COLUMN image_bytes TO asset_key;
ALTER TABLE native_users ADD FOREIGN KEY (asset_key) REFERENCES asset_files(asset_key);

ALTER TABLE native_user_avatars ADD CHECK (split_part(asset_key, '-', 2)::integer <= 1048576);
ALTER TABLE native_users ADD CHECK ((image_mime_type IS NULL) = (asset_key IS NULL)
    AND (image_mime_type IS NULL OR btrim(image_mime_type) <> ''));
ALTER TABLE native_playlists ADD CHECK ((image_mime_type IS NULL) = (asset_key IS NULL)
    AND (image_mime_type IS NULL OR image_mime_type IN ('image/jpeg','image/png','image/webp')));
DROP FUNCTION melodee_migrated_asset_key(bytea);
