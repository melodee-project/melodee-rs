# Contributing

## Before opening a change

- Read `AGENTS.md`, `PRD.md`, `SPEC.md`, and the relevant ADRs.
- Work from a requirement ID or an issue that links to one.
- Keep changes small enough to review and revert safely.
- Do not mix architecture decisions, broad refactors, and feature work in one PR.

## Branches and commits

Use descriptive branches such as:

```text
phase0/capture-native-openapi
phase0/compatibility-harness
phase1/server-bootstrap
```

Use conventional commits where practical:

```text
feat(domain): add validated ingestion transition model
fix(compat): preserve omitted nullable field behavior
build(ci): enforce FOSS dependency policy
adr: accept PostgreSQL work-item queue decision
```

## Pull requests

Every PR should identify:

- requirement IDs;
- ADRs applied or proposed;
- compatibility impact;
- migration/data impact;
- security impact;
- tests and benchmarks executed; and
- follow-up work intentionally left out.

## Quality gates

Run:

```bash
./scripts/check.sh
```

The first dependency-resolving change must generate and commit `Cargo.lock`.

## Architecture decisions

Use:

```bash
python3 scripts/new-adr.py "short descriptive title"
```

A proposed ADR can accompany implementation exploration, but irreversible implementation must wait
for acceptance.
