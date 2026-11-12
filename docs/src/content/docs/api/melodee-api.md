---
title: Melodee API
description: Overview of the native Melodee API for client developers.
sidebar:
  order: 1
---

The native Melodee API is the full-featured API used by native Melodee clients. It is a
**compatibility contract**: routes, fields (including legacy names such as `album`/`song` where
the contract uses them), error shapes, authentication, and pagination are preserved deliberately
and are never renamed silently.

Implemented native-v1 surfaces include:

- **Authentication** — password and browser-cookie login, stateless access JWTs, refresh-token
  rotation/replay protection, logout/revocation, one-use password reset, and optional Google
  account linking.
- **Catalog endpoints** — artists, releases/albums, tracks/songs, search, images, streaming.
- **Feature endpoints** — playlists, shares, requests, podcasts, scrobbling, per-user playback
  settings, equalizer presets, charts, recommendations, listening analytics, audio features,
  playback-backend status, endpoint registry, and themes.
- **Errors and pagination** — exact envelope and nullability behavior.

Playback settings and equalizer presets are bearer-authenticated and scoped to the current user.
Settings updates and same-name preset writes are replay-safe. Equalizer lists are bounded; presets
accept at most 64 finite frequency/gain bands. Requests, protected shares, and podcasts use the same
native bearer authentication. Public share links are the intentional anonymous exception.

Access JWTs use the captured 15-minute lifetime and remain valid until expiry after logout;
logout revokes the user's refresh families. Refresh tokens rotate on every use, and reuse of an
old token revokes its entire family. Password-reset requests do not disclose whether an account
exists. Google authentication and linking are disabled unless the administrator configures the
provider; linked provider subjects are never returned by the API.

Direct Track URLs are HMAC-signed, bound to the authenticated user, Track, resolved client address,
and a seven-day expiry. `GET`, `HEAD`, and one RFC 7233 byte range are supported. Every request
rechecks the user's current library access before opening the canonical asset; invalid credentials
and restricted-library requests return the same non-enumerating `401` response. Canonical media
paths are database facts and symlinks or traversal paths are never followed.

Catalog browsing, detail, recent/random lists, combined and advanced search, suggestions, genres,
lyrics, favorites/ratings, scrobbling, listening history, and per-user statistics are available
under `/api/v1`. A user's library-group membership is applied consistently to browse, detail,
search, mutation, scrobble, and stream URL generation; inaccessible catalog entities are returned
as not found and are not included in counts or user-history results.

Native-v1 playlist create and update requests continue to accept the legacy `comment` request
field for client compatibility. The captured contract does not copy that field into the returned
playlist `description`; existing migrated descriptions remain readable.

Image URLs returned by native DTOs use the anonymous `/images/{id}/{size}` compatibility route so
clients can render art before adding authentication headers. Typed Artist, Album, Song, Playlist,
and User IDs are resolved through PostgreSQL facts. Missing stored art uses the captured default
image for an entity that exists — including an Artist with no artwork yet, and a Release withheld by
policy, which gets the default rather than its own cover — while unknown or malformed IDs preserve
the legacy empty `200` response.
Canonical Release artwork is opened only beneath the configured canonical storage root with
symlink/traversal protection. Numeric and named sizes are decoded/resized under fixed input-byte,
dimension, pixel, and concurrency limits; unsupported or oversized images fail closed. Responses
include an ETag and public cache headers. Derived sizes are cached in-process,
keyed by the immutable publication hash, so repeat views and ETag revalidations do
not decode the source image again; `original` is served from storage directly.

`GET /user/last-played` is the play-history feed: each item is
`{id, song, playedAt, duration}`, where `id` identifies the play event (a Track
played twice appears twice), `song` is the same Song DTO the other catalog routes
return — including a signed `streamUrl`, so an entry can be replayed directly — and
`duration` is the recorded play length in seconds. `meta.totalCount` counts the
same plays the feed returns. Chart lists and details additionally expose
`name`/`trackCount` alongside the canonical `title`/`itemCount`; the two are always
equal.

Artwork is chosen in a fixed order: an operator-uploaded override first, then the cover published
with the Release, then a provider (enrichment) cover, then the captured default image. Provider art
therefore never replaces a cover that was imported with the Release; it only fills the gap a Release
without imported art leaves. Sizes are `thumbnail` (80 px), `small` (160 px), `medium` (640 px),
`large` (1024 px), any numeric size up to 4096 px, and `original` (or `source`), which returns the
stored bytes untransformed. A Release withheld by policy (see
[Releases](../library-management/releases/)) serves no artwork through any client surface.

Theme discovery and theme assets are public. Setting a personal theme and administering custom
theme packs preserve the legacy browser-cookie authentication boundary; a bearer token alone does
not authorize those routes. Admin ZIP imports are bounded, validated entirely in memory, reject
links/traversal/duplicate paths, and commit metadata plus assets transactionally to PostgreSQL.

The authoritative field, nullability, status, and legacy-name contract remains the captured OpenAPI
and golden fixture corpus in the repository's `/compatibility` directory.
