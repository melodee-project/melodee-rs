---
title: Artists
description: Artist identity, aliases, and merging in the Melodee catalog.
sidebar:
  order: 3
---

An Artist in Melodee is an owned identity, not a name string:

- Same-name bands remain distinct Artists.
- Name variations resolve to one Artist through evidence and aliases.
- Artist merges and splits never move media files; they change catalog relationships only.

See [Identity Review](../admin/identity-review/) for the administrative workflow that resolves
uncertain artist matches.

:::caution[Planned documentation]
Artist identity is implemented in the Rust rewrite's catalog phase. Management operations will be
documented against the shipped server and admin UI.
:::
