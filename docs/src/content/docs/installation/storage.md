---
title: Storage
description: Inbound directories, library storage, and how Melodee treats the filesystem.
sidebar:
  order: 3
---

## Storage lifecycle

Melodee uses separate locations for ingestion, publication and mutable artwork.

| Role | Configuration | Lifecycle | Permission |
| --- | --- | --- | --- |
| Inbound | `MELODEE__INGEST__SOURCE_ROOT` / `MELODEE__INGEST__SOURCES_JSON` | External transient filesystem drop directory; drained after verified acquisition. | Read plus rename/remove of acquired candidate directories; never rewrite media bytes. |
| Staging | `MELODEE__INGEST__STAGING_ROOT` | Durable Melodee-owned original source for processing, review, quarantine, and failed recovery. | Writable and persistent. |
| Work area | `MELODEE__NORMALIZATION__WORK_AREA` | Disposable attempt-isolated generated output. | Writable; never staging. |
| Canonical library | `MELODEE__STORAGE__ROOT` | Long-term immutable published Release bundles. | Writable for publication. |
| Asset files | `MELODEE__STORAGE__ASSET_ROOT` | Provider artwork, editor uploads, avatars, playlist images and theme files. | Persistent, writable by the service account. |

The staging root is required whenever inbound ingestion is configured. The ingestion and publication paths must be
absolute, symlink-safe, and non-overlapping. `Staged` remains an ingestion state-machine name; it
does not name this filesystem root.

When inbound and staging are on one filesystem, Melodee uses an atomic no-replace directory move.
Across filesystems it copies under a hidden staging directory, verifies streamed BLAKE3 hashes,
fsyncs, finalizes without replacement, records the receipt, then removes the matching inbound
candidate. A crash converges from that receipt without reopening historical inbound paths.

## Artwork, avatars and themes

Set an existing absolute asset directory outside inbound, staging, work and the published library.
Use a filesystem that supports hard links, file sync and directory sync. Compose provisions a
dedicated named volume. Without an explicit setting, native installations use
`$XDG_DATA_HOME/melodee/assets` or `$HOME/.local/share/melodee/assets`.

The server writes each distinct file once and verifies its SHA-256 before recording a reference.
Keep the hidden `.melodee-asset-store` identity marker with the files when copying or restoring
the directory. Replaced images can still be required by a database backup. Keep the complete
store, including unreferenced files. System statistics show the logical bytes and number of
registered unique files separately from PostgreSQL disk usage.
[Deployment configuration](./configuration/)

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

### Publishing to a Linux NFS library

The service account needs permission to create directories, hard links, and files on the export. Enable server-coordinated file locking for clients that share the library. Keep the hidden per-Release publication lock files in place.

When the NAS also serves playback, set `MELODEE__PUBLICATION__CONCURRENCY=1` for the first bulk import. The publisher shares an 8 MiB/s default budget across its media and artwork writes and verification reads. Each new asset uses the budget for one write and one final verification read. Resuming a partial publication also verifies existing assets, and adopting an existing sealed Release rehashes the complete bundle. Directory operations, filesystem syncs, kernel caching, playback and transfers by other applications affect the NAS separately. Adjust `MELODEE__PUBLICATION__NFS_MIB_PER_SECOND` based on observed playback latency and NAS health. Starting a service with ingestion enabled can resume outstanding publications automatically.

For a supervised first Release, keep automatic ingestion paused and run `melodee-cli ingest publish` once with ingestion enabled for that invocation, publication concurrency set to one, and the same service account, paths and database as the server. The command claims at most one eligible Release with those settings. Check its completed, failed and error counts before repeating it. [Publication settings](./configuration/#throughput-tuning)

A file browser may show the new Release directory as Tracks arrive. Melodee installs `manifest.json` after the media and artwork are complete. A Release enters the catalog after the publisher verifies its manifest and files. An interrupted worker can resume its own partial Release. If publication reports a target conflict, preserve the directory and inspect its contents. A crash during initial reservation can leave an empty directory without an ownership marker. With the service stopped, move that directory aside after confirming the conflict path, then retry the work item. Staged originals are retained.

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

On supported local filesystems, Melodee assembles a hidden directory, verifies and flushes its
contents, then atomically renames the directory into place. On Linux NFS, Melodee installs each
verified file in the reserved Release directory and commits the manifest last. Both paths can
adopt a matching complete Release on retry and preserve conflicting files for inspection.
