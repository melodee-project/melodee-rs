---
title: Database
description: PostgreSQL requirements and setup for Melodee.
sidebar:
  order: 4
---

Melodee uses PostgreSQL as its authoritative state store.

- **Required:** PostgreSQL 18 (PostgreSQL 17 is a supported target).
- All catalog, identity, queue, and policy state lives in PostgreSQL.
- The search index is a rebuildable projection; losing it never requires re-ingesting media.

Schema changes ship as migrations with rollback plans. Upgrades apply migrations before the HTTP
listener opens, but this is not an online-migration guarantee: ordinary PostgreSQL `CREATE INDEX`
and backfills can block concurrent writers. Put every Melodee server and worker that can write to
the database behind a maintenance fence before starting one candidate instance to migrate. See
[Upgrading](./upgrading/).

The reference Compose deployment provisions the `melodee` database and persists PostgreSQL under
the `melodee_postgres` named volume. For an external database, create a dedicated database and
least-privilege application role, keep `MELODEE__DATABASE__URL` in your deployment secret store,
and use TLS/network controls appropriate to your environment. The server applies its embedded SQLx
migrations before it starts listening, so a failed migration prevents readiness rather than serving
against an unknown schema.

Migration `0038_catalog_genre_serving_projection.sql` is additive but performs a one-time scan of
catalog manifests to build indexed native/OpenSubsonic genre serving facts. On a large catalog,
budget the maintenance window and PostgreSQL temporary/WAL space for that backfill. Do not create
the migration-owned indexes manually with `CREATE INDEX CONCURRENTLY`; doing so diverges SQLx
migration history from the schema. Migration `0039_native_normalized_search_indexes.sql` then builds
four trigram GIN indexes over normalized Artist/Release/Track search facts; include their build time
and disk/WAL growth in the same maintenance budget. If either migration fails, keep traffic fenced,
retain the failed database for diagnosis, and restore the verified pre-upgrade backup into an
isolated target.

Back up PostgreSQL before every image upgrade. The [upgrade guide](./upgrading/) gives the
container-oriented backup, validation, and rollback sequence.

## Backup and isolated restore

Use a custom-format PostgreSQL dump and validate its table of contents before changing a
deployment. Keep the exact dump and its protected checksum in the change record; do not put a
database URL, password, private hostname, or secret value in documentation or committed evidence.

```bash
export BACKUP_FILE=/approved/backup/location/melodee-before-X.Y.Z.dump
pg_dump --dbname="$MELODEE__DATABASE__URL" --format=custom --file="$BACKUP_FILE"
pg_restore --list "$BACKUP_FILE" > "$BACKUP_FILE.list"
```

Restore into an isolated database or disposable PostgreSQL instance before relying on the backup.
Verify catalog/user/settings counts, public IDs, media references, migrations, `/healthz`,
`/readyz`, a PostgreSQL-backed search rebuild, compatibility probes, and representative playback.
Restore PostgreSQL before rebuilding Tantivy: the search directory is a rebuildable projection and
is never the authority. Retain the pre-change database, persistent-volume manifests, and secret-
store version identifiers until acceptance signoff and the documented retention period.

The [legacy migration runbook](https://github.com/melodee-project/melodee-rs/blob/main/design/runbooks/CUTOVER-AND-ROLLBACK.md) adds the
source-snapshot, Rust-target, final-delta, freeze, and rollback retention checks for a C# to Rust
migration. Production backup/restore and rollback execution remain operator-approved actions.

## Reset the development database

:::danger
`make postgres-reset` permanently drops and recreates the `melodee` application database in the
fixed local development PostgreSQL container. It removes all users, catalog data, settings, and
audit state stored in that database. It preserves the Docker volume and does not delete media
directories or compatibility captures. Never use it for a database containing data you need.
:::

From the repository root, reset PostgreSQL to a new empty database with the current schema:

```bash
make postgres-reset
```

The command verifies that `compose.dev.yaml` contains only the expected `postgres` service and
`melodee_pg_data` volume. It then starts PostgreSQL, waits for its health check, force-closes
connections to the development application database, recreates that database, and applies every
current migration. It does not stop the Compose project or ask Docker to delete a volume. The
resulting application database has no administrator; create the first administrator before running
`melodee-cli config validate`.

For a release decision, a successful local backup or restore is not enough: retain the verified
source and target backups through the acceptance window, verify the restored PostgreSQL authority,
rebuild search from PostgreSQL, and keep the failed Rust target for rollback forensics. Cleanup is
a separate, named, approval-gated action.

If Docker volume operations are unresponsive, the command stops after ten seconds without changing
the database and asks you to restart Docker Engine or Docker Desktop before retrying.
