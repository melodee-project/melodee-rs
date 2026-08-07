---
title: Docker
description: Run Melodee with Docker and Docker Compose.
sidebar:
  order: 1
---

:::caution[Planned documentation]
The Rust rewrite has not yet published server images. The layout below is the intended deployment
shape and will be validated against released images before it is presented as working
instructions. The currently shipping C# server ships its own Docker images; see the legacy
[Melodee repository](https://github.com/melodee-project/melodee) for current-image guidance.
:::

The documented deployment will consist of:

- a Melodee server container;
- a PostgreSQL 18 database (PostgreSQL 17 supported);
- host-mounted volumes for inbound media, library storage, and database data.

A reference `compose.yaml` will be published and tested with the first server release.
