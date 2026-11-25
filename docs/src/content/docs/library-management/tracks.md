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
**detail view** with its Artist credits, contributors, and media files, and a link back to the
Release it belongs to. When the Track has a playable media file and your administrator account
may stream it, the detail view also includes an audio preview. The detail view offers an
**Edit** panel with the editable metadata fields: title, sort title, track number, and
duration. Saves are partial updates validated server-side (blank values, a non-positive track
number, or a negative duration are rejected with `400`), recorded as an audit event in the
entity's history, and applied with last-write-wins semantics — each save bumps the row revision
so the search projection refreshes, and a concurrent edit by another administrator simply wins
if it lands last; the other editor sees the current values on refresh. Edits change catalog
metadata only; they never rewrite media files or tags. Deeper curation decisions are made at
the Release level in [Release Review](./release-review/).

## Deleting Tracks

Delete a Track from its detail screen or select Tracks in **Catalog → Tracks** and choose
**Delete selected Tracks**. The confirmation dialog shows the exact impact and one
**Delete permanently** button; no reason, scope, or typed phrase is required.

Deletion is a hard delete of the Track, its credits, contributions, observations, novelty
evidence, playlist/playback rows, and its media file when no other Track references that file.
The Release and its other Tracks remain, and the Release aggregate is recomputed. Artwork belongs
to the Release and is not removed by a Track deletion. If canonical storage is not configured
while a file would be removed, the action fails closed and nothing is deleted.

An administrator or editor can delete any Track at any time. There is no undo: the surviving
record is the `track_hard_deleted` audit event.

The same routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
