# Checklist 09 — Scripting hooks and migration path (WP-11)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Script event/hook inventory from legacy source | PRD-SCRIPT-002 | ⬜ | WP-11: evidence document first |
| FOSS JS runtime via ADR + security review | PRD-SCRIPT-003 | ✅ (decision) | ADR-0135 (Boa); executor lands in WP-11 |
| Sandboxed/constrained execution | PRD-SCRIPT-004 | ⬜ | WP-11: capability-less sandbox, timeout, circuit breaker, `script_runs` audit |
| Hook compatibility or documented migration path | PRD-SCRIPT-001/005 | ⬜ | WP-11: per-hook migration guide |
| Fail-degraded semantics | PRD-SCRIPT-004, PRD-PRINCIPLE-006 | ⬜ | WP-11: fault-injection tests (timeout/throw/oversize) |
