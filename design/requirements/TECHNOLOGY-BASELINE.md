# Technology Baseline

**Audit date:** 2026-10-08 — stable registry releases and compatible transitive refresh
**Policy:** Candidate versions are revalidated and locked when first used. Required functionality must
remain fully FOSS.

| Area | Candidate/baseline | Version/status at audit | License | Decision |
|---|---|---|---|---|
| Language | Rust Edition 2024 | 1.99.0 | MIT/Apache-2.0 project licensing | Accepted |
| Async runtime | Tokio | 1.53.2 | MIT | Scaffolded |
| HTTP | Axum | 0.8.9 | MIT | Scaffolded |
| HTTP middleware | tower-http | 0.7.1 | MIT | Scaffolded |
| Outbound HTTP | reqwest | 0.13.5 | MIT/Apache-2.0 | Accepted — ADR-0123; rustls-only, bounded podcast-discovery HTTPS client |
| JWT/JWK verification | jsonwebtoken | 11.1.0 | MIT | Accepted — ADR-0124; RS256 Google ID-token verification using bounded HTTPS JWKS, exact issuer/audience/lifetime checks, and the already-required aws-lc-rs backend |
| XML feed parsing | quick-xml | 0.42.0 | MIT | Accepted — ADR-0123; bounded streaming RSS/Atom metadata parsing |
| CLI | clap | 4.6.7 | MIT/Apache-2.0 | Scaffolded |
| Serialization | Serde / serde_json | 1.0.229 / 1.0.151 | MIT/Apache-2.0 | Scaffolded |
| ZIP archives | zip | 8.6.0 | MIT | Accepted — ADR-0123; theme packs only, in-memory, Stored/DEFLATE codecs, hard entry/size/path bounds |
| Image processing | image-rs/image | 0.25.40 | MIT/Apache-2.0 | Accepted — ADR-0123; native artwork only, JPEG/PNG/WebP/GIF codecs, bounded input/pixels/dimensions/concurrency, deterministic output |
| Domain errors | thiserror | 2.0.21 | MIT/Apache-2.0 | Scaffolded |
| IDs | uuid | 1.27.0 | MIT/Apache-2.0 | Scaffolded |
| Filesystem primitives | rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Accepted — ADR-0116; safe Unix no-follow/no-replace publication boundary |
| Tracing | tracing / tracing-subscriber | 0.1.44 / 0.3.23 | MIT | Scaffolded |
| Database | PostgreSQL | 18.4; 17 supported target | PostgreSQL License | Accepted concept |
| Data access | SQLx | 0.9.0 stable line | MIT/Apache-2.0 | Proposed ADR-0110 |
| Search | Tantivy | 0.26.2 | MIT | Accepted ADR-0107 and ADR-0121; exact centralized dependency, revalidated 2026-08-11 |
| Metadata | Lofty | 0.25.4 | MIT/Apache-2.0 | Proposed |
| Media tools | FFmpeg/ffprobe | 8.1 stable line at PRD audit | LGPL/GPL | Proposed ADR-0108 |
| Fingerprint | Chromaprint/fpcalc | 1.6.1 at PRD audit | MIT library | Proposed ADR-0108 |
| Admin UI | React + Carbon | Carbon v11 / `@carbon/react` 1.x | MIT + Apache-2.0 | Proposed ADR-0109 |
| Dependency audit | cargo-deny | 0.20.2 at audit | MIT/Apache-2.0 | Required tooling |
| Documentation site | Astro + Starlight | astro 7.3.7, @astrojs/starlight 0.42.5 | MIT | Proposed ADR-0112 |
| Docs validation | @astrojs/check, starlight-links-validator | 0.9.10, 0.26.0 | MIT | Proposed ADR-0112 |
| Docs image pipeline | sharp | 0.35.5 | Apache-2.0 | Proposed ADR-0112 |
| Docs toolchain | Node.js + pnpm + TypeScript | Node 26.11.1, pnpm 12.10.1, TS 6.0.3 | MIT / Apache-2.0 | Proposed ADR-0112 |
| Docs search | Pagefind (via Starlight) | bundled by Starlight | MIT | Proposed ADR-0112 |

The 2026-10-08 refresh is recorded in [ADR-0179](../adr/ADR-0179-refresh-pinned-runtime-dependencies-while-preserving-execution-boundaries.md).
Direct Rust dependencies use exact workspace pins; Cargo and pnpm lockfiles record the newest
versions allowed by their upstream dependency ranges. DecentDB is pinned to 2.17.2 at commit
`b15ee763def5d696feb07d3ff29d467b7914a603`; Boa is 0.22.0 with its existing feature restrictions.
Rich is pinned to 15.0.0 for Python repository tooling. Reference CI/container builds use Node
26.11.1 and pnpm 12.10.1; the container SBOM tool uses Syft 1.54.1.

The Admin UI uses React 19.3.0, Carbon React 1.118.0, Vite 8.3.3, Vitest 5.0.3, Playwright
1.64.0, and TypeScript 7.0.2. Full direct version inventories remain in each package manifest.
The docs checker remains on TypeScript 6.0.3 because the latest Astro checker rejects 7.0.2;
the Admin UI lint-toolchain alias also retains the latest supported TypeScript 6 API.

The docs workspace narrowly overrides `postcss-selector-parser` below 7.1.6 to 7.1.6 because
Expressive Code still brings the vulnerable 6.x parser (GHSA-rj75-hqrm-r3gf). Earlier source-map-js,
js-yaml, and nanoid overrides are no longer needed after the upstream refresh. Both JavaScript
audits continue to include development dependencies and fail on advisories without suppression.
All refreshed direct dependencies retain their existing FOSS licenses and runtime boundaries.

The following paragraphs preserve prior feature-boundary revalidation evidence; the table and
committed manifests above record the current pins.

Reqwest 0.13.4 was revalidated against the crates.io package metadata on 2026-08-12. Melodee
disables its default features and enables only JSON, query encoding, streaming response bodies,
and rustls. Native TLS, system proxies, cookies, compression, HTTP/2, and HTTP/3 are not enabled.
Quick-xml 0.42.0 was revalidated against crates.io on 2026-08-26. It is used without optional
features and receives only response bodies already capped by the podcast outbound client.

zip 8.6.0 was revalidated against crates.io metadata on 2026-08-26. It is MIT licensed, requires
Rust 1.88 or newer, and the repository enables only the DEFLATE implementation needed for legacy
theme packs. AES, bzip2, deflate64, LZMA, PPMd, XZ, and Zstandard features remain disabled.

image 0.25.10 was revalidated against crates.io metadata on 2026-08-12. It is MIT/Apache-2.0
licensed. Default features are disabled; only JPEG, PNG, WebP, and GIF codecs required by the
native image contract are enabled. The server applies byte, pixel, dimension, and concurrency
bounds before decoding and never mutates canonical artwork.

jsonwebtoken 11.0.0 uses its `aws_lc_rs` backend instead of the broad RustCrypto feature set. This
reuses rustls' existing audited provider and excludes the RSA 0.9 dependency carrying
RUSTSEC-2023-0071. Reqwest's rustls platform verifier includes Mozilla-derived root-certificate data
under `CDLA-Permissive-2.0`; that OSI-compatible permissive license is explicitly allowlisted in
`deny.toml`. No certificate data, JWT key, or provider service is proprietary or hosted-only.

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
