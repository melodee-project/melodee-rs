# Technology Baseline

**Audit date:** 2026-08-07; updated 2026-08-10 for the Phase 3 publication dependency
**Policy:** Candidate versions are revalidated and locked when first used. Required functionality must
remain fully FOSS.

| Area | Candidate/baseline | Version/status at audit | License | Decision |
|---|---|---|---|---|
| Language | Rust Edition 2024 | 1.97.1 | MIT/Apache-2.0 project licensing | Accepted |
| Async runtime | Tokio | 1.52.3 | MIT | Scaffolded |
| HTTP | Axum | 0.8.9 | MIT | Scaffolded |
| HTTP middleware | tower-http | 0.7.0 | MIT | Scaffolded |
| CLI | clap | 4.6.4 | MIT/Apache-2.0 | Scaffolded |
| Serialization | Serde / serde_json | 1.0.229 / 1.0.150 | MIT/Apache-2.0 | Scaffolded |
| Domain errors | thiserror | 2.0.19 | MIT/Apache-2.0 | Scaffolded |
| IDs | uuid | 1.24.0 | MIT/Apache-2.0 | Scaffolded |
| Filesystem primitives | rustix | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Accepted — ADR-0116; safe Unix no-follow/no-replace publication boundary |
| Tracing | tracing / tracing-subscriber | 0.1.44 / 0.3.23 | MIT | Scaffolded |
| Database | PostgreSQL | 18.4; 17 supported target | PostgreSQL License | Accepted concept |
| Data access | SQLx | 0.9.0 stable line | MIT/Apache-2.0 | Proposed ADR-0110 |
| Search | Tantivy | 0.26.1 | MIT | Proposed ADR-0107 |
| Metadata | Lofty | 0.24.0 | MIT/Apache-2.0 | Proposed |
| Media tools | FFmpeg/ffprobe | 8.1 stable line at PRD audit | LGPL/GPL | Proposed ADR-0108 |
| Fingerprint | Chromaprint/fpcalc | 1.6.1 at PRD audit | MIT library | Proposed ADR-0108 |
| Admin UI | React + Carbon | Carbon v11 / `@carbon/react` 1.x | MIT + Apache-2.0 | Proposed ADR-0109 |
| Dependency audit | cargo-deny | 0.20.2 at audit | MIT/Apache-2.0 | Required tooling |
| Documentation site | Astro + Starlight | astro 7.2.0, @astrojs/starlight 0.41.7 | MIT | Proposed ADR-0112 |
| Docs validation | @astrojs/check, starlight-links-validator | 0.9.10, 0.25.2 | MIT | Proposed ADR-0112 |
| Docs image pipeline | sharp | 0.35.3 | Apache-2.0 | Proposed ADR-0112 |
| Docs toolchain | Node.js + pnpm + TypeScript | Node 22.12+, pnpm 11.20.0, TS 6.0.3 | MIT / Apache-2.0 | Proposed ADR-0112 |
| Docs search | Pagefind (via Starlight) | bundled by Starlight | MIT | Proposed ADR-0112 |

Documentation-site versions were verified against the npm registry on 2026-08-07 (all packages
active, current stable line, licenses as listed, no paid/open-core requirement for any feature
Melodee uses). The committed `docs/pnpm-lock.yaml` is the authoritative lock.

## Dependency approval checklist

- Repository active and not archived.
- Current stable release exists.
- Required feature set is FOSS.
- License compatible with distribution.
- No required proprietary cloud/service.
- Simpler existing solution considered.
- Exact version captured by lockfile.
- Upgrade/exit strategy documented.

## Explicit non-baseline infrastructure

RabbitMQ, Kafka, Redis, Elasticsearch/OpenSearch, GraphQL, Next.js, Kubernetes, graph databases, and
proprietary UI grids are not required initially.
