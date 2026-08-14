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
