# ADR-0192: Persist installation-scoped Jellyfin server identity

- **Date:** 2026-11-13
- **Status:** Accepted
- **Decision owners:** User-directed review remediation program
- **Supersedes:** The fixed production Jellyfin ServerId implementation; other Jellyfin contracts remain unchanged
- **Superseded by:** None
- **Related requirements:** PRD-API-JF-001, PRD-API-JF-002, PRD-DEPLOY-001, PRD-MIG-002

## Context

Every installation previously advertised `93cd3cc3a120127ccfda124620290cae`. Clients that index
accounts or caches by ServerId cannot distinguish independent Melodee installations. Existing
Jellyfin discovery/login tests capture that fixture identity. The user authorized fixing this review
finding with a stable per-installation identity and explicit migration semantics.

## Decision

Migration 0046 creates a singleton installation identity, initialized once with PostgreSQL UUIDv7.
The production composition root loads that identity and exposes its 32-digit hexadecimal form in all
Jellyfin ServerId fields. Missing identity fails startup rather than inventing a new ID on each boot.
No catalog/public User, Artist, Release or Track ID is changed. Fixture states may retain their
explicit historical ServerId so unrelated golden compatibility contracts remain independently tested.

## Upgrade, restore, clone and rollback

An upgraded pre-release installation receives its own ID once. Jellyfin clients may need to remove
and re-add the server and refresh local caches; operators should record existing client settings first.
All clients of the installation see the same ID after restart and subsequent upgrades.
A full database backup/restore preserves identity and represents the same installation. An independently
exposed clone must deliberately rotate the singleton identity before serving clients, as documented
in the Jellyfin API guide; no startup code automatically guesses whether a restore is a clone.
The additive table is retained on binary downgrade. An older binary reintroduces the fixed ID and
may require client reconnection again. There is no destructive down migration.

## Alternatives considered

A random ID on every startup invalidates client state. A hostname-derived ID changes with deployment
configuration and collides under shared hostnames. A database singleton follows the authoritative
application-state backup and has neither weakness.

## Consequences

Independent installations have distinct identities, and backup continuity is explicit. Existing
pre-release client caches are reset once. Client acceptance against the qualified candidate remains
required; integration tests alone do not qualify every real Jellyfin client.

## Validation

Database tests verify distinct fresh installations, stable repeated reads, and singleton constraints.
Existing adapter tests cover discovery/login ServerId consistency. The production composition root
uses the persisted value instead of the frozen fixture value.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
