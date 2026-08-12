---
title: Upgrading
description: How to upgrade a Melodee server, including migrations and rollback.
sidebar:
  order: 5
---

:::caution[Planned documentation]
The Rust rewrite is pre-release; no upgrade path between Rust releases exists yet. Migration from
the C# server is planned for the final rewrite phase and will be documented here with dry-run,
rollback, and backup guidance before any migration tool ships.
:::

Upgrade documentation will cover, for each release:

- applying the new version and its database migrations;
- rollback behavior and backups required before upgrading;
- migration guidance when moving from the legacy C# server, preserving public IDs and user state.

## Phase 7 development migrations

Pre-release development builds apply migrations `0010` through `0019` automatically at server
startup. They add the native authentication and public-ID authority, per-user catalog state,
community and podcast state, genre preferences, discovery state, playlists, endpoint sessions,
catalog browse indexes, and bounded route-state constraints. These migrations do not read, move, or
delete music-library media. If the podcast worker is enabled, retention can remove only downloaded
enclosures beneath its separately configured, marker-owned podcast root.

Create and verify a backup before applying a Phase 7 development build to a database that matters:

```bash
pg_dump --format=custom --file=/safe/backup/location/melodee-pre-0010.dump "$MELODEE__DATABASE__URL"
pg_restore --list /safe/backup/location/melodee-pre-0010.dump >/dev/null
cargo run -p melodee-cli -- migrate
psql "$MELODEE__DATABASE__URL" --set ON_ERROR_STOP=1 --command \
  "SELECT version, description, success FROM _sqlx_migrations WHERE version BETWEEN 10 AND 19 ORDER BY version"
```

A binary downgrade leaves the additive Phase 7 relations in place. Test the exact older binary
against an isolated database copy before cutover; do not drop Phase 7 tables in place. Full schema
rollback means draining writes, restoring the verified pre-`0010` backup into a new empty database,
checking migration history and catalog counts, and only then repointing the service. Keep both the
post-upgrade database and backup until the restored service passes health and client checks.
