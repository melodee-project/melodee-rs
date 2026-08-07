---
title: Search and MQL
description: Catalog search and the Melodee Query Language (MQL).
sidebar:
  order: 6
---

Melodee search is backed by a dedicated index that is a rebuildable projection of the catalog —
rebuilding it never requires re-ingesting media. Search covers Artists, Releases, Tracks, aliases,
and display credits.

**MQL** (Melodee Query Language) is the structured query syntax for precise catalog queries.

:::caution[Planned documentation]
The search index and MQL parser are implemented in the Rust rewrite's search phase. The MQL
syntax reference and compatibility notes will be documented here against shipped behavior and its
compatibility tests.
:::
