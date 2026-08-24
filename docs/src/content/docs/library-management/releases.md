---
title: Releases
description: Releases in the Melodee catalog and how they are managed.
sidebar:
  order: 4
---

A Release is the curated unit of the Melodee catalog — the primary thing processing, review, and
policy act on. Compatibility APIs may expose Releases as "albums"; the catalog itself is
release-centric.

## Browsing and detail

Open **Catalog → Releases** (`/admin/catalog/releases`) in the admin UI. The list is searched,
sorted, and paged server-side, with the view state in the URL. Selecting a Release opens the
**detail view** with tabs for overview, Tracks, Artists, contributors, files, classification,
provenance, and history — including the recorded policy decision and its evidence where one
exists. The Tracks tab lists each Track with its format facts and, when the Track has a
playable media file and your administrator account may stream it, an audio preview.
The detail view also offers an **Edit** panel with the editable metadata fields: title,
sort title, display artist, release date, and release date text. Saves are partial updates
(only changed fields are written), validated server-side (blank values and malformed dates are
rejected with `400`), recorded as an audit event in the entity's history, and applied with
last-write-wins semantics — each save bumps the row revision so the search projection
refreshes, and a concurrent edit by another administrator simply wins if it lands last; the
other editor sees the current values on refresh. Edits change catalog metadata only: they
never touch media files, canonical
identifiers, or identity relationships. The remaining curation actions are the audited
decisions in [Release Review](./release-review/) (including reprocess) and the
[Policies](../admin/policies/) rule set.

An administrator-confirmed deletion removes the Release from this listing and deletes its
canonical and retained staged media. Immutable identifiers and audit evidence remain as a
non-browsable tombstone; see [Release Review](./release-review/#deciding).

The same routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
