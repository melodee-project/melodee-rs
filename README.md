# Melodee Rust Rewrite

This repository is the implementation home for the ground-up Rust replacement of the existing
[`melodee-project/melodee`](https://github.com/melodee-project/melodee) C# application.

The rewrite is **not** a line-by-line port. The legacy repository is the product-behavior oracle;
this repository defines the new architecture, specifications, tests, and implementation.

## Current phase

**Phase 3 — Media inspection, normalization, and manifests (complete)**

Phases 0–3 are complete. Phase 3 now publishes verified manifest v3 Release bundles through a
durable, retry-safe, non-destructive canonical-publication protocol. Phase 4 (catalog, Artist
identity, and contributions) is the next boundary, but schema work requires an assigned work
package and accepted schema ADR. See [IMPLEMENTATION.md](IMPLEMENTATION.md) and the
[Phase 3 report](design/evidence/phase-3/PHASE-3-REPORT.md).

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
6. [design/agents/DEFINITION-OF-DONE.md](design/agents/DEFINITION-OF-DONE.md) — completion and
   evidence requirements for every implementation slice.

## Repository structure

```text
apps/                       Deployable Rust binaries
  melodee-server/           Axum server, ingestion workers, and Admin diagnostics API
  melodee-cli/              Administrative and ingestion diagnostics CLI
crates/
  melodee-domain/           Framework-independent core vocabulary and invariants
  melodee-persistence/      PostgreSQL migrations, durable work items, and leases
  melodee-ingestion/        Reconciliation, inspection, normalization, and publication workers
  melodee-media/            Media observation, conversion, artwork, and fingerprinting
compatibility/              Captured external contracts and golden responses
design/
  adr/                      Architecture Decision Records
  agents/                   Agent work packages and Definition of Done
  architecture/             Architecture views and state machines
  requirements/             Parity and requirement traceability
  ui/                       Carbon UI direction and canonical mockups
tests/                       Compatibility, media, fault, and performance test homes
scripts/                     Repository validation and baseline-capture utilities
admin-ui/                    Reserved for the admin SPA in its implementation phase
migrations/                  Legacy placeholder; active SQLx migrations live with persistence
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

## Current implementation evidence

Completed phase reports live under `design/evidence/phase-0/` through `design/evidence/phase-3/`.
The phase plan, accepted ADRs, and an assigned work package determine what may be implemented next;
historical work packages remain as completed project evidence.

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
