---
title: Build from source
description: Build a native Melodee executable for development or community use.
sidebar:
  order: 6
---

Melodee does **not** publish precompiled native executables, installers, operating-system packages,
or binary archives as official release artifacts. The official deployment artifact is the
[GHCR OCI image](./docker/).

You can still build native executables from source for development or an advanced community
deployment. This is not part of the official production release-distribution matrix and does not
promise support for every operating system or architecture.

## Prerequisites

- Rust `1.97.1` (the repository's pinned toolchain).
- PostgreSQL 17 or 18.
- FFmpeg with the `libmp3lame` encoder, `ffprobe`, and Chromaprint `fpcalc`.
- Node.js 22.12+ and pnpm 11 to build the admin SPA.
- Python 3.11+ for repository validation. Docker Compose is useful for the development PostgreSQL
  service but is not required for a native build.

## Build and run

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs
corepack enable
(cd admin-ui && pnpm install --frozen-lockfile && pnpm build)
cargo build --release -p melodee-server -p melodee-cli
```

Copy [`.env.example`](../../../../../.env.example) to a private local `.env`, replace the secret
placeholders, and configure separate absolute inbound, work, canonical-storage, and search roots.
Start PostgreSQL (for example `make postgres-up` for the repository's development service), export
the configuration, and run the server from the checkout:

```bash
set -a && . ./.env && set +a
target/release/melodee-server
```

The server applies SQLx migrations at startup. In another terminal, use
`target/release/melodee-cli users bootstrap-admin ...` with
`MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD` supplied through the environment. The admin SPA is served
from `admin-ui/dist` by default; set `MELODEE__ADMIN_UI__DIST` if you deploy it elsewhere.

Run `cargo fmt --all && make smoke` while iterating and `./scripts/check.sh` before submitting a
change. See [Configuration](./configuration/) and [Storage](./storage/) for the safety-critical
runtime settings.
