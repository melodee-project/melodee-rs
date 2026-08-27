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

The catalog importer preserves each raw Release/Track Artist observation. A Track's single Artist
observation resolves to a Release Artist identity when exactly one primary Artist observation on
that same Release has the same normalized value. Other differently named Artists billed on the
Release do not make that exact match ambiguous. If no Release Artist or multiple Release Artist
observations match, the Track keeps a separate provisional identity for review. Same-name Artists
from separate Releases remain separate unless stronger evidence or an administrator decision
resolves them. Strong retained provider evidence may schedule an auditable reconciliation, but it
never bypasses the accepted merge policy. Melodee never merges Artists from normalized-name
equality alone.

## Structured Track billing

Inspection treats the **Album Artist** tag as Release billing. A differing **Artist** tag belongs to
the individual Track, so values such as `6arelyhuman/Baby Jane` no longer create a compound Release
Artist. When every Album Artist tag is absent, Track Artist values remain the fallback Release
observation and may require Identity Review.

For Track billing, Melodee preserves the exact tag as `DisplayArtist` and separately records ordered
Artists. `Featuring`, `feat.`, `ft.`, and `with` phrases mark later names as featured Artists. A slash
value is split only when one component uniquely matches a primary Artist on the same Release; this
turns `9000 Rondae/Glockboyz Teejaee/OnFully/Roadrunner Glockboyz Tez` into one primary and three
featured Track Artists without globally splitting literal names such as `$NOT/Jeht` or `AC/DC`.
Every component still passes through identity resolution—parsing text never merges Artists by name.

Already imported compound identities are not rewritten automatically because a separator may be
part of a real name and the identity may have administrator or provider evidence. For disposable
source-development catalogs, run the
[guided fresh development setup](../installation/build-from-source/#guided-fresh-development-setup)
and re-import the media. Retained catalogs should use Identity Review rather than deleting or
editing identity rows directly.

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
