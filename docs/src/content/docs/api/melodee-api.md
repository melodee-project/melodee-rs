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

Image URLs returned by native DTOs use the anonymous `/images/{id}/{size}` compatibility route so
clients can render art before adding authentication headers. Typed Artist, Album, Song, Playlist,
and User IDs are resolved through PostgreSQL facts. Missing stored art uses the captured default
image for an existing entity; unknown or malformed IDs preserve the legacy empty `200` response.
Canonical Release artwork is opened only beneath the configured canonical storage root with
symlink/traversal protection. Numeric and named sizes are decoded/resized under fixed input-byte,
dimension, pixel, and concurrency limits; unsupported or oversized images fail closed. Responses
include an ETag and public cache headers.

Theme discovery and theme assets are public. Setting a personal theme and administering custom
theme packs preserve the legacy browser-cookie authentication boundary; a bearer token alone does
not authorize those routes. Admin ZIP imports are bounded, validated entirely in memory, reject
links/traversal/duplicate paths, and commit metadata plus assets transactionally to PostgreSQL.

The authoritative field, nullability, status, and legacy-name contract remains the captured OpenAPI
and golden fixture corpus in the repository's `/compatibility` directory.
