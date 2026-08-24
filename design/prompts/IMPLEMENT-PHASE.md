# Implement an Entire Melodee Phase

Use this template when the user asks for the next coding-agent assignment, the next phase, or a
phase-level outcome. Replace the bracketed placeholders from the governing roadmap. Do not reduce
the assignment to the first issue or workstream.

## Assignment

Own and complete **[phase number and name]** end to end in the Melodee Rust rewrite. The objective
is **[phase outcome]**. Continue until every phase exit criterion is satisfied and evidenced; do
not stop after planning, a prerequisite, the first green work package, or the first mergeable
slice.

## Governing scope

- Read `AGENTS.md`, `PRD.md`, `SPEC.md`, `IMPLEMENTATION.md`, the phase plan/work package,
  `design/agents/DEFINITION-OF-DONE.md`, linked accepted ADRs, and applicable prior-phase reports.
- Identify the phase's requirement IDs, entry conditions, internal workstreams, dependencies, and
  exit criteria before editing.
- Treat issues, waves, and work packages as your internal execution plan. Order them yourself and
  work through all of them without requiring a new user prompt for each.
- Include all necessary code, migrations, tests, compatibility evidence, observability,
  operational behavior, public documentation, traceability, and phase-closeout evidence.
- Preserve unrelated worktree changes and never broaden the phase beyond its product and safety
  boundaries.

## Execution model

1. Establish the current baseline and reconcile safe documentation or test-harness drift as the
   entry wave instead of returning it as a separate assignment.
2. Produce a concise dependency-aware plan for the complete phase, then execute it. Planning is
   not the deliverable.
3. Implement the phase in coherent, reviewable slices. Run focused tests during each slice and
   keep already-completed behavior green.
4. Resolve ordinary implementation choices from the authority hierarchy and repository evidence.
   Draft required ADRs when directed by repository policy; do not invent product decisions.
5. Continue through integration, fault, compatibility, performance, security, documentation, and
   operational validation appropriate to the phase.
6. Run `make check-fast` during iteration and `make check` before declaring the phase complete,
   plus every phase-specific gate.
7. Update requirement status, parity records, public documentation, and the phase report only from
   reproducible evidence.

## Pause conditions

Pause and request direction only when a material conflict cannot be resolved from the authority
hierarchy, a decision is explicitly reserved for product/operator approval, a destructive or
external action needs authorization, or an external dependency makes all remaining safe phase
work impossible. A difficult task, a large diff, an intermediate test failure, or completion of
one workstream is not a reason to return control.

## Completion report

Report:

- phase objective and exit decision;
- files and major workstreams completed;
- requirements satisfied and traceability updates;
- ADRs applied, proposed, or requiring approval;
- tests and validation commands with results;
- compatibility and real-client impact;
- data, migration, rollback, and operational impact;
- security, dependency, and performance impact;
- documentation impact using the exact repository-required wording; and
- remaining risks or externally blocked items.

Do not claim phase completion while any in-scope requirement or exit criterion lacks an
implemented, tested, deferred, waived, or otherwise explicitly authorized terminal disposition.
