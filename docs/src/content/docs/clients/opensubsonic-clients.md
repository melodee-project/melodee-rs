---
title: OpenSubsonic Clients
description: Connecting OpenSubsonic-compatible clients to Melodee.
sidebar:
  order: 2
---

Melodee implements an OpenSubsonic-compatible API, so a wide range of third-party music clients
can connect. OpenSubsonic responses keep their own terminology (for example `album` and `song`)
and envelope formats (JSON, XML, JSONP).

Connection generally means pointing the client at your Melodee server URL with your Melodee
credentials.

## Credentials

- Clients that send the password directly (the `p` authentication parameter) use your Melodee
  username and login password.
- Clients that use token+salt authentication (most modern clients) authenticate with your
  per-user **OpenSubsonic credential** — a generated secret distinct from your login password,
  matching the legacy server's behavior. Where the client asks for a "password", enter that
  credential.

## Compatibility notes

- The endpoint matrix, extension support, and intentionally absent endpoints follow the captured
  compatibility contracts. Unsupported endpoints intentionally return documented `410`/`501`
  behavior where the contract requires it.
- Streaming honors the standard transcoding parameters: `format=mp3|aac|opus` transcodes the
  track with the administrator's configured FFmpeg templates, and a `maxBitRate` below the
  source bitrate transcodes to MP3 at the requested cap. `format=raw` (or no parameters) keeps
  direct play of the original file. Unsupported formats, malformed templates, and a saturated or
  disabled transcoder return the standard error envelope with code `0`. See
  [Configuration — Transcoding](../installation/configuration/#transcoding).
- The shipping Feishin client completes the documented smoke workflows (connect, browse,
  playback, search, favorites) against this surface.
- Artist, album, and song cover art reflect administrator-managed catalog artwork overrides;
  a song uses its Release artwork. Without an override, clients receive the captured default
  artwork behavior.
