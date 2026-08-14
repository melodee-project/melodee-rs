# ADR-0135: Boa JavaScript runtime and sandboxing for scripting hooks

- **Date:** 2026-08-13
- **Status:** Accepted
- **Decision owners:** Phase 9 work package
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SCRIPT-001, PRD-SCRIPT-002, PRD-SCRIPT-003, PRD-SCRIPT-004,
  PRD-SCRIPT-005, PRD-ARCH-006, PRD-SEC-001

## Context

The legacy server exposes JavaScript event scripting hooks (PRD-SCRIPT-001). PRD-SCRIPT-003
requires a FOSS JS runtime chosen by ADR with a security review; PRD-SCRIPT-004 requires
sandboxed/constrained execution; PRD-SCRIPT-002 requires an inventory of script
events/inputs/returns/side effects before implementation. Scripts are installed by the server
operator (administrator trust boundary); there is no user-submitted script surface.

Constraints from the architecture rules: fully FOSS (ADR-0103), no new infrastructure service
(ADR-0111), `unsafe` forbidden in our code (dependency-internal unsafe is the dependency's
concern), exact pinned versions, and the dependency must pass `cargo deny`.

## Decision

1. **Runtime: `boa_engine`** (Boa), a pure-Rust ECMAScript engine, evaluated against
   `rquickjs` (QuickJS bindings) below. Exact version pinned in workspace dependencies;
   `cargo deny` must pass.
2. **Sandboxing model:** scripts execute with **no host capabilities by default** — no
   filesystem, network, process, or module-system objects are registered. The host injects only
   the event payload (plain data) and reads back the documented return value. This satisfies
   PRD-SCRIPT-004 by construction: there is nothing to escape to.
3. **Resource limits:** every execution runs on a dedicated bounded worker with a wall-clock
   timeout (default 250 ms, configurable per script within a hard ceiling) and a bounded
   output/return size. Because Boa has no built-in instruction fuel, CPU runaway is bounded by
   the wall-clock timeout plus executing on a separate OS thread that is abandoned on timeout
   (the thread is detached and never joined after deadline; a leaked-thread counter bounds total
   abandoned threads and trips a circuit breaker that disables the failing script and records a
   `script_runs` audit row). Memory is bounded indirectly: event payloads and return values are
   size-capped, and the circuit breaker disables scripts that exceed limits repeatedly.
4. **Failure semantics (PRD-SCRIPT-002 inventory drives per-hook detail):** a script error,
   timeout, or limit trip is fail-degraded — the triggering operation completes as if the hook
   returned its documented default, and the failure is recorded in `script_runs` (ADR-0134).
   Hooks never mutate catalog/identity state directly; they return decisions the host validates.
5. **Migration path (PRD-SCRIPT-001/005):** WP-11 ships the executor, the event inventory
   evidence document, and per-hook migration guidance; preserving user-visible format
   parsers/provider behaviors takes precedence over preserving the legacy plugin architecture.

## Alternatives considered

### rquickjs (QuickJS bindings)

Benefits: native memory limit (`set_memory_limit`), stack limit, and an interrupt handler giving
deterministic CPU cancellation — stronger resource control than Boa. Costs: embeds a C engine
compiled at build time (larger supply-chain/build surface, requires a C toolchain in every build
environment), and the interrupt/memory APIs are crate-internal unsafe machinery we cannot audit to
this repository's standard. Not selected: for an operator-trust-boundary feature with small
payloads, Boa's pure-Rust supply chain plus capability-less sandbox meets PRD-SCRIPT-004 with a
simpler audit story; the deterministic-interrupt advantage is mitigated by the wall-clock +
circuit-breaker design above. If production evidence shows runaway scripts defeating the wall-clock
model, this ADR can be superseded in favor of rquickjs without changing the hook contract.

### No scripting; document a migration guide only

Benefits: zero runtime risk. Costs: fails PRD-SCRIPT-001..004 outright; the parity matrix would
carry an unapproved gap. Rejected.

## Rationale

Boa is the lowest-supply-chain-risk FOSS JS engine in the Rust ecosystem, and the capability-less
sandbox (nothing registered except the event payload) is stronger than trying to constrain a
fully-wired runtime. Scripts in the legacy product automate operator workflows; they do not need
ambient authority.

## Consequences

### Positive

- Pure-Rust engine: no C toolchain, no new unsafe audit boundary, clean `cargo deny` story.
- Sandbox strength derives from absence of capabilities, not from correctly configuring limits.

### Negative / trade-offs

- No deterministic mid-execution interrupt: a pathological script burns its worker thread until
  the wall-clock deadline; leaked threads are bounded and circuit-broken but not instantly
  reclaimed.
- Boa implements modern ECMAScript but not every legacy-corner behavior; the WP-11 inventory must
  record any legacy script relying on engine quirks.

### Follow-up work

- WP-11: script event inventory evidence (PRD-SCRIPT-002), executor + sandbox, `script_registry` /
  `script_runs` migrations (ADR-0134), per-hook migration guide, fault-injection tests (timeout,
  error, oversize return, circuit breaker).

## Validation

- Unit tests: sandbox proves no fs/net/process capability objects exist in the global scope;
  timeout and oversize-return cases produce fail-degraded defaults plus `script_runs` rows.
- Fault injection: infinite loop, throwing hook, and resource-hog scripts never block or fail the
  triggering operation.
- `cargo deny check` and the FOSS gate pass with `boa_engine` pinned exactly.

## References

- [PRD.md](../../PRD.md) §36
- [ADR-0103](ADR-0103-require-fully-foss-dependencies.md),
  [ADR-0111](ADR-0111-do-not-require-broker-or-redis-initially.md),
  [ADR-0134](ADR-0134-phase-9-parity-domain-schema-for-radio-device-profiles-onboarding-notifications-scripts-and-providers.md)
- `design/agents/PHASE-9-PLAN.md` §3
