---
title: MeloAmp and the Melodee server
description: Which product owns listener workflows and which product owns administration.
sidebar:
  order: 2
---

MeloAmp is the listener application for Melodee. It owns listener browsing, search, playback,
playlists, ratings, recommendations, and related user workflows through the native Melodee API.
The Rust server remains the durable API, ingestion, catalog, storage, streaming, and operational
runtime behind that client.

The server's browser application is the Carbon Admin UI at `/admin`. Use it for catalog review,
identity decisions, artwork overrides, processing, settings, users, and diagnostics. The server
does not ship a second full listener-facing SPA at `/`; this keeps listener behavior in one
maintained client and avoids duplicate application state and compatibility surfaces.

Narrow browser-only pages may still exist when a public link or security flow genuinely requires
HTML, such as a public-share landing or password-reset completion. Those pages are not a listener
application.

See [Native Melodee clients](./native-melodee-clients/) for connection and CORS configuration and
[System and diagnostics](../admin/system-diagnostics/) for server operations.
