---
title: What is Melodee?
description: Overview of the Melodee self-hosted music server.
sidebar:
  order: 1
---

Melodee is a self-hosted music server. It ingests your owned media library, normalizes it into a
curated catalog of **Artists**, **Releases**, and **Tracks**, and serves that catalog to music
clients over the native Melodee API, the OpenSubsonic API, and a Jellyfin-compatible API.

Key characteristics:

- **Self-hosted.** You run Melodee on your own hardware with your own storage. No hosted service
  is required.
- **Release-centric.** The catalog is organized around Releases and Tracks rather than loose
  files, with explicit Artist identity so same-name artists stay distinct.
- **Client choice.** Use native Melodee clients, any OpenSubsonic-compatible client, or supported
  Jellyfin-compatible clients.
- **Fully FOSS.** Melodee and every capability it requires are free and open source software.

:::note[Status]
This page describes the current shipping C# Melodee server. The Rust rewrite in the
[`melodee-rs`](https://github.com/melodee-project/melodee-rs) repository is pre-release and does
not yet implement the full feature set. See the repository's `IMPLEMENTATION.md` for the phased
plan.
:::

## Next steps

- [Quick Start](./quick-start/)
- [Concepts](./concepts/)
- [Installation with Docker](../installation/docker/)
