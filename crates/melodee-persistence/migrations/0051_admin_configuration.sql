-- Existing installations retain their configured precedence until an administrator saves a key.
ALTER TABLE server_settings ADD COLUMN admin_preferred boolean NOT NULL DEFAULT false;

CREATE TABLE server_startup_overrides (
    key text PRIMARY KEY,
    value text NOT NULL,
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX release_publication_receipts_published_at_idx ON release_publication_receipts (published_at);
CREATE INDEX catalog_imports_imported_at_idx ON catalog_imports (imported_at);
