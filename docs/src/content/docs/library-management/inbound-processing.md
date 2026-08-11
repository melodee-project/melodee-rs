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
5. **Normalization and fingerprinting** — a bounded leased worker produces verified MP3 work-area
   output, normalizes usable artwork, parses supported sidecars, and fingerprints source audio with
   Chromaprint. Successful work publishes manifest schema v2 and advances to `Validating`.
6. **Structural validation** — another fenced lease verifies required local metadata, every planned
   work-area file, path containment, size, and BLAKE3 hash. Passing work receives one immutable
   Release ID and a durable, non-destructive publication authorization; uncertainty goes to
   `NeedsReview` instead of being guessed.
7. **Canonical publication** — Melodee copies a complete schema-v3 Release bundle into a hidden
   target-filesystem directory, verifies and flushes it, then atomically finalizes it without
   replacing an existing target. A crash retry adopts an identical completed bundle.
8. **Catalog import** — an independently bounded worker consumes the PostgreSQL manifest-v3,
   publication plan, and receipt without reopening media. One transaction creates the Release,
   Tracks, MediaAssets, provisional local Artist and AudioIdentity records, semantic overlay,
   aggregates, audit evidence, and future-projection outbox records.

Discovery and settling are source-correctness gates only in this phase. The candidate is not interpreted as
media yet.

## Observed states

For each discovered candidate the queue uses these states:

- `Discovered` — discovered but not yet snapshotted/classified by settling rules.
- `Settling` — snapshotted and waiting for stability.
- `Ready` — stable evidence exists and the candidate can be claimed by workers.
- `Inspecting` — a leased worker is observing one coherent settled source snapshot.
- `Normalizing` — a leased worker is creating and verifying attempt-isolated temporary output.
- `Validating` — normalization observations were published atomically; canonical publication has
  not occurred yet.
- `NeedsReview` — local presentation/ordering facts require an operator, or a Phase 5 `Review` or
  `Delete` policy decision is pending. Uncertainty never authorizes media deletion.
- `Staged` — an immutable Release ID, structural authorization, and publication plan are durable.
- `Publishing` — a fenced worker is copying, verifying, finalizing, or adopting the canonical
  bundle.
- `Published` — the complete canonical bundle and durable receipt agree; it is ready for catalog
  import.
- `Cataloging` — a fenced catalog worker owns a bounded PostgreSQL attempt, or a transient failure
  is waiting for its durable retry time before another claim.
- `Cataloged` — catalog rows, immutable semantic overlay, import receipt, and work-item transition
  committed atomically and the Release is eligible for policy evaluation.
- `Indexing` — an accepted policy decision is complete and the Release awaits the later search
  projection worker.
- `Ignored` — a non-destructive `Ignore` policy decision completed.
- `Quarantined` — a non-destructive `Quarantine` policy decision completed.

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
- Inspection and normalization are observationally read-only with respect to inbound files. They
  never rewrite tags, extract artwork into the source, rename files, or delete candidates.
- Each normalization execution owns a distinct temporary attempt directory. A stale worker cannot
  remove or overwrite a retry's files, and lease loss cancels and reaps active FFmpeg or `fpcalc`
  processes before results are discarded.
- Source paths are canonicalized and candidates outside source roots (including symlink escapes) are ignored.
- Disappearing or unreadable candidates do not delete work items; they keep durable history and are marked for
  operator review.
- Canonical publication reads only planned regular, non-symlink work-area assets. Traversal,
  symlink, size, or hash mismatches fail before a partial final Release can become visible.
- A successful Phase 3 publication retains the inbound tree byte-for-byte. It is never authority to
  move or delete the source.
- Catalog import does not read source media, canonical media, or the canonical `manifest.json`.
  Artist identity correction changes PostgreSQL relationships and overlay history, never media.

## Inspection outcomes

The manifest preserves observed tag values without resolving canonical Artist identities. Missing tags are
recorded as metadata-poor observations; valid tagless audio is still inspectable. Corrupt or unsupported
audio produces a durable failure instead of a partial manifest. A source change detected before or during
inspection discards the in-progress result and returns the work item to `Settling`.

For development diagnostics, `melodee ingest inspect` runs one bounded inspection batch and
`melodee ingest normalize` runs one bounded normalization batch.
`melodee ingest validate` and `melodee ingest publish` do the same for their respective stages and
require an explicit canonical storage root.
`melodee ingest manifest <work-item-id>` prints the durable manifest. These commands are not a
replacement for the server worker.

For the catalog handoff, `melodee catalog import` runs one bounded import pass,
`melodee catalog status` prints catalog counts and worker limits, and
`melodee catalog inspect <work-item-id>` prints the immutable import receipt when present. The
periodic catalog worker runs independently from inbound discovery, so already-published work can
continue to catalog while inbound scanning is disabled. During graceful server shutdown, a claimed
Release whose catalog transaction has not started releases its lease for another worker. A fenced,
atomic catalog transaction that has already started is allowed to finish before shutdown completes.

The independent policy worker claims at most the lower of its batch and concurrency limits. It
renews leases before loading facts and before the atomic commit, evaluates only PostgreSQL catalog
evidence, and releases pre-commit claims on graceful cancellation. `melodee policy test`,
`evaluate`, `inspect`, and `decide` provide bounded operator diagnostics and audited review; see
[Policies](../admin/policies/).

## Normalization policy

- Existing, probeable MP3 files with exactly one audio stream are completely decode-verified and
  retained byte-for-byte. They are not re-encoded merely to force 48 kHz or a VBR setting.
- Other supported audio is converted with `libmp3lame`, VBR quality 4, and a 48 kHz sample rate.
  FFmpeg receives the legacy-seeded `-b:a 384k` argument too, but libmp3lame ignores that bitrate
  setting when VBR quality mode is active; it is not a 384 kbps cap.
- Converted output must be non-empty, hashable, probeable as one MP3 audio stream, have the expected
  sample rate and channel behavior, and remain within duration tolerance. Any essential verification
  failure prevents the transition to `Validating`.
- BLAKE3 records exact source and output bytes. Chromaprint records acoustic evidence from the source
  audio. Neither hash alone decides Track identity.
- External and embedded JPEG/PNG artwork within resource bounds is decoded and written as padded
  1600×1600 JPEG. Malformed or unsafe artwork is skipped with a structured manifest finding.
- CUE, M3U, SFV, NFO, and LRC sidecars are parsed into structured observations. Malformed content is
  retained as findings; CUE-driven audio splitting is not part of this sub-slice.
