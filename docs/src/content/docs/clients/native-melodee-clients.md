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
in `MELODEE__NATIVE__TRUSTED_PROXIES`; untrusted forwarding headers are ignored.

Native clients can browse and search Artists, Albums, and Songs; manage ratings and favorites;
use playlists; and submit now-playing/played scrobbles. Results and signed stream URLs are limited
to the authenticated user's library-group access.
