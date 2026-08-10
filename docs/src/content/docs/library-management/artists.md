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

The current catalog importer preserves each raw Release/Track Artist observation and creates
conservative provisional local identities. It never merges Artists from normalized-name equality
alone and does not call an external identity provider.

See [Identity Review](../admin/identity-review/) for the administrative workflow that resolves
uncertain artist matches.

:::caution[Planned documentation]
Manual alias, negative-assertion, merge/split, and identity-review management operations will be
documented when their server and admin UI surfaces ship.
:::
