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
- A native C/C++ build toolchain and Clang/libclang for native dependencies; the repository
  Dockerfile records the reference Linux builder setup.
- PostgreSQL; the reference development and CI environments use PostgreSQL 18.
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
cargo build --locked --release -p melodee-server -p melodee-cli
```

For a database-free checkout validation, run `make bootstrap`. It runs repository checks, Python
tests, formatting checks, and Rust library/binary unit tests. Media tools are required for the media
tests. PostgreSQL-backed integration tests are available through
`make test-integration` and are part of the full `make check` gate. They require the development
database or an explicitly configured test database.

Copy [`.env.example`](../../../../../.env.example) to a private local `.env`, replace the secret
placeholders, and configure separate absolute inbound, staging, work, canonical-storage, and
(when enabled) search roots. Create the inbound directory and give your runtime user access to
the configured roots. Start PostgreSQL (for example `make postgres-up`, which exposes the
development service on host port 5433) and run the server from the checkout. The server and CLI
load `.env` automatically; existing environment variables take precedence:

```bash
target/release/melodee-server
```

The server applies SQLx migrations at startup. In another terminal, use
`target/release/melodee-cli users bootstrap-admin ...` with
`MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD` supplied through the environment. The admin SPA is served
from `admin-ui/dist` by default; set `MELODEE__ADMIN_UI__DIST` if you deploy it elsewhere.

## Guided fresh development setup

The repository also includes an interactive source-development launcher that performs the manual
local sequence and keeps the server attached until Ctrl+C:

```bash
python3 -m pip install -r scripts/requirements.txt
python3 scripts/reset_and_run_server.py
```

:::danger
This launcher permanently recreates the `melodee` database in the fixed `compose.dev.yaml`
development PostgreSQL service. It refuses any other database URL and asks for confirmation unless
you explicitly pass `--yes`.

The launcher separately lists every logging, inbound, staging, work, canonical-storage, search,
podcast-storage, and JSON-configured inbound root explicitly set in `.env` and asks whether to wipe
them. Accepting permanently deletes every file and subdirectory in those roots, including source and
canonical media; back up anything you need first. The root directories and Docker volume remain.
`--yes` does not bypass this separate filesystem confirmation.
:::

The launcher fails before mutation when the repository-root `.env` is absent, validates that the
configured server port is free, and refuses broad, relative, linked, mounted-below, or overlapping
wipe targets. If the filesystem wipe is confirmed, it empties every eligible root before recreating
the database. It then installs the Admin UI lockfile, builds the Admin UI and locked Rust executables,
starts Melodee, and securely prompts for the first administrator's username, email, and password. The
password is passed only through the CLI's documented environment variable and is never placed in
command arguments.

Run `cargo fmt --all` and `make check-fast` while iterating. Before committing or submitting
a change, run:

```bash
python3 scripts/do-prechecks.py
```

`make prechecks` runs the same script. It checks staged and unstaged Git diffs, runs the full
repository gate, and validates/builds the documentation site, stopping at the first failure
with live output and timings. Install cargo-deny in addition to the prerequisites above;
prechecks require all tools rather than skipping missing pnpm or cargo-deny. PostgreSQL must
be available for integration tests, and dependency installs and audits need network access.
Formatting is checked without rewriting source files. Tests run against the working checkout,
including unstaged edits; this command does not install a Git hook.

For Admin UI changes, install Playwright's Chromium browser and run
`python3 scripts/do-prechecks.py --include-e2e` to include end-to-end/visual tests. Release
benchmarks, fresh compatibility captures, real-client acceptance, and authorized operator
rehearsals still require their separate documented runs.

See [Configuration](./configuration/) and [Storage](./storage/) for the safety-critical
runtime settings.
