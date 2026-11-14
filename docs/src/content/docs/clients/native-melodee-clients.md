---
title: Native Melodee Clients
description: Clients that use the native Melodee API.
sidebar:
  order: 1
---

Native Melodee clients speak the Melodee API directly and get the full feature set. Known native
clients include the Melodee Android app (including Android Auto) and MeloAmp.

:::note[Compatibility]
The Rust rewrite preserves the native Melodee API v1 contract so existing clients keep working
without a synchronized client release. Client-facing behavior is validated by differential
compatibility tests before each release.
:::

Configure a client with the server's public origin and authenticate with a native Melodee account.
The server returns absolute image and signed direct-play URLs, so deployments behind a reverse
proxy must set `MELODEE__NATIVE__PUBLIC_ORIGIN` to the externally reachable `http` or `https`
origin. If the proxy terminates client connections, also configure only its exact address or CIDR
in `MELODEE__SERVER__TRUSTED_PROXIES`; untrusted forwarding headers are ignored. The historical
`MELODEE__NATIVE__TRUSTED_PROXIES` name remains a fallback alias.

MeloAmp accepts either the server origin, such as `http://127.0.0.1:8080`, or the complete native
API base, such as `http://127.0.0.1:8080/api/v1`. Its browser renderer calls the server from a
different origin, so that renderer must be in `MELODEE__NATIVE__CORS__ALLOWED_ORIGINS`. The current
source UI uses `http://localhost:3000`; the packaged desktop application uses
`http://localhost:3001`. Both repository environment examples include those exact local origins.
Restart Melodee after changing the allowlist. A browser-level `Failed to fetch` during MeloAmp's
version check usually means the renderer origin is absent from the allowlist or the server cannot
be reached.

Native clients can browse and search Artists, Albums, and Songs; manage ratings and favorites;
use playlists; and submit now-playing/played scrobbles. Results and signed stream URLs are limited
to accepted Releases within the authenticated user's library-group access. Releases awaiting
review or carrying quarantine, ignore, or delete dispositions remain available to administrators
but are not returned to API clients.
