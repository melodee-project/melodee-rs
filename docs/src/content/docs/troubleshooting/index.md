---
title: Troubleshooting
description: Diagnosing and resolving common Melodee problems.
sidebar:
  order: 1
---

This section collects diagnostic steps for common operational problems. Each entry links back to
the relevant feature or configuration documentation.

## Server and queue checks

- Request `GET /healthz` to confirm that the HTTP service is running. Because the listener starts
  after PostgreSQL connection and migration, a connection failure during startup prevents this
  route from becoming available.
- Request `GET /readyz` to run the bounded PostgreSQL readiness check. HTTP 503 with
  `status: "not_ready"` means the process is live but PostgreSQL did not answer in time.
- Use the [Admin diagnostics API](../api/admin-api/) to inspect queue counts, bounded work-item
  listings, one item's last error and history, and active lease evidence.
- Run `melodee work list` or `melodee work show <id>` for the same durable work state from the CLI.
  These commands require a working `MELODEE__DATABASE__URL` connection.
- Treat an active lease as database evidence, not worker liveness. An expired lease is recovered by
  the durable work engine; an unexpired lease does not prove its owner process is alive.

The readiness route is deliberately narrow. It does not yet report individual readiness for
filesystems, disk space, media tools, the future search index, or compatibility adapters. See
[System and Diagnostics](../admin/system-diagnostics/) for the exact implemented boundary.

## Media normalization checks

Implemented normalization checks:

- Run `melodee doctor` and confirm `ffmpeg`, `ffprobe`, and `fpcalc` report `ok`.
- Verify the configured executable paths and timeouts under
  `MELODEE__NORMALIZATION__*`. FFmpeg must include the `libmp3lame` encoder.
- Inspect the work item's durable error code and manifest findings. Tool failures, timeouts,
  fingerprint failures, source changes, and output-verification failures are retryable and do not
  publish partial normalization results.
- Ensure `MELODEE__NORMALIZATION__WORK_AREA` is writable and has enough temporary space. Attempt
  directories are deliberately retained for diagnosis; they are not canonical library storage.
- Set `MELODEE__STORAGE__ROOT` to a dedicated canonical-library root before running validation or
  publication. It must not overlap an inbound root or the normalization work area.
- Use `melodee ingest validate` and `melodee ingest publish` to run one bounded batch while
  diagnosing a worker. A target conflict is never overwritten; inspect the durable work-item and
  publication journal evidence before correcting the external filesystem state.

Remaining topics will be added as their behavior ships:

- database connection and migration problems;
- client connection and authentication problems;
- search index rebuild and recovery.

:::note[Rust pre-release]
These checks describe the implemented Rust development server. If you hit a problem with the
currently shipping C# server, refer to the legacy
[Melodee repository](https://github.com/melodee-project/melodee).
:::
