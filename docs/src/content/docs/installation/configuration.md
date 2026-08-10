---
title: Configuration
description: Melodee server configuration sources and settings reference.
sidebar:
  order: 2
---

Melodee reads settings from explicit environment variables only.

## Configuration sources

1. built-in defaults;
2. environment variables;
3. database-backed overrides where the setting is runtime-editable.

Only documented keys are supported.

## Implemented settings

### Core runtime

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__DATABASE__URL` | — (required) | PostgreSQL connection URL. |
| `MELODEE__WORK__LEASE_SECONDS` | `300` | PostgreSQL lease duration for claimed work items. |
| `MELODEE__WORK__RETRY_BASE_SECONDS` | `60` | Base retry delay (`base * 2^(attempt-1)`). |
| `MELODEE__WORK__RETRY_MAX_SECONDS` | `3600` | Maximum retry delay. |
| `MELODEE__WORK__MAX_ATTEMPTS` | `5` | Retry attempts before parking a failed item. |
| `MELODEE__WORK__CLAIM_BATCH_SIZE` | `4` | Max items claimed in one SQL `FOR UPDATE SKIP LOCKED` poll. |

### Inbound discovery and settling

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__INGEST__ENABLED` | `true` | Whether inbound discovery/scheduler is enabled. |
| `MELODEE__INGEST__SOURCES_JSON` | — | JSON array of inbound sources (`id?`, `root`, `name?`). Supports multiple source roots. |
| `MELODEE__INGEST__SOURCE_ROOT` | — | Single inbound root (legacy compatibility path when JSON is not set). |
| `MELODEE__INGEST__SOURCE_LIBRARY_ID` | — | Optional library id for the single-root mode. |
| `MELODEE__INGEST__SOURCE_LIBRARY_NAME` | — | Optional source name for the single-root mode. |
| `MELODEE__INGEST__RECONCILIATION_INTERVAL_SECONDS` | `30` | Period between periodic scans. |
| `MELODEE__INGEST__SETTLING_QUIET_SECONDS` | `10` | Minimum quiet period before `Settling`→`Ready`. |
| `MELODEE__INGEST__READY_MARKER_NAME` | `.melodee-ready` | Marker filename that can short-circuit settling. |
| `MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS` | `0` | Quiet-period override when marker is present. |

### Media inspection

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__INSPECTION__CONCURRENCY` | `2` | Maximum number of Release inspections claimed by one server worker batch. |
| `MELODEE__INSPECTION__FFPROBE_PATH` | `ffprobe` | Explicit ffprobe executable path. |
| `MELODEE__INSPECTION__FFPROBE_TIMEOUT_SECONDS` | `30` | Maximum time allowed for one ffprobe process. |

### Media normalization and fingerprints

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__NORMALIZATION__CONCURRENCY` | `2` | Maximum Releases normalized simultaneously by one server process. |
| `MELODEE__NORMALIZATION__FFMPEG_PATH` | `ffmpeg` | Explicit FFmpeg executable path. |
| `MELODEE__NORMALIZATION__FFMPEG_TIMEOUT_SECONDS` | `300` | Maximum time for one FFmpeg encode, decode verification, or artwork operation. |
| `MELODEE__NORMALIZATION__FPCALC_PATH` | `fpcalc` | Explicit Chromaprint `fpcalc` executable path. |
| `MELODEE__NORMALIZATION__FPCALC_TIMEOUT_SECONDS` | `60` | Maximum time for one fingerprint operation. |
| `MELODEE__NORMALIZATION__WORK_AREA` | `/tmp/melodee-work` | Temporary, non-authoritative root for attempt-isolated normalized output. |

### Notes

- `MELODEE__INGEST__SOURCES_JSON` and the legacy single-root variables are mutually additive only through configuration code: JSON takes precedence when present.
- If no inbound source is configured, startup remains healthy and periodic reconciliation is not started.
- Media inspection requires `ffprobe`. Normalization also requires an FFmpeg build with
  `libmp3lame` and the Chromaprint `fpcalc` command.
- Work leases, timeouts, retry settings, batch sizes, and concurrency must be positive. The
  normalization lease heartbeat is derived from the configured lease duration.
- Inspection and normalization do not move, rename, rewrite, or delete source files. Normalized
  files stay in the temporary work area until a later publication phase verifies and publishes
  them; the current implementation does not publish canonical library files.
