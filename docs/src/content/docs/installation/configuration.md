---
title: Configuration
description: Melodee server configuration sources and settings reference.
sidebar:
  order: 2
---

Melodee reads settings from layered sources. Environment variables and the optional config file
always win over database-backed settings.

## Configuration sources

Effective values resolve through four layers, highest precedence first:

1. environment variables (`MELODEE__*` — the documented keys below, plus any registry key as
   `MELODEE__` + the key with `.` replaced by `__`, uppercased);
2. an optional JSON config file (`MELODEE__CONFIG_FILE` points at a bounded
   `{"key": value, …}` object). On supported Unix platforms, the file and every
   parent component are opened without following symlinks; a symlink or special-file
   replacement fails closed;
3. database-backed settings (`server_settings`, managed at runtime through the CLI and the admin
   API);
4. built-in defaults (the legacy registry's seeded values).

Only documented keys are supported. The database layer covers the 190-key legacy settings
registry; the fixed keys in the tables below (worker, ingestion, search, auth, and email
transport plumbing) remain environment-driven.

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
| `MELODEE__DATABASE__URL` | — (required) | PostgreSQL connection URL. |
| `MELODEE__DATABASE__MAX_CONNECTIONS` | `32` | Maximum database pool connections. |
| `MELODEE__DATABASE__MIN_CONNECTIONS` | `2` | Minimum idle database pool connections. |
| `MELODEE__DATABASE__ACQUIRE_TIMEOUT_SECONDS` | `30` | Timeout in seconds for acquiring a connection from the pool. |
| `MELODEE__DATABASE__IDLE_TIMEOUT_SECONDS` | `600` | Timeout in seconds for idle connections before closing. |
| `MELODEE__ADMIN_UI__DIST` | `admin-ui/dist` | Directory containing the built admin SPA, served at `/admin` with client-side routing fallback to `index.html`. A missing directory is not fatal: the server starts, all APIs serve normally, and `/admin` answers 404 until the bundle is deployed. |
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
| `MELODEE__INGEST__STAGING_ROOT` | — (required with inbound sources) | Absolute, durable Melodee-owned original-source root. It must be writable and disjoint from inbound, work, and canonical storage. |
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
| `MELODEE__NORMALIZATION__WORK_AREA` | `/tmp/melodee-work` | Temporary, non-authoritative root for attempt-isolated normalized output. It is never used as durable staging. |

### Validation and canonical publication

| Key | Default | Effect |
| --- | --- | --- |
| `MELODEE__VALIDATION__CONCURRENCY` | `2` | Maximum normalized Releases structurally validated simultaneously by one process. |
| `MELODEE__PUBLICATION__CONCURRENCY` | `2` | Maximum complete Release bundles copied and finalized simultaneously by one process. |
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
| `MELODEE__POLICY__RULE_SET_VERSION` | `1` | Immutable version for the configured built-in rule set. Increment it whenever a preference changes. |
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
| `MELODEE__SEARCH__SNAPSHOT_PAGE_SIZE` | `1000` | Maximum PostgreSQL rows held per entity page during rebuild. |
| `MELODEE__SEARCH__DEFAULT_LIMIT` | `200` | Default operator query limit. |
| `MELODEE__SEARCH__MAX_LIMIT` | `1000` | Maximum operator query limit. |

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
| `MELODEE__NATIVE__PUBLIC_ORIGIN` | Listener origin | External origin used in native-v1 image, stream, and browser-login redirect URLs. It must be an absolute `http` or `https` origin without credentials, a path, or a query. Set it to the reverse proxy's public origin in production. |
| `MELODEE__NATIVE__STREAM_CONCURRENCY` | `32` | Maximum direct media responses open concurrently in one server process (`1`–`4096`). Additional requests wait with bounded backpressure. |
| `MELODEE__NATIVE__AUTH__JWT_KEY` | — (required) | HS256 signing secret for native-v1 15-minute access tokens and protected browser session cookies. Generate at least 32 random bytes (for example, `openssl rand -base64 48`) and keep the same secret on every server process. The example placeholder is rejected at startup. |
| `MELODEE__NATIVE__AUTH__MAX_SESSION_DAYS` | `90` | Absolute refresh-family lifetime (`30`–`365` days); individual refresh tokens remain valid for at most 30 days and rotate on use. |
| `MELODEE__NATIVE__AUTH__DEVELOPMENT_RESET_TOKENS` | `false` | Includes the one-time reset token and URL in a successful password-reset request. Enable only on isolated development systems. |
| `MELODEE__NATIVE__AUTH__RATE_ATTEMPTS` | `10` | Authentication attempts allowed for one normalized actor/address key per fixed window (`1`–`10000`). |
| `MELODEE__NATIVE__AUTH__RATE_WINDOW_SECONDS` | `60` | PostgreSQL-backed authentication rate window (`1`–`86400` seconds). |
| `MELODEE__NATIVE__AUTH__RATE_MAX_KEYS` | `100000` | Maximum active rate-counter keys retained cluster-wide (`1`–`10000000`). New keys fail closed at capacity. |
| `MELODEE__NATIVE__CORS__ALLOWED_ORIGINS` | empty | Comma-separated exact `http`/`https` browser origins allowed to call native v1. Wildcards are rejected. The policy admits native `GET`/`POST`/`PUT`/`PATCH`/`DELETE` requests and exposes only the paging, range, duration, and ETag response headers used by browser clients. Include MeloAmp's exact development origin (for example, `http://localhost:3001`) only when needed. |
| `MELODEE__NATIVE__TRUSTED_PROXIES` | empty | Comma-separated exact proxy IPs or CIDRs allowed to supply `Forwarded`/`X-Forwarded-For`. Untrusted forwarding headers are ignored. For a trusted peer, Melodee walks the forwarded chain from right to left, removes trusted proxy hops, and uses the first untrusted address as the client. List only proxy infrastructure you control. |
| `MELODEE__NATIVE__AUTH__GOOGLE__ENABLED` | `false` | Enables Google ID-token exchange only when the client ID and JWKS configuration also validate. |
| `MELODEE__NATIVE__AUTH__GOOGLE__CLIENT_ID` | — | Exact Google OAuth client ID used as the required token audience. |
| `MELODEE__NATIVE__AUTH__GOOGLE__JWKS_URL` | Google v3 certificates URL | HTTPS JWKS endpoint. Redirects are rejected and the response/cache are bounded. |
| `MELODEE__NATIVE__AUTH__GOOGLE__HOSTED_DOMAINS` | empty | Optional comma-separated lowercase hosted-domain allowlist checked after signature, issuer, audience, lifetime, and verified-email validation. |

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

### Email notifications

Melodee sends user-facing email (password resets, share-created and request-fulfilled
notifications) through a configured SMTP relay. Delivery is disabled by default; while disabled,
every notification is still recorded in the delivery audit as `skipped` and the operation that
triggered it is unaffected. Delivery failures are likewise recorded and logged, never rolled back
into the triggering request. The password is never logged and never returned by any API.

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
also part of the database-backed settings layer (registry keys `email.*`): environment and
config-file values still win over database rows.

### Last.fm scrobbling

Last.fm integration is disabled by default. The registry keys `scrobbling.lastFm.Enabled`
(environment `MELODEE__SCROBBLING__LASTFM__ENABLED`), `scrobbling.lastFm.apiKey`,
`scrobbling.lastFm.sharedSecret`, and `security.secretKey` control it; the API key and shared
secret are classified secret and are redacted from every API, export, and log. See
[Last.fm scrobbling](../features/lastfm/) for the linking flow and failure behavior.

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
  Phase 5 worker and CLI do not complete destructive actions or remove media.
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
