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
observations match, the Track keeps a separate Active identity for review.

Across Releases, an exact normalized name is a **reuse hint**, not an identity key. When a credit
names an Artist whose normalized value is exactly equal to exactly one existing Artist that has no
disambiguation hint and no conflicting trusted provider identifier, the importer reuses that Artist
instead of creating a second row — the same lookup the previous Melodee server performed, which is
why one artist appearing on many Releases is one Artist. Reuse never matches fuzzily (`The Coral` is
not `Coral`), never matches through aliases, and never overrides a provider conflict or a
disambiguation hint, so genuinely different same-name artists still get their own rows.

Catalogues written before that rule can still contain one row per Release. Melodee converges those
duplicates for both primary Release credits and Track credits when the complete same-name group has
no differentiating evidence. This repairs ordinary import duplicates such as one `Alice Cooper` row
per Release, including rows left by older binaries; the bounded repair runs after imports and when
the catalog scheduler starts. Every consolidation is audited and reversible, and the surviving
Artist remains Active.

External IDs, disambiguation, Artist type, active dates, memberships, relationships, manual or
locked decisions, **Never Match** assertions, differing aliases, and ambiguous same-Release billing
block that contextual rule for the complete name group. Blocked and name-only candidates remain
separate in Identity Review. Candidate cards include Artist disambiguation text, so legitimate
same-name identities such as `Nirvana — UK psychedelic group` and `Nirvana — US grunge band` can be
reviewed without changing their canonical names. When source tags provide neither disambiguation
nor other identity evidence, two real homonyms are unknowable — the import may reuse one row for
them, exactly as the previous server did; an administrator can split an incorrect consolidation
without losing raw observations or public-ID redirects.

The stronger cross-Release path converges Artists when both carry the same trusted, well-formed
provider identifier. Known conflicts—different IDs from the same provider,
incompatible Artist types, disjoint active dates, disjoint known group membership, locked decisions,
or do-not-match assertions—block that merge. Successful strong-evidence convergence retains raw
observations, writes a reversible merge tombstone and automated audit history, and leaves the
surviving identity Active. Strong-ID reconciliation runs before the contextual primary-credit
rule so provider evidence wins whenever it is available.

