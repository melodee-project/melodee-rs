# Melodee

[![CI](https://github.com/melodee-project/melodee-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/melodee-project/melodee-rs/actions/workflows/ci.yml)
[![Documentation build](https://github.com/melodee-project/melodee-rs/actions/workflows/docs.yml/badge.svg)](https://github.com/melodee-project/melodee-rs/actions/workflows/docs.yml)
[![Documentation](https://github.com/melodee-project/melodee-rs/actions/workflows/docs.yml/badge.svg)](https://melodee-project.github.io/melodee-rs/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Melodee is a self-hosted, free and open-source music server for a library you own. It ingests
media into a durable, release-centric catalog of **Artists**, **Releases**, and **Tracks**, then
serves it through the native Melodee API, OpenSubsonic, and a music-focused Jellyfin-compatible
API.

The Rust implementation is a ground-up replacement for the
[legacy C# Melodee server](https://github.com/melodee-project/melodee), not a line-by-line port.
The legacy application remains a behavior oracle for compatibility; this repository owns the
current architecture and implementation.

> **Project status:** the core server, catalog and processing pipeline, client adapters,
> user-domain parity, and admin SPA are implemented here. Legacy import, production cutover,
> and rollback rehearsal are still planned and are **not yet available**. Do not replace a
> running legacy server with this checkout until that migration release gate has been completed.

## Highlights

- **Safe, durable ingestion.** Inbound is a transient filesystem drop directory: bytes remain immutable while
  evaluated, then a verified handoff drains the candidate into durable Melodee-owned staging. PostgreSQL-backed,
  lease-based Release work items drive discovery, inspection, normalization, validation, and
  publication, so a failure in one Release does not block the rest of the library.
- **Canonical media without filename identity.** Canonical bundles use immutable Release IDs,
  content verification, versioned manifests, and retry-safe no-replace publication. BLAKE3 proves
  byte identity; Chromaprint is acoustic evidence, not a Track identity.
- **A real music catalog.** Melodee models Artists, aliases, Releases, Tracks, credits,
  contributions, and provisional identity separately. Names and filesystem paths never decide
  identity by themselves.
- **Policy and discovery.** Release traits, auditable policy decisions, dry runs and overrides,
  MQL, and a rebuildable Tantivy search projection are all backed by PostgreSQL as the
  authoritative state store.
- **Client choice.** Native Melodee API v1, OpenSubsonic, and Jellyfin music compatibility are
  separate, captured contracts with their own DTOs and authentication behavior. Direct playback,
  range requests, playlists, queues, scrobbling, shares, and user state are supported where the
  applicable adapter exposes them.
- **Operations and administration.** Health/readiness probes, durable worker diagnostics,
  quarantine and reprocessing, native authentication, library access groups, and the Carbon-based
  admin SPA at `/admin` are included.
- **FOSS by design.** Melodee is MIT-licensed and its dependency policy is checked with
  `cargo deny`.

## Architecture at a glance

```text
transient inbound directory
        │
        ▼
durable staging source ──► inspection / normalization / validation
        │                                      │
        │                                      ▼
        │                           immutable canonical media bundles
        ▼                                      │
PostgreSQL catalog, identity, policy, users ◄─┘
        │
        ├──► Tantivy search projection (rebuildable)
        └──► Native API v1 / OpenSubsonic / Jellyfin / Admin SPA
```

PostgreSQL is the transactional source of truth. Tantivy is deliberately disposable and can be
rebuilt without re-ingesting media. See the [core concepts](docs/src/content/docs/getting-started/concepts.md)
and [storage model](docs/src/content/docs/installation/storage.md) for the data model and safety
invariants.

## Deploy with the official container image

Melodee production releases are distributed exclusively as OCI images from:

```text
ghcr.io/melodee-project/melodee
```

GitHub Releases provide the version record, release notes, and GitHub-generated source archives.
They do **not** include precompiled Linux, Windows, or macOS binaries; installers; `.deb`/`.rpm`;
AppImages; Homebrew artifacts; or ZIP/TAR native binary bundles. Official images support
`linux/amd64` and `linux/arm64` and run with FOSS Docker Engine/Moby plus Compose-compatible tooling
on Linux. Docker Desktop is not required.

The primary deployment path is the committed reference Compose file:

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs
cp compose.env.example .env
# Set unique secrets, a public HTTPS origin, and an absolute writable inbound-media path.
docker compose pull
docker compose up -d
curl --fail http://127.0.0.1:8080/readyz
```

The Compose file pulls `ghcr.io/melodee-project/melodee:${MELODEE_IMAGE_TAG:-latest}`, persists
PostgreSQL, staging, canonical media, normalization work, and Tantivy separately. Inbound is
writable only for candidate-directory rename/removal after verified acquisition. For a reproducible production deployment, set `MELODEE_IMAGE_TAG` to an exact `X.Y.Z`
release (or a digest), not `latest`. See the [container deployment guide](docs/src/content/docs/installation/docker.md)
for required configuration, image tags, upgrades, backups, attestation verification, and first-time
GHCR package visibility.

## Build from source

Developers and advanced users can still build native executables for their target system. This is a
supported source-build workflow, but native executables are not official production release
artifacts or part of the tested release-distribution matrix. See
[Build from source](docs/src/content/docs/installation/build-from-source.md) for prerequisites and
commands.

## Quick start for contributors

The pinned toolchain is Rust **1.97.1**. For a full local environment you also need Python 3.11+,
Docker Compose, PostgreSQL tooling, FFmpeg (`ffmpeg` and `ffprobe`), Chromaprint (`fpcalc`),
Node.js 22.12+ and pnpm 11 for the admin UI and documentation.

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs

# Run repository validation and database-free Rust unit tests.
make bootstrap

# PostgreSQL is required to run the server, integration tests, and full gate.
make postgres-up

# The admin SPA is served from admin-ui/dist at /admin.
corepack enable
(cd admin-ui && pnpm install --frozen-lockfile && pnpm build)
```

Copy [`.env.example`](.env.example) to `.env`, replace both authentication-secret placeholders,
and set the inbound, staging, work, canonical-storage, and (when enabled) search roots to distinct absolute
paths. `MELODEE__LOGGING__LEVEL` controls server verbosity; optionally set
`MELODEE__LOGGING__DIRECTORY` to append console-equivalent output to daily UTC files named
`YYYY-MM-DD.log` in that directory. The example is deliberately local-development-only;
it must not contain real credentials.

```bash
cp .env.example .env
# Edit .env. Generate each secret with: openssl rand -base64 48
# Then export the values for a local process:
set -a && . ./.env && set +a

cargo run -p melodee-server
```

In a second terminal with the same environment, create the first administrator. The password is
read from the environment rather than a command-line argument.

```bash
export MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD='use-a-long-unique-password'
cargo run -p melodee-cli -- users bootstrap-admin \
  --username admin --email admin@example.test
unset MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
```

Visit `http://127.0.0.1:8080/admin` and use the onboarding flow. It checks the live database,
storage roots, media tools, search index, administrator account, and email configuration before it
can be completed.

## Interfaces and clients

| Surface               | Scope                                                                                                                  | Reference                                                     |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Native Melodee API v1 | First-party catalog, user, playback, sharing, request, podcast, and administration features                            | [Native API](docs/src/content/docs/api/melodee-api.md)        |
| OpenSubsonic          | Captured protocol routes, JSON/XML/JSONP forms, auth variants, direct streaming, and music-client behavior             | [OpenSubsonic API](docs/src/content/docs/api/opensubsonic.md) |
| Jellyfin              | Music-focused discovery, login, catalog, artwork, playlists, sessions, and direct-play compatibility                   | [Jellyfin API](docs/src/content/docs/api/jellyfin.md)         |
| Admin UI              | Localized, accessible Carbon SPA for processing, catalog, identity, policy, access, settings, workers, and diagnostics | [Admin overview](docs/src/content/docs/admin/overview.md)     |

The three API adapters intentionally do not share wire DTOs. Compatibility means preserving
observable contracts, not retaining known security defects. Consult
[`compatibility/`](compatibility/) before changing any externally visible route, field, error,
identifier, or authentication behavior.

## Development workflow

Use the fast gate for ordinary iteration:

```bash
cargo fmt --all
make check-fast
```

It validates repository metadata and requirement traceability, checks Rust formatting and
Clippy, and runs workspace library tests. It deliberately omits PostgreSQL integration,
compatibility captures, the Admin UI static gate, and dependency auditing.

Run the complete gate before merge, release, or changes that affect those omitted areas:

```bash
make check
```

`make check` validates the repository and requirement index, runs Python contract tests, the admin
UI type/lint/i18n/unit/build gate when pnpm is available, Rust formatting and Clippy, the full
Rust test suite, and `cargo deny` when installed. CI supplies PostgreSQL and media tools.

GitHub runs the fast gate for pushes and pull requests. The full validation workflow runs every
night and can be started manually from the Actions tab before a release.

Useful focused commands:

```bash
make bootstrap                           # database-free repository and unit-test bootstrap
make postgres-up                         # development PostgreSQL on localhost:5433
make postgres-reset                      # DESTROYS and recreates only the development database
cargo test -p melodee-domain             # one Rust crate
make test-unit                           # database-free Rust unit tests
make test-integration                    # PostgreSQL-backed Rust integration tests
make check                               # complete local release gate; requires PostgreSQL
make lint                                # full workspace Clippy gate
make docs                                # repository/documentation metadata checks
make admin-ui-check                      # Admin UI static checks, unit tests, and build
make admin-ui-e2e                        # Playwright end-to-end and visual tests
make docs-site-check                     # documentation-site checks, links, and build
```

Make uses `python3` by default for repository orchestration. On systems where Python 3 is
installed as `python`, use `make PYTHON=python <target>`.

Read [AGENTS.md](AGENTS.md) before changing this repository. It defines the authority order,
dependency boundaries, data-safety rules, compatibility process, documentation expectations, and
the required definition of done. The primary engineering references are the
[PRD](PRD.md), [SPEC](SPEC.md), [technical specification](SPEC.md),
[accepted ADRs](design/adr/README.md), and [phase evidence](design/evidence/).

## Documentation, compatibility, and migration

- Public documentation source lives in [`docs/`](docs/). The documentation workflow validates and
  builds it; GitHub Pages deployment is opt-in through `MELODEE_PAGES_ENABLED` repository
  configuration.
- Captured compatibility contracts, golden responses, media evidence, and client exercises live
  in [`compatibility/`](compatibility/). They are executable release evidence, not examples to
  hand-edit.
- completion reports and requirement traceability live in [`design/evidence/`](design/evidence/) and
  [`design/requirements/`](design/requirements/).
- The legacy C# repository is valuable compatibility evidence, but its classes and service
  boundaries must not be copied into the Rust implementation.

## License

Melodee is released under the [MIT License](LICENSE).
