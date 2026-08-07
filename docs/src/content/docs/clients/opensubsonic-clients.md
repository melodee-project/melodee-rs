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

:::caution[Planned documentation]
The supported endpoint matrix, extension support, and intentionally absent endpoints will be
documented from the rewrite's OpenSubsonic adapter and its captured compatibility contracts — not
from assumptions. Unsupported endpoints intentionally return documented `410`/`501` behavior where
the contract requires it.
:::
