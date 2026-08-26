# Melodee Rust Rewrite — Copilot Instructions

Read `AGENTS.md` before editing anything. The following rules are non-negotiable:

- The existing C# repository defines product behavior, not the Rust architecture.
- Do not mechanically translate C# classes, services, repositories, plugins, or jobs.
- Use `Release` and `Track` in the Rust core. `Album` and `Song` exist only in compatibility DTOs.
- Preserve native Melodee API v1, OpenSubsonic, and Jellyfin compatibility behavior through explicit
  adapters and executable contract tests.
- Do not change a published contract without captured legacy evidence and an accepted decision.
- Follow accepted ADRs. Create a proposed ADR before introducing a hard-to-reverse decision.
- Required dependencies must be active, modern, fully FOSS, self-hostable, and justified.
- Do not introduce Redis, RabbitMQ/Kafka, Elasticsearch/OpenSearch, GraphQL, Next.js, Kubernetes,
  a graph database, a proprietary UI/grid, or another component library without an accepted ADR.
- Keep the domain crate independent of Axum, Tokio, SQLx, filesystems, FFmpeg, and protocol DTOs.
- Production Rust forbids unsafe code, unwrap, expect, panic, todo, and unimplemented.
- Add tests and traceability for every behavior change.
- Do not claim completion until the applicable completion criteria and Definition of Done are met.
