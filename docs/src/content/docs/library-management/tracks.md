---
title: Tracks
description: Tracks in the Melodee catalog.
sidebar:
  order: 5
---

A Track is one playable occurrence within a Release, with explicit Artist credits and contributions.
Two Tracks may refer to the same underlying recording or performance through `AudioIdentity`.
Compatibility APIs may expose Tracks as "songs".

## Browsing and detail

Open **Catalog → Tracks** (`/admin/catalog/tracks`) in the admin UI. The list is searched,
sorted, and paged server-side, with the view state in the URL. Selecting a Track opens the
**read-only detail view** with its Artist credits, contributors, and media files, and a link
back to the Release it belongs to. When the Track has a playable media file and your
administrator account may stream it, the detail view also includes an audio preview.
Track-level metadata edit forms do not exist in this build; curation decisions are made at the
Release level in [Release Review](./release-review/).

The same routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
