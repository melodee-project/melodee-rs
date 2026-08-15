---
title: Jellyfin-Compatible Clients
description: Connecting Jellyfin-compatible clients to Melodee.
sidebar:
  order: 3
---

Melodee serves a [Jellyfin-compatible API](../api/jellyfin.md) covering music workflows:
discovery, login, library views, artists, genres, playlists, images, and audio playback.
Non-music Jellyfin behavior is intentionally out of scope.

## Connecting a client

1. Enter your Melodee server URL (for example `https://music.example.com`) as the Jellyfin/Emby
   server address. Clients can probe the server with or without the `/api/jf` prefix — both forms
   work.
2. Sign in with your Melodee username and password. Melodee does not list public users on the
   login screen, so quick-select/pin login is unavailable; type the username.
3. The server's music libraries appear as views. Browsing, search, favorites, playlists, and
   direct-play streaming work as usual.

## Verified behaviors

- Direct-play streaming of the original audio file, including HTTP range requests (seeking).
  Transcoding is not available; clients configured to force transcoding should be switched to
  direct play/original quality.
- Timed lyrics from `.lrc` sidecar files.
- Playlist creation, editing, reordering, and deletion from the client.
- Favorite and played-state toggles.
- Playback reporting drives Melodee's now-playing and play-history state.

## Known limitations

- No transcoding (direct play only).
- Artist and Release images use an administrator's catalog artwork override when present, then
  published canonical artwork. Clients show their own placeholder art only when neither source
  exists.
- Non-music libraries and features (video, TV, books, Live TV, plugins) are not present.

:::note[Verification status]
The wire contract is covered by frozen compatibility captures and integration tests, and the
shipping Feishin client completes the documented smoke workflows (sign-in, browse, playback,
search, favorites) against the Jellyfin surface. Acceptance evidence:
`design/evidence/phase-8/REAL-CLIENT-ACCEPTANCE.md` in the repository. Finamp and Streamyfin
mobile smoke runs are planned hardening on top of the same exercised contract.
:::
