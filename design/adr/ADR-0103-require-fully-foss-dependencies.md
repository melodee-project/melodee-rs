# ADR-0103: Require fully FOSS dependencies for required functionality

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-PRINCIPLE-007, PRD-SEC-007

## Context

The rewrite is partly motivated by dependency churn and must not introduce required features behind
commercial UI tiers, proprietary services, or abandoned projects.

## Decision

A required runtime/build dependency must be actively maintained, stable enough for its role, and
licensed under a recognized FOSS license. Every capability Melodee relies on must be available in the
FOSS project; commercial support is acceptable, required proprietary features are not.

CI uses `cargo-deny` and lockfiles. New dependencies require documented justification and license/
maintenance review.

## Consequences

- Some convenient open-core products are excluded.
- Dependency choices take more up-front verification.
- Melodee remains self-hostable and auditable without license surprises.
