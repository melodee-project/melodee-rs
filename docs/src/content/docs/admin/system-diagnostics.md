---
title: System and Diagnostics
description: Health, workers, and diagnostics for operating Melodee.
sidebar:
  order: 6
---

The Rust server exposes service health and version metadata plus durable queue diagnostics. These
surfaces are available today even though the graphical admin diagnostics screen is not yet built.

## Service health

```http
GET /healthz
```

The route returns HTTP 200 with service, version, and status fields:

```json
{
  "service": "melodee-server",
  "version": "0.1.0",
  "status": "ok"
}
```

The same metadata is available to Admin API consumers at
`GET /api/admin/v1/system/info`.

## Service readiness

```http
GET /readyz
```

The readiness route performs a bounded PostgreSQL query. When the search worker is enabled it also
requires a readable, schema-compatible active search generation. It returns HTTP 200 with
`status: "ready"` when those configured dependencies answer and HTTP 503 with
`status: "not_ready"` otherwise.
The server also connects to PostgreSQL and applies migrations before binding its listener.

It does not test media tools, disk space, or every worker process; use the diagnostics below for
those checks.

## Queue and lease diagnostics

The implemented [Admin API](../api/admin-api/) provides:

- per-state work-item counts;
- bounded, filtered work-item listings;
- one work item's state, attempts, timestamps, and last error;
- append-only transition history; and
- active, unexpired lease evidence.

An active lease proves that PostgreSQL contains an unexpired lease. It does not prove that the
owner process is currently alive.

## Command-line diagnostics

- `melodee doctor` checks whether `ffmpeg`, `ffprobe`, and `fpcalc` are executable and whether the
  database URL variable is present. It does not connect to PostgreSQL.
- `melodee work list` and `melodee work show <id>` inspect durable queue state and event history.
- `melodee ingest status` prints a concise ingestion queue summary.
- `melodee catalog status` prints bounded catalog entity/outbox counts and the effective worker
  limits.
- `melodee catalog inspect <id>` resolves a work-item ID or API key and prints its immutable catalog
  import receipt. A pending or failed item reports `catalog_import: none` alongside its current
  durable state, attempt count, next retry time, and last error.
- `melodee catalog import` runs one bounded import pass using the same lease, retry, and catalog
  concurrency configuration as the server worker.
- `melodee policy config` and `melodee policy rules` print the active safe preferences and exact
  immutable rule-set identity.
- `melodee policy test --limit 100 --actor <administrator-id>` runs a bounded catalog test and reports proposed outcomes,
  matched rules, existing dispositions/states, destructive impact, and sample Release IDs.
- `melodee policy evaluate <release-id>` proposes one decision without persisting it;
  `melodee policy inspect <release-id>` prints the current durable decision and audit history.
- `melodee policy decide <release-id> <disposition> --actor <id> --reason <reason>` records an
  explicit administrator override. It does not complete or execute deletion.
- `melodee search status` reports unleased pending rows, separately leased rows, permanently failed
  outbox rows, oldest availability, checkpoint, latest rebuild, active generation, sealed snapshot
  and current live document counts, and index-open errors. Retryable rows become permanent failures
  after `MELODEE__SEARCH__MAX_OUTBOX_ATTEMPTS` so a poison row cannot churn forever.
- `melodee search rebuild --actor <administrator-id>` synchronously builds, validates, catches up,
  and activates a new generation. `search query`, `autocomplete`, `parse`, and `explain` exercise
  the same bounded query contracts used by the worker.
- `melodee worker-pools list` prints every registered worker pool with its durable pause state and
  audit fields. `melodee worker-pools pause <pool> --by <operator>` durably pauses a pool and
  `melodee worker-pools resume <pool> --by <operator>` resumes it; the `--by` operator identity is
  mandatory and recorded with the transition (the last-pause audit survives a resume). The podcast,
  policy, and discovery workers honor the durable row between claim batches: a paused pool claims
  no new work, while already-claimed work finishes or is released under the normal lease contract —
  pausing never strands a lease. Pausing a pool name that has no wired worker yet registers the row
  so the pool is paused from its first check once it exists.
- `melodee config list` prints every known setting's effective value and its source layer;
  secrets are always redacted. `melodee config validate` runs the first-run preflight checks and
  exits non-zero when a required check fails. `melodee config export [--output <path>]` writes the
  redacted export document and `melodee config import <path> [--apply] [--allow-unknown]` previews
  (default) or applies a validated import. See
  [Configuration](../installation/configuration/#import-and-export) for the secret-handling
  contract.

See [Policies](./policies/) for disposition and safety behavior, and
[Troubleshooting](../troubleshooting/) for operational checks.

:::caution[Partial implementation]
The health and queue diagnostics above are implemented. A graphical diagnostics screen and
media-tool/filesystem readiness reporting remain planned and will be documented when they ship.
:::
