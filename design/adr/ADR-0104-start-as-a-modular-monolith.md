# ADR-0104: Start as a modular monolith

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARCH-001, PRD-ARCH-002, PRD-ARCH-006

## Context

Melodee needs strong boundaries but does not need microservice operational complexity for a homelab
server processing hundreds of inbound Releases.

## Decision

Ship one primary server executable with compile-time module/crate boundaries. Durable worker state and
leases are process-safe so workers may be split later, but the initial deployment does not require a
service mesh, broker, or orchestrator.

## Consequences

- Simple installation and debugging.
- In-process calls remain available for common paths.
- Boundaries must be enforced by dependencies and tests, not network calls.
