---
title: Artists
description: Artist identity, aliases, and merging in the Melodee catalog.
sidebar:
  order: 3
---

An Artist in Melodee is an owned identity, not a name string:

- Same-name bands remain distinct Artists.
- Name variations resolve to one Artist through evidence and aliases.
- Artist merges and splits never move media files; they change catalog relationships only.

The catalog importer preserves each raw Release/Track Artist observation. When a Release has one
Artist and a Track's single Artist observation exactly matches it, both observations resolve to the
same provisional Artist identity within that Release. Different names, multi-Artist Release
contexts, and same-name Artists from separate Releases remain separate unless stronger evidence or
an administrator decision resolves them. Strong retained provider evidence may schedule an
auditable reconciliation, but it never bypasses the accepted merge policy. Melodee never merges
Artists from normalized-name equality alone.

See [Identity Review](../admin/identity-review/) for the administrative workflow that resolves
uncertain artist matches.

## Browsing and detail

Open **Catalog → Artists** (`/admin/catalog/artists`) in the admin UI. The list is searched,
sorted, and paged server-side — the search box binds to the backend query filter, sorting is
restricted to the backend whitelist, and the view state lives in the URL so it can be shared.
Selecting an Artist opens the **detail view** with tabs for overview, Releases, appearances,
contributions, relationships, identity, images, and history (the audit timeline). The detail
view also offers an **Edit** panel with the editable metadata fields: name, sort name, and
artist type. Saves are partial updates validated server-side (blank names and unknown artist
types are rejected with `400`), recorded as an audit event in the entity's history, and applied
with last-write-wins semantics — each save bumps the row revision, and a concurrent edit by
another administrator simply wins if it lands last; the other editor sees the current values on
refresh. Edits change display metadata only: they never alter
Artist identity, and merges and splits remain decision actions in
[Identity Review](../admin/identity-review/) (confirm/create-new/never-match and merge/split).

The same browsing and detail routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
