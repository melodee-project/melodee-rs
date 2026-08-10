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
- **Library storage** holds stable canonical assets created in later phases.

Important rules for current inbound processing:

- Filesystem paths never determine semantic identity.
- Inbound media is read-only during discovery, settling, inspection, and normalization.
- Watchers (if enabled in later phases) are accelerators only; reconciliation scans and durable work items are
  the source of truth.

## Path safety rules

- Candidate roots are canonicalized and path containment is enforced against each inbound root.
- Symlink targets that escape an inbound root are ignored.
- Discovery uses sorted, bounded reads to avoid loading the entire disk tree into memory for large libraries.
