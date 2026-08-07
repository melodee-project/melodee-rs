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
