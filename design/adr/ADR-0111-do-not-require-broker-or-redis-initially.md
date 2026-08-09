# ADR-0111: Do not require a message broker or Redis initially

- **Date:** 2026-08-07
- **Status:** Accepted
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

## Phase 0 ratification (2026-08-09)

- **Status change:** Proposed -> Accepted (Issue 10 ratification authority).
- **PRD support:** PRD-ARCH-006: "The parity release shall not require RabbitMQ, Kafka, Redis,
  Elasticsearch, OpenSearch, or Kubernetes unless an ADR with measured evidence demonstrates a
  problem the simpler architecture cannot solve." PRD-WORK-001 mandates durable work queues
  over the Quartz mental model.
- **Phase 0 evidence:** the entire legacy capture/benchmark environment ran brokerless; the
  legacy pipeline's non-transactional failure modes (media-baseline.md section 11) are
  precisely what PostgreSQL work rows + leases fix. No measured need for a broker exists.
- No new dependency; consequences documented above.