Repeated structured Track credits inside one Release use a separate, narrower rule. If the same
exact normalized credit received several separate track-only identities, Melodee converges them
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
shows a note that it is one Active identity and links straight to Identity Review. For disposable
source-development catalogs, run the
[guided fresh development setup](../installation/build-from-source/#guided-fresh-development-setup)
and re-import the media. Retained catalogs should use Identity Review rather than deleting or
editing identity rows directly.

See [Identity Review](../admin/identity-review/) for the administrative workflow that resolves
uncertain artist matches.

## Active versus Verified identity

Every Artist created by import is **Active**. That is deliberate: a tag value is an observation, not
an identity, so the importer records the name and its credits and refuses to claim authority it does
not have. An Active Artist is fully usable — it appears in browse, search, and clients, and it is
never treated as unfinished work.

`Verified` means a person asserted that this is a confirmed identity. It is a quality signal on top of
a working Artist, not a gate: nothing in the catalogue is hidden or disabled while an Artist is
Active.

An administrator can set the identity status explicitly: **Catalog → Artists → (open an Artist) →
Edit → Identity status**, choosing `Active` or `Verified`. The change bumps the row revision, records
an `artist_metadata_updated` audit event carrying the field delta, and refreshes the search
projection. `merged` and `deprecated` are system states owned by the merge and delete flows; a
metadata edit rejects them.

ADR-0190 renamed this state from `provisional` to `active`. The state did not change, and existing rows
were relabelled in place by migration `0044`; public IDs, merge tombstones, decisions, and evidence are
untouched.

Provider-confirmed verification (promoting on a verified MusicBrainz/Spotify identity) is not
implemented yet, so verification is an operator decision. Unconfigured providers surface as
`provider enrichment attempt failed` warnings and never change status.

## Provider matching (MusicBrainz, Deezer, iTunes, Discogs, Last.fm, Spotify)

The enrichment worker searches the configured metadata providers for each Artist and Release. Two
kinds of result are stored:

- **Identity candidates** — provider, external ID, the query that produced it, its rank and a
  confidence score. These are *evidence*: they never create an external identity and never authorize
  a merge on their own.
- **Provider artwork** — cover/portrait images, used by client apps as artwork fallback when no
  operator image exists.

The **Identity** tab shows exactly one **provider match** per provider: the highest-ranked candidate
wins, and its rank, confidence, and observation time are shown. The other ranked search results
remain stored as evidence. An **Active** Artist — the imported, unverified state that older builds
called Provisional — can expand the remaining ranked candidates for review. A **Verified** Artist,
where an operator has asserted the identity, shows only the winner. This keeps a provider search
result set from reading as several competing matches while retaining the full evidence for review.
Provider credentials are per-provider settings (`searchEngine.<provider>.enabled`, `…baseUrl`, and a
token where one is required — Discogs `userToken`, Spotify `accessToken`); MusicBrainz, Deezer, and
iTunes need none.
A provider that is unreachable logs `provider enrichment attempt failed` and retries on a durable
schedule; the Artist keeps its Active identity meanwhile.

External IDs under the same tab come from the source tags (for example an embedded MusicBrainz ID),
not from provider searches, which is why a catalog imported without MBIDs shows none.

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

## Deleting Artists

From **Catalog → Artists**, select one or more Artists on the current page and choose
**Delete selected Artists**. The confirmation dialog shows the exact impact, requires an audit
reason, and asks you to type `DELETE`.

Two scopes are available:

- **Delete catalog records only (keep media files)** — the default. The Artist disappears from the
  catalog and every Release, Track, credit, and media file stays untouched. Use this for a
  duplicate identity or an import artifact whose content belongs elsewhere.
- **Delete catalog records and media files** — also retires every Release this Artist owns alone
  (no other Artist is billed as its primary Release artist) and removes that Release's canonical
  and staged media from configured storage. Releases billed as primary together with another
  Artist are kept, and the dialog says how many.

Nothing is erased from the evidence graph: the deleted Artist and its Releases become tombstones
that keep their public ids, credits, contributions, provider evidence, per-user state, and audit
history. Deleted entries no longer appear in any catalog list, search result, or resolver
candidate. Deletion is terminal — an Artist that was merged into another cannot be deleted, and
split the Release first if you need to remove a merged one.

The same operations are available over
[Admin API — Catalog deletion](../api/admin-api/#catalog-deletion) for scripting and automation.

## Browsing and detail

Open **Catalog → Artists** (`/admin/catalog/artists`) in the admin UI. The list is searched,
sorted, and paged server-side — the search box binds to the backend query filter, sorting is
restricted to the backend whitelist, and the view state lives in the URL so it can be shared.
Selecting an Artist opens the **detail view** with tabs for overview, **Tracks**, Releases,
appearances, contributions, relationships, identity, images, and history (the audit timeline).

To change the Artist’s primary portrait, open **Images**, choose a JPEG, PNG, or WebP,
and select **Upload artwork**. The new portrait appears in the page header and in the
Images tab. **Remove custom artwork** restores the fallback image. [Artwork upload limits](../api/admin-api/#catalog-artwork-overrides).

The summary numbers are **links into the rows they count**, so a statistic is never a dead end: the
**Track credits** card (every Track the Artist is credited on, whatever the role) and
**Appearances** (the performance subset) both open the **Tracks** tab, which lists the Track, the
Release, the billing role, and whether the credit is a performance credit or a written/production
credit. Each Track appears once even when the Artist has several billing or contribution records
for it; a Track is marked as a performance credit if any of those records is a performance.
The earliest billing supplies the displayed role, while the Contributions tab retains the separate
credits. The list and its count use the same distinct Tracks. The detail
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

## Import reuse and identity evidence

Import can reuse a live Artist when exactly one Artist has the observed normalized name, that Artist
has no disambiguation hint, and any embedded MusicBrainz Artist identifiers already match its recorded
identifiers. Multiple provider identifiers on one Artist do not make it multiple candidates. Two
live Artists with the same name remain ambiguous even if only one has a disambiguation hint.

Reuse preserves the existing public ID and identity status. Import records that reuse in its decision
history, retains a snapshot of the final Artist references, refreshes counts, and queues a new search
revision. Retrying an already completed import does not create another identity or duplicate the
revision. See [upgrading](../installation/upgrading/) for repair of snapshots and search status
created by earlier builds.
