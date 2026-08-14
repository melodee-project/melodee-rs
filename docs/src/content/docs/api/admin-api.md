---
title: Admin API
description: The Rust-native administration and diagnostics API for operators.
sidebar:
  order: 2
---

The Admin API is a Rust-native administration and curation API, separate from the legacy Melodee
API v1, OpenSubsonic, and Jellyfin compatibility surfaces. It uses Melodee's own vocabulary
(`Release` work items, not `Album`) and is independently versioned under `/api/admin/v1`.

It is intended for operators and administrators monitoring ingestion, diagnosing failures, and
eventually curating the catalog. It is **not** a compatibility surface for existing clients.

:::caution[Pre-release access control]
The current Rust development server does not yet have an application authentication layer. Its
listener defaults to `127.0.0.1`; do not expose these operator diagnostics to an untrusted network.
Authentication and permission behavior will be documented when that implementation ships.
:::

## Implemented endpoints

The following diagnostics endpoints are implemented and tested against real PostgreSQL. They
expose durable work-item state and evidence only — no secrets, no arbitrary filesystem contents,
and no raw database rows.

### System info

```http
GET /api/admin/v1/system/info
```

Returns service health and version metadata (`service`, `version`, `status`).

### Work-item state counts

```http
GET /api/admin/v1/ingest/work-items/counts
```

Returns per-state counts for the ingestion queue (e.g. `ready`, `failed`, `needs_review`).

### Work-item listing

```http
GET /api/admin/v1/ingest/work-items?state=ready&limit=50&offset=0
```

Returns a bounded, server-side paginated list of work items. Supports optional `state` filtering
and `limit`/`offset` pagination. `limit` is clamped to a server maximum; clients cannot pull the
whole queue in one request.

### Work-item detail

```http
GET /api/admin/v1/ingest/work-items/{id}
```

Returns the full durable work item by internal id or operator-safe api key, including state,
attempts, lease evidence, last error, and timestamps.

### Work-item event history

```http
GET /api/admin/v1/ingest/work-items/{id}/events
```

Returns the append-only transition/audit history for a work item — the answer to "what happened to
this Release and why".

### Active leases

```http
GET /api/admin/v1/ingest/leases?limit=50
```

Returns work items currently holding an **active (unexpired) lease**, with the lease owner,
acquired/expiry timestamps, and last activity. This is lease evidence proven by PostgreSQL — it
shows that a row holds an unexpired lease, not that the owner process is currently alive (no
heartbeat store exists yet).

## User and access-group administration

The user and group routes below are additionally gated: every request must carry a valid native
access token (`Authorization: Bearer …`) for an enabled, unlocked account with administrator
rights. Requests without a token return `401`; authenticated non-administrators receive `403`.
Every mutation is audit-logged with the acting administrator's public user id. Passwords are
accepted only on creation, are stored as bcrypt hashes, and never appear in responses or logs.

### Groups

```http
GET /api/admin/v1/groups
POST /api/admin/v1/groups
GET /api/admin/v1/groups/{id}
PUT /api/admin/v1/groups/{id}
DELETE /api/admin/v1/groups/{id}
PUT /api/admin/v1/groups/{id}/members/{userId}
DELETE /api/admin/v1/groups/{id}/members/{userId}
PUT /api/admin/v1/groups/{id}/libraries/{libraryId}
DELETE /api/admin/v1/groups/{id}/libraries/{libraryId}
```

Groups are addressed by their immutable public id; names are display values. `POST` and `PUT`
take `{"name": "…"}` (non-blank, at most 200 characters, unique per exact name). `GET` detail
includes member public ids/usernames and bound library ids. `DELETE` fails closed with
`409 group_not_empty` while the group still has members or library bindings — detach them first.
Membership changes take effect on the next request: once any group binds a library, only members
of a bound group can access it (see [Users and Access](../admin/users-and-access/)).

### Users

```http
POST /api/admin/v1/users
PUT /api/admin/v1/users/{id}
```

