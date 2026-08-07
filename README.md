# Melodee Rust Rewrite

This repository is the implementation home for the ground-up Rust replacement of the existing
[`melodee-project/melodee`](https://github.com/melodee-project/melodee) C# application.

The rewrite is **not** a line-by-line port. The legacy repository is the product-behavior oracle;
this repository defines the new architecture, specifications, tests, and implementation.

## Current phase

**Phase 0 — Baseline freeze and compatibility capture**

The first work is to capture the observable behavior of the existing server, establish measurable
performance baselines, and ratify the foundational ADRs. Do not start broad feature implementation
until the Phase 0 exit criteria in [IMPLEMENTATION.md](IMPLEMENTATION.md) are satisfied.

## Non-negotiable product commitments

- Preserve the native **Melodee API v1** as a core first-party API surface.
- Preserve the implemented **OpenSubsonic** compatibility surface.
- Preserve the implemented music-focused **Jellyfin compatibility** surface.
- Preserve existing public IDs, users, playlists, ratings, scrobbles, requests, shares, and other
  durable user state during migration.
- Replace filesystem-sweep ingestion with durable, per-Release work items.
- Use `Release` and `Track` in the Rust core; compatibility adapters may continue to expose
  `Album` and `Song` where existing contracts require them.
- Use only modern, active, fully FOSS dependencies for required functionality.
- Record significant decisions as ADRs.

## Read these first

1. [AGENTS.md](AGENTS.md) — mandatory instructions for coding agents.
2. [PRD.md](PRD.md) — product requirements and complete parity scope.
3. [SPEC.md](SPEC.md) — technical system specification and invariants.
4. [IMPLEMENTATION.md](IMPLEMENTATION.md) — phased delivery plan and exit criteria.
5. [design/adr/README.md](design/adr/README.md) — ADR process and current decisions.
6. [design/agents/PHASE-0-WORK-PACKAGE.md](design/agents/PHASE-0-WORK-PACKAGE.md) — the first
   executable work package.

## Repository structure

```text
apps/                       Deployable Rust binaries
  melodee-server/           Axum server bootstrap
  melodee-cli/              Administrative CLI bootstrap
crates/
  melodee-domain/           Framework-independent core vocabulary and invariants
compatibility/              Captured external contracts and golden responses
design/
  adr/                      Architecture Decision Records
  agents/                   Agent work packages and Definition of Done
  architecture/             Architecture views and state machines
  requirements/             Parity and requirement traceability
  ui/                       Carbon UI direction and canonical mockups
tests/                       Compatibility, media, fault, and performance test homes
scripts/                     Repository validation and baseline-capture utilities
admin-ui/                    Reserved for the Carbon React admin SPA in its implementation phase
migrations/                  Reserved for SQLx/PostgreSQL migrations after schema ADR acceptance
```

## Local bootstrap

Prerequisites:

- Rust toolchain declared in `rust-toolchain.toml`
- Python 3.11+
- Docker Engine/Moby with Docker Compose support
- FFmpeg and ffprobe
- Chromaprint `fpcalc` when fingerprint work begins

```bash
cp .env.example .env
docker compose -f compose.dev.yaml up -d postgres
./scripts/bootstrap.sh
```

Run the standard checks:

```bash
./scripts/check.sh
```

The first successful Cargo resolution must produce `Cargo.lock`; commit it in the same change.

## First agent assignment

Start with [design/agents/PHASE-0-WORK-PACKAGE.md](design/agents/PHASE-0-WORK-PACKAGE.md).
The agent should complete one small issue at a time from
[design/agents/FIRST-10-ISSUES.md](design/agents/FIRST-10-ISSUES.md), with a focused PR and evidence
for every acceptance criterion.

## Legacy repository relationship

A recommended local workspace is:

```text
workspace/
  melodee/       # existing C# product baseline, read-only except for test-oracle helpers
  melodee-rs/    # this repository
```

Do not copy legacy classes or service boundaries into Rust. Extract behavior, contracts, fixtures,
and acceptance tests instead.

## License

MIT. See [LICENSE](LICENSE).
