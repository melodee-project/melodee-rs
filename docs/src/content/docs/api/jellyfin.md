---
title: Jellyfin Compatibility
description: Melodee's Jellyfin-compatible API surface.
sidebar:
  order: 3
---

Melodee serves a music-focused Jellyfin-compatible API. Music clients that speak the Jellyfin
protocol (Finamp, Feishin, Streamyfin, and similar) can discover the server, sign in, browse the
catalog, manage playlists, and stream audio.

The surface is a compatibility subset, not a general Jellyfin server. Non-music Jellyfin features
(video, TV, books, Live TV, plugins, server administration) are intentionally absent and answer
plain `404` responses.

## Discovery and routing

Jellyfin endpoints are served both at the server root (for example `/System/Info/Public`) and
under the canonical `/api/jf` prefix (for example `/api/jf/System/Info/Public`); both forms are
equivalent.

| Endpoint | Behavior |
| --- | --- |
| `GET /System/Info/Public` | Server name, version, and id; no authentication required. |
| `GET /System/Info` | Full system information envelope. |
| `GET`/`POST /System/Ping` | `204 No Content`; used by clients as a liveness probe. |
| `GET /` | `200` with an empty body when the request carries a Jellyfin marker (a `MediaBrowser` authorization header, a Jellyfin token header, or an `api_key` query parameter). |

## Authentication

`POST /Users/AuthenticateByName` accepts `{"Username": "...", "Pw": "..."}` and returns the user
object, session information, an access token, and the server id. The request field is `Pw`; a
body that only sets `Password` is rejected.

Clients can then present the access token through any of these conventions:

- `Authorization: MediaBrowser Token="..."` (with the usual `Client`/`Device`/`DeviceId`/`Version`
  attributes),
- `X-Emby-Authorization` (same scheme),
- `X-MediaBrowser-Token` or `X-Emby-Token` headers, or
- the `api_key` query parameter.

Tokens expire after 168 hours; each user keeps at most 10 active tokens (the oldest is revoked
when the limit is exceeded). `POST /Sessions/Logout` revokes the presented token immediately and
always answers `204`.

`GET /Users/Me` returns the authenticated user; `GET /Users/{id}` returns a user to itself or to
an administrator; `GET /Users` lists users (administrators see all, others see only themselves);
`GET /Users/Public` is an empty list — sign-in always goes through username/password.

## Browsing

- `GET /UserViews` (and the `/Users/{id}/Views` alias) lists the server's music libraries.
- `GET /Items` supports `includeItemTypes` (`MusicArtist`, `MusicAlbum`, `Audio`), `parentId`
  (artist → its releases, release → its tracks, library → all tracks), `searchTerm` (normalized,
  diacritic-insensitive contains match), and `startIndex`/`limit` paging.
- `GET /Items/{id}` returns artist, release, or track detail by id.
- `GET /Items/Filters` returns the distinct genres and release years.
- `GET /Artists`, `GET /Artists/AlbumArtists`, `GET /Artists/{id}`, and
  `GET /Artists/{id}/Similar` cover artist browsing.
- `GET /Genres` and `GET /MusicGenres` list genres.
- `GET /Items/{id}/Similar`, `GET /Items/{id}/InstantMix`, and
  `GET /Songs/{id}/InstantMix` return related tracks in a deterministic order.
- `POST /Items/{id}/Refresh` is accepted as a no-op (`204`).
- `GET /Users/{id}/Items` and `GET /Users/{id}/Items/{itemId}` redirect (`307`) to the
  corresponding `/Items` routes.

List endpoints support ETag-based caching (`If-None-Match` → `304`).

## User state

- `POST`/`DELETE /Users/{userId}/FavoriteItems/{itemId}` mark and unmark favorites (tracks,
  releases, artists).
- `POST`/`DELETE /Users/{userId}/PlayedItems/{itemId}` mark tracks and releases played (counting
  plays) or unplayed (resetting the counter).

Both require the path user id to match the authenticated user.

## Playlists

`GET`/`POST /Playlists`, `GET`/`POST`/`DELETE /Playlists/{id}`,
`GET`/`POST`/`DELETE /Playlists/{id}/Items`, and
`POST /Playlists/{id}/Items/{itemId}/Move/{index}` cover listing, creation, update, deletion, and
item management. `DELETE /Items/{id}` also deletes an owned playlist. Playlist item positions in
responses are 1-based; per-user playback/favorite state on playlist entries is included only when
requested with `enableUserData=true`.

## Audio

- `GET /Audio/{id}/stream`, `GET /Audio/{id}/stream.{container}`, and `GET /Audio/{id}/universal`
  stream the original audio bytes (direct play). Transcoding is not supported: requests that ask
  for a container or bitrate conversion without `static=true` are rejected with `400`.
- HTTP range requests are honored (`206` with `Content-Range`); unsatisfiable ranges answer `416`.
- `GET /Audio/{id}/lyrics` returns timed lyrics parsed from a sibling `.lrc` file, or an empty
  list when none exists.
- `GET /Items/{id}/Download` and `GET /Items/{id}/File` download the original file (requires the
  download permission).
- `GET`/`POST /Items/{id}/PlaybackInfo` returns the media-source description for a track.

Streaming requires the stream permission; downloads require the download permission.

## Images

`GET /Items/{id}/Images/{type}`, `GET /Items/{id}/Images/{type}/{index}`, and
`GET /Artists/{id}/Images/{type}` resolve artist and release artwork. Artwork storage is not yet
populated by the catalog pipeline, so these routes currently answer `404` for catalog entities;
they begin serving images automatically once durable artwork storage lands.

## Sessions

`GET /Sessions` lists active playback sessions. Clients report playback with
`POST /Sessions/Playing`, `POST /Sessions/Playing/Progress`, and
`POST /Sessions/Playing/Stopped` (which records the play; `"Failed": true` skips recording).
`POST /Sessions/Playing/Ping`, `POST /Sessions/Capabilities`, and
`POST /Sessions/Capabilities/Full` are acknowledged with `204`.

## Client guides

See [Jellyfin-compatible clients](../clients/jellyfin-clients.md) for connection walkthroughs.
