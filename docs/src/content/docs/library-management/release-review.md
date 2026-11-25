---
title: Release Review
description: Reviewing, accepting, and rejecting inbound Releases.
sidebar:
  order: 2
---

Release review is the human gate between inbound processing and the published catalog. Review
decisions are explainable and audited: policy says *why* a Release was flagged, and operator
decisions are remembered. The graphical review workflow lives in the
[Release Review workspace](#review-queue-and-release-review-workspace) below; the CLI is the
parallel headless surface.

The CLI can inspect and override a pending decision:

```bash
melodee-cli policy inspect <release-id>
melodee-cli policy decide <release-id> review \
  --actor <administrator-id> \
  --reason "<operator-safe reason>"
```

The override requires the exact current sealed-manifest fingerprint and records the administrator
and reason in append-only audit history. Use `accept`, `ignore`, or `quarantine` to complete and
route a non-destructive decision. The CLI can record a pending `delete` authorization but does not
remove media; complete destructive deletion through the Admin UI workflow below. See
[Policies](../admin/policies/) for the full command and safety contract.

## Review Queue and Release Review workspace

Open **Processing → Review Queue** (`/admin/processing/review`) in the admin UI. The queue lists
the review-able slice of the pipeline — `needs_review`, `failed`, and quarantined work items —
as a server-paged table with state filter pills fed by the listing's own counts. There is no
bulk mutation over review items; every decision is per-Release and audited.

Selecting a row opens the **Release Review workspace** (`/admin/processing/review/{id}`), which
presents evidence first and choices last:

- **Classification and evidence** — overall confidence, the classifier version, and each
  finding (for example `Title contains "Greatest Hits" → greatest_hits`).
- **Catalog comparison** — known, new, alternate-version, and uncertain Track counts with
  percentages, evaluated timestamp, and evaluator version. Comparison uses durable evidence
  recorded at import: an identical canonical byte hash makes a Track an **exact duplicate**, and an
  identical acoustic fingerprint (the Chromaprint observation every import already computes) makes
  it a **known** recording even when the file itself differs, for example a copy that was only
  retagged. A Track stays **uncertain** until a catalogue comparison actually concludes something —
  a locally computed fingerprint is evidence, not a verdict — and re-encodes at a different bitrate
  keep that honest verdict until a similarity matcher exists.
- **Track breakdown** — per-Track duration, format, and novelty result, with an **inline audio
  preview** transport (never autoplaying; a missing preview degrades gracefully).
- **Policy match** — the matched rule ids, the recommended disposition, the decision state, the
  immutable rule-set version, and the evidence summary.
- **Review timeline** — the append-only transition history for the work item.

![The Release Review workspace](../admin/images/release-review-light.png)

### Deciding

An administrator or editor can delete a Release marked **Needs review**, including one that
stopped during validation before it reached the catalog. Open the Release, choose **Delete**,
check the affected items, type the confirmation shown (`DELETE` in English), and select **Delete
Release**. **Reason (optional)** lets you leave a note for the audit history.

Melodee records who confirmed deletion before removing files. For a Release that stopped during
validation, it removes that work item's managed staging and normalization directories. A file
later copied into the original inbound location is safe. For a cataloged Release, deletion also
removes its published bundle and retires the Release from browsing, search and playback.
If storage becomes unavailable, correct the storage issue and confirm **Delete** again. The
retry uses the original authorization and finishes the outstanding cleanup.

Administrators can also **Accept**, **Quarantine**, or **Ignore** a Release and supply a reason.
**Accept** requires a pending catalog policy decision. **Reprocess** returns a failed or
quarantined Release to the queue and accepts an optional reason.

## Safe metadata transformations

Administrators can open a published Release's metadata transformation panel from its catalog
detail screen. The panel previews field-level changes and affected files before an apply action;
apply requires the returned preview hash and an explicit confirmation. Supported operations cover
featuring cleanup, artist replacement/clearing, year updates, track renumbering, and bounded text
replacement. Every apply records the acting administrator, deltas, hashes, decoded-audio
fingerprints, and recovery evidence. The rewrite changes tags only: paths, extensions, audio
payloads, and Melodee-owned identifiers remain unchanged.

The history view shows successful and failed attempts. A successful attempt can be reverted while
its same-directory recovery copies remain available. A failed write or integrity check restores
the originals and leaves an operator-visible failed audit. This workflow never deletes source
media and is separate from the policy disposition controls below.

The underlying routes are documented under
[Admin API — Release review](../api/admin-api/#release-review).
