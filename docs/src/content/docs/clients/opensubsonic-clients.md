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
- The shipping Feishin client completes the documented smoke workflows (connect, browse,
  playback, search, favorites) against this surface; acceptance evidence lives in
  `design/evidence/phase-8/` in the repository.
- Artist, album, and song cover art reflect administrator-managed catalog artwork overrides;
  a song uses its Release artwork. Without an override, clients receive the captured default
  artwork behavior.
