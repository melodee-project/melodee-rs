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

:::caution[Planned documentation]
Connection settings, role/privilege guidance, and backup procedures will be documented and tested
as the Rust server's persistence layer lands.
:::
