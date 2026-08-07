# ADR-0100: Build Melodee as a ground-up Rust reimplementation

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-PRINCIPLE-001, PRD-ARCH-001

## Context

The existing C# Melodee product contains years of valuable product behavior but also carries
filesystem-sweep ingestion, broad service responsibilities, dependency churn, and architecture that
should not be reproduced. The goal is a durable, high-performance replacement, not a language port.

## Decision

Build a new Rust repository and implementation. Use the C# repository as a behavioral/product oracle
and source of fixtures, tests, contracts, and migration data. Do not translate legacy classes,
services, repositories, or dependency structures line-for-line.

## Alternatives considered

### Patch/refactor C# in place

Rejected by product direction. It would keep the rewrite constrained by existing architecture and does
not meet the owner's decision to start over.

### Automated class-by-class translation

Rejected because it would create `Melodee.CSharp.rs` and preserve tactical decisions the rewrite is
intended to replace.

## Rationale

A clean repository gives Rust-native architecture, dependency restraint, durable work modeling, and
clear compatibility adapters without destabilizing the production C# system during development.

## Consequences

### Positive

- Legacy production remains available as the oracle and rollback platform.
- New architecture can be designed around Release work items and explicit identities.
- Agents receive clear product/specification boundaries.

### Negative / trade-offs

- Feature parity is a substantial multi-phase program.
- Compatibility and migration work must be explicit and automated.
- Duplicate implementations coexist during development.

### Follow-up work

- Complete Phase 0 compatibility capture.
- Maintain a living parity matrix.
- Build repeatable migration and rollback tooling.
