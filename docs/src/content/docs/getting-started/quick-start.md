---
title: Quick Start
description: Get a Melodee server running and ingest your first library.
sidebar:
  order: 2
---

:::caution[Planned documentation]
The Rust rewrite of Melodee is pre-release and has no published server distribution yet. The steps
below describe the intended quick-start flow and will be validated against a real release before
they are presented as working instructions. For the currently shipping server, see the legacy
[Melodee repository](https://github.com/melodee-project/melodee).
:::

The quick start will cover:

1. Starting Melodee with Docker and PostgreSQL.
2. Pointing Melodee at an inbound media directory and library storage.
3. Creating the first administrator account.
4. Reviewing and accepting inbound Releases into the catalog.
5. Connecting a client.

Each step will be validated end-to-end against a shipped release before this page loses its
planned status.

## First-run onboarding (implemented)

The server carries an implemented first-run preflight that the steps above will use:

1. `melodee users bootstrap-admin --username <name> --email <email>` creates the first
   administrator (the password comes from `MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD`, never a
   command-line argument).
2. `melodee config validate` runs the preflight checks — database connectivity and migrations,
   storage/library paths, media tools (`ffmpeg`/`ffprobe`), search-index readiness, administrator
   account, and email sanity — each with a remediation hint, exiting non-zero while a required
   check fails.
3. Administrators can read the same evaluation over `GET /api/admin/v1/onboarding/status` and
   record durable completion with `POST /api/admin/v1/onboarding/complete` (see the
   [Admin API](../api/admin-api/)). Servers migrated from a legacy installation bypass onboarding
   automatically when the database shows an existing library or imported settings.

The graphical setup wizard remains planned and will be documented when the admin UI ships.

