---
title: Configuration
description: Melodee server configuration sources and settings reference.
sidebar:
  order: 2
---

:::caution[Planned documentation]
The Rust server's configuration surface is still being implemented (configuration layering is a
Phase 1 deliverable). Settings documented here will be validated against the shipped server.
:::

Melodee configuration is layered, in increasing order of precedence:

1. built-in defaults;
2. configuration file;
3. environment variables;
4. database-backed settings, where a setting is runtime-editable.

The configuration reference will document every supported setting, its default, and its effect.
New settings introduced by a change must be documented here in the same change.

## Implemented settings

The following environment keys are implemented today (durable work execution, Phase 1A). Only
explicitly documented keys are read — the server never absorbs arbitrary environment variables.

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__DATABASE__URL` | — (required) | PostgreSQL connection URL. Never logged (may contain a password). |
| `MELODEE__WORK__LEASE_SECONDS` | `300` | Duration of a work-item lease claimed by a worker. Expired leases become recoverable automatically. |
| `MELODEE__WORK__RETRY_BASE_SECONDS` | `60` | Base delay for exponential retry backoff (`base * 2^(attempt-1)`). |
| `MELODEE__WORK__RETRY_MAX_SECONDS` | `3600` | Maximum retry backoff delay. |
| `MELODEE__WORK__MAX_ATTEMPTS` | `5` | Automatic attempts before a Release work item parks for operator attention. |
| `MELODEE__WORK__CLAIM_BATCH_SIZE` | `4` | Maximum work items claimed per worker poll (bounded queue fetch). |

The `melodee-cli` binary in the repository provides development diagnostics for this
infrastructure (`migrate`, `work list`, `work show <id>`). It is not yet a shipped
administrative tool; its user-facing command set will be documented when it ships.
