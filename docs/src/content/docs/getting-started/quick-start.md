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

The admin UI carries a guided first-run wizard at **`/admin/onboarding`**. Sign in with an
administrator account; while onboarding is incomplete, the [Overview](../admin/overview/)
dashboard also shows a banner linking to it.

The wizard walks the six preflight checks — database connectivity and migrations, storage/library
paths, media tools (`ffmpeg`/`ffprobe`), search-index readiness, administrator account, and email
sanity — each with a pass/fail state, operator-safe detail, and a remediation hint when failing.
The checks evaluate live on the server every time the page loads or refreshes, so fixing a problem
and re-running shows the real result. Completion is **fail-closed**: the finish action is enabled
only when every required check passes, and the server re-evaluates on completion — a regression
between display and submit is rejected and the failing checks are shown again. Servers migrated
from a legacy installation bypass onboarding automatically when the database shows an existing
library or imported settings; the wizard detects that state and redirects to the dashboard.

The equivalent headless path:

1. `melodee users bootstrap-admin --username <name> --email <email>` creates the first
   administrator (the password comes from `MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD`, never a
   command-line argument).
2. `melodee config validate` runs the same preflight checks, exiting non-zero while a required
   check fails.

The same evaluation is available over `GET /api/admin/v1/onboarding/status` and durable
completion over `POST /api/admin/v1/onboarding/complete` (see the
[Admin API](../api/admin-api/#onboarding)).

