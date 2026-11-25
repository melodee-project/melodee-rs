-- ADR-0210. Additive preparation for a verified, restartable migration.
CREATE TABLE asset_store_binding (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    store_id uuid NOT NULL
);
CREATE TABLE asset_files (
    asset_key text PRIMARY KEY,
    size_bytes integer NOT NULL CHECK (size_bytes BETWEEN 1 AND 10485760),
    CHECK (asset_key ~ '^[0-9a-f]{64}-[1-9][0-9]{0,7}$'
           AND split_part(asset_key, '-', 2) = size_bytes::text)
);
