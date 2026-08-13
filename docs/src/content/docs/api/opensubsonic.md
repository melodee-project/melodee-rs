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
equivalent POST form fields). Per-user API tokens are managed in Melodee. Requests from the
server's own host and image URLs are exempt, matching the captured compatibility behavior.

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
