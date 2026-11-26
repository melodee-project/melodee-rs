---
title: Upgrading
description: Upgrade a containerized Melodee server safely.
sidebar:
  order: 5
---

Upgrade by selecting a released GHCR image tag. Do not use `latest` for unattended production
updates unless you intentionally accept each new stable release automatically. Prefer an exact
`X.Y.Z` tag or image digest.

During `0.x`, a minor update can require migration steps. From `1.0.0` onward, a major update
signals an incompatible change to a supported interface. See the [Melodee versioning policy](https://github.com/melodee-project/melodee-rs/blob/main/VERSIONING.md).

## Moving existing artwork to file storage

Before upgrading past database migration 0053, provision the persistent asset directory for every
server, worker and file-importing CLI. Stop writers, take a full database backup and test-restore
it. Allow free space for the original payloads on the asset filesystem and temporary table
rewrites and WAL on PostgreSQL. Run `melodee-cli migrate` as the service user with the same
configuration as the server. It copies one payload at a time, verifies a fresh read of every
file, then atomically replaces the six binary columns with references. An interrupted copy can
be retried against the same store. A missing or corrupt file fails verification. Preserve the
original and investigate the filesystem before retrying.

For subsequent backups, take the database snapshot first, then back up the entire immutable
asset directory including its marker. Retain both for the same restore point. Test restoration
on a separate database and directory and retrieve a known avatar or cover. Rolling back across
0055 requires restoring the **pre-upgrade database** and the matching old server revision.
[Asset storage](./storage/)


## Performance remediation upgrade (migrations 0060–0063)

This upgrade adds predicate-aligned indexes, corrects concurrent genre aggregate maintenance,
makes discovery maintenance progress durable, and changes the search rebuild activation protocol so
a late-committing catalog change can no longer be omitted from the new search generation.

1. Follow **Before the upgrade** below: stop every server and worker that writes to this database,
   take a validated PostgreSQL backup, and keep the pre-upgrade image digest.
2. Start exactly one candidate instance so it applies migrations `0060`–`0063`. These are ordinary
   transactional migrations, so expect write unavailability for the index build; the maintenance
   fence is required.
3. Verify the genre serving aggregates and repair existing drift. The check is read-only; the
   repair is idempotent, bounded, and resumable:

   ```bash
   docker compose exec melodee melodee-cli catalog repair-genres
   docker compose exec melodee melodee-cli catalog repair-genres --apply
   docker compose exec melodee melodee-cli catalog repair-genres
   ```

   The final check must report `genre aggregates are consistent`.
4. Rebuild the search projection once with the corrected activation protocol. The previous
   generation stays active until the replacement validates and activates:

   ```bash
   docker compose exec melodee melodee-cli search rebuild --actor upgrade
   docker compose exec melodee melodee-cli search status
   ```

   Do not repair search by replaying only currently pending outbox rows: events already processed
   against the previous generation may be missing from it, and the corrected rebuild replays them.
5. Durable discovery progress initializes itself; no manual step is required. The discovery worker
   resumes each work class from its stored cursor instead of restarting at the Track prefix.
6. Verify health and one ordinary import before ending the maintenance window, then resume workers
   and replicas.

**Do not** run an older binary against a database migrated to `0063`. The combined schema and
application change is a stop-all-writers upgrade; mixed old/new workers are unsupported for this
release. Rolling back requires restoring the pre-upgrade database and the matching old image.

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

Migration `0040_repair_multi_artist_release_track_identities.sql` repairs unverified Track Artist
identities created by the earlier singleton-only matching rule. It acts only when one primary
Release Artist on the same Release exactly matches the Track observation. Raw observations and
prior decisions remain; repaired sources become audited merge tombstones. Verify the active Artist
count and representative multi-Artist Releases after startup. Reversing the migration requires an
audited Artist split or restoring the pre-upgrade PostgreSQL backup—it is not rolled back by merely
starting an older image.

Migration `0044_rename_artist_status_provisional_to_active.sql` relabels the Artist identity state
`provisional` to `active` (ADR-0190) and tightens `artists_status_check`. It rewrites only the status
column: public IDs, merge tombstones, decisions, evidence, aggregates, and audit history are untouched.
Migration `0045_repair_artist_projection_and_overlay_references.sql` then advances live Artist
revisions and queues search updates, including for catalogs that already applied `0044`. Wait for
search projection work to catch up before checking status filters; a full rebuild and media
re-ingestion are not required.

Migration `0045` also identifies current import snapshots with inconsistent Artist references.
Startup repairs these in bounded transactions by appending corrected snapshots and retaining the
original evidence. Interrupted repairs resume on the next startup; a repair failure prevents startup
from completing and must be resolved before opening traffic. Include the Artist revision/outbox
update and any snapshot repair in maintenance-window and WAL headroom. Verify the runtime audit and
representative Artist searches afterward. An older image expects the previous schema/vocabulary,
so restore the pre-upgrade database backup with the previous image for rollback; do not delete
snapshot history or decrement entity revisions.

## Manifest evidence migration (0059) and search-outbox retention (0058)

Migration `0059_manifest_evidence_files.sql` adds the manifest evidence reference, compact query
facts, resumable progress and per-row receipts, and makes the inline `manifest_payload` column
nullable. It does **not** remove inline bytes. The upgrade then runs, through the normal
`melodee-cli migrate` entry point (or the same gate at server startup):

1. **backfill** — up to 50 manifests per checkpoint in stable ID order, one large manifest in
   flight, each object read back and compared with its inline source before a receipt is written;
2. **independent verification** — every evidence file read from its final path, SHA-256 and length
   checked, reconstructed JSON compared with any remaining inline payload, and recomputed query
   facts compared with the stored facts;
3. **finalize** — inline payloads cleared only after that check, per row, fenced by the manifest
   hash.

Read-only operator commands are safe before and after the upgrade:

```bash
docker compose exec melodee melodee-cli migrate --preflight
docker compose exec melodee melodee-cli migrate --status
docker compose exec melodee melodee-cli migrate --verify
```

`--preflight` reports migration currency, the bound asset store and its root, headroom, manifest and
evidence byte counts, search-outbox backlog and resolved provider eligibility. `--status` reports
resumable progress. `--verify` runs the independent check and **exits nonzero** on any discrepancy.
A populated SQL-only upgrade that lacks verified receipts fails closed; do not clear inline payloads
by hand.

Space and time: allow free space on the asset filesystem for the full serialized manifests (larger
than the JSONB they replace, because serialization is not compressed) on top of the existing store,
and PostgreSQL headroom for WAL and the small fact/receipt tables. The inline `jsonb`/TOAST bytes are
released only when the finalize transaction commits. An interrupted backfill resumes from the last
committed row; the source remains authoritative until then.

An interrupted upgrade is recovered by re-running `melodee-cli migrate`. Retrying with the candidate
is safe. To return to the previous binary instead, restore the **pre-upgrade database backup and the
matching pre-upgrade asset store directory**, because the schema and evidence references move
together.

Migration `0058_search_outbox_retention.sql` is additive. It adds exact hourly totals, per-projection
maintenance state and bounded replay tombstones; successful detail starts expiring under the
configured retention after the first hourly maintenance pass. Perfectly safe before the first pass;
the chart keeps serving from raw detail until each hour is finalized.

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

## Authentication and installation identity changes

OpenSubsonic requires credentials by default even for loopback requests or a matching Host header.
If a proxy forwards requests to the server, configure `MELODEE__SERVER__TRUSTED_PROXIES` for the
actual proxy addresses; the historical `MELODEE__NATIVE__TRUSTED_PROXIES` name remains an alias.
The explicit `MELODEE__OPENSUBSONIC__LEGACY_AUTH_BYPASS=true` compatibility option restores the
historical bypass and should only be used in an appropriately isolated deployment. See
[OpenSubsonic authentication](../api/opensubsonic/) and [configuration](./configuration/).

Jellyfin-compatible discovery now reports a persistent installation identity instead of a shared
constant. Existing clients may need to reconnect once after this upgrade. Keep the installation
identity when restoring the same installation; independent clones need distinct identities. See
[the Jellyfin guide](../api/jellyfin/) for the supported clone procedure and client impact.
