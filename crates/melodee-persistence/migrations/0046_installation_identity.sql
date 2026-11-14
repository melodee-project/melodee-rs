-- ADR-0192: installation identity belongs to the database backup, not a process.
-- Additive migration: retain this singleton on binary downgrade. A deliberately
-- independent clone rotates only this ID before serving Jellyfin clients.
CREATE TABLE server_installation_identity (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    installation_id uuid NOT NULL UNIQUE DEFAULT uuidv7(),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

INSERT INTO server_installation_identity (singleton) VALUES (true);
