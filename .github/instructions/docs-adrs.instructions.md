---
applyTo: "**/*.md,design/adr/**"
---

# Documentation and ADR instructions

- `PRD.md` defines product scope; `SPEC.md` defines system invariants; `IMPLEMENTATION.md` defines
  sequencing; accepted ADRs define durable decisions.
- Significant, cross-cutting, hard-to-reverse, or likely-to-be-debated decisions require an ADR.
- Never delete historical ADRs. Supersede them with a new ADR.
- Use `python3 scripts/new_adr.py "Title"` to allocate the next number.
- Requirements use stable PRD IDs and must remain traceable to implementation and tests.
- Write concrete acceptance criteria and distinguish current fact, proposal, and future option.
- Mermaid diagrams are preferred for architecture/state/data-flow diagrams.
- Do not present an AI-generated mockup as exact behavior; written specifications take precedence.
