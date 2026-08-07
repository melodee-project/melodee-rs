---
title: Inbound Processing
description: How new media moves from an inbound directory into the catalog.
sidebar:
  order: 1
---

Inbound processing turns dropped media into reviewed catalog Releases:

1. **Discovery** — inbound directories are scanned; file watchers only accelerate discovery.
2. **Work items** — each candidate Release becomes one durable work item with its own retry and
   failure state, so one bad Release never blocks the rest.
3. **Inspection and normalization** — metadata is probed, audio is fingerprinted, and media is
   normalized into canonical files with a versioned manifest.
4. **Review** — Releases await operator review where policy requires it; see
   [Release Review](./release-review/).
5. **Publication** — accepted Releases are published to library storage and become visible to
   clients.

:::caution[Planned documentation]
The durable ingestion engine is being built in the Rust rewrite (Phases 2–3). Concrete queue
behavior, statuses, and operator actions will be documented against the shipped implementation.
:::
