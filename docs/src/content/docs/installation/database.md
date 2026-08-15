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
