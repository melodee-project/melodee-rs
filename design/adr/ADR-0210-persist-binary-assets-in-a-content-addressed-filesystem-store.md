# ADR-0210: Persist binary assets in a content-addressed filesystem store

- **Date:** 2026-11-25
- **Status:** Accepted
- **Decision owners:** Melodee project maintainer
- **Related decisions:** [ADR-0134](./ADR-0134-phase-9-parity-domain-schema-for-radio-device-profiles-onboarding-notifications-scripts-and-providers.md) and [ADR-0143](./ADR-0143-catalog-artwork-override-storage-and-image-endpoint-precedence.md)

## Context

Provider artwork alone occupied 1.8 GiB in the production database. The avatar,
playlist, theme and catalog curation implementations also allowed full payloads in PostgreSQL.

## Decision

All file payloads belong on configuration-driven persistent storage. This revises the physical
storage decisions in ADR-0134 and ADR-0143. The authorization, precedence, audit and public
serving contracts continue to apply. PostgreSQL records an immutable SHA-256 and length key,
ownership, MIME type and provenance. SQLx migration checksums and authentication material are
fixed-purpose metadata. File payloads encoded into text or JSON also violate the storage rule.

The service publishes files without replacement and verifies the final contents before
registering each key. Foreign keys require a receipt before an owner can reference a file.
A store identity marker binds database backups to their asset store. Independent writers
use their existing database transaction or register the file before committing its owner.
A failed transaction can leave a harmless unreferenced immutable file.

## Consequences

Operators must provision and back up this directory along with the database. Deleting or
replacing a reference retains the file for backups. The application does not garbage collect
it. Existing canonical media follows its publication lifecycle. A database-only restore taken
after migration cannot recover artwork.

## Migration and validation

Run the migration with the old writers stopped. Migration 0054 installs the receipt table.
The Rust migrator reads the old columns through bounded cursors, installs and verifies immutable
files, then migration 0055 derives each reference from the actual database bytes, requires the
verified receipt and rewrites the six tables in one transaction. SQL-only migration of a populated
database fails if receipts are missing. Restore an independently verified pre-upgrade database
to use old binaries. The migration tests cover file identity, idempotence, failed verification,
payload-column removal and the cryptographic metadata allowlist.
