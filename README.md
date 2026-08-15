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

> **Project status:** implementation phases 0–10 are complete: the core server, catalog and
> processing pipeline, client adapters, user-domain parity, and admin SPA are present here.
> Legacy import, production cutover, and rollback rehearsal are Phase 11 work and are **not yet
> available**. Do not replace a running legacy server with this checkout until that migration
> release gate has been completed.

## Highlights

- **Safe, durable ingestion.** Inbound media is treated as read-only. PostgreSQL-backed,
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
read-only inbound media
        │
        ▼
durable Release work items ──► inspection / normalization / validation
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

## Quick start for contributors

The pinned toolchain is Rust **1.97.1**. For a full local environment you also need Python 3.11+,
Docker Compose, PostgreSQL tooling, FFmpeg (`ffmpeg` and `ffprobe`), Chromaprint (`fpcalc`),
Node.js 22.12+ and pnpm 11 for the admin UI and documentation.

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs

# Start the development PostgreSQL service (host port 5433).
make postgres-up

# Check repository prerequisites, contracts, formatting, and the Rust workspace.
./scripts/bootstrap.sh

# The admin SPA is served from admin-ui/dist at /admin.
corepack enable
(cd admin-ui && pnpm install --frozen-lockfile && pnpm build)
```

Copy [`.env.example`](.env.example) to `.env`, replace both authentication-secret placeholders,
and set the inbound, work, canonical-storage, and (when enabled) search roots to distinct absolute
paths. The example is deliberately local-development-only; it must not contain real credentials.

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

## Deploy from source

There is currently no published Melodee server container image. The committed
[`compose.dev.yaml`](compose.dev.yaml) supplies **only a development PostgreSQL service**. A
simple source deployment therefore consists of a release build, PostgreSQL 17 or 18, persistent
filesystem mounts, and a TLS-terminating reverse proxy.

1. Create a dedicated service account and persistent directories for canonical storage,
   normalization work, and (if enabled) search. Give the service account write access only to its
   owned directories. Mount inbound media read-only.
2. Provision PostgreSQL and a least-privilege application role. Store the database URL and
   secrets in the service manager or a secret store, not in the checkout.
3. Build the server, CLI, and admin UI from the tagged source you intend to run:

   ```bash
   corepack enable
   (cd admin-ui && pnpm install --frozen-lockfile && pnpm build)
   cargo build --release -p melodee-server -p melodee-cli
   ```

4. Set, at minimum, these configuration values in the service environment. Use real absolute
   paths and generate independent secrets of at least 32 random bytes.

   ```text
   MELODEE__DATABASE__URL=postgres://melodee:...@db.example:5432/melodee
   MELODEE__SERVER__BIND=127.0.0.1:8080
   MELODEE__NATIVE__PUBLIC_ORIGIN=https://music.example.com
   MELODEE__NATIVE__AUTH__JWT_KEY=<unique-random-secret>
   MELODEE__JELLYFIN__TOKEN_PEPPER=<different-unique-random-secret>
   MELODEE__INGEST__SOURCE_ROOT=/srv/melodee/inbound
   MELODEE__INGEST__SOURCE_LIBRARY_NAME=main
   MELODEE__NORMALIZATION__WORK_AREA=/var/lib/melodee/work
   MELODEE__STORAGE__ROOT=/srv/melodee/library
   MELODEE__SEARCH__ENABLED=true
   MELODEE__SEARCH__ROOT=/var/lib/melodee/search
   MELODEE__ADMIN_UI__DIST=/opt/melodee/admin-ui/dist
   ```

   Set `MELODEE__NATIVE__TRUSTED_PROXIES` only to proxy addresses or CIDRs you control, and set
   `MELODEE__NATIVE__CORS__ALLOWED_ORIGINS` only to exact browser origins that need it. See the
   [configuration reference](docs/src/content/docs/installation/configuration.md) for all bounds,
   defaults, and security behavior.

5. Start `target/release/melodee-server` with its working directory set to the checkout (or set
   `MELODEE__ADMIN_UI__DIST` explicitly). The server applies its SQLx migrations at startup.
   Check `GET /healthz` for process health and `GET /readyz` for database/search readiness, then
   bootstrap the first administrator with `target/release/melodee-cli users bootstrap-admin …`.

Back up PostgreSQL before upgrading a deployment that matters. The current server can apply its
own additive schema migrations, but there is no supported migration/cutover path from the legacy
C# application yet. That release gate is intentionally deferred to Phase 11.

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

Use the smoke gate for ordinary iteration:

```bash
cargo fmt --all
make smoke
```

It validates repository metadata and requirement traceability, checks Rust formatting and
Clippy, and runs workspace library tests. It deliberately omits PostgreSQL integration,
compatibility captures, the Admin UI static gate, and dependency auditing.

Run the complete gate before merge, release, or changes that affect those omitted areas:

```bash
./scripts/check.sh
```

`check.sh` validates the repository and requirement index, runs Python contract tests, the admin
UI type/lint/i18n/unit/build gate when pnpm is available, Rust formatting and Clippy, the full
Rust test suite, and `cargo deny` when installed. CI supplies PostgreSQL and media tools.

GitHub runs the smoke gate for pushes and pull requests. The full validation workflow runs every
night and can be started manually from the Actions tab before a release.

Useful focused commands:

```bash
make postgres-up                         # development PostgreSQL on localhost:5433
cargo test -p melodee-domain             # one Rust crate
make test-smoke                          # workspace Rust library tests only
make check-full                          # complete local release gate
make lint                                # full workspace Clippy gate
make docs                                # repository/documentation metadata checks
(cd docs && pnpm check && pnpm check:links && pnpm build)
(cd admin-ui && pnpm test:e2e)           # Playwright end-to-end and visual tests
```

Read [AGENTS.md](AGENTS.md) before changing this repository. It defines the authority order,
dependency boundaries, data-safety rules, compatibility process, documentation expectations, and
the required definition of done. The primary engineering references are the
[PRD](PRD.md), [SPEC](SPEC.md), [implementation plan](IMPLEMENTATION.md),
[accepted ADRs](design/adr/README.md), and [phase evidence](design/evidence/).

## Documentation, compatibility, and migration

- Public documentation source lives in [`docs/`](docs/). The documentation workflow always
  validates and builds it; GitHub Pages publication is currently unavailable on this repository's
  plan and is explicitly disabled until a supported deployment target is configured.
- Captured compatibility contracts, golden responses, media evidence, and client exercises live
  in [`compatibility/`](compatibility/). They are executable release evidence, not examples to
  hand-edit.
- Phase reports and requirement traceability live in [`design/evidence/`](design/evidence/) and
  [`design/requirements/`](design/requirements/).
- The legacy C# repository is valuable compatibility evidence, but its classes and service
  boundaries must not be copied into the Rust implementation.

## License

Melodee is released under the [MIT License](LICENSE).
