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
melodee policy inspect <release-id>
melodee policy decide <release-id> review \
  --actor <administrator-id> \
  --reason "<operator-safe reason>"
```

The override requires the exact current sealed-manifest fingerprint and records the administrator
and reason in append-only audit history. Use `accept`, `ignore`, or `quarantine` to complete and
route a non-destructive decision. Choosing `delete` leaves the Release pending in `NeedsReview`;
Melodee does not execute deletion or access media from this flow. See
[Policies](../admin/policies/) for the full command and safety contract.

## Review Queue and Release Review workspace

Open **Processing → Review Queue** (`/admin/processing/review`) in the admin UI. The queue lists
the review-able slice of the pipeline — `needs_review`, `failed`, and quarantined work items —
as a server-paged table with state filter chips fed by the listing's own counts. There is no
bulk mutation over review items; every decision is per-Release and audited.

Selecting a row opens the **Release Review workspace** (`/admin/processing/review/{id}`), which
presents evidence first and choices last:

- **Classification and evidence** — overall confidence, the classifier version, and each
  finding (for example `Title contains "Greatest Hits" → greatest_hits`).
- **Catalog comparison** — known, new, alternate-version, and uncertain Track counts with
  percentages, evaluated timestamp, and evaluator version.
- **Track breakdown** — per-Track duration, format, and novelty result, with an **inline audio
  preview** transport (never autoplaying; a missing preview degrades gracefully).
- **Policy match** — the matched rule ids, the recommended disposition, the decision state, the
  immutable rule-set version, and the evidence summary.
- **Review timeline** — the append-only transition history for the work item.

![The Release Review workspace](../admin/images/release-review-light.png)

### Deciding

The decision rail offers the ordered actions **Accept**, **Quarantine**, **Ignore**, and
**Delete**. Every decision requires an operator-supplied **reason**, recorded in the audit with
the acting administrator. `Accept`, `Quarantine`, and `Ignore` complete the non-destructive
decision and route the work item. **Delete** is destructive-class: it opens an impact dialog
that states what deletion authorizes, requires typed confirmation, and sends the decision with
an explicit override flag. Even then, `Delete` records the authorized decision and leaves the
Release pending — no media is accessed, moved, or removed by the review flow. Illegal state
transitions and stale-fingerprint conflicts surface as localized inline errors rather than
silent failures.

**Reprocess** re-queues the Release for another processing pass, with an optional reason.

The underlying routes are documented under
[Admin API — Release review](../api/admin-api/#release-review).
