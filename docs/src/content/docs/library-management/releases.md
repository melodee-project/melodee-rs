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
Release status is presented in operator language: a withheld policy result appears as **Needs
review**, **Quarantined**, or **Ignored**, with its evidence when available, rather than exposing
the internal `deprecated` catalog projection. Only accepted (`Active`) Releases serve Track audio
and canonical artwork. An ordinary `Unknown` classification is accepted by the built-in policy;
Unknown review occurs only when an administrator explicitly configures that rule.
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

To change the primary cover, open **Images**, choose a JPEG, PNG, or WebP, and select
**Upload artwork**. Tracks on the Release inherit its cover. **Remove custom artwork** restores
the fallback cover. [Artwork upload limits](../api/admin-api/#catalog-artwork-overrides).

The initial values come from the media, not from a guess: the release title and artists are the
observed tags, and the release date is the date tag carried by most of the Release's Tracks, with
the full date stored as the source text alongside the editable date. A Release whose audio carries
no date can still receive date *text* from a `.blackbeard.provenance.json` sidecar (a year, for
example), and any value can be corrected through the Edit panel.

An administrator-confirmed deletion removes the Release from this listing and deletes its
canonical and retained staged media. Immutable identifiers and audit evidence remain as a
non-browsable tombstone; see [Release Review](./release-review/#deciding).

## Merging duplicate Releases

From **Catalog → Releases**, select two or more Releases on the current page and choose
**Merge selected Releases**. Selection is not restricted to one Artist: a Release duplicated
under a wrong or "Various Artists" credit can be folded into the correct one. Pick the
**Surviving Release**, enter an audit reason, review the server-calculated impact, and type
`MERGE` to confirm.

The chosen Release keeps its title, metadata, Release artist, and existing artwork. Every
selected source contributes:

- its Tracks — with their observations, contributions, and media — appended to the surviving
  Release's Track order;
- its cover artwork, if the surviving Release has none (one deterministic source, otherwise the
  surviving Release's cover wins and the source covers stay on their tombstones);
- provider artwork for any provider the surviving Release has no image for;
- per-user state (stars, ratings, pins, play counts) for every user who has none on the
  surviving Release — a user with both keeps the surviving Release's row, so no user data is
  summed or lost;
- its release-level billing as tombstone evidence; the surviving Release is never re-credited to
  a source Artist.

Classification and novelty evidence is immutable history: it stays with the Release whose
processing evaluation recorded it and is never rewritten by a merge, so a merge does not
re-evaluate the surviving Release's policy facts.

The preview reports Tracks moved, the Track count after the merge, credits and observations
moved, shared recordings, adopted artwork, and metadata conflicts before anything is written.
The apply step is one transaction: if any selected source cannot be merged, none of them change.
Each source becomes a redirect tombstone with its public id preserved, and its recorded merge
event can be reversed from the
[Release identity correction](../api/admin-api/#release-identity-correction) API, which restores
the Tracks, credits, billing links, and adopted artwork. No media files are moved or deleted.

Duplicate candidates can also be reviewed from the admin API's
[duplicate candidate scan](../api/admin-api/#duplicate-candidate-scans); a scan result is a
review suggestion, never an automatic merge.

## Deleting Releases

The same list offers **Delete selected Releases** beside merge. The confirmation dialog shows the
exact impact, requires an audit reason, and asks you to type `DELETE`, with two scopes:

- **Delete catalog records only (keep media files)** — the Releases disappear from the catalog;
  their Tracks, credits, and media files stay on disk and remain auditable.
- **Delete catalog records and media files** — also removes the selected Releases' canonical and
  staged media from configured storage, using only durable publication evidence. If any selected
  Release lacks that evidence, or canonical storage is not configured, nothing is changed.

Deleted Releases become non-browsable tombstones that keep their public ids, Tracks, credits,
observations, policy evidence, per-user state, and audit history. A merged Release must be split
first, and a Release Review deletion remains the path for a single processing disposition.

The same browsing routes are available over
[Admin API — Catalog browsing and detail](../api/admin-api/#catalog-browsing-and-detail).
