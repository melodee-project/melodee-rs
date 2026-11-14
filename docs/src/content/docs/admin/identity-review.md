---
title: Identity Review
description: Resolving uncertain Artist matches and merge/split decisions.
sidebar:
  order: 4
---

Identity review is where uncertain artist matches are resolved: an observed name string can match
zero, one, or many Artists, and the operator (or evidence rules) decide. Merge and split decisions
are recorded with history and never move media files.

## Identity Review screen

Open **Catalog → Identity Review** (`/admin/catalog/identity`). The screen lists the unresolved
queue: observed artist strings that have no authoritative match decision, with the display, raw,
and normalized values, the source Release (and Track when the observation is track-scoped), and
the pending candidate and evidence counts. The queue is server-paged and sortable; filter state
lives in the URL.

Selecting an observation opens the **candidate comparison**: every candidate Artist with its
confidence, the full persisted evidence breakdown, and any do-not-match assertions already on
record.

![The Identity Review screen with a candidate comparison](./images/identity-review-light.png)

## Decisions

Every decision requires a non-blank reason and is recorded in the append-only audit with the
acting administrator:

- **Confirm** — lock the observation to the chosen candidate Artist. A locked administrator
  decision is never superseded by an automated resolver, and the observation leaves the
  unresolved queue.
- **Create New** — create a new Artist from the observation's display value and lock
  the observation to it.
- **Leave Unresolved** — an honest, audited no-op: no decision row is written and the
  observation stays queued for a later pass.
- **Never Match** — a durable, locked *negative* assertion against the exact
  observation/candidate pair; automated resolvers then fail closed for that pair. Because this
  shapes future automation, the dialog is danger-styled and the reason is mandatory.

A **recent decisions** feed on the same screen shows the append-only identity audit — matches,
evidence, assertions, and Artist-level events — newest first, with actor and reason.

## AI-assisted proposals

An AI curation harness can contribute evidence without changing identity authority. It calls
`POST /api/admin/v1/identity/observations/{id}/propose` with a candidate Artist and one or more
evidence facts. Melodee stores the evidence and a non-authoritative **proposed** decision; the
observation stays in the queue and no Release or Track credit changes. The candidate comparison
shows the proposal like any other resolver proposal, and an administrator still confirms,
creates a new Artist, or rejects it. See
[Propose an observation match](../api/admin-api/#propose-an-observation-match).

Harnesses can also use the read-only
[duplicate candidate scans](../api/admin-api/#duplicate-candidate-scans) to find likely duplicate
Artists and Releases with the supporting evidence and explicit blockers. A scan result is a
review suggestion, never an automatic merge.

## Merge and split

The screen also corrects Artist identities directly:

- **Merge** folds a source Artist into a target Artist. The source's public id is preserved as a
  redirect, the merged Artist can no longer participate in identity mutations, and merging an
  Artist into itself is rejected. Merge is destructive-class: the dialog requires typed
  confirmation and a reason.
- **Split** creates a new Artist and moves **only** the explicitly listed
  assignments (Release/Track credits, contributions, aliases, external identities) off the
  source Artist; an empty assignment list is rejected. In this build the form takes the durable
  row ids of the selected links directly (a picker UI is not built yet), so splits are
  typically easier to drive through the
  [Admin API](../api/admin-api/#split-an-artist) when many rows are involved.

Neither operation touches media files; they change catalog relationships only.

All of these flows use the [Admin API identity routes](../api/admin-api/#artist-identity-review),
which are the parallel surface for scripting and automation.

## What import resolves on its own

Import reuses an existing Artist when a credit's normalized name exactly equals exactly one
existing Artist that carries no disambiguation hint and no conflicting trusted provider identifier
(see [Artists](../library-management/artists/)). Duplicates that still exist in a catalogue written
before that rule are converged by the bounded post-import reconciliation pass, for primary Release
credits and Track credits alike, whenever the whole same-name group has no differentiating
evidence. Each convergence is recorded as an audited, reversible merge.

What remains in this queue is genuine ambiguity: same-name artists distinguished by provider
identity, disambiguation, Artist type, active dates, memberships, relationships, aliases, locked
decisions, or **Never Match** assertions, plus names no automated rule may resolve.

## Artist status

Artists created from imported credit text start as **Active**: the catalogue knows the name
and the credits are usable, while the identity itself is not yet asserted. Confirming that an
observation names an existing Artist (the `confirm` resolution) promotes that Artist to
**Verified** in the same transaction, so the Artists list shows how much identity work has been
done. The `create-new` resolution records a distinct Artist and leaves it Active. You can also set
the status directly in the Artist's Edit panel. ADR-0190 renamed this state from `provisional`;
migration `0044` relabelled existing rows in place.
