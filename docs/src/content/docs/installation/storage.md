---
title: Storage
description: Inbound directories, library storage, and how Melodee treats the filesystem.
sidebar:
  order: 3
---

Melodee separates **inbound** media from **library** storage:

- **Inbound** is where you drop new or changed media for processing. Files here are inputs, never
  catalog truth.
- **Library storage** holds the canonical, normalized media and versioned manifests that the
  catalog serves from.

Important rules:

- Filesystem paths never determine semantic identity; a renamed or moved directory does not change
  which Artist, Release, or Track something is.
- Melodee never deletes source media because classification is uncertain.
- File watchers only accelerate discovery; scans and durable work items are the source of truth.

:::caution[Planned documentation]
Directory layout, permissions, and mount examples will be validated against the shipped Rust
server during the ingestion phases of the rewrite.
:::
