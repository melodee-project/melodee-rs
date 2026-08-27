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
observations match, the Track keeps a separate provisional identity for review.

Across Releases, exact name equality never merges Artists automatically. The catalog keeps each
provisional identity and records the other same-name identities as non-authoritative proposals in
Identity Review. A later administrator confirmation or merge can resolve a true duplicate, while a
**Never Match** decision durably suppresses an incorrect candidate. Candidate cards include Artist
disambiguation text, so legitimate same-name identities such as `Nirvana — UK psychedelic group`
and `Nirvana — US grunge band` can be reviewed without changing their canonical names.

The catalog automatically converges cross-Release Artists only when both carry the same trusted,
well-formed provider identifier. Known conflicts—different IDs from the same provider,
incompatible Artist types, disjoint active dates, disjoint known group membership, locked decisions,
or do-not-match assertions—block that merge. Successful strong-evidence convergence retains raw
observations, writes a reversible merge tombstone and automated audit history, and leaves the
surviving identity provisional. The bounded strong-ID and proposal checks run after imports and
once at server startup.

Repeated structured Track credits inside one Release use a separate, narrower rule. If the same
exact normalized credit received several provisional track-only identities, Melodee converges them
to one active Artist only when every occurrence belongs to that Release and no Release Artist,
verified identity, locked decision, do-not-match assertion, or conflicting provider ID makes the
group ambiguous. The Release must also have exactly one primary Release Artist and no `Various
Artists` billing or durable compilation evidence. Primary and featured roles do not create separate
identities. This reconciliation also runs after imports and at startup, so repeated credits such as
`Glockboyz Teejaee` across several Tracks repair without deleting observations or resetting the
catalog; ambiguous and compilation contexts remain in Identity Review.

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
