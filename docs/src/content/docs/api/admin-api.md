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

## Authentication and authorization

Every `/api/admin/v1` route requires a valid native access token
(`Authorization: Bearer …`) or valid browser session cookie (`Cookie: melodee_auth=…`) for an
enabled, unlocked account with administrator rights. Requests without valid credentials return
`401`; authenticated non-administrators receive `403`. Both use the stable
[error envelope](#error-behavior). The infrastructure probes `/healthz` and `/readyz` are
separate routes and remain anonymous.

The admin single-page application (served by the server at `/admin` when the built bundle is
deployed — see [Configuration](../installation/configuration/)) signs in through the native
cookie sign-in endpoints (`POST /api/v1/auth/cookie/sign-in`) and is bound by the same gate;
route guards in the UI are a convenience only.

The browser-facing `/admin` and `/api/admin/v1` responses carry a dedicated security-header
policy: a same-origin Content Security Policy, `X-Content-Type-Options: nosniff`,
`X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, and a restrictive
`Permissions-Policy`. The native v1, OpenSubsonic, Jellyfin, media, and health compatibility
responses are not modified by this policy, so their captured headers remain stable. Configure
HSTS at the TLS-terminating reverse proxy when serving the application over HTTPS.

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
GET /api/admin/v1/ingest/work-items?state=ready&limit=50&offset=0&sort=addedAt:desc
```

Returns a bounded, server-side paginated list of work items inside a pagination envelope:

```json
{
  "items": [ /* work items */ ],
  "total": 137,
  "limit": 50,
  "offset": 0
}
```

`total` is the row count for the current filter (it respects `state` but ignores `limit`/`offset`),
so clients can render paging honestly. Supports optional `state` filtering and `limit`/`offset`
pagination. `limit` defaults to 50 and is clamped to a server maximum (500); clients cannot pull
the whole queue in one request.

The optional `sort` parameter is a strict whitelist in `key:asc|desc` form; keys are `addedAt`,
`updatedAt`, `state`, and `attemptCount`. An omitted `sort` keeps the newest-first ordering
(`addedAt:desc`). Unknown keys, unknown directions, or a missing `:asc|:desc` suffix return `400`
with code `unknown_sort`.

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

These routes follow the same administrator gate described above. Every mutation is audit-logged
with the acting administrator's public user id. Passwords are accepted only on creation and on
the password-set route, are stored as bcrypt hashes, and never appear in responses or logs.

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
GET /api/admin/v1/users?query=ann&limit=50&offset=0&sort=username:asc
POST /api/admin/v1/users
PUT /api/admin/v1/users/{id}
POST /api/admin/v1/users/{id}/password
```

`GET` returns the paginated user listing in the same envelope shape as the work-item listing
(`{"items", "total", "limit", "offset"}`). `query` is an optional case-insensitive substring
filter on username or email; `limit` defaults to 50 and is clamped to 500; `sort` is a strict
whitelist in `key:asc|desc` form with keys `username` and `createdAt` (default `username:asc`);
unknown sorts return `400 unknown_sort`. Items contain operator-safe account fields only — never
password hashes or signing keys.

`POST` creates an enabled, unlocked account from `{"username", "email", "password", "isAdmin"?,
"isEditor"?, "roles"?}`; when `roles` is omitted the account receives the streaming role. A
duplicate username returns `409 user_conflict`; passwords shorter than 8 characters are rejected.
`PUT` applies a field-optional update (`isAdmin`, `isEditor`, `enabled`, `locked`, `roles`); any
change bumps the credential revision, and disabling or locking an account revokes all of its
refresh sessions immediately. Unknown ids return `404`.

`POST …/{id}/password` sets a new password from `{"password": "…"}` (8–1024 characters) and
answers `204`. The credential revision is bumped and **all** of the account's refresh sessions are
revoked immediately, exactly as when an account is disabled or locked — the user must sign in
again on every device. The change is audit-logged with the acting administrator's public user id;
the new password never appears in responses or logs.

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

## Release review

These routes back the operator review workflow for ingestion work items parked in
`needs_review` (awaiting an operator decision), `failed`, or `quarantined`. All routes require
an administrator's native access token; every mutation is audit-logged with the acting
administrator's public user id.

### Review queue

```http
GET /api/admin/v1/review/queue?state=needs_review&limit=50&offset=0&sort=addedAt:desc
```

Returns the bounded, server-side paginated queue of review-able work items plus
`countsByState` for the three review states (so the UI can render tab badges from one request).
The optional `state` filter only accepts the review-able states (`needs_review`, `failed`,
`quarantined`); any other value returns `400` (`state_not_reviewable` or `unknown_state`).
`sort` uses the same whitelist as the work-item listing. Each row additionally carries
`releaseId` when the work item is bound to a catalog Release.

### Review detail

```http
GET /api/admin/v1/review/{workItemId}
```

Returns the full review picture for one work item: the durable work item, the manifest summary
(track inventory with per-Track fingerprint status, artwork and sidecar observations, finding
count — never the raw payload blob), the persisted classification with its evidence, the catalog
comparison (Known/New/Alternate/Uncertain aggregates grouped from the per-Track novelty
categories plus the durable counts), the durable policy decision (matched rules, evidence
summary, override reason), and the append-only timeline.

### Apply a review decision

```http
POST /api/admin/v1/review/{workItemId}/decision
```

Body: `{"action": "accept"|"quarantine"|"ignore"|"delete", "reason": "…", "override": false}`.
A non-blank `reason` is required and audited. The work item must be in `needs_review`
(otherwise `409 invalid_state`).

When the Release has a pending policy decision, the decision applies the durable override path:
`accept` resumes the pipeline (the work item moves to `indexing`), `quarantine`, `ignore`, and
`delete` record the corresponding disposition. `delete` additionally requires the explicit
`"override": true` acknowledgement (`400 override_required` otherwise). The server commits the
authorization, removes the exact canonical and retained staging directories, and then atomically
marks the Release/work item deleted, appends audit evidence, and schedules search removal. Success
returns `pendingExecution: false` and `workItem.state: "deleted"`. A filesystem/configuration error
returns `500 delete_execution_failed` or `503 delete_storage_unavailable`; the authorization remains
pending and the confirmed action is safe to retry. For items parked before cataloging (no policy decision),
`quarantine` and `ignore` park the work item directly; `accept` and `delete` are unavailable in
that state (`409`).

### Reprocess a work item

```http
POST /api/admin/v1/review/{workItemId}/reprocess
```

Body: `{"reason": "…"}` (optional). Moves a `failed` or `quarantined` work item back to `ready`
so the pipeline picks it up again. Any other state returns `409 not_reprocessable`.

### Pre-publication track preview

```http
GET /api/admin/v1/review/{workItemId}/tracks/{trackIndex}/preview
```

Streams one track's audio for review **before** publication: the normalized work-area output
when available, otherwise the observed source file. The file is located exclusively from the
durable manifest and the configured durable staging root / inbound source roots / work area —
paths are canonicalized and must remain inside those configured roots, so no client input can
steer the read outside them. After durable staging acquisition, the observed source file lives
under the staging root. HTTP `Range` is honored (`206` with `Content-Range`) so clients can seek;
responses carry `Cache-Control: no-store` because pre-publication bytes are not cacheable catalog
content. When no previewable file exists under the configured roots the route returns
`404 preview_unavailable`.

## Artist identity review

These routes back the Artist identity curation workflow: resolving observed artist strings to
Melodee-owned Artist identities, recording do-not-match assertions, and correcting identities
through explicit merge/split operations. All routes require an administrator's native access
token; every mutation is audit-logged with the acting administrator's public user id.

### Unresolved observation queue

```http
GET /api/admin/v1/identity/unresolved?limit=50&offset=0&sort=createdAt:asc
```

Returns the bounded, server-side paginated queue of observed artist strings that have no
authoritative match decision. Each row carries the display/raw/normalized values, the source
Release (and Track when the observation is track-scoped), and the pending candidate match and
evidence counts. `sort` is a strict whitelist with keys `createdAt` and `displayValue`;
unknown sorts return `400 unknown_sort`.

### Observation detail

```http
GET /api/admin/v1/identity/observations/{id}
```

Returns the observation, the current authoritative decision when one exists, every candidate
Artist with its latest decision and the full persisted evidence breakdown, and any recorded
do-not-match assertions.

### Resolve an observation

```http
POST /api/admin/v1/identity/observations/{id}/resolve
```

Body: `{"resolution": "confirm"|"create-new"|"leave-provisional", "artistId": "…", "reason": "…"}`.
A non-blank `reason` is required and audited.

- `confirm` requires `artistId` (the Artist public id) and records a locked administrator
  decision, so no automated resolver supersedes it; the observation leaves the unresolved queue.
- `create-new` must not carry `artistId`; it creates a new provisional Artist from the
  observation's display value and records the same locked decision.
- `leave-provisional` must not carry `artistId` and is an honest, audited no-op — no decision
  row is written and the observation stays queued for a later review pass
  (`"changed": false`).

### Record a do-not-match assertion

```http
POST /api/admin/v1/identity/observations/{id}/never-match
```

Body: `{"artistId": "…", "reason": "…"}`. Records a durable, locked negative assertion against
the exact observation/candidate pair; automated resolvers then fail closed for that pair.
Repeating the same pair returns `409`.

### Identity decision feed

```http
GET /api/admin/v1/identity/decisions?limit=50&offset=0
```

Returns the append-only audit feed of identity decisions (observed-artist matches, evidence,
assertions, and Artist-level events), newest first, with actor and reason.

### Merge two Artists

```http
POST /api/admin/v1/identity/artists/merge
```

Body: `{"sourceArtistId": "…", "targetArtistId": "…", "reason": "…"}`. Merges the source Artist
into the target; the source's public id is preserved as a redirect and the merged Artist can no
longer participate in identity mutations (`409 artist_not_resolvable`). Merging an Artist into
itself returns `400 self_merge`.

### Preview and merge multiple Artists

```http
POST /api/admin/v1/identity/artists/merge/preview
POST /api/admin/v1/identity/artists/merge/batch
```

Preview body:

```json
{
  "sourceArtistIds": ["…", "…"],
  "targetArtistId": "…"
}
```

The preview accepts one through 100 distinct source public IDs, none equal to the target, and
returns the resolved target and sources, affected Release/Track/credit and identity-row counts,
user/provider/image conflict counts, and the scalar metadata fields for which the selected target
will win. Unknown, merged, or deprecated identities fail closed without changing data.

The batch route accepts the same selection plus a required audit reason:

```json
{
  "sourceArtistIds": ["…", "…"],
  "targetArtistId": "…",
  "reason": "Duplicate import identities"
}
```

Every source is merged in one PostgreSQL transaction. Release and Track billing, contributions,
aliases, external identities, relationships, memberships, observations, discovery links, native
per-user Artist state, compatible artwork, and provider data are coalesced. The selected target's
scalar identity metadata and existing operator artwork win conflicts. Each source becomes a
redirecting tombstone and each merge receives a distinct audit ID; the response returns
`mergeIds`, `sourceArtistIds`, and `targetArtistId`. A validation or correction-bound failure rolls
back the complete batch. No canonical media is moved or deleted.

### Split an Artist

```http
POST /api/admin/v1/identity/artists/{id}/split
```

Body: `{"reason": "…", "newArtist": {"canonicalName": "…", "sortName"?, "artistType"?,
"disambiguation"?}, "assignments": [{"kind": "…", "id": "…"}]}`. Creates a new provisional
Artist and moves **only** the explicitly selected assignments off the source Artist —
`assignments` must be non-empty (`400 empty_split`). `kind` is one of `releaseArtist`,
`trackArtist`, `contribution`, `alias`, or `externalIdentity`; each `id` is the internal durable
row identity of the selected link.

## Catalog browsing and detail

These routes back the catalog browsing tables and the Artist/Release/Track detail views. They
require an administrator's native access token and use Melodee's own vocabulary (`Artist`,
`Release`, `Track`) with public ids as lookup keys; unknown ids return `404 not_found`. Every
listing uses the shared pagination envelope (`items`, `total`, `limit`, `offset`; `limit`
defaults to 50 and clamps to 500; `offset` must not exceed 1,000,000) and a strict
`key:asc|desc` sort whitelist (`400 unknown_sort` otherwise). An excessive offset returns
`400 invalid_offset`. The optional `query` parameter is a bounded case-insensitive substring
filter (overlong queries return `400 invalid_query`). Each catalog listing has a 10-second
server-side deadline covering its count and page query; deadline exhaustion returns
`503 query_timeout` instead of retaining a database connection indefinitely. Browse and read
routes are read-only; the one write surface in this section is
[Catalog metadata editing](#catalog-metadata-editing), which updates the display-metadata fields
of an existing entity.

### Browse Artists

```http
GET /api/admin/v1/catalog/artists?query=…&limit=50&offset=0&sort=name:asc
```

Returns Artists with identity facts (`name`, `sortName`, `artistType`, `disambiguation`,
`status`), set-based catalog counts (`releaseCount`, `trackCount`), and an `imageId`. Sort keys:
`name`, `createdAt`, `updatedAt` (default `name:asc`). `query` matches the canonical/sort name
and recorded aliases.

### Browse Releases

```http
GET /api/admin/v1/catalog/releases?query=…&artistId=…&limit=50&offset=0&sort=releaseDate:desc
```

Returns Releases with `title`, `displayArtist`, `releaseDate` (extended ISO `YYYY-MM-DD` or
`null`), `releaseDateText`, `status`, aggregate `trackCount`/`durationMs`, and an `imageId`.
Sort keys: `title`, `releaseDate`, `createdAt`, `updatedAt` (default `title:asc`). `query`
matches the title; `artistId` (an Artist public id) restricts to Releases that Artist is billed
on.

### Browse Tracks

```http
GET /api/admin/v1/catalog/tracks?query=…&releaseId=…&limit=50&offset=0&sort=title:asc
```

Returns Tracks with `title`, `displayArtist`, `ordinal`, `discNumber`/`trackNumber`,
`durationMs`, the Release linkage (`releaseId`, `releaseTitle`), and an `imageId`. Sort keys:
`title`, `durationMs`, `createdAt` (default `title:asc`). `query` matches the title;
`releaseId` (a Release public id) restricts to one Release.

### Artist detail

```http
GET /api/admin/v1/catalog/artists/{id}
```

Returns the full operator image of one Artist: identity facts (name, sort name, type,
disambiguation, status, begin/end dates, merge target when merged), set-based stats
(`releaseCount`, `billedTrackCount`, `performanceTrackCount`, `allCreditTrackCount`), typed
aliases, external provider identities, the Releases the Artist is billed on, contributions
(appearances), Artist relationships, group memberships, an `imageId`, and the append-only audit
history. Every sub-list is bounded (at most 100 items) and carries its full `total` as
`{items, total}` so truncation is visible.

### Release detail

```http
GET /api/admin/v1/catalog/releases/{id}
```

Returns the full operator image of one Release: overview facts (title, sort title, display
artist, release date, status, revision), aggregate stats (`trackCount`, `durationMs`,
`mediaBytes`), the ordered Track list with media format facts (container, codec, byte size)
and a per-Track `streamUrl` (see [Track detail](#track-detail) below),
billed Artists with roles and join phrases, contributors, the distinct media files, the
persisted classification (traits, confidence, classifier version) when policy has evaluated the
Release, the immutable import provenance (work item, manifest ids and hashes, resolver version,
import time), an `imageId`, and the audit history. Sub-lists are bounded as above.

### Track detail

```http
GET /api/admin/v1/catalog/tracks/{id}
```

Returns one Track: title/sort title, display artist, ordinal and disc/track numbers, duration,
source path, audio identity id, the Release linkage, billed Artists, contributors, every
linked media asset with its purpose (exactly one `playable`), and `streamUrl`.

`streamUrl` is a short-lived signed audio preview URL (PRD-UI-007) for the acting
administrator on the native `/song/stream/{trackId}/{userId}/{token}` route, issued only when
the Track has a playable media asset; it is `null` when there is no playable asset or the
administrator is not stream-authorized (no streaming role, or no access to the source
library). The URL is bound to the requesting client address like any native stream URL.

The `imageId` values returned by these routes are consumed by the existing
`/images/{imageId}/{size}` endpoint shared with the native API; its id prefixes (`album_`,
`artist_`, `song_`) are that endpoint's frozen contract. Artwork bytes are never served by the
admin API itself.

### Catalog artwork overrides

Operators can upload or remove custom artwork for one Artist or Release (ADR-0143). An
override is served with precedence by the shared `/images/{imageId}/{size}` endpoint for the
`album_`, `artist_`, and `song_` id prefixes (a Song serves its Release's override); entities
without an override keep the publication-plan/placeholder behavior unchanged. Overrides are
curation data only: upload and removal never touch canonical media files or publication plans.

```http
PUT /api/admin/v1/catalog/artists/{id}/image
PUT /api/admin/v1/catalog/releases/{id}/image
```

Raw request body with `Content-Type: image/jpeg` or `image/png`; the declared type must match
the magic bytes. Uploads are validated against the shared image pipeline bounds (at most
10 MiB encoded, dimension and pixel caps, decodability) — invalid, truncated, mismatched, or
oversized uploads return `400 invalid_image`. Unknown entity ids return `404 not_found`. The
response is `{"message", "imageId", "updatedAt"}` where `imageId` carries the frozen endpoint
prefix (`artist_…` / `album_…`). Re-uploading replaces the override.

```http
DELETE /api/admin/v1/catalog/artists/{id}/image
DELETE /api/admin/v1/catalog/releases/{id}/image
```

Removes the override row only, restoring plan artwork or the placeholder. Idempotent: `204 No
Content` whether or not an override existed; `404 not_found` for unknown entity ids. An
optional JSON body `{"reason": "…"}` is recorded in the audit event. Every upload and removal
is audit-logged (`catalog_audit_events`, event types `image_override_set` /
`image_override_removed`) with the acting administrator id.

### Catalog metadata editing

Operators can edit the display metadata for a Release, Artist, or Track. All three routes use
a **partial-patch** model: every field in the JSON body is optional; `null` or absent means
"leave unchanged". An empty or whitespace-only string for a name field is rejected with
`400 invalid_request`.

```http
PUT /api/admin/v1/catalog/releases/{id}/metadata
PUT /api/admin/v1/catalog/artists/{id}/metadata
PUT /api/admin/v1/catalog/tracks/{id}/metadata
```

**Release fields** (camelCase JSON):

| Field | Type | Notes |
|---|---|---|
| `title` | string | non-blank when present; recomputes `sortTitle` if not overridden |
| `sortTitle` | string | operator-supplied sort key wins over the computed value |
| `displayArtist` | string | non-blank when present |
| `releaseDate` | string (ISO-8601 date) | e.g. `2008-01-15` |
| `releaseDateText` | string | free-form display text |

**Artist fields**:

| Field | Type | Notes |
|---|---|---|
| `canonicalName` | string | non-blank when present; recomputes `sortName` if not overridden |
| `sortName` | string | operator-supplied sort key wins |
| `artistType` | string | one of: `person`, `group`, `orchestra`, `ensemble`, `choir`, `character`, `other`, `unknown` |

**Track fields**:

| Field | Type | Notes |
|---|---|---|
| `title` | string | non-blank when present |
| `sortTitle` | string | operator-supplied sort key |
| `displayArtist` | string | non-blank when present |
| `ordinal` | integer | must be positive when present |
| `durationMs` | integer | must be non-negative when present |

All three routes return `204 No Content` on success. Unknown entity ids return
`404 not_found`. Validation failures (blank names, non-positive ordinal, negative duration,
unknown artist type) return `400 invalid_request`. Every successful edit writes a
`catalog_audit_events` row (event types `release_metadata_updated`,
`artist_metadata_updated`, `track_metadata_updated`) and bumps the entity revision so the
search projection refreshes. Concurrency is last-write-wins: there is no client-supplied
revision guard, so two concurrent edits both succeed and the later one's values win.


### Scheduled jobs

```http
GET /api/admin/v1/jobs/history?jobName=discovery&limit=50&offset=0
GET /api/admin/v1/jobs/failures/summary?hours=24
POST /api/admin/v1/jobs/{name}/run-now
POST /api/admin/v1/jobs/{name}/pause
POST /api/admin/v1/jobs/{name}/resume
```

Job history is durable PostgreSQL data. A row records the job name, scheduled tick (when
applicable), start/completion times, duration, success, manual-trigger flag, triggering
principal, and a bounded failure reason. `status` is `running`, `completed`, `failed`, or
`stale`; stale means an unfinished row is older than 15 minutes and is not treated as successful.
The list is server-paged and capped at 500 rows.

The manual run allow-list is `discovery`, `podcast`, `enrichment`, `now-playing-cleanup`, and
`job-history-cleanup`. Unknown names return `400 unsupported_job`. The pause/resume endpoints
operate on the durable worker-pool state for the named job and return the worker-pool status.
Cleanup is bounded and audited; it never removes source media or catalog records.

### Release metadata transformations

The metadata rewrite endpoints are administrator-only and scoped to one Release:

```http
POST /api/admin/v1/catalog/releases/{id}/metadata-transforms/preview
POST /api/admin/v1/catalog/releases/{id}/metadata-transforms/apply
GET /api/admin/v1/catalog/releases/{id}/metadata-transforms?limit=50
POST /api/admin/v1/catalog/releases/{id}/metadata-transforms/{auditId}/revert
```

Preview returns a stable preview hash, affected-file count, field-level deltas, and validation
errors. Apply requires that preview hash. Supported operations include featuring/title cleanup,
artist replacement or clearing, year updates, track renumbering, and text replacement. Writes
use same-directory temporary files and replacement, retain a recovery copy, verify decoded audio
fingerprints, and record an audit row. Paths, extensions, audio payloads, and semantic catalog
identifiers are not changed. Revert restores the retained originals and updates the media hash.

## Dashboard

```http
GET /api/admin/v1/dashboard
```

One aggregated read for the overview page. All figures come from durable PostgreSQL state with
bounded, set-based queries:

- `catalog` — total `artists`, `releases`, `tracks`, `mediaAssets`, and the pending
  search-projection outbox depth (`pendingSearchOutbox`).
- `workItems` — `countsByState` for the ingestion queue plus the rolling 24-hour windows
  `failedLast24h` (items currently `failed` whose last update is inside the window) and
  `completedLast24h` (items with a `completedAt` inside the window).
- `attention` — queue depths needing an operator: `unresolvedIdentities` (observed artist
  strings without an authoritative match decision), `pendingPolicyDecisions` (durable decisions
  awaiting completion), `failedProcessing`, and `quarantined`.
- `health` — the subsystem panel described under [System health](#system-health) below.
- `activity.completedPerHour` — completed work items per hour over the trailing 24 hours
  (`[{hour, count}]`, oldest first; hours without completions are omitted).

## Policies

These routes manage the versioned policy rule set that maps Release classification and Track
novelty facts to a disposition. Rule sets are **immutable and versioned**: every mutation
produces a new `(name, version)` row validated through the policy engine invariants (the
administrator-override rule first, the corrupt-media quarantine rule second, per-type
disposition safety, unique rule ids); nothing is edited in place and history is never
rewritten. A concurrent mutation loses with `409 rule_set_conflict`. The running policy worker
adopts a new version when its configured rule-set version matches
(`MELODEE__POLICY__RULE_SET_VERSION`); authoring a version here never interrupts in-flight
evaluations and never touches a work item or decision.

### List rules

```http
GET /api/admin/v1/policies/rules
```

Returns the active rule-set metadata (`ruleSet: {id, name, version, createdAt, createdBy}`) and
the rule table inside the standard pagination envelope. Each rule carries `id`, `ruleType`,
`disposition`, `confidenceThreshold` (basis points, 0–10000), the engine `predicate`
(`required_traits`, `existing_dispositions`), `priority` (zero-based evaluation position — lower
wins, first match applies), `enabled`, and `matchCount` (how many durable decisions the rule
produced). Rules removed from the active version remain listed from immutable history with
`enabled: false` and `priority: null` so they can be re-enabled.

### Create, update, delete

```http
POST /api/admin/v1/policies/rules
PUT /api/admin/v1/policies/rules/{id}
DELETE /api/admin/v1/policies/rules/{id}
```

Create/update bodies: `{"id": "…", "ruleType": "…", "disposition": "…",
"confidenceThreshold"?: 9000, "predicate": {…}}`. `ruleType` is one of `administrator_override`,
`corrupt_media`, `greatest_hits_redundancy`, `greatest_hits_novelty`, `greatest_hits_uncertain`,
`unknown_classification`, `trait_disposition`; `disposition` is one of `accept`, `review`,
`ignore`, `quarantine`, `delete`. New rules are appended at the lowest priority of a new
version; updates preserve the rule's position (`400 rule_id_mismatch` when the path and body
ids differ — renaming a rule is delete + create). Rules violating the engine's per-type
disposition safety (e.g. `delete` without a confidence floor) are rejected with
`400 invalid_rule` and never persist. The two safety rules (`administrator_override` first,
`corrupt_media` second) can be neither deleted nor disabled (`409 protected_rule`).

### Enable and disable

```http
POST /api/admin/v1/policies/rules/{id}/enable
POST /api/admin/v1/policies/rules/{id}/disable
```

Enable/disable are expressed as rule membership in the latest immutable version. Disabling
removes the rule in a new version (its definition survives in history); enabling restores the
most recent historical definition at the lowest priority (`404` when the rule never existed).
Both are idempotent: repeating the current state returns `200` with `changed: false` and creates
no version.

### Test a rule against the catalog

```http
POST /api/admin/v1/policies/rules/test
```

Body: exactly one of `{"ruleId": "…"}` (test an enabled rule of the active rule set) or
`{"rule": {…}}` (test an ad-hoc definition appended to the active set **without** persisting
it); optional `limit` clamps the evaluated Release count (default and maximum 500). The dry run
classifies and evaluates a bounded catalog snapshot read-only and returns `evaluated`,
`matchedCount` (Releases matched by the tested rule), per-rule `matchedRuleCounts`,
`proposedDispositionCounts`, `existingDispositionCounts`, `destructiveImpact` (Releases whose
proposed disposition is `delete`), `destructive` (whether the tested rule's action is
destructive-class), and a bounded `sample` of matched Releases (`{releaseId, title, artist}`).

Runs against a stored rule are persisted to the durable test-run audit (`persisted: true` with
the `testRunId`). Runs carrying an ad-hoc rule cannot be persisted — the audit table references
durably stored rule sets by foreign key, and storing the candidate would silently change the
active policy — so they return `persisted: false`.

## System, workers, and search

### Worker pools

```http
GET /api/admin/v1/workers/pools
POST /api/admin/v1/workers/pools/{name}/pause
POST /api/admin/v1/workers/pools/{name}/resume
```

The pool table lists every registered pool (`policy`, `podcast`, `discovery` are seeded) with
its durable pause state and audit (`paused`, `pausedBy`, `pausedAt`, `resumeRequestedAt`) plus
queue metrics where the pool's worker owns a durable queue table: `queued`, `running` (unexpired
leases — lease evidence, not a worker liveness claim), `oldestQueuedAgeSeconds`,
`completedLastHour`, `throughputPerMinute`, and `failedLast24h`. Pools without a durable queue
(discovery maintenance runs bounded idempotent passes) report `metricsAvailable: false` and null
metrics instead of invented zeros.

Pause/resume use the same lease-coordinated durable control plane as
`melodee-cli worker-pools pause|resume`: workers honor the pool row between claim batches, so a
pause stops new claims without stranding in-flight leases, and resume stamps
`resumeRequestedAt`. Unknown-but-valid pool names upsert a paused row (a pool wired in a later
binary is paused the moment it first checks); invalid names return `400 invalid_pool_name`.

### Recent failures

```http
GET /api/admin/v1/workers/failures?limit=50&offset=0&sort=updatedAt:desc
```

Bounded, paginated listing of work items currently in `failed`, inside the standard pagination
envelope, with the same whitelisted `sort` keys as the work-item listing. Each row is the full
operator-safe work item (state, attempts, last error code/message, lease evidence).

### Search index administration

```http
GET /api/admin/v1/search/status
POST /api/admin/v1/search/rebuild
```

`status` reports the projection (`pending`/`failed`/`leased` outbox rows, last processed outbox
id, active generation id, latest rebuild status/error) and the active generation's document
counts (`artists`, `releases`, `tracks`, `schemaVersion`) when one is validated; when the search
worker is disabled the response is `{"enabled": false}`.

`rebuild` records the durable rebuild request through the same path as
`melodee-cli search rebuild` and returns `202` with the `jobId` — the running search worker claims
and executes the job, so the HTTP call never blocks on the rebuild. A second request while an
unfinished job exists returns `409 rebuild_in_progress`; a disabled search worker returns
`409 search_disabled`.

### System health

```http
GET /api/admin/v1/system/health
```

Aggregated subsystem health: `postgresql` (bounded probe, as in `readyz`), `tantivy` (active
search-generation diagnostic, or `disabled` when the search worker is off), `storage` and
`ffmpeg` (the onboarding preflight checks), `podcastStorage` (podcast storage-root
writability, or `disabled` when the podcast worker is unconfigured), `disk` (free space across
the storage, work-area, and podcast roots), `workers` (stale work-item leases and durable
queue depth), and `apiSurfaces` (the process-local API surfaces, `serving`). Each subsystem
reports `status` and an operator-safe `detail`. The subsystem status vocabulary is:

- `ok` — healthy;
- `degraded` — impaired but the process keeps serving;
- `disabled` — a deliberately unconfigured optional subsystem, never a failure;
- `unavailable` — a required dependency cannot be reached;
- `readOnly` — the root exists but is not writable;
- `misconfigured` — required configuration is absent or invalid;
- `capacityExhausted` — a bounded resource is at its floor (for `disk`: under 1 GiB free or
  under 5% free on any checked root).

The top-level `status` is `ok` when PostgreSQL is reachable and Tantivy is healthy or
disabled, otherwise `degraded`; the `workers` subsystem degrades on stale leases (expired
beyond the one-hour grace) or a durable queue whose oldest ready item is more than 24 hours
old.

`postgresqlPool` reports the SQLx pool's point-in-time `maxConnections`, `openConnections`,
`idleConnections`, `inUseConnections`, and `utilizationPercent`. It contains no URL, database name,
hostname, username, or credential. The PostgreSQL subsystem detail repeats the current in-use/open/
idle counts so the Admin health panel surfaces saturation without requiring raw JSON inspection.

The payload also carries a `resourceBounds` array: a point-in-time snapshot of every
semaphore-bounded request-path resource class (`streams.direct_play`, `streams.transcoding`,
`images.processing`, `podcast.discovery.http`, `notifications.dispatch`,
`provider.lastfm.dispatch`). Each entry reports `class`, `max` (the class capacity),
`inFlight`, `waiting`, `oldestWaitMs` (age of the oldest queued waiter, `0` when idle),
`admittedTotal`, `rejectedTotal` (non-blocking admissions refused at capacity), and
`saturated` (`true` when the class is at capacity or has queued waiters). These counters are
observational only — they never change admission decisions. Background worker classes are
bounded separately and reported through the durable queue metrics on the workers surface.

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

Status codes follow standard HTTP conventions: `400` for malformed requests (including unknown
state filters and sort keys), `401` for missing/invalid/expired credentials, `403` for
authenticated non-administrators, `404` for unknown resources, `409` for conflicts, and `500` for
upstream persistence failures.

:::note[Admin UI]
Every endpoint above backs the Carbon-based admin single-page application served at `/admin`;
see [Admin Overview](../admin/overview/) for the screens built on these routes. Remaining
catalog curation endpoints will be documented here when implemented.
:::
