---
title: Inbound Processing
description: How new media moves from an inbound directory into the processing queue.
sidebar:
  order: 1
---

Inbound processing discovers candidate Releases from inbound roots and creates one durable work item per candidate.

1. **Discovery** — each enabled inbound source is scanned into its own reconciliation pass. Filesystem
   events are an optional optimization only if a watcher integration is enabled later.
2. **Settling** — candidate contents are snapshotted deterministically and compared across passes.
3. **Ready** — once stable for the configured quiet period (or marker-shortcut period) the candidate is
   promoted to `Ready`.
4. **Processing pipeline handoff** — `Ready` items are available for worker execution in the durable
   queue.

Discovery and settling are source-correctness gates only in this phase. The candidate is not interpreted as
media yet.

## Observed states in Phase 1B

For each discovered candidate the queue uses these states:

- `Discovered` — discovered but not yet snapshotted/classified by settling rules.
- `Settling` — snapshoted and waiting for stability.
- `Ready` — stable evidence exists and the candidate can be claimed by workers.

The `transition` table is durable and persisted in PostgreSQL, so restart and missed scans recover correctly.

## Release boundary

Candidates are immediate subdirectories of each configured inbound source root. `melodee` does not treat file
extensions or names as identity by themselves.

## Stability and ready marker

- A snapshot compares deterministic file tree signature (path, type, size, modification time).
- If the signature changes, the candidate remains `Settling` and `stableSince` is reset.
- If unchanged across the quiet period, the candidate becomes `Ready`.
- If `.melodee-ready` is present, settling can use a shorter marker shortcut than the default quiet period.

## Safety constraints

- Inbound source locations are treated as read-only by discovery/settling.
- Source paths are canonicalized and candidates outside source roots (including symlink escapes) are ignored.
- Disappearing or unreadable candidates do not delete work items; they keep durable history and are marked for
  operator review.
