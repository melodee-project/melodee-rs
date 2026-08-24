# Implement a Traced Issue

Use this template only when the user explicitly assigns one issue, bug, or narrowly bounded work
package. Do not use it when the user asks for the next phase or for the next coding-agent assignment
without specifying task-level granularity; use `design/prompts/IMPLEMENT-PHASE.md` in that case.

Use the issue's PRD requirement IDs and phase. Read the authority hierarchy in `AGENTS.md`. Inspect the
legacy system only to determine product behavior. Produce the smallest tested change meeting acceptance
criteria. Add or update ADRs before hard-to-reverse decisions. Report commands, tests, compatibility,
performance, security, migration, and dependency impacts. Do not declare done until the Definition of
Done is satisfied.
