---
title: Storage
description: Inbound directories, library storage, and how Melodee treats the filesystem.
sidebar:
  order: 3
---

## Storage lifecycle

Melodee uses four separate storage roles. They are not interchangeable.

| Role | Configuration | Lifecycle | Permission |
| --- | --- | --- | --- |
| Inbound | `MELODEE__INGEST__SOURCE_ROOT` / `MELODEE__INGEST__SOURCES_JSON` | External transient filesystem drop directory; drained after verified acquisition. | Read plus rename/remove of acquired candidate directories; never rewrite media bytes. |
| Staging | `MELODEE__INGEST__STAGING_ROOT` | Durable Melodee-owned original source for processing, review, quarantine, and failed recovery. | Writable and persistent. |
| Work area | `MELODEE__NORMALIZATION__WORK_AREA` | Disposable attempt-isolated generated output. | Writable; never staging. |
| Canonical library | `MELODEE__STORAGE__ROOT` | Long-term immutable published Release bundles. | Writable for publication. |

The staging root is required whenever inbound ingestion is configured. All four paths must be
absolute, symlink-safe, and non-overlapping. `Staged` remains an ingestion state-machine name; it
does not name this filesystem root.

When inbound and staging are on one filesystem, Melodee uses an atomic no-replace directory move.
Across filesystems it copies under a hidden staging directory, verifies streamed BLAKE3 hashes,
fsyncs, finalizes without replacement, records the receipt, then removes the matching inbound
candidate. A crash converges from that receipt without reopening historical inbound paths.

## Libraries are access scopes, not mutable path records

Unlike the C# server, Melodee does not store a mutable library entity with a name, type, and
filesystem path. Storage topology is deployment configuration (the four roles above), while a
*library ID* is an authorization scope used by group bindings and per-library browse, search, and
stream fencing. Adding a new media source is a configuration change plus a restart; library IDs and
group bindings are managed at runtime in **System → Users** (or the group admin API/CLI).

Legacy library rows are preserved for ACL bindings during migration; their names and paths are
reference-only configuration evidence and never become runtime truth. The decision and its
trade-offs are recorded in the repository ADR set (`design/adr/` — ADR-0182). The legacy
system-export `libraries` array is likewise imported as evidence only.

Important rules:

- Filesystem paths never determine semantic identity.
- Inbound bytes remain immutable through discovery, settling, and acquisition. After verified
  staging finalization and durable receipt, the acquired directory leaves inbound.
- Inspection, normalization, validation, review, and quarantine read staged source; the work area
  contains only generated output. Staged source remains after publication unless a future explicit
  retention policy authorizes cleanup.
- Watchers (if enabled in future work) are accelerators only; reconciliation scans and durable work items are
  the source of truth.

### Retained copies and disk planning

A processed Release exists in up to four places, and only one of them is the library:

| Location | Content | Reclaimed? |
|---|---|---|
| canonical storage root | the published bundle (`manifest.json`, `media/`, `artwork/`) | never (it *is* the library) |
| staging root | the verified staged source the publication receipt points at | not automatically |
| normalization work area | generated output from inspection/normalization | not automatically |
| search root | the Tantivy generation | replaced on rebuild |

Measured on a 25-Release / 1.8 GB batch: 1.8 GB published, 1.8 GB retained staged source, 1.8 GB
in the work area — about **3× the inbound bytes**. Size the host for roughly 3× the largest batch
you intend to ingest in one go, on top of the library itself.

There is no automatic garbage collector for the staging root or the work area: staged source is
retained deliberately because the publication receipt references it as recovery evidence, and the
work area has no reclamation path yet. Removing either is an operator decision made with the
server stopped — it costs recovery evidence, never catalog correctness. Until a retention policy
exists, treat both roots as append-only when planning capacity.

## Path safety rules

- Candidate roots are canonicalized and path containment is enforced against each inbound root.
- Symlink targets that escape an inbound root are ignored.
- Discovery uses sorted, bounded reads to avoid loading the entire disk tree into memory for large libraries.
- Inbound, staging, normalization-work, and canonical-storage roots cannot overlap, and configured roots
  cannot be symlinks.

## Canonical Release layout

Publication writes immutable-ID-centric, lowercase paths:

```text
<storage-root>/
  releases/<shard-0>/<shard-1>/rel_<release-id>/
    manifest.json
    media/<blake3>.mp3
    artwork/<blake3>.jpg
```

The two shards come from the immutable Release ID. Artist names, Release titles, and source paths
never determine storage identity. Database paths are storage-root-relative so the library can be
relocated as a unit.

Melodee copies each planned regular file into a hidden incoming directory on the target filesystem,
checks its size and BLAKE3 hash, seals the schema-v3 manifest, flushes the bundle, and performs an
atomic no-replace finalization. A retry adopts an already-complete matching bundle. A mismatching
target is a durable conflict and is never overwritten.
