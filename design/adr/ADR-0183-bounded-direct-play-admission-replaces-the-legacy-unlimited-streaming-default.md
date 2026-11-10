# ADR-0183: bounded direct-play admission replaces the legacy unlimited streaming default

- **Date:** 2026-11-09
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-STREAM-001, PRD-PERF-003, PRD-PERF-004, PRD-API-OS-007,
  PRD-API-JF-006

## Context

The C# server exposes `streaming.maxConcurrentStreams.global` and
`streaming.maxConcurrentStreams.perUser` (both default `0` = unlimited) and consults them in
`StreamingLimiter` from the native `/songs/{id}/stream` route and the OpenSubsonic
`stream`/`download` routes. When a configured limit is exceeded the request is rejected with
`429` (`TOO_MANY_REQUESTS` / `{error: "Too many concurrent streams"}`) rather than queued.

Before this decision the Rust server:
- read the process-wide cap only from `MELODEE__NATIVE__STREAM_CONCURRENCY` (default 32),
  ignoring both registered settings;
- had no per-user cap at all;
- *queued* waiters on the global semaphore instead of rejecting.

That left the configuration surface misleading (operators changing the settings saw no
effect) and created an unbounded per-user fan-out risk, but the Rust bounded default also
protects a process that previously had no ceiling.

## Decision

1. `streaming.maxConcurrentStreams.global` is honored when greater than zero. Because the
   setting's registry default is `0` ("unlimited" in the legacy model), a value of `0`
   resolves to the process safety default of **32** concurrent direct-play responses; the
   `MELODEE__NATIVE__STREAM_CONCURRENCY` environment override is still accepted for that
   case. This is the intentional difference: Melodee Rust never runs an unbounded streaming
   semaphore. Values are clamped to `1..=4096`.
2. `streaming.maxConcurrentStreams.perUser` is honored when greater than zero. Exceeding it
   fails fast with the legacy `429` shape on every streaming surface:
   - native `/song/stream/...` → `429 {"code":"TOO_MANY_REQUESTS","message":"Too many concurrent streams"}`
   - OpenSubsonic `stream`/`download` → `429 {"error":"Too many concurrent streams"}`
   - Jellyfin `/Audio/{id}/stream`/`universal`/`stream.*` → `429` ProblemDetails with the
     same detail
   `0` disables the per-user cap (legacy default).
3. The global cap keeps the existing bounded-queue behavior (waiters are cancellation-safe,
   visible in the resource gauge, and released on body drop or client disconnect) because
   queueing is strictly friendlier for a legitimately bursty client at process scale; the
   per-user cap provides the deterministic rejection the legacy limiter provided.
4. Admission applies to direct play on every adapter and to admin review previews (keyed by
   the acting administrator). Bounded ancillary reads (artwork, lyrics) pass no user key so
   they cannot exhaust a user's streaming budget.

## Alternatives considered

### Alternative A — Match legacy exactly: `0` means unlimited

Benefits: literal configuration parity.

Costs and why not selected: an unbounded in-memory semaphore is not a bound at all; a small
number of stalled clients can exhaust file descriptors and memory. The operator can still
approximate unlimited by setting a very large value; the documented default is a safety
property, not a product behavior clients can observe (no captured contract asserts the
absence of a cap).

### Alternative B — Reject on the global cap too (legacy behavior)

Benefits: exact legacy response semantics when a global limit is configured.

Costs and why not selected: the Rust server's bounded queue is already shipped and tested
(`native_stream.rs`, gauge tests). Rejection would make a transient burst fail playback that
would otherwise succeed; the global cap is already an operator-selected safety valve.

## Rationale

The per-user limit is the contract that clients can observe, and it is now identical to the
legacy rejection shape. The global limit remains a process-safety device; keeping a bounded
default and a queue is the safer replacement than an unbounded semaphore, and the difference
is documented rather than silent.

## Consequences

### Positive

- Both registered streaming settings now have a runtime consumer (settings inventory).
- Per-user stream budgets work on native, OpenSubsonic and Jellyfin surfaces.
- A misconfigured `0` can no longer remove all admission control.

### Negative / trade-offs

- The default global cap (32) differs from the legacy unlimited default; a deployment with
  more than 32 simultaneously streaming sessions on the old default must set
  `streaming.maxConcurrentStreams.global` (or the env override) explicitly.
- A per-user limit rejects rather than queues, so clients must retry (legacy identical).

### Follow-up work

- Document the default and both keys in the configuration and troubleshooting guides.
- MEL-16 (target-scale acceptance) should record observed admission behavior at scale.

## Validation

- `apps/melodee-server/tests/native_stream.rs`: `per_user_stream_cap_rejects_only_the_over_limit_user`,
  `per_user_cap_zero_disables_the_admission_bound`,
  `dropping_a_body_releases_bounded_stream_capacity`.
- `apps/melodee-server/tests/native_stream_acl.rs`, `opensubsonic_*` and
  `jellyfin_media_api.rs` continue to pass unchanged (defaults preserve them; assertions
  exercise the new mapping where a limit is configured).
- `apps/melodee-server/tests/admin_runtime_diagnostics_api.rs` reports admission state and
  the effective per-user cap.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0147](ADR-0147-configured-transcoding-for-opensubsonic-and-jellyfin-streaming.md)
- [Feature parity matrix](../requirements/FEATURE-PARITY-MATRIX.md)
