---
title: OpenSubsonic API
description: Melodee's OpenSubsonic-compatible API surface.
sidebar:
  order: 2
---

Melodee's OpenSubsonic adapter implements the OpenSubsonic API for third-party clients, with
JSON, XML, and JSONP response envelopes. Every endpoint's behavior is verified against captured
compatibility evidence by differential tests — never assumed.

## Authentication

Requests authenticate with the OpenSubsonic token scheme (`u`, `t`, `s` query parameters or
equivalent POST form fields), or username/password (`u`, `p`). Per-user API tokens are managed in
Melodee. Protected routes require credentials even on loopback and when the Host matches the
server. Public ping/extension discovery and separately served native artwork keep their existing
access rules.

For upgrades from the pre-release localhost/baseUrl bypass, configure credentials in clients before
upgrading. An isolated transition environment can explicitly set
`MELODEE__OPENSUBSONIC__LEGACY_AUTH_BYPASS=true` to restore that legacy behavior. This grants
anonymous catalog access to matching-host/loopback requests and must stay disabled for public
reverse-proxy deployments. Configure the shared trusted-proxy setting separately; see
[Configuration](../installation/configuration/#native-api-urls-streaming-and-proxies).

## Browser clients and CORS

For a browser client hosted on another origin, allow that application's origin in the server
configuration. `make up` includes hosted Feishin through the
`x-melodee-server-environment` defaults in `compose.dev.yaml`. To configure the setting
explicitly, add this to the repository-root `.env`:

```dotenv
MELODEE__OPENSUBSONIC__CORS__ALLOWED_ORIGINS=https://feishin.vercel.app
```

Restart with `make down` followed by `make up`. For the packaged Compose deployment, set
`MELODEE_OPENSUBSONIC_CORS_ALLOWED_ORIGINS` in its environment file and recreate the Melodee
service. Use comma-separated exact origins without trailing slashes or URL paths. Only add
browser applications you trust to read your OpenSubsonic data. The server's default list is
empty; the development launcher supplies Feishin. Root `.env` overrides the development
default, and exported variables take precedence over `.env`. Set the value to empty to
disable browser access.

This setting covers OpenSubsonic API, artwork, and streaming requests. It supports preflight
requests and preserves the existing token authentication scheme; cookie credentials are not
enabled. The native API's `MELODEE__NATIVE__CORS__ALLOWED_ORIGINS` setting is separate and does
not apply to `/rest` routes. Non-browser clients do not require a CORS origin setting.

If the browser reports HTTP `200` together with “No Access-Control-Allow-Origin header,” the
server answered but the browser could not expose that response to the client. Check that the
client's exact origin is in this list and restart the server after changing it. See
[Configuration](../installation/configuration/) for the complete setting reference.

## Supported endpoint groups

- **System:** `ping`, `getLicense`, `getMusicFolders`, `getOpenSubsonicExtensions`.
- **Browsing:** artists, indexes, music directories, artist/album/song detail, album and artist
  info, top songs, similar artists and songs.
- **Lists:** album lists (`getAlbumList`/`getAlbumList2`), starred items (`getStarred`/
  `getStarred2`), now playing, genres, songs by genre, and search (`search2`/`search3`).
- **Annotation:** star/unstar, ratings, and scrobbling.
- **Playlists, bookmarks, and play queue:** creation, update, deletion, and shared playback
  queue state. Playlist updates apply cleanly; entries are returned in catalog order.
- **Media:** streaming and download with byte-range support, cover art, avatars, and lyrics.
- **Sharing and internet radio:** share CRUD and radio station CRUD for authorized users.
- **User management:** user lookup and creation for admin users.

Podcast endpoints answer the captured "disabled" behavior unless the podcast feature is
enabled. Jukebox and other backend-dependent endpoints answer the documented `410`/`501`
classes; unsupported non-music behavior is intentionally absent.

## Streaming

Streams serve the original audio bytes with single-pass byte ranges (`206` responses carry a
correct `Content-Range` total). Invalid or unsatisfiable ranges and unsupported time offsets
answer `404` with an OpenSubsonic error envelope. Transcoding parameters are accepted for
compatibility but silently ignored.

## Cover art

`getCoverArt` serves an administrator's catalog artwork override for `artist_`, `album_`, and
`song_` identifiers when one exists; a `song_` identifier uses its Release override. Without an
override, catalog identifiers retain the captured default-artwork behavior. Playlist artwork is
separate user-state data. Stored artwork responses carry an ETag and
`Cache-Control: public, max-age=86400`.

For stored artwork, the optional OpenSubsonic `size` parameter accepts `small`, `medium`, `large`,
and `extralarge` (100, 200, 500, and 1000 pixels respectively). Resizing preserves aspect ratio
and never upscales; each generated variant has its own ETag. An omitted size preserves the stored
bytes. Corrupt or unsupported stored images fall back to the normal artwork response rather than
failing the request, and administrator overrides retain precedence over stored artwork.