`POST` creates an enabled, unlocked account from `{"username", "email", "password", "isAdmin"?,
"isEditor"?, "roles"?}`; when `roles` is omitted the account receives the streaming role. A
duplicate username returns `409 user_conflict`; passwords shorter than 8 characters are rejected.
`PUT` applies a field-optional update (`isAdmin`, `isEditor`, `enabled`, `locked`, `roles`); any
change bumps the credential revision, and disabling or locking an account revokes all of its
refresh sessions immediately. Unknown ids return `404`.

### Internet radio stations

```http
GET /api/admin/v1/radio/stations?limit=50&offset=0
POST /api/admin/v1/radio/stations
GET /api/admin/v1/radio/stations/{id}
PUT /api/admin/v1/radio/stations/{id}
DELETE /api/admin/v1/radio/stations/{id}
```

These routes administer the same internet radio stations the OpenSubsonic surface serves
(`getInternetRadioStations` and friends) — a station created here appears there and vice versa.
Station ids are immutable public ids (the UUID the OpenSubsonic API renders as `radio_{uuid}`)
and are never regenerated.

Listing and reading require any authenticated, enabled account (native access token). Creating,
updating, and deleting additionally require administrator rights (`isAdmin` or the
`Administrator` role — the same gate as the OpenSubsonic radio mutations); non-administrators
receive `403`, anonymous callers `401`.

`GET` (list) is bounded and server-side paginated (`limit` clamped to a server maximum, default
50) and ordered by name. `POST` takes `{"name", "streamUrl", "homePageUrl"?}`; the name and
stream URL must be non-blank (at most 500 and 2000 characters respectively, homepage at most
2000) and violations return `400` with `invalid_name`, `invalid_stream_url`, or
`invalid_home_page_url`. `PUT` applies a field-optional update; an explicit `"homePageUrl": null`
clears the homepage link. `DELETE` answers `204`. Unknown ids return `404`.

### Device profiles

```http
GET /api/admin/v1/device-profiles?limit=50&offset=0
POST /api/admin/v1/device-profiles
GET /api/admin/v1/device-profiles/{id}
PUT /api/admin/v1/device-profiles/{id}
DELETE /api/admin/v1/device-profiles/{id}
POST /api/admin/v1/device-profiles/{id}/default
```

Device profiles are operator-managed **metadata records only**. They preserve the legacy
preview feature: a named profile carrying free-form client-matcher metadata (`clientMatch`, a
JSON object of bounded string values such as `{"clientName": "feishin"}`) and transcoding-intent
metadata (`profile`, e.g. `{"directPlay": false, "targetCodec": "opus", "maxBitrate": 192}`).

:::caution[Profiles do not control streaming]
Stored profiles are **not** consulted by any streaming path. Creating or editing a profile does
not change how media is delivered to any client. Profile-driven stream selection is planned
post-parity work and will be introduced through an explicit API contract, not silently.
:::

All routes require an administrator's native access token; anonymous callers receive `401` and
non-administrators `403`. Validation preserves the legacy rules: names must be non-blank (at
most 255 characters); a `profile` with `"directPlay": true` must not set `targetCodec` or
`maxBitrate`; a `profile` with `"directPlay": false` must set `targetCodec`; numeric fields keep
their legacy bounds (`maxBitrate` 1–9999, `resampleRate` 1000–192000, `priority` 0–100).
Violations return `400 invalid_profile`. At most one profile may be the default: creating a
profile with `"isDefault": true`, or `POST …/{id}/default`, atomically clears the previous
default. The listing is bounded and server-side paginated (default first, then name). Unknown
ids return `404`.

### Notification templates and delivery

```http
GET /api/admin/v1/notifications/templates?limit=50&offset=0
GET /api/admin/v1/notifications/templates/{templateKey}/{locale}
PUT /api/admin/v1/notifications/templates/{templateKey}/{locale}
DELETE /api/admin/v1/notifications/templates/{templateKey}/{locale}
GET /api/admin/v1/notifications/deliveries?limit=50&offset=0
POST /api/admin/v1/notifications/test-send
```

These routes administer the email notification templates used for password resets, share-created
and request-fulfilled messages. All routes require an administrator's native access token;
anonymous callers receive `401` and non-administrators `403`.

