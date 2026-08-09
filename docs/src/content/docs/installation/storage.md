---
title: Storage
description: Inbound directories, library storage, and how Melodee treats the filesystem.
sidebar:
  order: 3
---

Melodee separates **inbound** media from **library** storage:

- **Inbound** is where new data is staged. In Phase 1B this path is discovered and snapshotted for
  settling; it is not deleted or renamed by discovery.
- **Library storage** holds stable canonical assets created in later phases.

Important rules for discovery/snapshot behavior:

- Filesystem paths never determine semantic identity.
- Inbound media is read-only during discovery and settling.
- Watchers (if enabled in later phases) are accelerators only; reconciliation scans and durable work items are
  the source of truth.

## Path safety rules

- Candidate roots are canonicalized and path containment is enforced against each inbound root.
- Symlink targets that escape an inbound root are ignored.
- Discovery uses sorted, bounded reads to avoid loading the entire disk tree into memory for large libraries.
