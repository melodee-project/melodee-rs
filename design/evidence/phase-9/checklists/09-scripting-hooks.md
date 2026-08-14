# Checklist 09 — Scripting hooks and migration path (WP-11)

| Capability | Requirements | Status | Evidence |
|---|---|---|---|
| Script event/hook inventory from legacy source | PRD-SCRIPT-002 | ✅ | `design/evidence/phase-9/script-events-legacy-inventory.md` (frozen-tree inventory, file:line citations) |
| FOSS JS runtime via ADR + security review | PRD-SCRIPT-003 | ✅ (decision) | ADR-0135 (Boa); executor landed in WP-11: `boa_engine =0.21.1` (Unlicense OR MIT, pure Rust, default features trimmed), `cargo deny check` green |
| Sandboxed/constrained execution | PRD-SCRIPT-004 | ✅ | `apps/melodee-server/src/scripting.rs` (capability-less Boa sandbox, dedicated-thread wall-clock timeout, leaked-thread bound, ctx/return caps, circuit breaker with transactional trip+disable); unit tests prove absent fs/net/process globals, timeout, throw, oversize return; `apps/melodee-server/tests/scripting_service.rs` (breaker trip, fail-degraded gate) |
| Hook compatibility or documented migration path | PRD-SCRIPT-001/005 | ✅ | `design/evidence/phase-9/script-hook-migration-guide.md` (per-hook dispositions); `directoryProcessingStart` wired at the pre-inspection gate (`crates/melodee-ingestion/src/gate.rs`, `inspect.rs`; stub-gate test in `crates/melodee-ingestion/tests/inspection.rs`); legacy example script parity test; admin `/api/admin/v1/scripts/*` + `melodee scripts list/test`; migration guide §2b documents the shell-hook re-expression paths |
| Fail-degraded semantics | PRD-SCRIPT-004, PRD-PRINCIPLE-006 | ✅ | Fault-injection tests: infinite loop times out without blocking the caller, throwing script is an audited error + allow, oversize ctx/return rejected, breaker disables after 3 consecutive timeouts, store-gone gate still allows (`apps/melodee-server/tests/scripting_service.rs`, `scripting.rs` unit tests) |

Notes:

- Only `directoryProcessingStart` is wired (the one server-side enforcement hook legacy actually
  enforced). The six legacy Blazor UI-gate events are accepted in the registry but inert (marked
  `wired`/`inert` in the admin API); their server-side enforcement points are future work per the
  migration guide dispositions. `directoryProcessingDelete` and `shareCreateStart` are deliberately
  not recreated (do-not-preserve, inventory §5).
- `onDeny` is fixed to `skip` in this slice: physical quarantine moves are deferred (they require
  the durable Phase 5 decision/audit/authorization contract) and legacy `delete` stays rewritten
  to `skip`. Recorded in the migration guide §2a/§4.
- `hasTrackNumberGaps` uses the shipped legacy definition (any-start contiguous sequences are
  gap-free; duplicates are gaps) — resolves migration-guide uncertainty #1.
