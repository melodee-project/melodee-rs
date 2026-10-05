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

:::caution[Pre-release status]
The Rust server is working toward its initial v1.0.0 release and is not yet a qualified production
replacement for the C# server. Legacy custom content blocks, per-user device profiles,
notification-template files, and theme-pack preservation still need implementation or explicit
release dispositions. Current-candidate client/integration acceptance, performance and recovery
qualification, release artifacts, and migration/cutover/rollback rehearsal also remain.

The [migration tooling](./installation/legacy-migration/) is implemented and ready for isolated
rehearsal. Container instructions describe deployment of a published release; verify its release
notes and image availability before using them. Source builds are available for development.
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
