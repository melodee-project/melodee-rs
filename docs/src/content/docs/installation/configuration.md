---
title: Configuration
description: Melodee server configuration sources and settings reference.
sidebar:
  order: 2
---

Melodee reads its deployment configuration on startup. Administrators can override ordinary settings in **System → Settings → Configuration**.

## Configuration sources

An explicit per-setting Admin UI override takes precedence for editable keys. Without one, registry values resolve through these layers, highest precedence first:

1. environment variables (`MELODEE__*` — the documented keys below, plus any registry key as
   `MELODEE__` + the key with `.` replaced by `__`, uppercased);
2. an optional JSON config file (`MELODEE__CONFIG_FILE` points at a bounded
   `{"key": value, …}` object). On supported Unix platforms, the file and every
   parent component are opened without following symlinks; a symlink or special-file
   replacement fails closed;
3. database-backed settings (`server_settings`, managed at runtime through the CLI and the admin
   API);
4. built-in defaults (the legacy registry's seeded values).

Worker and other startup controls read their saved tuning overrides, then the environment, then
their defaults. A change takes effect when the server restarts. The Settings page shows the
source and any value pending restart. PostgreSQL, credentials used to sign sessions, storage
roots, media executable paths, and other deployment controls remain host-managed. To recover
from an incorrect worker override while the service is stopped, an operator can delete that
key from `server_startup_overrides` in the installation database and restart the service.

## Database-backed settings

`melodee-cli config list` prints every known setting's effective value with the layer that supplied
it (`environment`, `config-file`, `database`, or `default`). Secret settings are always redacted
to a fixed marker — there is no show-secrets option, and no API or CLI surface returns a secret
value.

`melodee-cli config validate` runs the first-run preflight checks (database and migrations, storage
paths, media tools, search index, administrator account, email sanity) and exits non-zero when a
required check fails. See [Quick Start](../getting-started/quick-start/) for the first-run flow.

### Import and export

`melodee-cli config export [--output <path>]` writes a JSON document of the database and defaults
layer (environment and config-file overlays are deployment concerns and are not captured).
Administrators can also use `GET /api/admin/v1/config/export` (see the
[Admin API](../api/admin-api/)).

When `--output` is supplied, the file is created atomically without following symlinks; an
existing destination is refused. Choose a new path or explicitly remove an old export before
writing another one.

The export carries a header warning: **every secret value is redacted to a fixed marker and must
be re-entered after import.** Handle exported files as sensitive even so — non-secret values can
reveal infrastructure details.

`melodee-cli config import <path>` is a **preview by default**: it prints the diff (added, changed,
unchanged keys; secret keys by name only with an explicit warning; unknown keys) without writing
anything. The input must be a regular file and is opened without following symlinks on supported
Unix platforms. `melodee-cli config import <path> --apply` validates every value against the registry's
type/range rules and writes the rows atomically; keys outside the known registry are rejected
unless `--allow-unknown` is passed. The redaction marker is never accepted as a value, so a
redacted export can never overwrite a stored secret. The admin API equivalent is
`POST /api/admin/v1/config/import?preview=true|false&allowUnknown=true|false`.

## Implemented settings

### Core runtime

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__SERVER__BIND` | `127.0.0.1:8080` | HTTP listener address. |
| `MELODEE__SERVER__MAX_CONNECTIONS` | `0` (disabled) | Optional cap on accepted HTTP connections, from `1` to `65536`; `0` disables the cap. At capacity, new connections wait in the operating system's TCP backlog. The cap includes idle keep-alive connections and slow or long-running streams, so set it with enough capacity for all clients and monitor overload. This is a process-wide connection cap, not a per-user limit or a reservation for interactive requests. |
| `MELODEE__LOGGING__LEVEL` | `info` | Server log verbosity: `trace`, `debug`, `info`, `warn`, or `error` (case-insensitive). When this setting is absent, a valid advanced `RUST_LOG` filter is honored for compatibility. The Melodee setting takes precedence when both are present. |
| `MELODEE__LOGGING__DIRECTORY` | — (console only) | Optional directory for daily UTC files named `YYYY-MM-DD.log`. Melodee creates the directory, continues writing the same events to the console, appends within a day, and switches files after UTC midnight. Startup fails if the current file cannot be opened. Older files are retained; configure external retention and restrict access because operational logs can contain media paths and account identifiers. |
| `MELODEE__DATABASE__URL` | — (required) | PostgreSQL connection URL. |
| `MELODEE__DATABASE__MAX_CONNECTIONS` | `32` | Maximum database pool connections. |
| `MELODEE__DATABASE__MIN_CONNECTIONS` | `2` | Minimum idle database pool connections. |
| `MELODEE__DATABASE__ACQUIRE_TIMEOUT_SECONDS` | `30` | Timeout in seconds for acquiring a connection from the pool. |
| `MELODEE__DATABASE__IDLE_TIMEOUT_SECONDS` | `600` | Timeout in seconds for idle connections before closing. |
| `MELODEE__SHUTDOWN__GRACE_SECONDS` | `30` | Process-wide graceful-drain deadline (`1`–`600` seconds). On SIGTERM/CTRL+C, new direct-play and transcode admissions close immediately; HTTP responses and workers may finish within this window before remaining work is terminated. |
| `MELODEE__ADMIN_UI__DIST` | `admin-ui/dist` | Directory containing the built admin SPA, served at `/admin` with client-side routing fallback to `index.html`. A missing directory is not fatal: the server starts, all APIs serve normally, and `/admin` answers 404 until the bundle is deployed. |
| `MELODEE__WORK__LEASE_SECONDS` | `300` | PostgreSQL lease duration for claimed work items. |
| `MELODEE__WORK__RETRY_BASE_SECONDS` | `60` | Base retry delay (`base * 2^(attempt-1)`). |
| `MELODEE__WORK__RETRY_MAX_SECONDS` | `3600` | Maximum retry delay. |
| `MELODEE__WORK__MAX_ATTEMPTS` | `5` | Retry attempts before parking a failed item. |
| `MELODEE__WORK__CLAIM_BATCH_SIZE` | `4` | Max items claimed in one SQL `FOR UPDATE SKIP LOCKED` poll. |

### Local playback backends

These database-backed settings select the optional operator-owned playback process. They are
resolved through the same environment, config-file, database, and default layers described above;
secret values are redacted by configuration listing and export.

| Key | Default | Effect |
| --- | --- | --- |
| `jukebox.enabled` | `false` | Enables the live local playback control surface. Disabled operation is fail-closed. |
| `jukebox.backendType` | — | `mpd` selects the MPD TCP backend; `mpv` selects the MPV IPC backend. |
| `mpd.host` | `localhost` | MPD host name or address. |
| `mpd.port` | `6600` | MPD TCP port. |
| `mpd.password` | — | Optional MPD password; never displayed in diagnostics. |
| `mpd.timeoutMs` | `10000` | Bounded MPD connect/command timeout, clamped to 1–60000 ms. |
| `mpd.initialVolume` | `0.8` | Volume sent during initialization, expressed as a 0–1 setting. |
| `mpv.socketPath` | — | Operator-created MPV JSON IPC socket path. The server does not spawn MPV. |
| `mpv.timeoutMs` | `5000` | Bounded MPV IPC connect/command timeout, clamped to 1–60000 ms. |
| `mpv.initialVolume` | `0.8` | Volume sent during MPV initialization, expressed as a 0–1 setting. |

The legacy MPV process-launch fields remain configuration-compatible but are inert in the Rust
server because process creation is intentionally operator-owned. Use systemd, a service manager,
or an equivalent supervisor to start MPV with the configured socket.

### Inbound discovery and settling

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__INGEST__ENABLED` | `true` | Whether inbound discovery/scheduler is enabled. |
| `MELODEE__INGEST__SOURCES_JSON` | — | JSON array of inbound sources (`id?`, `root`, `name?`). Supports multiple source roots. |
| `MELODEE__INGEST__SOURCE_ROOT` | — | Single inbound root (legacy compatibility path when JSON is not set). |
| `MELODEE__INGEST__STAGING_ROOT` | — (required with inbound sources) | Absolute, durable Melodee-owned original-source root. It must be writable and disjoint from inbound, work, and canonical storage. |
| `MELODEE__INGEST__SOURCE_LIBRARY_ID` | — | Optional library id for the single-root mode. |
| `MELODEE__INGEST__SOURCE_LIBRARY_NAME` | — | Optional source name for the single-root mode. |
| `MELODEE__INGEST__RECONCILIATION_INTERVAL_SECONDS` | `30` | Period between periodic scans. |
| `MELODEE__INGEST__RETAINED_RECHECK_INTERVAL_SECONDS` | `300` | Delay before rechecking an unchanged retained source; must be between 1 and 2,592,000 seconds. |
| `MELODEE__INGEST__RETAINED_RECHECK_MIB_PER_SECOND` | `8` | Per-process maximum scheduled read rate for retained-source rechecks, in MiB/s. |
| `MELODEE__INGEST__SETTLING_QUIET_SECONDS` | `10` | Minimum quiet period before `Settling`→`Ready`. |
| `MELODEE__INGEST__READY_MARKER_NAME` | `.melodee-ready` | Marker filename that can short-circuit settling. |
| `MELODEE__INGEST__READY_MARKER_SHORTCUT_SECONDS` | `0` | Quiet-period override when marker is present. |

:::caution[Planned documentation]
Protected intake is not active yet. The optional `MELODEE__INGEST__PROTECTED_CUSTODY_ROOT`
and `MELODEE__INGEST__PROTECTED_PRODUCER_UID` settings are reserved for its future
Linux implementation and do not enable it. If supplied, both are required: the
root must be an absolute path, and the numeric producer UID must be non-root and
different from the service UID. Leaving both unset preserves existing behavior.
The future protocol will also validate filesystem isolation and ownership; setting
these values alone does not establish them. Operators must enforce that all drop
writers use the declared unprivileged identity and cannot run as the service user;
the setting does not verify who actually writes inbound files.
:::

### Throughput tuning

The default worker concurrency is `2` per stage. Even one publisher can keep a shared disk busy.
Retained-source rechecks share one read budget within a process. If multiple configurations set
different rates, the strictest rate remains in effect until the process restarts. The setting
limits application reads; operating-system caching and readahead affect actual storage traffic.
For a 25-Release, 1.8 GB batch on a 24-core host, the median wait before a Release was claimed was ~200 s while the
work itself took seconds to tens of seconds per stage.

Workers can drain consecutive full batches between poll ticks. The automatic publisher pauses
after a failed batch. With default settings it waits at least 60 seconds. Consecutive batches
that fail to publish anything double that delay up to one hour. These limits follow
`MELODEE__WORK__RETRY_BASE_SECONDS` and `MELODEE__WORK__RETRY_MAX_SECONDS`. The reconciliation
interval is also a minimum wait after a failure. A successful batch resets the delay.

Tune the stages independently.

- `MELODEE__INSPECTION__CONCURRENCY` controls parallel hashing and media probes. Increase it
  only while the source and staging disks have headroom.
- `MELODEE__NORMALIZATION__CONCURRENCY` — the CPU bound (transcoding). Keep it at or below half the
  physical cores so the database and API keep responsive.
- `MELODEE__VALIDATION__CONCURRENCY` controls verification against the work area. For a shared
  NFS library, start with `MELODEE__PUBLICATION__CONCURRENCY=1` and check playback latency during
  publication. All publisher workers in the service share its NFS asset bandwidth budget.
- `MELODEE__CATALOG__CONCURRENCY` — PostgreSQL bound; 2–4.
- `MELODEE__INGEST__SETTLING_QUIET_SECONDS` — the floor on per-Release latency (default `10`); the
  ready marker (`MELODEE__INGEST__READY_MARKER_NAME`) skips the wait when the source can write it.

### Media inspection

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__INSPECTION__CONCURRENCY` | `2` | Maximum number of Release inspections executed concurrently by one server worker (claim batch size and actual in-flight inspections). |
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
| `MELODEE__NORMALIZATION__WORK_AREA` | `/tmp/melodee-work` | Temporary, non-authoritative root for attempt-isolated normalized output. It is never used as durable staging. |

### Validation and canonical publication

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__VALIDATION__CONCURRENCY` | `2` | Maximum normalized Releases structurally validated simultaneously by one process. |
| `MELODEE__PUBLICATION__CONCURRENCY` | `2` | Maximum complete Release bundles copied and finalized simultaneously by one process. |
| `MELODEE__PUBLICATION__NFS_MIB_PER_SECOND` | `8` | Positive integer. Combined MiB/s for canonical media and artwork writes and verification reads by one Linux NFS publisher service. Separate processes have separate budgets. |
| `MELODEE__STORAGE__ASSET_ROOT` | Per-user data directory, or the assets volume in Compose | Existing absolute directory for artwork, avatars and theme files. Mount the same store for the server, workers and CLI imports that write files. |
| `MELODEE__STORAGE__ROOT` | — (publication disabled) | Dedicated canonical-library root. Setting it enables validation/publication workers. It is distinct from staging. |

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
| `MELODEE__POLICY__RULE_SET_VERSION` | `2` | Immutable version for the configured built-in rule set. Version 1 is obsolete; increment this whenever a preference changes. |
| `MELODEE__POLICY__DESTRUCTIVE_CONFIDENCE_BPS` | `8000` | Confidence threshold in basis points (`1`–`10000`) for a rule that proposes a destructive disposition. |
| `MELODEE__POLICY__REDUNDANT_GREATEST_HITS_DISPOSITION` | `ignore` | Outcome for fully redundant Greatest Hits Releases: `ignore` or `delete`. Delete remains pending review. |
| `MELODEE__POLICY__NOVEL_GREATEST_HITS_DISPOSITION` | `review` | Outcome when a Greatest Hits Release contains new/alternate material: `review` or `accept`. |
| `MELODEE__POLICY__UNCERTAIN_GREATEST_HITS_DISPOSITION` | `review` | Outcome when Greatest Hits novelty is uncertain: `review` or `quarantine`. |

### Search projection

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__SEARCH__ENABLED` | `false` | Starts the PostgreSQL outbox/rebuild worker. Enabling requires an explicit search root. |
| `MELODEE__SEARCH__ROOT` | — when enabled | Absolute, dedicated root for owned Tantivy generations. It must not overlap inbound, staging, work, or canonical storage roots. |
| `MELODEE__SEARCH__LEASE_SECONDS` | `60` | PostgreSQL writer/rebuild lease duration. |
| `MELODEE__SEARCH__BATCH_SIZE` | `256` | Outbox/catch-up rows per bounded pass (maximum `512`). |
| `MELODEE__SEARCH__MAX_OUTBOX_ATTEMPTS` | `5` | Failed attempts before a retryable outbox row becomes a permanent, operator-visible failure (maximum `100`). |
| `MELODEE__SEARCH__POLL_INTERVAL_MS` | `1000` | Delay between worker passes. |
- `MELODEE__SEARCH__SNAPSHOT_COMMIT_DOCUMENTS` (default `10000`) and
  `MELODEE__SEARCH__SNAPSHOT_COMMIT_INTERVAL_MS` (default `2000`) bound how many documents and how
  much wall time a rebuild buffers before committing. Lower them to bound uncommitted memory more
  tightly or raise them to reduce commit overhead.
| `MELODEE__SEARCH__SNAPSHOT_PAGE_SIZE` | `1000` | Maximum PostgreSQL rows held per entity page during rebuild. |
| `MELODEE__SEARCH__DEFAULT_LIMIT` | `200` | Default operator query limit. |
| `MELODEE__SEARCH__MAX_LIMIT` | `1000` | Maximum operator query limit. |
| `MELODEE__SEARCH__PROCESSED_OUTBOX_RETENTION_DAYS` | `7` | Days of successful search-outbox detail retained after indexing (`2`–`90`). Exact hourly index totals are kept for 400 days regardless. Also editable in the Admin settings editor as `search.processedOutboxRetentionDays`. |

### Manifest evidence storage

Full Release manifests — including provider provenance documents, raw fallback text, lyrics,
parsed sidecars and complete acoustic fingerprints — are stored as immutable, verified objects
under `MELODEE__STORAGE__ASSET_ROOT`. PostgreSQL keeps the manifest row, its version and hashes,
one verified reference, and compact query facts (release title, source path, corrupt-media flag,
release traits, per-Track audio fields, fingerprint digests, genre and lyrics presence). Browse,
search, policy batches and Admin listings never read a full manifest file. Evidence above 10 MiB
is stored as fixed 4 MiB parts behind a versioned descriptor.

`MELODEE__STORAGE__ASSET_ROOT` must be an existing absolute directory on persistent storage. It
is part of the backup pair with PostgreSQL; a database-only restore cannot rebuild manifests.

### Native API podcasts

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__PODCAST__ENABLED` | `true` | Enables role-gated native-v1 podcast routes. `false`, `0`, or `no` makes the surface return `404`. |
| `MELODEE__PODCAST__DISCOVERY_ENDPOINT` | `https://itunes.apple.com/` | Base URL for podcast search, trending, and lookup. Production endpoints must use HTTPS; loopback HTTP is accepted only by deterministic tests. |
| `MELODEE__PODCAST__WORKER__ENABLED` | `false` | Enables bounded feed refresh, download, retention, and orphan-recovery passes while podcasts are enabled. Enabling requires an owned storage root. |
| `MELODEE__PODCAST__STORAGE_ROOT` | — when worker enabled | Dedicated absolute root for podcast downloads. It must not overlap inbound, staging, work, canonical-media, or search roots. |
| `MELODEE__PODCAST__WORKER__POLL_INTERVAL_SECONDS` | `30` | Delay between worker passes (`1`–`86400`). |
| `MELODEE__PODCAST__WORKER__BATCH_SIZE` | `4` | Maximum downloads claimed per pass (`1`–`32`). |
| `MELODEE__PODCAST__WORKER__LEASE_SECONDS` | `300` | Cross-process download lease (`60`–`3600` seconds). |
| `MELODEE__PODCAST__WORKER__MAX_ATTEMPTS` | `5` | Failed attempts before a download becomes a permanent operator-visible failure (`1`–`100`). |
| `MELODEE__PODCAST__MAX_DOWNLOAD_BYTES` | `536870912` | Maximum enclosure body size (`1 MiB`–`4 GiB`). |

### Discovery maintenance

A periodic worker reconciles the materialized discovery projections
(recommendation popularity and chart-item catalog links) against committed listening and catalog
facts. Every pass is bounded, set-based, and idempotent, so the worker is safe to leave enabled.

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__DISCOVERY__WORKER__ENABLED` | `true` | Whether the server periodically reconciles recommendation candidates and retries chart-item linkage. |
| `MELODEE__DISCOVERY__WORKER__POLL_INTERVAL_SECONDS` | `300` | Delay between maintenance passes (`1`–`86400`). |
| `MELODEE__DISCOVERY__WORKER__BATCH_SIZE` | `500` | Maximum rows touched by one reconciliation statement (`1`–`10000`). |
| `MELODEE__DISCOVERY__WORKER__MAX_BATCHES_PER_PASS` | `100` | Maximum batches in one pass (`1`–`10000`); a capped pass reports truncation and the next pass resumes the work. |

### Native API URLs, streaming, and proxies

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__NATIVE__PUBLIC_ORIGIN` | Listener origin | External origin used in native-v1 image, stream, browser-login redirect, and password-reset notification URLs. It must be an absolute `http` or `https` origin without credentials, a path, or a query. Set it to the reverse proxy's public origin in production. |
| `MELODEE__NATIVE__STREAM_CONCURRENCY` | `32` | Maximum direct media responses open concurrently in one server process (`1`–`4096`). Additional requests wait with bounded backpressure. Used when `streaming.maxConcurrentStreams.global` is `0`; see [Streaming admission](#streaming-admission). |
| `MELODEE__NATIVE__AUTH__JWT_KEY` | — (required) | HS256 signing secret for native-v1 15-minute access tokens and protected browser session cookies. Generate at least 32 random bytes (for example, `openssl rand -base64 48`) and keep the same secret on every server process. The example placeholder is rejected at startup. |
| `MELODEE__NATIVE__AUTH__MAX_SESSION_DAYS` | `90` | Absolute refresh-family lifetime (`30`–`365` days); individual refresh tokens remain valid for at most 30 days and rotate on use. |
| `MELODEE__NATIVE__AUTH__DEVELOPMENT_RESET_TOKENS` | `false` | Includes the one-time reset token and URL in a successful password-reset request. Enable only on isolated development systems. |
| `MELODEE__NATIVE__AUTH__RATE_ATTEMPTS` | `10` | Authentication attempts allowed for one normalized actor/address key per fixed window (`1`–`10000`). |
| `MELODEE__NATIVE__AUTH__RATE_WINDOW_SECONDS` | `60` | PostgreSQL-backed authentication rate window (`1`–`86400` seconds). |
| `MELODEE__NATIVE__AUTH__RATE_MAX_KEYS` | `100000` | Maximum active rate-counter keys retained cluster-wide (`1`–`10000000`). New keys fail closed at capacity. |
| `MELODEE__NATIVE__CORS__ALLOWED_ORIGINS` | empty | Comma-separated exact `http`/`https` browser origins allowed to call native v1. Wildcards are rejected. The policy admits native `GET`/`POST`/`PUT`/`PATCH`/`DELETE` requests and exposes only the paging, range, duration, and ETag response headers used by browser clients. The source-build and Compose examples explicitly allow MeloAmp's source UI (`http://localhost:3000`) and packaged desktop renderer (`http://localhost:3001`). |
| `MELODEE__SERVER__TRUSTED_PROXIES` | empty | Shared by native, OpenSubsonic, Jellyfin, and Admin requests. Comma-separated exact proxy IPs or CIDRs allowed to supply `Forwarded`/`X-Forwarded-For`. Untrusted forwarding headers are ignored. For a trusted peer, Melodee walks the forwarded chain from right to left, removes trusted proxy hops, and uses the first untrusted address as the client. List only proxy infrastructure you control. |
| `MELODEE__NATIVE__TRUSTED_PROXIES` | empty | Backward-compatible fallback alias, used only when `MELODEE__SERVER__TRUSTED_PROXIES` is absent. An explicitly empty shared value disables trust even when this alias is set. |
| `MELODEE__NATIVE__AUTH__GOOGLE__ENABLED` | `false` | Enables Google ID-token exchange only when the client ID and JWKS configuration also validate. |
| `MELODEE__NATIVE__AUTH__GOOGLE__CLIENT_ID` | — | Exact Google OAuth client ID used as the required token audience. |
| `MELODEE__NATIVE__AUTH__GOOGLE__JWKS_URL` | Google v3 certificates URL | HTTPS JWKS endpoint. Redirects are rejected and the response/cache are bounded. |
| `MELODEE__NATIVE__AUTH__GOOGLE__HOSTED_DOMAINS` | empty | Optional comma-separated lowercase hosted-domain allowlist checked after signature, issuer, audience, lifetime, and verified-email validation. |

Password hashing and verification share a bounded pool of four active operations and at most 64
admitted requests per process. Excess password work uses the endpoint's existing backend-failure
response: native sign-in returns service unavailable, while OpenSubsonic and Jellyfin retain their
non-enumerating authentication failure envelopes. Password creation/reset operations retain their
existing failure responses. This keeps authentication load off async request workers.

### OpenSubsonic browser clients

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__OPENSUBSONIC__LEGACY_AUTH_BYPASS` | `false` | Unsafe legacy localhost/matching-host authentication bypass. Enable only for isolated client migration. Never enable behind a public reverse proxy. Startup logs a warning when enabled. |
| `MELODEE__OPENSUBSONIC__BASE_URL_HOST` | absent | Host compared by the legacy bypass only when explicitly enabled; matching a host is not authentication. |
| `MELODEE__OPENSUBSONIC__CORS__ALLOWED_ORIGINS` | empty | Comma-separated exact HTTP(S) origins allowed to read OpenSubsonic responses from browsers, for example `https://feishin.vercel.app`. Rejects wildcards, credentials, trailing slashes, paths, queries, fragments, and more than 100 origins. Applies to API and media requests with `Origin`, including OPTIONS preflight, and admits GET/POST/HEAD with authentication, content-type, conditional, and range request headers. Exposes paging, range, duration, disposition, and cache-validation response headers. Cookie credentials are not enabled. |

OpenSubsonic `download` is gated by `system.isDownloadingEnabled` (default `true`); the legacy
environment switch `MELODEE__OPENSUBSONIC__DOWNLOADS_ENABLED` is used when the setting is absent.

This is independent of native API CORS. Browser streaming requires an allowed origin; non-browser
stream headers remain unchanged. `make up` reads the Feishin development default from
`compose.dev.yaml` (`x-melodee-server-environment`). Root `.env` and then exported variables
override this default; an explicitly empty value disables browser access.
For the packaged Compose deployment, set
`MELODEE_OPENSUBSONIC_CORS_ALLOWED_ORIGINS` in its environment file. Restart after configuration
changes. See [OpenSubsonic browser clients](../api/opensubsonic/#browser-clients-and-cors).

### Transcoding

Melodee can transcode audio on demand for OpenSubsonic and Jellyfin-compatible clients.
Transcoding is configured entirely through database-backed settings (change them with
`melodee-cli config import` or the Admin API); there are no environment overlays. Native Melodee
clients always receive direct play and expose no transcoding parameters.

| Key | Default | Effect |
| --- | --- | --- |
| `transcoding.command.aac` | AAC template | JSON object `{"format": "Aac", "bitrate": 256, "command": "ffmpeg ..."}` — the FFmpeg command line used for AAC (ADTS) output. |
| `transcoding.command.mp3` | MP3 template | Same shape for MP3 output (requires an FFmpeg build with `libmp3lame`). |
| `transcoding.command.opus` | Opus template | Same shape for Opus-in-Ogg output (requires `libopus`). |
| `transcoding.default` | `raw` | Fallback format (`raw`, `aac`, `mp3`, or `opus`) when a client request does not name one. `raw` keeps direct play. |
| `transcoding.maxConcurrent` | `4` | Maximum simultaneous FFmpeg transcode processes across all clients, independent of the direct-play stream limit. `0` disables transcoding: transcode requests fail closed while direct play is unaffected. |

Template commands are parsed as argument arrays and executed directly — no shell is involved —
and must contain these placeholders:

- `%s` — the source input. Melodee substitutes `pipe:0` and feeds the validated, containment-checked
  media file to FFmpeg's standard input. A template without `%s` is rejected and transcode requests
  fail closed.
- `%t` — the requested start time in seconds (currently always `0`; seek-before-transcode is not
  supported).
- `%bk` — the target bitrate in kilobits per second, rendered with FFmpeg's `k` suffix (for example
  `128k`). A client-requested bitrate below the template's default lowers it; higher requests are
  clamped to the template bitrate.

Malformed templates fail closed: the request is rejected and the event is logged. Direct play
(content type, range/seek, ETag, and caching behavior) is unchanged whenever no transcode is
requested. Transcoded responses are chunked (no `Content-Length` or `ETag`), ignore `Range`
headers, and are cancelled — killing the FFmpeg process — when the client disconnects.

When transcoding is disabled or saturated, OpenSubsonic clients receive the standard error
envelope with code `0` (HTTP 200, per the Subsonic generic-error convention) and Jellyfin clients
receive `503`. Jellyfin requests for an unsupported container receive `400`.

### Jellyfin-compatible authentication

Jellyfin-compatible access tokens are a separate, stateful authentication surface. They are not
native-v1 JWTs and are invalidated immediately on Jellyfin logout or when the account is disabled,
locked, or blacklisted. Every server process that serves the same database must use the same
pepper; changing it intentionally invalidates all existing Jellyfin access tokens.

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__JELLYFIN__TOKEN_PEPPER` | — (required) | Secret HMAC pepper for stored Jellyfin access-token digests. Supply at least 32 random bytes, for example `openssl rand -base64 48`. The server refuses to start when it is missing or too short; never log, commit, or share it. |
| `MELODEE__JELLYFIN__TOKEN_EXPIRES_AFTER_HOURS` | `168` | Jellyfin access-token lifetime in hours (`1`–`8760`). At most ten active tokens are retained per user; issuing another revokes the oldest active token. |
| `jellyfin.enabled` | `true` | When `false`, the Jellyfin-compatible routes are not mounted (requests fall through to the normal 404 behavior). Changing it requires a restart. |
| `system.siteName` | `Melodee` | Server name reported by Jellyfin `System/Info` and `System/Info/Public`. |

### Email notifications

Melodee sends user-facing email (password resets, share-created and request-fulfilled
notifications) through a configured SMTP relay. Delivery is disabled by default; while disabled,
every notification is still recorded in the delivery audit as `skipped` and the operation that
triggered it is unaffected. Delivery failures are likewise recorded and logged, never rolled back
into the triggering request. The password is never logged and never returned by any API.

Password-reset messages link to the bounded Rust `/reset-password` security page. The page
validates the one-use token through the native API and does not create a listener web application.
Keep the public-origin setting aligned with the reverse proxy; otherwise notification links will
point at the wrong origin. See [Users & Access](../admin/users-and-access/) for the user-facing
completion flow and its token-handling protections.

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__EMAIL__ENABLED` | `false` | Enables SMTP delivery. When `true`, `MELODEE__EMAIL__FROM_EMAIL` and `MELODEE__EMAIL__SMTP_HOST` are required and validated at startup. |
| `MELODEE__EMAIL__FROM_NAME` | `Melodee` | From display name. |
| `MELODEE__EMAIL__FROM_EMAIL` | — | From address; required when delivery is enabled. Must be a valid `user@domain` mailbox. |
| `MELODEE__EMAIL__SMTP_HOST` | — | SMTP relay hostname; required when delivery is enabled. |
| `MELODEE__EMAIL__SMTP_PORT` | `587` | SMTP port (`587` StartTLS, `465` implicit TLS, `25` plaintext). |
| `MELODEE__EMAIL__SMTP_USERNAME` | empty | SMTP auth username. When empty, no AUTH is attempted. |
| `MELODEE__EMAIL__SMTP_PASSWORD` | empty | SMTP auth password. Keep it in the environment or a secret store; it is redacted from logs and diagnostics. |
| `MELODEE__EMAIL__SMTP_USE_SSL` | `false` | Implicit TLS connection (conventionally port 465). Mutually exclusive with StartTLS. |
| `MELODEE__EMAIL__SMTP_USE_START_TLS` | `true` | StartTLS upgrade (conventionally port 587). Mutually exclusive with implicit TLS. When both flags are `false`, the relay is contacted in plaintext — use only on trusted networks. |

Notification templates (subject, plain-text and HTML bodies with `{{placeholder}}` substitution)
are stored in the database and managed through the
[admin API](../api/admin-api/) or `melodee-cli notifications templates list`. The email keys above are
also part of the database-backed settings layer (registry keys `email.*`): absent an explicit Admin UI override, environment and
config-file values still win over database rows.

### Last.fm scrobbling

Last.fm integration is disabled by default. The registry keys `scrobbling.lastFm.Enabled`
(environment `MELODEE__SCROBBLING__LASTFM__ENABLED`), `scrobbling.lastFm.apiKey`,
`scrobbling.lastFm.sharedSecret`, and `security.secretKey` control it; the API key and shared
secret are classified secret and are redacted from every API, export, and log. See
[Last.fm scrobbling](../features/lastfm/) for the linking flow and failure behavior.

### Streaming admission

The database-backed keys `streaming.maxConcurrentStreams.global` and
`streaming.maxConcurrentStreams.perUser` are enforced on every streaming surface (native v1,
OpenSubsonic, Jellyfin) and on Admin review previews.

| Key | Default | Effect |
| --- | --- | --- |
| `streaming.maxConcurrentStreams.global` | `0` (registry) | Global direct-play admission. `0` resolves to the process safety default of 32 (or `MELODEE__NATIVE__STREAM_CONCURRENCY` when set); a positive value is used directly. Values are clamped to `1`–`4096`. Additional requests wait with bounded backpressure. |
| `streaming.maxConcurrentStreams.perUser` | `0` | Per-user concurrent direct-play cap. `0` disables the cap. When a user exceeds it, the request is rejected with the legacy `429` shape (`TOO_MANY_REQUESTS` / `"Too many concurrent streams"`) instead of queued, so a client retries. |

Melodee deliberately keeps a bounded global default where the legacy server was unlimited (the
decision and its trade-offs are recorded in the repository ADR set, `design/adr/` — ADR-0183).
Operators migrating a deployment that relied on more than 32 simultaneous streams should set
the global key explicitly.

### Security policy settings

| Key | Default | Effect |
| --- | --- | --- |
| `security.blacklistedEmails` | empty | JSON array (stored as a JSON string) of email addresses denied on native authentication and on every authenticated native-v1 request. Matching is case-insensitive. A match returns `403` with code `BLACKLISTED`. |
| `security.blacklistedIPs` | empty | JSON array (stored as a JSON string) of exact client IPs denied the same way. The IP is the trusted-proxy-resolved client address. |
| `security.passwordResetTokenExpiryMinutes` | `60` | Lifetime of a one-time password-reset token. Values below `1` are clamped to `1`. The confirmation page and API honor the same expiry. |

The deny lists are cached for 10 seconds; an operator change takes effect without a restart. A
transient database failure keeps the last successfully loaded lists. Blocked requests are
logged at `WARN` with the rule name only — never the address or email.

### Podcast retention and download policy

| Key | Default | Effect |
| --- | --- | --- |
| `podcast.download.maxEnclosureBytes` | `2147483648` | Per-episode download ceiling. The process keeps its own lower cap, so configuring a larger value cannot exceed the deployment's `MELODEE__PODCAST__MAX_DOWNLOAD_BYTES`. |
| `podcast.download.maxConcurrent.global` | `2` | Maximum downloads claimed per worker pass. |
| `podcast.download.maxConcurrent.perUser` | `1` | Maximum episodes one user may have downloading at once across channels and worker processes. `0` disables the per-user bound. |
| `podcast.http.timeoutSeconds` | `30` | Outbound feed/enclosure HTTP timeout. |
| `podcast.quota.maxBytesPerUser` | `5368709120` | Per-user downloaded-bytes quota. Once a user's downloaded episodes reach the quota, no further episodes are claimed for that user. `0` disables the quota. |
| `podcast.retention.downloadedEpisodesInDays` | `0` | Delete downloads older than N days. `0` disables the age policy. |
| `podcast.retention.keepLastNEpisodes` | `0` | Keep only the newest N downloaded episodes per channel. `0` relies on each channel's own limits. |
| `podcast.retention.keepUnplayedOnly` | `false` | Delete episodes the user has finished playing. |

Retention runs after each worker pass. Bookmarked episodes are always retained: deleting a file
a user bookmarked would silently break resume state. Every deletion re-validates the recorded
owned path inside a row-locked transaction before removing the file.

### Playlist artwork uploads

`system.maxUploadSize` (default `5242880` bytes) bounds playlist artwork uploads. The process
hard cap is 10 MiB; a larger configured value is clamped, and an unreadable settings layer falls
back to the legacy default rather than unbounded.

### Logging

| Variable | Default | Effect |
| --- | --- | --- |
| `MELODEE__LOGGING__LEVEL` | `info` | `trace`, `debug`, `info`, `warn`, or `error` for Melodee and HTTP server targets. |
| `MELODEE__LOGGING__FORMAT` | `text` | `text` for human-readable lines, `json` for one structured JSON object per event (for log shippers and automation). |
| `MELODEE__LOGGING__DIRECTORY` | unset | Additionally append to `<directory>/YYYY-MM-DD.log` (UTC daily files). |

Every request records method, normalized route, status, latency, and correlation id. `5xx`
responses, `429` responses, and requests slower than 10 seconds are logged at `WARN`; other
completed requests at `DEBUG`. The most recent 200 `WARN`/`ERROR` events are also kept in memory
and exposed through the [diagnostics API](../api/admin-api/#runtime-diagnostics) and the bundled
support report, so a troubleshooting agent does not need container log access.

### Notes

- `MELODEE__INGEST__SOURCES_JSON` and the legacy single-root variables are mutually additive only through configuration code: JSON takes precedence when present.
- If no inbound source is configured, startup remains healthy and no inbound sources are scanned.
  Background cleanup can still remove expired temporary scan data.
- Media inspection requires `ffprobe`. Normalization also requires an FFmpeg build with
  `libmp3lame` and the Chromaprint `fpcalc` command.
- Work leases, timeouts, retry settings, batch sizes, and concurrency must be positive. The
  normalization lease heartbeat is derived from the configured lease duration.
- `MELODEE__STORAGE__ROOT`, every inbound root, and the normalization work area must be separate,
  non-overlapping locations. Symlink roots are rejected. The server does not publish canonical
  media unless the storage root is explicitly configured.
- Inbound media bytes are never moved, renamed, rewritten, or deleted while a candidate settles
  and is acquired. After the verified durable staging handoff, acquisition removes the acquired
  candidate directory from inbound (see
  [Storage](./storage/)); inspection, normalization, validation, and publication then operate on
  the durable staged source. Successful publication records the source disposition as `retained`,
  meaning the durable staged source is retained.
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
  policy worker and CLI do not complete destructive actions or remove media.
- `MELODEE_BIND` remains a compatibility fallback for the earlier bootstrap, but
  `MELODEE__SERVER__BIND` is the canonical key.
- Search never reads audio. Its root is ownership-marked, symlink components and unowned non-empty
  roots are rejected, and retirement only removes a validated direct generation child. When search
  is enabled, missing/corrupt active-generation state makes `/readyz` return not ready while
  `/healthz` remains a liveness check.
- Podcast discovery sends only the user-supplied query/country/genre and bounded result limit to
  the configured endpoint. It does not send Melodee credentials or catalog data. Redirects are
  rejected, at most eight requests run concurrently, and time, response size, and result count are
  bounded.
- Stream credentials bind to the authoritative client address. Configure trusted proxies only when
  those proxies overwrite forwarding headers; trusting a client-controlled forwarding hop can make
  signed URLs unusable or weaken client-address binding. Direct streaming is available only when
  `MELODEE__STORAGE__ROOT` opens as the canonical publication root.