Templates are keyed by `(templateKey, locale)` — for example `user.password_reset` in `en` — and
carry a `subject` plus optional plain-text (`bodyText`) and HTML (`bodyHtml`) bodies with simple
`{{placeholder}}` substitution (unknown placeholders render verbatim; substituted values are
HTML-escaped in HTML bodies). `PUT` is a full-replacement upsert that bumps `version`; an explicit
`null` clears a field. Template keys must be non-blank (at most 100 characters), locales must be
BCP-47-shaped tags of at most 20 characters, subjects at most 500 characters, and each body at
most 64 KiB; violations return `400 invalid_template`. The server seeds default `en` templates for
`user.password_reset`, `share.created`, and `request.fulfilled`; deleting a seeded template is
allowed and disables the rendered body for that event (deliveries are then recorded as `skipped`).

`GET …/deliveries` lists the append-only delivery audit, newest first (bounded, server-side
paginated). Rows are metadata only — template key, recipient, `sent`/`failed`/`skipped` status,
and a bounded operator-safe error; message content is never stored or returned. Rows older than 30
days are purged automatically.

`POST …/test-send` takes `{"templateKey", "recipient", "locale"?}` (locale defaults to `en`, with
fallback to `en` for missing locales), renders the template, and attempts a real delivery through
the configured [email transport](../installation/configuration/), returning the honestly recorded
audit row — including `skipped` when delivery is disabled and `failed` when the relay refuses.
Template placeholders that a triggering event would supply (for example `resetUrl`) render
verbatim in test sends.

### Configuration export/import

```http
GET /api/admin/v1/config/export
POST /api/admin/v1/config/import?preview=true&allowUnknown=false
```

These routes administer the database-backed settings layer (see
[Configuration](../installation/configuration/)). Both require an administrator's native access
token; anonymous callers receive `401` and non-administrators `403`.

`GET …/export` returns a `melodee-config-export/v1` JSON document containing every known registry
setting's database-or-default value plus custom stored keys. Every secret value is replaced by a
fixed redaction marker and the document carries a header warning; secret values never leave the
server through this route.

`POST …/import` takes an export document as its body and validates every value against the
registry's per-key type, range, and enum rules. `preview` defaults to `true` and returns the diff
(`added`, `changed`, `unchanged`, `secretsReentered`, `secretsSkipped`, `unknown`, `warnings`)
without writing anything; secret keys are named only. With `preview=false` the import applies
atomically and records bounded audit evidence. Keys outside the known registry are rejected with
`400 invalid_document` unless `allowUnknown=true`; a redaction marker submitted as a value is
rejected with `400 redacted_marker_rejected` (fail closed — a redacted export can never overwrite
a stored secret); a value violating its key's rule returns `400 invalid_value`.

### Onboarding

```http
GET /api/admin/v1/onboarding/status
POST /api/admin/v1/onboarding/complete
```

First-run onboarding status and completion (administrator-gated). `status` returns the six
preflight checks (`database`, `storage_paths`, `media_tools`, `search_index`, `admin_user`,
`email`) with pass/fail and remediation hints, plus `completed`, `bypassed`, and `canComplete`. Installations migrated from a legacy server report
`bypassed: true` (existing catalog data or imported settings) and never require completion.
`complete` re-evaluates the checks and fails closed with `409 onboarding_checks_failed` while any
required check fails; otherwise it records the durable completion marker with the check evidence.

## Error behavior

Errors use a stable JSON envelope with a machine-readable `code`, an operator-safe `message`, and
a `correlationId`:

```json
{
  "code": "not_found",
  "message": "work item not found: 0192abcd-...",
  "correlationId": "0192efgh-..."
}
```

Status codes follow standard HTTP conventions: `400` for malformed requests, `404` for unknown
resources, `500` for upstream persistence failures.

:::caution[Partial implementation]
The endpoints above are the diagnostics subset plus user/access-group administration. Full
catalog curation, policy, and identity-review endpoints arrive in later phases and will be
documented here when implemented.
:::
