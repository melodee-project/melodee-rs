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
4. **Inspection** — a leased worker reads native tags with Lofty, obtains technical facts from ffprobe,
   inventories artwork and sidecars, and streams a BLAKE3 byte hash for audio files.
5. **Processing pipeline handoff** — successful inspection publishes one schema-versioned manifest and
   advances the item to `Normalizing`; later phases consume that manifest.

Discovery and settling are source-correctness gates only in this phase. The candidate is not interpreted as
media yet.

## Observed states

For each discovered candidate the queue uses these states:

- `Discovered` — discovered but not yet snapshotted/classified by settling rules.
- `Settling` — snapshotted and waiting for stability.
- `Ready` — stable evidence exists and the candidate can be claimed by workers.
- `Inspecting` — a leased worker is observing one coherent settled source snapshot.
- `Normalizing` — inspection has published the durable manifest for the next phase; normalization is not
  implemented yet.

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
- Inspection is also observationally read-only. It never rewrites tags, extracts artwork into the source,
  renames files, or deletes candidates.
- Source paths are canonicalized and candidates outside source roots (including symlink escapes) are ignored.
- Disappearing or unreadable candidates do not delete work items; they keep durable history and are marked for
  operator review.

## Inspection outcomes

The manifest preserves observed tag values without resolving canonical Artist identities. Missing tags are
recorded as metadata-poor observations; valid tagless audio is still inspectable. Corrupt or unsupported
audio produces a durable failure instead of a partial manifest. A source change detected before or during
inspection discards the in-progress result and returns the work item to `Settling`.

For development diagnostics, `melodee ingest inspect` runs one bounded inspection batch and
`melodee ingest manifest <work-item-id>` prints the durable manifest. These commands are not a replacement
for the server worker.
