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
python3 scripts/new-adr.py "short title"
```

## ADR quality rules

- State the decision, not merely the chosen technology.
- Explain alternatives and why they were rejected.
- Record consequences, including negative ones.
- Link PRD/SPEC requirement IDs and implementation evidence.
- Do not modify an accepted ADR to erase history. Supersede it.
- Proposed ADRs may contain open questions; accepted ADRs may not hide unresolved critical risks.
