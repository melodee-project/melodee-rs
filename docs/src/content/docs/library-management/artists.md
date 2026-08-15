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

The current catalog importer preserves each raw Release/Track Artist observation and creates
conservative provisional local identities. It never merges Artists from normalized-name equality
alone and does not call an external identity provider.

See [Identity Review](../admin/identity-review/) for the administrative workflow that resolves
uncertain artist matches.

## Browsing and detail

Open **Catalog → Artists** (`/admin/catalog/artists`) in the admin UI. The list is searched,
sorted, and paged server-side — the search box binds to the backend query filter, sorting is
restricted to the backend whitelist, and the view state lives in the URL so it can be shared.
Selecting an Artist opens the **detail view** with tabs for overview, Releases, appearances,
contributions, relationships, identity, images, and history (the audit timeline). Catalog detail in this build is **read-only**: curation happens through
the decision surfaces — [Identity Review](../admin/identity-review/) for confirm/create-new/
never-match and merge/split — not through metadata edit forms, which do not exist yet.

The same browsing and detail routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
