---
title: Processing
description: Monitoring and steering the inbound processing queue.
sidebar:
  order: 2
---

The processing queue shows one durable work item per inbound Release, with per-item status,
history, and a read-only inspector. A stalled or failed Release never blocks the others.

## Processing Queue screen

Open **Processing → Processing Queue** (`/admin/processing`). The screen is a dense table backed
by the server-side work-item listing: state filter pills show live counts per durable state
(`discovered`, `settling`, `ready`, `inspecting`, `normalizing`, `needs_review`, `failed`, and so
on), sorting is restricted to the backend whitelist (`addedAt`, `updatedAt`, `state`,
`attemptCount`), and paging is envelope-driven with honest totals. Filter, search, sort, page, and
selection state live in the URL, so a filtered view can be shared as a link.

Each row shows the Release the work item belongs to (its title and billed Artist once the import
is bound, and the work item's own path before that), its durable state, its **stage progress** in
the Release lifecycle, when it was added, and the media size once known. Stage progress is a
position in the documented lifecycle — `Discovered → Processing → Review → Cataloged` — not an
elapsed-time estimate, and `Attempts` stands in the place a priority column would occupy because
Melodee has no priority concept. The **Search Releases** box filters the queue server-side by
Release title, billed Artist, or work-item path.

Selecting a row opens the **inspector**, which shows the work item's full durable state —
attempts, lease evidence, last error, timestamps — together with its append-only **event
timeline**: the answer to "what happened to this Release and why". Both come from the
[Admin API](../api/admin-api/#work-item-detail) diagnostics endpoints and show only durable
PostgreSQL state; nothing is inferred from a running process.

![The Processing Queue with the state filter pills, Release-level rows, and stage progress](./images/processing-queue-light.png)

Bulk actions are deliberately not offered, and the queue screen itself is read-only: every
queue mutation in this build is a per-item, audited decision made from the
[Release Review](../library-management/release-review/) workspace or the CLI.

## Phases, states, and what "playable" means

The queue filters by **phase**, not by raw state. A phase is a coarse grouping the operator can
reason about; the state button in each row stays the precise value.

| Phase | States | Meaning |
|---|---|---|
| Inbound | discovered, settling, ready | Arrived, being snapshotted, or waiting for a worker |
| Processing | inspecting, normalizing, validating | Being read, converted, and checked |
| Publishing | staged, publishing, published | Canonical publication in progress or verified |
| Catalog | cataloging, cataloged | Catalog import in progress or complete |
| Search | indexing, indexed | Search projection in progress or complete |
| Review | needs_review, quarantined | Parked on an operator decision |
| Failed | failed | Retrying or awaiting an operator |
| Closed | ignored, deleted | Final non-accepted disposition |

A Release becomes **playable** when the policy decision is `Accept` (its catalog rows and canonical
files exist and its status is active). It becomes **searchable** one step later, at `indexed`, when
the search projection that makes it appear in browse, search, and list results is durable. So if a
Release is playable but missing from search results, it is between those two points — the queue row
will still read `Search`.

## Quarantine and History

- **Processing → Quarantine** (`/admin/processing/quarantine`) lists quarantined work items —
  Releases held out of the catalog by policy or by an operator decision — with the same paging
  and inspector behavior as the main queue.
- **Processing → History** (`/admin/processing/history`) lists work items in a terminal state —
  `indexed`, `cataloged`, `ignored`, and `deleted` — over the append-only transition record, so a
  Release disappears from the active queue exactly when its processing is finished or its
  disposition is final.

`Indexed` is the successful end of the lifecycle: the search projection for the Release, its
Tracks, and its billed Artists is durable and queryable, and the work item is marked complete.
A work item that is still `indexing` is unfinished work and is deliberately not shown in History.

## Steering processing

Queue throughput is controlled at the worker-pool level, not per item: pause or resume pools from
[System and Diagnostics](./system-diagnostics/) (or `melodee-cli worker-pools pause|resume`). A
paused pool claims no new work; already-claimed items finish or are released under the normal
lease contract, so pausing never strands a lease.

Reprocessing a Release that already carries a policy decision is a review action — see
[Release Review](../library-management/release-review/).

The equivalent CLI surfaces are `melodee ingest status`, `melodee-cli work list`,
`melodee-cli work show <id>`, and `melodee-cli catalog status`; see
[System and Diagnostics](./system-diagnostics/#command-line-diagnostics).
