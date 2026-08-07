# ADR-0102: Preserve public APIs as separate compatibility adapters

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Melodee project owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-PRINCIPLE-002 through PRD-PRINCIPLE-004, PRD-API-M1-001,
  PRD-API-OS-001, PRD-API-JF-001

## Context

Melodee currently exposes a performant first-party Melodee API v1, OpenSubsonic compatibility, and a
music-focused Jellyfin compatibility subset. They serve different client ecosystems and use different
authentication, routing, DTO, and error conventions.

## Decision

Implement each surface in a separate adapter/module with protocol-owned DTOs and tests. Do not merge
them into a generic API model. The new admin API is independently versioned and may expose the new
Rust domain vocabulary.

## Consequences

- Some mapping code is intentionally duplicated across adapters.
- Compatibility can be tested and released independently.
- Internal refactors do not require client contract changes.
