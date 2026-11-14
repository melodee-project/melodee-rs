# ADR-0193: Enforce terminable script execution resource limits

- **Date:** 2026-11-13
- **Status:** Accepted
- **Decision owners:** User-directed review remediation program
- **Supersedes:** [ADR-0135](ADR-0135-boa-javascript-runtime-and-sandboxing-for-scripting-hooks.md), resource enforcement in decision 3 only
- **Superseded by:** None
- **Related requirements:** PRD-SCRIPT-003, PRD-SCRIPT-004, PRD-SCRIPT-005, PRD-SEC-001

## Context

A detached thread continues running after a wall-clock timeout. Input/output size caps do not
bound VM allocations. The earlier ADR-0135 resource argument therefore does not establish
CPU or memory isolation. The user authorized correcting the review's runaway-script finding.
Boa 0.22 exposes loop, recursion and stack limits, but those are not heap limits.

## Decision

Each evaluation executes in a single-use `melodee-script-worker` process installed beside the
server and CLI. The parent sends bounded JSON on stdin and accepts a bounded verdict on stdout.
It clears the child environment, never searches PATH, kills the child at its deadline, and waits
for process exit before returning. The service admits at most four workers concurrently and
retains its permit while blocking process supervision finishes, including cancellation of the
calling async task. No script runs in a detached server thread.

Before accepting input, the Linux child uses the safe rustix process API to enforce `RLIMIT_AS`:
128 MiB of fixed loader/VM/serialization headroom plus the registered `memory_bytes` budget
(4 KiB through 64 MiB). This is a hard total address-space bound, not a precise accounting of
JavaScript heap allocations. An inherited stricter hard limit is retained. Failure to install
limits prevents evaluation. Unsupported platforms fail-degrade with an explicit worker failure;
the qualified deployment platform remains the Linux OCI distribution.

Each fresh Boa context additionally has a 100,000 loop-iteration limit, recursion limit of 128,
and stack limit of 4,096. The parent deadline still applies to expensive built-ins, parsing,
serialization and work not covered by VM loop counting. Input is bounded to 2 MiB, source and
context retain their 256 KiB limits, and worker output is bounded to 32 KiB.

Keep the existing `check(ctx, scriptConfig)` language contract, fail-degraded hook verdicts,
audit vocabulary, and consecutive-failure circuit breaker. Resource failures are `error` or
`timeout`, never silent successful execution. Historical leaked-thread diagnostics return zero.

## Alternatives considered

### VM limits alone

Useful defense in depth, but exponential strings and built-ins can exhaust memory before a loop
limit fires. Rejected as the sole boundary.

### Detached threads

Cannot be terminated safely and share the server allocator. Superseded.

### Change JavaScript engines

Not needed: safe OS process limits provide termination and allocation isolation without another
engine, native SDK, or proprietary dependency. The rustix process feature extends an already
pinned, audited dependency; no new dependency version is selected.

## Consequences

Process startup adds latency and requires packaging the helper with server/CLI binaries. Operators
must install all three matching binaries when building from source. Existing script registry rows
remain valid; `memory_bytes` now contributes to an enforced process limit. No migration or public
route/DTO change is needed. Older binaries can still read the registry, but their unsafe timeout
behavior is not a recommended rollback. Scripts exceeding the documented limits must be corrected
or explicitly re-enabled after the normal failure circuit breaker trips.

## Validation

Exercise successful and denied hooks, missing capabilities, throwing scripts, oversized context
and return values, infinite loops, recursion, allocation amplification, timeout kill/reap, and a
successful execution after repeated failures. Verify native APIs and the CLI share the same executor.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0195](ADR-0195-share-application-services-between-server-and-cli.md)
