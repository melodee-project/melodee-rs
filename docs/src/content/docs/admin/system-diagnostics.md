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

The readiness route performs a bounded PostgreSQL query. It returns HTTP 200 with
`status: "ready"` when PostgreSQL answers and HTTP 503 with `status: "not_ready"` otherwise.
The server also connects to PostgreSQL and applies migrations before binding its listener.

This is deliberately a basic dependency-readiness probe. It does not individually test media
tools, filesystems, disk space, or worker processes; use the diagnostics below for those checks.

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

See [Troubleshooting](../troubleshooting/) for operational checks.

:::caution[Partial implementation]
The health and queue diagnostics above are implemented. A graphical diagnostics screen and
media-tool/filesystem readiness reporting remain planned and will be documented when they ship.
:::
