# Contributing

## Before opening a change

- Read `AGENTS.md`, `PRD.md`, `SPEC.md`, and the relevant ADRs.
- Work from a requirement ID or an issue that links to one.
- Keep changes small enough to review and revert safely.
- Do not mix architecture decisions, broad refactors, and feature work in one PR.

## Branches and commits

Use descriptive branches such as:

```text
compat/capture-native-openapi
compat/differential-harness
server/bootstrap
```

Use conventional commits where practical:

```text
feat(domain): add validated ingestion transition model
fix(compat): preserve omitted nullable field behavior
build(ci): enforce FOSS dependency policy
adr: accept PostgreSQL work-item queue decision
```

## Pull requests

Every PR should identify:

- requirement IDs;
- ADRs applied or proposed;
- compatibility impact;
- migration/data impact;
- security impact;
- tests and benchmarks executed; and
- follow-up work intentionally left out.

## Quality gates

Before committing or opening a PR, run:

```bash
python3 scripts/do-prechecks.py
```

`make prechecks` is the same entry point. The script checks unstaged and staged Git diffs,
runs the full `make check` gate, and validates/builds the documentation site. It streams
output, reports timings, and stops at the first failure. Use `make check-fast` while iterating
and `make check` before opening a PR. `make check` is the pre-merge tier: repository, Python and
Admin UI gates plus a full fmt/clippy workspace compile, with no Rust test execution. **All Rust
test suites are the nightly/release tier**: `make check-nightly` runs them, the scheduled CI
workflow runs them (and pushes to `main`), and `make test-migrations` runs just the historical
migration compatibility tests. It requires pnpm and cargo-deny
instead of accepting the full gate's local tool skips. Install the pinned Rust toolchain,
Node.js/pnpm, cargo-deny, FFmpeg/ffprobe, and fpcalc first, and prepare the development/test
PostgreSQL instance as described in [the source-build guide](docs/src/content/docs/installation/build-from-source.md).
The docs install also fetches the Node.js runtime pinned in `docs/package.json`. Astro uses
that runtime even when your shell has an older Node.js version. To rerun just the docs
checks, use `python3 scripts/check.py docs`. Dependency installs and audits require network access.

For Admin UI changes, run `python3 scripts/do-prechecks.py --include-e2e` to also run
Playwright end-to-end/visual tests; install its Chromium browser first. Prechecks validate
the working checkout, not an isolated snapshot of the staged files, and do not install Git
hooks. Run `cargo fmt --all` to fix formatting before retrying a formatting failure.
Release benchmarks, fresh compatibility captures, real-client acceptance, and authorized
operator rehearsals remain separate gates.

The first dependency-resolving change must generate and commit `Cargo.lock`.

## Architecture decisions

Use:

```bash
python3 scripts/new_adr.py "short descriptive title"
```

A proposed ADR can accompany implementation exploration, but irreversible implementation must wait
for acceptance.

## Maintainer release flow

Choose the next version and prepare the release commit using the [Melodee versioning policy](VERSIONING.md).

Official deployable releases are GHCR OCI images, not native binary assets. Before publishing a
GitHub Release, ensure the exact release commit is on `main`, its Cargo server/CLI version matches
the intended SemVer tag, and the full gate is green. Publish the GitHub Release with release notes;
do not upload executables, installers, binary archives, or operating-system packages.

The `Publish release container` workflow then validates the tag, runs the full quality gate,
builds/tests/scans the production image, and publishes the amd64/arm64 manifest to
`ghcr.io/melodee-project/melodee`. Stable releases receive `X.Y.Z`, `X.Y`, and `latest`; a GitHub
prerelease receives only its exact prerelease tag. Verify the manifest, GHCR package visibility,
image attestation, and an actual deployment after the workflow succeeds. The detailed operator
flow is in the [container deployment guide](docs/src/content/docs/installation/docker.md).
