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
by the server-side work-item listing. Filter pills show live counts by phase. Sortable fields
include when the Release was added, its state, and its attempt count. Filter, search, sort, page, and
selection state live in the URL, so a filtered view can be shared as a link.

Each row shows the Release the work item belongs to (its title and billed Artist once the import
is bound, and the work item's own path before that), its durable state, its **stage progress** in
the Release lifecycle, when it was added, and the media size once known. Stage progress is a
position in the lifecycle. **Waiting for publisher** (`staged`, 70%) means validation finished
and the Release is queued. When a publisher claims it, the state becomes `publishing` (80%).
**Added to queue** shows when Melodee discovered the Release. To see how long it has waited for
publication, select the row and find the transition to `staged` in its event timeline.
On NFS, all publishers share the configured bandwidth budget. A new asset uses that budget
once to write its bytes and once to read them back for final verification. At a 10 MiB/s budget,
the resulting files can account for at most 5 MiB/s before overhead. When that limit
is already in use, adding publisher workers cannot increase it.
On a page left open, use **Refresh** to fetch the latest row states.
If a Release stays in **Normalizing**, check its event timeline. Repeated claims without a
transition to **Validating** mean the worker keeps trying the same step. If PostgreSQL rejects
the character encoding of the normalized metadata, the worker marks the Release **Failed** with
`normalization.invalid_manifest_text`. Check the tags and sidecar files before retrying.
The **Search Releases** box filters the queue server-side by
Release title, billed Artist, or work-item path.

Selecting a row opens the **inspector**, which shows the work item's full durable state —
attempts, lease evidence, last error, timestamps — together with its append-only **event
timeline**: the answer to "what happened to this Release and why". Both come from the
[Admin API](../api/admin-api/#work-item-detail) diagnostics endpoints and show only durable
PostgreSQL state; nothing is inferred from a running process.

![The Processing Queue with the state filter pills, Release-level rows, and stage progress](./images/processing-queue-light.png)

The queue is otherwise read-only. The one bulk action is the failed-item **purge** described
below; every other disposition is a per-item, audited decision made from the
[Release Review](../library-management/release-review/) workspace or the CLI.

### Stage throughput and stalled work

Expand **Stage throughput** on the Processing Queue to see waiting and active counts, the oldest waiting Release, the last completed handoff and completions in the past hour. Select the oldest Release to open its history. A warning appears when a stage has eligible work at least 15 minutes old and has completed nothing for 15 minutes. The server also logs the warning and its recovery when the page is closed. A retry scheduled for later and a Release awaiting manual review are excluded. A single long-running Release can trigger the warning. Compare its progress with the worker logs.

### Submit an archive alongside ordinary arrivals

Before transferring the archive into your usual inbound directory, create a UTF-8 JSON manifest with its exact immediate directory names. For example:

```json
["Artist A - First Release", "Artist B - Second Release"]
```

Register the manifest against the same database and source as the ingestion worker:

```sh
melodee-cli ingest register-bulk --manifest batch.json
```

Use an absolute manifest path when running the CLI through a service wrapper. Pass `--manifest -` to read the JSON array from standard input. The command prints a submission UUID. Then copy or move those directories into the existing inbound directory as usual. Each reservation applies to the next occurrence of that name. When connecting directly to a database with multiple inbound sources, supply the corresponding library UUID with `--source-id`. Unlisted arrivals are ordinary. When both groups are waiting, three of every four claims at each worker stage go to ordinary Releases, and one goes to the archive. Either group can use all available slots when the other is empty.

```sh
melodee-cli ingest bulk-report SUBMISSION_UUID
```

The report starts at the first bulk directory Melodee discovers and ends when every registered entry has its first automatic outcome. Discovery follows inbound polling. If measuring from the start of a physical transfer, also record its start time. **Playable** means policy accepted the Release and activated it for listeners. The report includes ordinary arrivals discovered within the same window, including any that finish later. Each group shows the full denominator, the number playable within 30 minutes, mean, p95, slowest playable Release, failures, manual holds, exclusions and pending work. The report also shows the fraction of all ordinary arrivals playable within 30 minutes and how many bulk Releases were playable within 10 hours of the first discovery. A missing registered directory keeps the report open. Record the host uptime and array status alongside a production qualification run.

### Bulk purge of failed items

Select one or more rows on the **Failed** phase, choose **Bulk actions → Delete selected**, then
answer **Continue to delete?** with **Yes**. The purge removes the selected work items, their
append-only processing event log, their manifests and staging handoffs, and every owned directory
for the item: the inbound candidate (when no staging handoff exists), durable staging tree,
normalization work area, and any published canonical bundle. The item disappears from the queue as
if it had never been processed.

This action is intentionally restricted:

- only items in the `failed` state are accepted;
- an item already bound to a catalog Release is refused (`cataloged`) — use
  [Release Review](../library-management/release-review/) deletion so catalog identity and
  tombstone rules apply;
- paths are derived from durable evidence and must remain below the configured roots; a symlink,
  traversal, or ambiguous inbound root fails closed and leaves the item recoverable;
- a single independent `catalog_audit_events` row records who purged what and why, so operator
  accountability outlives the deleted processing history.

Partial batches are reported per item: successful items are deleted, refused items remain in the
queue with their failure code, and the dialog closes so you can refresh and retry.

## Phases, states, and what "playable" means

The queue filters by **phase**, not by raw state. A phase is a coarse grouping the operator can
reason about; the state button in each row stays the precise value.

| Phase | States | Meaning |
|---|---|---|
| Inbound | discovered, settling, ready | Arrived, being snapshotted, or waiting for a worker |
| Processing | inspecting, normalizing, validating | Being read, converted, and checked |
| Publishing | staged, publishing, published | Waiting for a publisher, publishing, or publication verified |
| Catalog | cataloging, cataloged | Catalog import in progress or complete |
| **Active** | indexing, indexed | **Live to clients: visible and playable.** The search projection may still be finishing |
| Review | needs_review, quarantined | Parked on an operator decision |
| Failed | failed | Retrying or awaiting an operator |
| Closed | ignored, deleted | Final non-accepted disposition |

**`Active` is the state you are looking for.** It means the policy decision was `Accept`, which sets
the Release status to `active` in the same transaction, and from that moment every client surface —
native Melodee API v1, OpenSubsonic, Jellyfin — lists and streams it. The admin and editor vocabulary
is intentionally "Active" rather than "Ready": `ready` already means *queued for a worker* earlier in
the same pipeline, which is not the same thing at all.

The search projection is **not** a gate. It runs inside the `Active` phase and only feeds native
smart-playlist evaluation; client browse, search, and playback read PostgreSQL directly. So:

- last state where clients **cannot** see the Release: `cataloged` (imported, not yet accepted);
- first state where they **can**: `indexing`;
- `indexed` means the background projection also finished — nothing is left running.

The work-item inspector states both facts explicitly (`Client availability`, `Search index`), so you
never have to infer them from lifecycle vocabulary.

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
