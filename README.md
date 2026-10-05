# Melodee

[![CI](https://github.com/melodee-project/melodee-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/melodee-project/melodee-rs/actions/workflows/ci.yml)
[![Documentation build](https://github.com/melodee-project/melodee-rs/actions/workflows/docs.yml/badge.svg)](https://github.com/melodee-project/melodee-rs/actions/workflows/docs.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Melodee is a self-hosted, free and open-source music server for a library you own. It ingests
media into a durable, release-centric catalog of **Artists**, **Releases**, and **Tracks**, then
serves it through the native Melodee API, OpenSubsonic, and a music-focused Jellyfin-compatible
API.

The Rust implementation is a ground-up replacement for the
[legacy C# Melodee server](https://github.com/melodee-project/melodee), not a line-by-line port.
The legacy application remains a behavior oracle for compatibility; this repository owns the
current architecture and implementation.

> **Project status: pre-release, working toward v1.0.0.** The core server, ingestion/catalog
> pipeline, API adapters, and Admin UI are implemented. Legacy migration tooling is implemented
> and ready for isolated rehearsal, but feature/data-preservation gaps and release qualification
> remain. This checkout is **not yet a qualified production replacement** for the C# server.

Start with the [user documentation](https://melodee-project.github.io/melodee-rs/),
[source-build guide](docs/src/content/docs/installation/build-from-source.md), or
[legacy migration guide](docs/src/content/docs/installation/legacy-migration.md).

## Compatibility and v1.0.0 readiness

Compatibility is measured against the frozen C# behavioral baseline at commit
`4028b58dbd3302ed431e2c5b32d9dde5634db218`, with intentional differences governed by accepted
ADRs. It does not imply compatibility with every later C# change or every Jellyfin feature.

Remaining work includes:

- Preserving or explicitly resolving legacy custom content blocks, per-user device profiles,
  notification-template files, and custom theme packs.
- Qualifying metadata transforms against canonical files and PostgreSQL, configured integrations,
  and performance/fault recovery at representative scale.
- Rehearsing migration, backup/restore, cutover, and rollback with authorized representative data.
- Completing current-candidate differential replay and Android phone, Android Auto/DHU, MeloAmp,
  OpenSubsonic, and Jellyfin client acceptance.
- Accepting the exact amd64/arm64 release images, security/supply-chain evidence, and final
  documentation before approved publication.

The [feature-parity matrix](design/requirements/FEATURE-PARITY-MATRIX.md),
[requirements index](design/requirements/REQUIREMENTS-INDEX.md), and
[PRD release gates](PRD.md#57-release-gates) define the remaining obligations. Hamstik project
`MEL`, epic `MEL-1`, tracks the v1.0.0 work program. Passing local checks or closing an
implementation ticket does not replace candidate-specific client and operator acceptance.

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
- **Account self-service.** Configuration-controlled registration, self-profile and avatar APIs,
  and a localized browser password-reset completion page are implemented. MeloAmp owns the
  listener account experience.
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

The official release distribution policy is OCI images at:

```text
ghcr.io/melodee-project/melodee
```

Published GitHub Releases provide the version record, release notes, and GitHub-generated source
archives. They do **not** include precompiled Linux, Windows, or macOS binaries; installers; `.deb`/`.rpm`;
AppImages; Homebrew artifacts; or ZIP/TAR native binary bundles. The release platform matrix is
`linux/amd64` and `linux/arm64`, using FOSS Docker Engine/Moby plus Compose-compatible tooling
on Linux. Docker Desktop is not required.

The following is the deployment procedure **for a published, qualified release**. Check its
release notes and available image tag first; the workflow and Compose file alone do not establish
that a v1.0.0 image or `latest` tag is available. For development before release, use the source
workflow below.

Use the committed [reference Compose file](compose.yml):

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs
cp compose.env.example .env
# Edit .env: set a published MELODEE_IMAGE_TAG, unique secrets, public HTTPS origin,
# matching PostgreSQL password/URL, and an absolute writable inbound-media path.
docker compose pull
docker compose up -d
curl --fail http://127.0.0.1:8080/readyz

# Run in Bash; enter the administrator password without echoing it.
read -r -s -p 'Administrator password: ' MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
export MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
docker compose exec -e MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD \
  melodee melodee-cli users bootstrap-admin --username admin --email admin@example.test
unset MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
```

The Compose file pulls `ghcr.io/melodee-project/melodee:${MELODEE_IMAGE_TAG:-latest}`, persists
PostgreSQL, staging, canonical media, normalization work, and Tantivy separately. Inbound is
writable for candidate-directory rename/removal after verified acquisition; inbound bytes are
not rewritten. Set `MELODEE_IMAGE_TAG` to an exact published version. Digest pinning requires
overriding the service's full `image` reference with `ghcr.io/melodee-project/melodee@sha256:…`;
the tag variable does not accept a bare digest.

The runtime uses UID/GID `10001`; the inbound directory must permit that user to read media and
rename/remove acquired candidates. The default HTTP binding is loopback. Configure a TLS reverse
proxy for remote access, then open `/admin` at the configured public origin and complete onboarding.
See the [container deployment guide](docs/src/content/docs/installation/docker.md)
for required configuration, image tags, upgrades, backups, attestation verification, and first-time
GHCR package visibility.

## Build from source

Developers and advanced users can still build native executables for their target system. This is a
supported source-build workflow, but native executables are not official production release
artifacts or part of the tested release-distribution matrix. See
[Build from source](docs/src/content/docs/installation/build-from-source.md) for prerequisites and
commands.

## Quick start for contributors

The pinned toolchain is Rust **1.97.1**. For a full local environment you also need:

- Python 3.11+ and Make for repository commands.
- A native C/C++ build toolchain and Clang/libclang for dependencies; the
  [Dockerfile](Dockerfile) records the reference Linux builder setup.
- Docker Compose for the reference PostgreSQL **18** development service, plus PostgreSQL
  client tools for backup/restore and migration exercises. Its host port is **5433**.
- FFmpeg (`ffmpeg` with `libmp3lame`, and `ffprobe`) and Chromaprint (`fpcalc`), including for
  media unit tests.
- Node.js 22.12+ and pnpm 11 for the Admin UI and documentation; exact pnpm versions are recorded
  in each `package.json`.
- `cargo-deny` for the complete dependency-policy gate.

```bash
git clone https://github.com/melodee-project/melodee-rs.git
cd melodee-rs

# Run repository validation, Python tests, and database-free Rust unit tests.
make bootstrap

# PostgreSQL is required to run the server, integration tests, and full gate.
make postgres-up

# The admin SPA is served from admin-ui/dist at /admin.
corepack enable
(cd admin-ui && pnpm install --frozen-lockfile && pnpm build)
```

For this native development workflow, copy [`.env.example`](.env.example) to `.env` (the Compose
workflow uses the separate `compose.env.example` format). Replace both authentication-secret
placeholders, and set the inbound, staging, work, canonical-storage, and (when enabled) search
roots to distinct absolute paths. `MELODEE__LOGGING__LEVEL` controls server verbosity; optionally set
`MELODEE__LOGGING__DIRECTORY` to append console-equivalent output to daily UTC files named
`YYYY-MM-DD.log` in that directory. The example is deliberately local-development-only;
it must not be committed with real credentials. Create the inbound directory and ensure all
configured storage roots are accessible to your runtime user. Search and podcast downloads are
disabled in this example; enabling either worker requires its dedicated storage root.

```bash
cp .env.example .env
# Edit .env. Generate each secret with: openssl rand -base64 48
# Server and CLI load .env automatically when run from this checkout.
cargo run --locked -p melodee-server
```

Startup validates the complete applied SQLx migration history and applies pending migrations
before opening the listener. For an existing database, follow the
[upgrade and maintenance-fencing procedure](docs/src/content/docs/installation/upgrading.md).
The server recognizes the repository's exact known comment-only variants of migrations 24–29 without
rewriting database metadata. Any other `VersionMismatch(<number>)` is unknown migration drift:
keep the database intact and follow the
[migration-checksum troubleshooting guidance](docs/src/content/docs/troubleshooting/#migration-checksum-mismatch).

In a second terminal at the repository root, create the first administrator. The password is
read from the environment rather than a command-line argument.

```bash
read -r -s -p 'Administrator password: ' MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
export MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
cargo run --locked -p melodee-cli -- users bootstrap-admin \
  --username admin --email admin@example.test
unset MELODEE__CLI__BOOTSTRAP_ADMIN_PASSWORD
```

Visit `http://127.0.0.1:8080/admin` and use the onboarding flow. It checks the live database,
storage roots, media tools, search configuration, administrator account, and email configuration.
Database and administrator checks block completion; storage, tools, search, and email checks are
advisory. Completing onboarding does not prove that every optional feature is configured.

For a guided fresh local setup, install the script's pinned Rich dependency and run the interactive
launcher instead of the manual PostgreSQL, Admin UI, server, and administrator commands above:

```bash
python3 -m pip install -r scripts/requirements.txt
python3 scripts/reset_and_run_server.py
```

The launcher requires a configured root `.env`, accepts only the fixed `compose.dev.yaml`
development database, and asks for confirmation before permanently recreating it. It also lists the
logging, inbound, staging, work, canonical-storage, search, podcast-storage, and JSON-configured
inbound roots explicitly set in `.env` and separately asks whether to empty them. If confirmed, every
file and subdirectory is permanently removed while each configured root directory is preserved.
Unsafe, linked, mounted-below, or overlapping targets fail closed before deletion. Back up anything
you need from those roots before accepting the wipe. The optional filesystem wipe runs before
the database is recreated.

The launcher builds the Admin UI and locked Rust executables, starts the server, securely prompts for
the first administrator's username, email, and password, and keeps the server attached until Ctrl+C.
Use `--yes` only when you intentionally want to skip the database-reset confirmation; it never skips
the separate directory-wipe or administrator prompts.

## Interfaces and clients

[MeloAmp](docs/src/content/docs/clients/meloamp.md) owns listener browsing, playback, playlists,
ratings, and recommendations. The server serves the Admin UI at `/admin`, APIs, and narrow
browser security flows such as `/reset-password`; it does not include a second listener SPA at
`/`. Jellyfin compatibility covers the declared music subset, not the full Jellyfin server.

| Surface               | Scope                                                                                                                  | Reference                                                     |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Native Melodee API v1 | First-party catalog, user, playback, sharing, request, podcast, and administration features                            | [Native API](docs/src/content/docs/api/melodee-api.md)        |
| OpenSubsonic          | Captured protocol routes, JSON/XML/JSONP forms, auth variants, direct streaming, and music-client behavior             | [OpenSubsonic API](docs/src/content/docs/api/opensubsonic.md) |
| Jellyfin              | Music-focused discovery, login, catalog, artwork, playlists, sessions, direct play, and configured transcoding        | [Jellyfin API](docs/src/content/docs/api/jellyfin.md)         |
| Admin UI              | Localized, accessible Carbon SPA for processing, catalog, identity, policy, access, settings, workers, and diagnostics | [Admin overview](docs/src/content/docs/admin/overview.md)     |

The three API adapters intentionally do not share wire DTOs. Compatibility means preserving
observable contracts, not retaining known security defects. Consult
[`compatibility/`](compatibility/) before changing any externally visible route, field, error,
identifier, or authentication behavior.

The [current migrated-client acceptance matrix](compatibility/clients/r4/README.md) records the
remaining release-candidate exercises. Historical client smoke tests are not a claim that every
current client/version has passed against this checkout.

## Development workflow

Use the fast gate for ordinary iteration:

```bash
cargo fmt --all
make check-fast
```

It validates repository metadata and requirement traceability, checks Rust formatting and
Clippy, and runs workspace library and binary unit tests. It deliberately omits PostgreSQL
integration, compatibility captures, the Admin UI static gate, and dependency auditing.

Run the full repository gate before merge, release, or changes that affect those omitted areas:

```bash
cargo fmt --all
make check
```

`make check` validates the repository and requirement index, runs Python contract tests, the admin
UI dependency audit and type/lint/i18n/unit/build gate when pnpm is available, Rust formatting
and Clippy, the full Rust test suite, and `cargo deny` when installed. CI supplies PostgreSQL
and media tools.
For release validation, install pnpm and cargo-deny rather than accepting local skips. Run
`make admin-ui-e2e` and `make docs-site-check` separately for browser/visual tests and the public
documentation audit and build. Both JavaScript audits include development dependencies.
Full-size benchmarks, fresh differential captures, real-client acceptance,
and operator rehearsals also require their documented explicit runs; `make check` does not
complete those release gates.

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
make check                               # full repository gate; requires PostgreSQL
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
[PRD](PRD.md), [SPEC](SPEC.md),
and [accepted ADRs](design/adr/README.md).

## Documentation, compatibility, and migration

The `melodee-cli legacy` command group provides inventory, planning, baseline, import, verification,
delta, freeze, validation, and status operations. It reads the legacy source without modifying it,
imports into a separate Rust target database, and adopts canonical media without mandatory
re-encoding. The source snapshot, public IDs, user state, filesystem assets, and explicit issue
dispositions must be verified before cutover. Start with the
[migration guide](docs/src/content/docs/installation/legacy-migration.md) and
[cutover/rollback runbook](design/runbooks/CUTOVER-AND-ROLLBACK.md). Production access, traffic
switching, rollback activation, and cleanup retain their documented operator approvals.

- Public documentation source lives in [`docs/`](docs/). The documentation workflow validates and
  builds it; GitHub Pages deployment is opt-in through `MELODEE_PAGES_ENABLED` repository
  configuration.
- Captured compatibility contracts, golden responses, media evidence, and client exercises live
  in [`compatibility/`](compatibility/). They are executable release evidence, not examples to
  hand-edit.
- Requirement traceability lives in [`design/requirements/`](design/requirements/). Historical
  phase completion reports were withdrawn from the repository on 2026-08-25 and remain
  recoverable from git history (see
  [Withdrawn evidence reports](design/adr/README.md#withdrawn-evidence-reports)).
- The legacy C# repository is valuable compatibility evidence, but its classes and service
  boundaries must not be copied into the Rust implementation.

For changes, follow [CONTRIBUTING.md](CONTRIBUTING.md) and keep user-facing documentation current.
See [CHANGELOG.md](CHANGELOG.md) for recorded changes. Report suspected vulnerabilities privately
as described in [SECURITY.md](SECURITY.md).

## License

Melodee is released under the [MIT License](LICENSE).
