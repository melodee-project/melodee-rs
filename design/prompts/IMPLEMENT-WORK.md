# Implement a substantive work program

Use this prompt when delegating a complete product outcome, compatibility domain, reliability
objective, or release-readiness program. Use `IMPLEMENT-ISSUE.md` only when the user explicitly
requests one narrowly bounded issue.

## Assignment

Implement the requested outcome end to end. Own the decomposition and ordering of all internal
workstreams; do not stop after the first prerequisite or passing slice when safe in-repository work
remains.

1. Read the applicable PRD requirements, `SPEC.md`, accepted ADRs, and current roadmap entries.
2. Resolve or clearly surface entry decisions that cannot be made from existing authority.
3. Inspect current Rust behavior and the C# oracle where compatibility is relevant.
4. Implement every in-scope requirement in coherent reviewable changes.
5. Add realistic unit, integration, differential, fault, performance, and end-to-end evidence as
   applicable.
6. Update public documentation, internal evidence, and requirement traceability in the same work.
7. Run the repository gates required by `AGENTS.md`.
8. Continue until the complete assigned outcome satisfies the Definition of Done or an external
   operator gate prevents further safe progress.

Do not perform production migration, cutover, rollback, publication, private-data access, external
credential use, or other operator-controlled actions without explicit authorization. Report those
as release-readiness gates rather than missing runtime features.

## Required closeout

Report files changed, requirements satisfied, decisions applied or proposed, tests and commands
run, compatibility impact, data and migration impact, documentation impact, external gates, and
remaining risks.
