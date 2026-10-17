# ADR-0181: Retire legacy custom content blocks without Rust replacement surface

- **Date:** 2026-10-17
- **Status:** Accepted
- **Decision owners:** Release owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MIG-004, PRD-MIG-005, MEL-4

## Context

The legacy C# `FileCustomBlockService` resolves and sanitizes active user-owned custom Markdown blocks from the Templates library at `custom-blocks/**/*.md`. Active placements include `dashboard.top`, `login.top`, `register.top`, and recovery-page blocks. These blocks are user-authored Markdown that the legacy service renders into specific UI locations.

The current Rust state has no custom-block type, importer, route, renderer, or migration issue. Active user-owned custom Markdown has neither a target representation nor an approved disposition.

The Rust rewrite rule prohibits recreating arbitrary server-side Blazor components (out of scope for MEL-4). The Admin UI is a Carbon-based SPA with no server-side Markdown rendering surface for custom blocks.

## Decision

All legacy `custom-blocks/**/*.md` assets are **retired**. No Rust replacement surface is created. The migration tooling records each discovered custom-block file as a `custom_block.retired` informational issue with the file path and placement, and the upgrade report identifies every source asset. Operators are notified that custom blocks are not carried forward to v1.0.0.

## Alternatives considered

### Alternative A: Implement a custom-block rendering surface in Rust

**Benefits:** Preserves user-authored custom Markdown placements.

**Costs:** Requires a new domain type, importer, route, renderer, sanitization pipeline, and Admin UI surface. Security risk from rendering unsanitized user Markdown in privileged admin surfaces. Contradicts the ground-up rewrite rule (no Blazor component recreation).

**Not selected:** The security and scope costs outweigh the preservation benefit. User-owned custom Markdown in admin surfaces is a security boundary that the Rust design explicitly avoids.

### Alternative B: Migrate custom blocks as static content assets

**Benefits:** Preserves the content without active rendering.

**Costs:** Custom blocks are placement-specific (dashboard.top, login.top, etc.) with no static asset equivalent in the Rust layout. Would require inventing a new content type with no consumer.

**Not selected:** No Rust surface consumes placement-specific custom blocks; migration would create orphaned content.

## Rationale

The legacy custom-block feature is a user-authored Markdown rendering surface with no equivalent in the Rust Admin UI. The rewrite rule explicitly excludes recreating Blazor components. The secure choice is to retire all custom blocks, record them as issues for operator awareness, and notify operators that custom placements are not carried forward. This matches the acceptance criterion that an Accepted ADR contains the affected-file inventory and retirement decision.

## Consequences

### Positive

- No security risk from rendering unsanitized user Markdown in admin surfaces.
- No orphaned content or broken placements after cutover.
- Clear operator notice of what is not carried forward.

### Negative / trade-offs

- Users lose custom dashboard/login/register/recovery-page block customizations.
- No replacement surface exists for equivalent functionality.

### Follow-up work

- The legacy migration tooling records each discovered `custom-blocks/**/*.md` file as a `custom_block.retired` issue with file path and placement.
- The upgrade report identifies every source asset.
- Operators are notified through the migration report and cutover documentation.

## Validation

- The ADR itself is the evidence of the retirement decision (satisfies MEL-4 acceptance criterion: "an Accepted ADR contains the affected-file inventory and retirement decision").
- The migration tooling's issue inventory (`custom_block.retired` codes) provides the per-asset inventory.
- No Rust code changes are required; no tests are needed for a retired feature.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0148: Legacy migration contract](ADR-0148-legacy-migration-contract.md)
- [MEL-4 work item](https://hamstik.com/sph/MEL/work/Melodee/MEL-4)
