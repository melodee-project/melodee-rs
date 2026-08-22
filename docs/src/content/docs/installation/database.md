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

Schema changes ship as migrations with rollback plans. Upgrades apply migrations automatically at
startup; see [Upgrading](./upgrading/).

The reference Compose deployment provisions the `melodee` database and persists PostgreSQL under
the `melodee_postgres` named volume. For an external database, create a dedicated database and
least-privilege application role, keep `MELODEE__DATABASE__URL` in your deployment secret store,
and use TLS/network controls appropriate to your environment. The server applies its embedded SQLx
migrations before it starts listening, so a failed migration prevents readiness rather than serving
against an unknown schema.

Back up PostgreSQL before every image upgrade. The [upgrade guide](./upgrading/) gives the
container-oriented backup, validation, and rollback sequence.

## Reset the development database

:::danger
`make postgres-reset` permanently deletes the `melodee-rs-dev_melodee_pg_data` development volume.
It removes all users, catalog data, settings, and audit state stored there. It does not delete media
directories or compatibility captures. Never use it for a database containing data you need.
:::

From the repository root, reset PostgreSQL to a new empty database with the current schema:

```bash
make postgres-reset
```

The command verifies that `compose.dev.yaml` contains only the expected `postgres` service and
`melodee_pg_data` volume before deleting anything. It then starts PostgreSQL, waits for its health
check, and applies every current migration. The resulting application database has no administrator;
create the first administrator before running `melodee-cli config validate`.
