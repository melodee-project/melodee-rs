# ADR-0111: Do not require a message broker or Redis initially

- **Date:** 2026-08-07
- **Status:** Proposed
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARCH-006, PRD-WORK-001

## Context

Melodee needs durable work and process-safe concurrency, but a separate broker/cache service adds
operational burden and does not make file/database operations idempotent.

## Decision

Proposed: use PostgreSQL work rows, leases, and optional LISTEN/NOTIFY wakeups. Do not require
RabbitMQ, Kafka, Redis, or another broker/cache for the parity release. Revisit only with measured
requirements.

## Consequences

- PostgreSQL carries queue load and needs appropriate indexes/retention.
- The deployment stays simple.
- Worker interfaces must still support later process separation.
