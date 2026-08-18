---
title: Melodee Documentation
description: User documentation for the Melodee self-hosted music server.
template: splash
hero:
  tagline: Self-hosted music server with a curated, release-centric catalog.
  image:
    file: ../../../public/logo.svg
    alt: Melodee logo
  actions:
    - text: Quick Start
      link: ./getting-started/quick-start/
      icon: right-arrow
      variant: primary
    - text: What is Melodee?
      link: ./getting-started/what-is-melodee/
      icon: information
---

:::note[Project status]
This documentation describes the Rust Melodee server and its official GHCR
container deployment. Legacy C# migration tooling is documented under
[Installation](./installation/legacy-migration/); its production cutover/rollback rehearsal is
the remaining release gate and is identified where relevant.
:::

## Documentation map

- **[Getting Started](./getting-started/what-is-melodee/)** — what Melodee is, quick start, core concepts.
- **[Installation](./installation/docker/)** — Docker deployment, configuration, storage, database, upgrades.
- **[Library Management](./library-management/inbound-processing/)** — inbound processing, release review, artists, releases, tracks, and search.
- **[Clients](./clients/native-melodee-clients/)** — native Melodee, OpenSubsonic, and Jellyfin-compatible clients.
- **[Features](./features/playlists/)** — playlists, podcasts, requests, sharing, and party mode.
- **[Administration](./admin/overview/)** — processing, policies, identity review, users, and diagnostics.
- **[API](./api/melodee-api/)** — Melodee API, OpenSubsonic, and Jellyfin compatibility.
- **[Troubleshooting](./troubleshooting/)** — diagnostics and common problems.
