---
title: Melodee API
description: Overview of the native Melodee API for client developers.
sidebar:
  order: 1
---

The native Melodee API is the full-featured API used by native Melodee clients. It is a
**compatibility contract**: routes, fields (including legacy names such as `album`/`song` where
the contract uses them), error shapes, authentication, and pagination are preserved deliberately
and are never renamed silently.

This section will document:

- **Authentication** — login, refresh, and logout flows.
- **Catalog endpoints** — artists, releases/albums, tracks/songs, search, images, streaming.
- **Feature endpoints** — playlists, shares, requests, podcasts, scrobbling, and more.
- **Errors and pagination** — exact envelope and nullability behavior.

:::caution[Planned documentation]
The authoritative source for the API contract is the captured OpenAPI documents and golden
fixtures in the repository's `/compatibility` directory. Prose here will summarize concepts and
link to reference material rather than duplicating full schemas.
:::
