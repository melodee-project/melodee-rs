---
title: Upgrading
description: Upgrade a containerized Melodee server safely.
sidebar:
  order: 5
---

Upgrade by selecting a released GHCR image tag. Do not use `latest` for unattended production
updates unless you intentionally accept each new stable release automatically. Prefer an exact
`X.Y.Z` tag or image digest.

## Before the upgrade

1. Read the GitHub Release notes for the target version.
2. Record the current image digest and retain the current image tag for a binary rollback.
3. Enter a maintenance window and stop **every** Melodee server and worker that can write to this
   PostgreSQL database. Verify there is no old instance still serving or claiming work. Ordinary
   PostgreSQL index creation and backfills can block writers; startup migration is not an online
   schema-change mechanism.
4. Back up PostgreSQL and validate the backup before changing the container:

   ```bash
   docker compose exec -T postgres pg_dump -U melodee -d melodee --format=custom \
     > melodee-before-X.Y.Z.dump
   pg_restore --list melodee-before-X.Y.Z.dump >/dev/null
   ```

5. Keep the named volumes for PostgreSQL, canonical media, work, and search. Do not recreate or
   delete volumes as part of an image upgrade.

Keep the pre-upgrade image digest, PostgreSQL dump, configuration/secret-store version
identifiers, and persistent-volume inventory for the full acceptance window. A backup that has
not passed `pg_restore --list` and an isolated restore check is not a rollback asset.

## Upgrade

Set `MELODEE_IMAGE_TAG=X.Y.Z` in `.env`, then pull the image. Start exactly one candidate instance
while traffic remains fenced; that instance applies the embedded migration set. Do not start the
remaining replicas until its migration completes and schema history is verified:

```bash
docker compose pull melodee
docker compose up -d --no-deps melodee
docker compose logs --follow melodee
```

At startup, Melodee applies embedded SQLx migrations before opening the HTTP listener. Check both
health endpoints and your ordinary client/admin flows before ending the maintenance window:

```bash
curl --fail http://127.0.0.1:8080/healthz
curl --fail http://127.0.0.1:8080/readyz
docker compose exec melodee melodee-cli config validate
```

Migration checksums remain fail-closed. Melodee recognizes only the exact historical,
comment-only checksum variants of migrations 24–29 created by this repository and does not rewrite
their migration records. Any other `VersionMismatch(<number>)` means the applied migration differs
from the candidate image. Stop the upgrade, keep the database and backup intact, and use the
[migration-checksum diagnostics](../troubleshooting/#migration-checksum-mismatch); never make an
unknown mismatch disappear by editing `_sqlx_migrations`.

Also run `docker compose exec melodee melodee-cli doctor --deep`, verify search status, and run a
representative public-ID lookup, artwork request, range stream, and client journey. `/healthz`
proves process liveness; `/readyz` proves PostgreSQL and the configured search generation are
usable.

Migration `0038_catalog_genre_serving_projection.sql` performs an additive, one-time catalog genre
backfill. Large catalogs need extra maintenance-window, WAL, and temporary-space headroom. Never
pre-create its indexes concurrently: migration-owned objects and SQLx history must advance as one
reviewed unit. Migration `0039_native_normalized_search_indexes.sql` subsequently builds four
catalog-scale trigram GIN indexes; include that disk, WAL, and wall time in the same window. After
verification, start the remaining replicas and restore traffic.

## Rollback

An image rollback and a database-schema rollback are different operations. The current migrations
are additive, so an older image may not understand every newer feature even if it can start against
the upgraded database. Test the exact older image against an isolated copy of the database before
cutover; never drop Melodee tables in place just to make an older image start.

To restore a failed upgrade, stop the service, restore the verified pre-upgrade PostgreSQL backup
into a new or emptied database instance, point the deployment at that recovered database, set the
previous image tag, and validate migration history, catalog counts, `/readyz`, and representative
client playback before returning traffic. Keep the failed database and original backup until the
restored service is verified.

For a C# to Rust migration, use the [cutover and rollback runbook](https://github.com/melodee-project/melodee-rs/blob/main/design/runbooks/CUTOVER-AND-ROLLBACK.md).
The Rust `rollback` commands generate topology-bound instructions and record durable audit
evidence; they do not switch traffic, restore C# state, or delete the failed Rust target.

Release-owner acceptance is separate from an image upgrade. Keep the exact pre-upgrade and
candidate digests, validated backups, search rebuild result, differential checks, and client
acceptance record until the acceptance window closes. Do not publish or clean up rollback assets
while any migration, security, client, or rollback gate is only prepared or historical.

## Legacy C# migration

Migration from an existing C# Melodee installation is handled by the
[legacy migration tooling](./legacy-migration/): inventory, plan, import, verify, delta, and
status commands that adopt the canonical library in place. Dual run, final delta sync, cutover,
and demonstrable rollback against a production snapshot remain rehearsal-gated; do not treat a
container upgrade as a legacy-server migration.
