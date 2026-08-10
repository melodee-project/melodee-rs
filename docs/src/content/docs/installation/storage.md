---
title: Storage
description: Inbound directories, library storage, and how Melodee treats the filesystem.
sidebar:
  order: 3
---

Melodee separates **inbound** media from **library** storage:

- **Inbound** is where new data is staged. This path is discovered, snapshotted, inspected, and
  normalized read-only; it is not deleted, renamed, or rewritten by those stages.
- **Normalization work area** holds attempt-isolated temporary output. It is non-authoritative and
  must not be treated as the library.
- **Library storage** holds verified canonical Release bundles. It is enabled only when
  `MELODEE__STORAGE__ROOT` is explicitly configured.

Important rules for current inbound processing:

- Filesystem paths never determine semantic identity.
- Inbound media remains read-only through discovery, settling, inspection, normalization,
  validation, and publication. Phase 3 always records its disposition as `retained`.
- Watchers (if enabled in later phases) are accelerators only; reconciliation scans and durable work items are
  the source of truth.

## Path safety rules

- Candidate roots are canonicalized and path containment is enforced against each inbound root.
- Symlink targets that escape an inbound root are ignored.
- Discovery uses sorted, bounded reads to avoid loading the entire disk tree into memory for large libraries.
- Inbound, normalization-work, and canonical-storage roots cannot overlap, and configured roots
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
