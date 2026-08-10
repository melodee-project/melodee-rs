---
title: Troubleshooting
description: Diagnosing and resolving common Melodee problems.
sidebar:
  order: 1
---

This section collects diagnostic steps for common operational problems. Each entry links back to
the relevant feature or configuration documentation.

Implemented normalization checks:

- Run `melodee doctor` and confirm `ffmpeg`, `ffprobe`, and `fpcalc` report `ok`.
- Verify the configured executable paths and timeouts under
  `MELODEE__NORMALIZATION__*`. FFmpeg must include the `libmp3lame` encoder.
- Inspect the work item's durable error code and manifest findings. Tool failures, timeouts,
  fingerprint failures, source changes, and output-verification failures are retryable and do not
  publish partial normalization results.
- Ensure `MELODEE__NORMALIZATION__WORK_AREA` is writable and has enough temporary space. Attempt
  directories are deliberately retained for diagnosis; they are not canonical library storage.

Other topics will be added as their behavior ships:

- server fails to start or fails health checks;
- database connection and migration problems;
- inbound Releases stuck or failed in processing;
- client connection and authentication problems;
- search index rebuild and recovery.

:::caution[Planned documentation]
Troubleshooting entries are written only against implemented, observable behavior. If you hit a
problem with the currently shipping C# server, refer to the legacy
[Melodee repository](https://github.com/melodee-project/melodee).
:::
