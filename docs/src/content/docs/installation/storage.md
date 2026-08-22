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

Important rules:

- Filesystem paths never determine semantic identity.
- Inbound bytes remain immutable through discovery, settling, and acquisition. After verified
  staging finalization and durable receipt, the acquired directory leaves inbound.
- Inspection, normalization, validation, review, and quarantine read staged source; the work area
  contains only generated output. Staged source remains after publication unless a future explicit
  retention policy authorizes cleanup.
- Watchers (if enabled in later phases) are accelerators only; reconciliation scans and durable work items are
  the source of truth.

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
