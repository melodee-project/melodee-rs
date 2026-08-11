---
title: Release Review
description: Reviewing, accepting, and rejecting inbound Releases.
sidebar:
  order: 2
---

Release review is the human gate between inbound processing and the published catalog. Review
decisions are explainable and audited: policy says *why* a Release was flagged, and operator
decisions are remembered.

The implemented CLI can inspect and override a pending decision:

```bash
melodee policy inspect <release-id>
melodee policy decide <release-id> review \
  --actor <administrator-id> \
  --reason "<operator-safe reason>"
```

The override requires the exact current sealed-manifest fingerprint and records the administrator
and reason in append-only audit history. Use `accept`, `ignore`, or `quarantine` to complete and
route a non-destructive decision. Choosing `delete` leaves the Release pending in `NeedsReview`;
Phase 5 does not execute deletion or access media. See [Policies](../admin/policies/) for the full
command and safety contract.

:::caution[Graphical review is planned]
The durable decision and CLI review workflow are implemented. Screens and keyboard/operator flows
remain planned; repository design mockups are not product screenshots.
:::
