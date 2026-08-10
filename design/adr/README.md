# Architecture Decision Records

ADRs capture durable decisions that affect architecture, product behavior, compatibility, storage,
security, dependencies, migration, or multiple modules.

## Why numbering starts at 0100

The existing C# repository already contains ADR-0001 through ADR-0007. This repository starts rewrite
ADRs at **ADR-0100** to avoid collision and preserve historical identity. See
[legacy/README.md](legacy/README.md).

## Status lifecycle

- **Proposed** — under review; not authority for irreversible implementation.
- **Accepted** — current decision and binding unless superseded.
- **Superseded** — replaced by a newer ADR; retain the file and link the replacement.
- **Deprecated** — no longer recommended, without a direct replacement.
- **Rejected** — considered and deliberately not selected; retained to avoid repeating the debate.

## When an ADR is required

Write an ADR when a decision is hard to reverse, likely to be debated, or affects:

- public API/client compatibility;
- domain language or identity;
- database/storage/search architecture;
- ingestion state or destructive behavior;
- security/authentication;
- deployment/infrastructure;
- production dependencies or FOSS/licensing policy;
- migration/cutover; or
- the UI design system.

## File naming

```text
ADR-0100-short-kebab-case-title.md
```

Use:

```bash
python3 scripts/new_adr.py "short title"
```

## Current Phase 4 decisions

- [ADR-0117: Use a PostgreSQL catalog, identity, and contribution schema with set-based manifest import](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [ADR-0118: Use AudioIdentity and immutable manifest v3 semantic overlays](ADR-0118-use-audioidentity-and-immutable-manifest-v3-semantic-overlays.md)
- [ADR-0119: Use audit-only corrections for legacy Releases without manifest v3](ADR-0119-use-audit-only-corrections-for-legacy-releases-without-manifest-v3.md)

## ADR quality rules

- State the decision, not merely the chosen technology.
- Explain alternatives and why they were rejected.
- Record consequences, including negative ones.
- Link PRD/SPEC requirement IDs and implementation evidence.
- Do not modify an accepted ADR to erase history. Supersede it.
- Proposed ADRs may contain open questions; accepted ADRs may not hide unresolved critical risks.
