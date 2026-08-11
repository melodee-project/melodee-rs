---
title: Configuration
description: Melodee server configuration sources and settings reference.
sidebar:
  order: 2
---

Melodee reads settings from explicit environment variables only.

## Configuration sources

1. built-in defaults;
2. environment variables.

Only documented keys are supported. Database-backed runtime overrides are planned but are not read
by the current server.

## Implemented settings

### Core runtime

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__SERVER__BIND` | `127.0.0.1:8080` | HTTP listener address. Keep the loopback default unless trusted network controls protect the pre-release diagnostics API. |
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

### Validation and canonical publication

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__VALIDATION__CONCURRENCY` | `2` | Maximum normalized Releases structurally validated simultaneously by one process. |
| `MELODEE__PUBLICATION__CONCURRENCY` | `2` | Maximum complete Release bundles copied and finalized simultaneously by one process. |
| `MELODEE__STORAGE__ROOT` | — (publication disabled) | Dedicated canonical-library root. Setting it enables validation/publication workers. |

### Catalog import

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__CATALOG__ENABLED` | `true` | Whether the server claims published manifest-v3 Releases for catalog import. |
| `MELODEE__CATALOG__CONCURRENCY` | `2` | Maximum catalog imports executed simultaneously by one server process. |
| `MELODEE__CATALOG__BATCH_SIZE` | `16` | Configured claim ceiling; the effective per-process claim is the smaller of this value and catalog concurrency. |
| `MELODEE__CATALOG__POLL_INTERVAL_SECONDS` | `5` | Period between PostgreSQL catalog-import polls. |

### Policy evaluation

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__POLICY__WORKER__ENABLED` | `true` | Whether the server evaluates cataloged Releases with the configured immutable rule set. |
| `MELODEE__POLICY__WORKER__POLL_INTERVAL_SECONDS` | `60` | Period between PostgreSQL policy-worker polls. |
| `MELODEE__POLICY__CONCURRENCY` | `4` | Maximum policy evaluations in flight in one server process (maximum `64`). |
| `MELODEE__POLICY__BATCH_SIZE` | `500` | Configured claim ceiling (maximum `10000`); the effective claim is the smaller of this value and concurrency. |
| `MELODEE__POLICY__RULE_SET_VERSION` | `1` | Immutable version for the configured built-in rule set. Increment it whenever a preference changes. |
| `MELODEE__POLICY__DESTRUCTIVE_CONFIDENCE_BPS` | `8000` | Confidence threshold in basis points (`1`–`10000`) for a rule that proposes a destructive disposition. |
| `MELODEE__POLICY__REDUNDANT_GREATEST_HITS_DISPOSITION` | `ignore` | Outcome for fully redundant Greatest Hits Releases: `ignore` or `delete`. Delete remains pending review. |
| `MELODEE__POLICY__NOVEL_GREATEST_HITS_DISPOSITION` | `review` | Outcome when a Greatest Hits Release contains new/alternate material: `review` or `accept`. |
| `MELODEE__POLICY__UNCERTAIN_GREATEST_HITS_DISPOSITION` | `review` | Outcome when Greatest Hits novelty is uncertain: `review` or `quarantine`. |

### Search projection

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__SEARCH__ENABLED` | `false` | Starts the PostgreSQL outbox/rebuild worker. Enabling requires an explicit search root. |
| `MELODEE__SEARCH__ROOT` | — when enabled | Absolute, dedicated root for owned Tantivy generations. It must not overlap inbound, work, or canonical storage roots. |
| `MELODEE__SEARCH__LEASE_SECONDS` | `60` | PostgreSQL writer/rebuild lease duration. |
| `MELODEE__SEARCH__BATCH_SIZE` | `256` | Outbox/catch-up rows per bounded pass (maximum `512`). |
| `MELODEE__SEARCH__POLL_INTERVAL_MS` | `1000` | Delay between worker passes. |
| `MELODEE__SEARCH__SNAPSHOT_PAGE_SIZE` | `1000` | Maximum PostgreSQL rows held per entity page during rebuild. |
| `MELODEE__SEARCH__DEFAULT_LIMIT` | `200` | Default operator query limit. |
| `MELODEE__SEARCH__MAX_LIMIT` | `1000` | Maximum operator query limit. |

### Notes

- `MELODEE__INGEST__SOURCES_JSON` and the legacy single-root variables are mutually additive only through configuration code: JSON takes precedence when present.
- If no inbound source is configured, startup remains healthy and periodic reconciliation is not started.
- Media inspection requires `ffprobe`. Normalization also requires an FFmpeg build with
  `libmp3lame` and the Chromaprint `fpcalc` command.
- Work leases, timeouts, retry settings, batch sizes, and concurrency must be positive. The
  normalization lease heartbeat is derived from the configured lease duration.
- `MELODEE__STORAGE__ROOT`, every inbound root, and the normalization work area must be separate,
  non-overlapping locations. Symlink roots are rejected. The server does not publish canonical
  media unless the storage root is explicitly configured.
- Inspection, normalization, validation, and publication do not move, rename, rewrite, or delete
  inbound files. Successful publication records the source disposition as `retained`.
- Catalog import reads the durable manifest-v3, publication-plan, and receipt records from
  PostgreSQL. It does not reopen source or canonical media. Catalog concurrency and claim size are
  independent of the ingestion-stage concurrency settings. The effective claim size is the lower
  of catalog batch size and catalog concurrency, so a claimed lease never waits behind the same
  process's concurrency queue.
- Policy evaluation reads authoritative Release, Track, AudioIdentity, byte-hash, catalog-context,
  and sealed-manifest facts from PostgreSQL; it does not reopen media. Invalid policy bounds or
  preferences fail startup. Rule-set name/version pairs are immutable: increment the version when
  changing a preference, or startup fails closed on the stored payload conflict.
- `Review` and `Delete` policy decisions remain pending in `NeedsReview`. `Accept`, `Ignore`, and
  `Quarantine` decisions complete atomically with their non-destructive work-item routing. The
  Phase 5 worker and CLI do not complete destructive actions or remove media.
- `MELODEE_BIND` remains a compatibility fallback for the earlier bootstrap, but
  `MELODEE__SERVER__BIND` is the canonical key.
- Search never reads audio. Its root is ownership-marked, symlink components and unowned non-empty
  roots are rejected, and retirement only removes a validated direct generation child. When search
  is enabled, missing/corrupt active-generation state makes `/readyz` return not ready while
  `/healthz` remains a liveness check.
