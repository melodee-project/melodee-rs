---
title: Last.fm scrobbling
description: Linking a Last.fm account and scrobbling plays from Melodee.
sidebar:
  order: 7
---

Melodee can submit now-playing and played scrobbles to [Last.fm](https://www.last.fm) for each
user who links their Last.fm account. Scrobbling to Last.fm is optional: the catalog, play counts,
and listening history work with no Last.fm configuration at all, and no external provider is ever
required for a Release to enter the catalog.

## Server configuration

The server needs a Last.fm API application (API key and shared secret from
[last.fm/api/accounts](https://www.last.fm/api/accounts)) and a server secret used to encrypt
stored session keys at rest. All four values are standard
[database-backed settings](../installation/configuration/) and can also be supplied through the
environment or config-file layers:

| Registry key | Environment variable | Default | Effect |
| --- | --- | --- | --- |
| `scrobbling.lastFm.Enabled` | `MELODEE__SCROBBLING__LASTFM__ENABLED` | `false` | Enables Last.fm scrobble submission. Users still scrobble only after linking their account. |
| `scrobbling.lastFm.apiKey` | `MELODEE__SCROBBLING__LASTFM__APIKEY` | empty | Last.fm API key. Secret: redacted from every API, export, and log. When blank, Last.fm linking and scrobbling are disabled. |
| `scrobbling.lastFm.sharedSecret` | `MELODEE__SCROBBLING__LASTFM__SHAREDSECRET` | empty | Last.fm shared secret used to sign API requests. Secret, same redaction rules. |
| `security.secretKey` | `MELODEE__SECURITY__SECRETKEY` | — | Server-held secret (at least 32 characters) from which the session-key encryption key is derived. Required to link accounts. Keep it stable: rotating it invalidates stored links and affected users must re-link. |

## Linking an account

Linking uses the Last.fm web authentication flow; Melodee never sees the user's Last.fm password:

1. The client requests `GET /api/v1/scrobble/lastfm/auth-url?callback=<absolute http(s) URL>` and
   receives a `https://www.last.fm/api/auth/` URL.
2. The user opens that URL and authorizes Melodee on last.fm; Last.fm redirects back to the
   callback with a token.
3. The client posts the token to `POST /api/v1/scrobble/lastfm/session`. On success the account is
   linked (`{ "message": "Last.fm linked" }`) and scrobbles begin.

The returned session key is stored encrypted (AEAD-sealed under a key derived from
`security.secretKey`); the raw key is never logged and never returned by any API. Only a SHA-256
digest of the key is kept for audit purposes.

`POST /api/v1/scrobble/lastfm/disconnect` removes the link. It is idempotent: it succeeds whether
or not an account is linked or Last.fm is configured.

## Behavior and failure semantics

- Scrobbles are stamped when Melodee processes them, not with a client-supplied play time.
- Scrobbles are sent one Track per request, once, with a 10-second timeout. There is no retry and
  no offline queue: a failed submission is dropped and logged, never blocking or failing playback,
  and the internal play count is unaffected.
- If Last.fm reports that the link is no longer valid (authentication errors 4, 9, or 10), Melodee
  clears the stored link; the user re-links to resume scrobbling. There is no token refresh flow.
- When Last.fm is not configured or the user has not linked, scrobble submission is a silent
  no-op.
