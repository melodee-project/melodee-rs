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

Across Releases, exact name equality by itself never merges Artists automatically. When that exact
name recurs specifically as the primary Artist credit on different non-deleted Releases, however,
Melodee converges the provisional rows if the complete same-name group has no differentiating
evidence. This repairs ordinary import duplicates such as one `Alice Cooper` row per Release,
including rows left by older binaries; the bounded repair runs after imports and when the catalog
scheduler starts. Every consolidation is audited and reversible, and the surviving Artist remains
provisional.

External IDs, disambiguation, Artist type, active dates, memberships, relationships, manual or
locked decisions, **Never Match** assertions, differing aliases, and ambiguous same-Release billing
block that contextual rule for the complete name group. Blocked and name-only candidates remain
separate in Identity Review. Candidate cards include Artist disambiguation text, so legitimate
same-name identities such as `Nirvana — UK psychedelic group` and `Nirvana — US grunge band` can be
reviewed without changing their canonical names. When source tags provide neither disambiguation
nor other identity evidence, two real homonyms are unknowable; an administrator can split an
incorrect provisional consolidation without losing raw observations or public-ID redirects.

The stronger cross-Release path converges Artists when both carry the same trusted, well-formed
provider identifier. Known conflicts—different IDs from the same provider,
incompatible Artist types, disjoint active dates, disjoint known group membership, locked decisions,
or do-not-match assertions—block that merge. Successful strong-evidence convergence retains raw
observations, writes a reversible merge tombstone and automated audit history, and leaves the
surviving identity provisional. Strong-ID reconciliation runs before the contextual primary-credit
rule so provider evidence wins whenever it is available.

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
part of a real name and the identity may have administrator or provider evidence. The Identity tab
of such an Artist says so explicitly — a name containing `&`, `,`, `;`, `feat.`, `ft.`, or ` x `
shows a note that it is one provisional identity and links straight to Identity Review. For disposable
source-development catalogs, run the
[guided fresh development setup](../installation/build-from-source/#guided-fresh-development-setup)
and re-import the media. Retained catalogs should use Identity Review rather than deleting or
editing identity rows directly.

See [Identity Review](../admin/identity-review/) for the administrative workflow that resolves
uncertain artist matches.

## Provisional versus Verified identity

Every Artist created by import is **Provisional**. That is deliberate: a tag value is an observation,
not an identity, so the importer records the name and refuses to claim authority it does not have.
`Verified` means a human (or an authoritative provider) has asserted that this is a real, confirmed
identity.

Until this change nothing could promote an Artist — import only ever writes `provisional` — so every
Artist in a freshly imported catalog read Provisional with no way out. Now an administrator can
assert it explicitly: **Catalog → Artists → (open an Artist) → Edit → Identity status**, choosing
`Provisional` or `Verified`. The change bumps the row revision, records an `artist_metadata_updated`
audit event carrying the field delta, and refreshes the search projection. `merged` and `deprecated`
are system states owned by the merge and delete flows; a metadata edit rejects them.

Provider-confirmed verification (promoting on a verified MusicBrainz/Spotify identity) is not
implemented yet, so verification is an operator decision. Unconfigured providers surface as
`provider enrichment attempt failed` warnings and never change status.

## Merging duplicate Artists

From **Catalog → Artists**, select between two and 101 Artists on the current page and choose
**Merge selected Artists**. Pick the identity that should survive, enter an audit reason, review
the server-calculated impact, and type `MERGE` to confirm. The apply step is one transaction: if
any selected source cannot be merged, none of the selected Artists are changed.

The selected target keeps its canonical name, sort name, Artist type, disambiguation, active dates,
and existing operator-selected image. Melodee coalesces Release and Track billing, contributions,
aliases, external identities, relationships, memberships, raw observations, discovery links,
compatible provider records, and per-user Artist state. Conflicting per-user flags are preserved,
ratings use the higher value, and play counts are summed. If the target has no operator image, one
source image is adopted deterministically; other source overrides remain recoverable on the merge
tombstones. The preview identifies counts and target-wins metadata conflicts before confirmation.

Each merged source remains a tombstone whose public ID redirects to the surviving Artist, and its
name becomes a search alias. Audit and provider histories stay attributable to their original
identity. Merging changes catalog relationships and rebuildable projections only: it never moves or
deletes canonical media files.

The single-source UUID form under **Identity Review → Artist operations** remains available for
advanced correction work. Splitting an incorrect consolidation is also performed there by moving
explicitly selected assignments to a new Artist.

## Browsing and detail

Open **Catalog → Artists** (`/admin/catalog/artists`) in the admin UI. The list is searched,
sorted, and paged server-side — the search box binds to the backend query filter, sorting is
restricted to the backend whitelist, and the view state lives in the URL so it can be shared.
Selecting an Artist opens the **detail view** with tabs for overview, **Tracks**, Releases,
appearances, contributions, relationships, identity, images, and history (the audit timeline).

The summary numbers are **links into the rows they count**, so a statistic is never a dead end: the
**Track credits** card (every Track the Artist is credited on, whatever the role) and
**Appearances** (the performance subset) both open the **Tracks** tab, which lists the Track, the
Release, the billing role, and whether the credit is a performance credit or a written/production
credit. Those rows come from the same association records the counts aggregate over, so the number
and the list can never disagree. The detail
view also offers an **Edit** panel with the editable metadata fields: name, sort name, artist type,
and identity status. **Artist type is a closed dictionary** (`person`, `group`, `orchestra`,
`ensemble`, `choir`, `character`, `other`, `unknown`) rendered as a select: the store rejects any
other value with `400`, so free text would only produce a failed save. Saves are partial updates
validated server-side (blank names and unknown artist types are rejected with `400`), recorded as an
audit event in the entity's history, and applied
with last-write-wins semantics — each save bumps the row revision, and a concurrent edit by
another administrator simply wins if it lands last; the other editor sees the current values on
refresh. Edits change display metadata only: they never alter Artist identity. Multi-Artist merge
is available from the Artist list; merge/split and observation decisions remain available in
[Identity Review](../admin/identity-review/).

The same browsing and detail routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
