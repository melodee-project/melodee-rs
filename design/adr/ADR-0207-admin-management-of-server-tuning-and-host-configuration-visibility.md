# ADR-0207: Admin management of server tuning and host configuration visibility

- **Date:** 2026-11-24
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Related requirements:** PRD-CONFIG-001, PRD-CONFIG-002, PRD-CONFIG-003, PRD-AUTH-007

## Context

Container deployments set worker limits in the service environment. The registry export omitted
these limits, so an administrator monitoring Release throughput had to inspect the host.

## Decision

Expose the registered settings and an explicit inventory of server startup controls through
the administrator API. Store an administrator's individual edit in PostgreSQL. For an editable
registry key this explicit override takes priority over the ordinary environment, file, stored
value, and default layers. Existing stored rows retain their precedence until explicitly edited.
For an editable startup control, load and validate the overrides after migrations and before
constructing workers. A running process keeps its own snapshot. Show running and next-start
values together when they differ. Report sources and redact secrets.

Host configuration includes database bootstrap, signing material, trust boundaries, filesystem
roots, and media executable paths. The API enforces host ownership on individual and bulk edits.
Registry import/export keeps its existing document format. Recovery and cloning of startup
overrides are covered by the installation database backup.

## Alternatives and consequences

Writing service environment files would require host-level write privileges and deployment-specific
restart machinery. Hot-reloading worker pools would require coordinating outstanding claims,
media processes, and in-flight NAS writes. Loading controls at startup keeps those claims under
the existing worker lifecycle. Multi-process installations restart each instance to pick up a change.

Migration 0051 adds an empty override table, a false-by-default preference flag on existing
settings, and time indexes for throughput queries. For a forward fix, retain the additive schema and redeploy a compatible binary.
For a full rollback, restore the pre-upgrade database backup together with the matching server binary.

## Validation

Admin API tests cover authorization, typed edits, rollback on validation failure, host-only
settings, secret masking, worker consumption, and reset. UI tests distinguish saved changes from
the running value. Stats tests exercise receipt and import counts using PostgreSQL.

## References

- [Product requirements](../../PRD.md)
- [Admin settings](../../docs/src/content/docs/admin/settings.md)
