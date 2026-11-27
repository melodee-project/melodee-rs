# ADR-0219: Supervise media subprocess trees with bounded pipe draining

- **Date:** 2026-11-26
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SCALE-002, PRD-PERF-003, PERF-007, RM-12

## Context

Media inspection and normalization invoke external tools with bounded output capture. A child may
exit while a descendant still holds a pipe open, so waiting for a pipe reader after killing only
its immediate child can hang forever. Abandoning reader threads instead leaks resources and breaks
admission accounting. Windows must assign a process to a Job Object before it can create descendants;
assigning it after a normal spawn leaves a race.

## Decision

Admit each media tool before creating its scoped supervisor thread. Run asynchronous bounded pipe
readers inside that thread, enforce one deadline over admission, execution, and draining, and retain
the admission slot through termination and direct-child reaping. Captured output remains bounded;
the decoded-audio integrity fingerprint is streamed and is not replaced by filesystem metadata.

On Unix launch an isolated process group and terminate it while the unreaped group leader still pins
its identifier. On Unix targets where rustix exposes `waitid`, observe a completed child without
reaping before terminating its remaining group and collecting the direct child's exit status. On
OpenBSD, use the safe `nix` kqueue API to observe `EVFILT_PROC`/`NOTE_EXIT`; an `ESRCH` when
registering an already-exiting owned child also triggers group termination before reaping. This
relies on the supervisor retaining exclusive ownership of the child and not waiting elsewhere.
On Windows use `process-wrap` 10.0.1
with its Tokio Job Object and kill-on-drop wrappers. Its implementation starts the process suspended,
assigns the Job Object, then resumes it, and keeps the job handle until cleanup. Fail closed if the
job cannot be created or assigned. The project code does not use unsafe Rust.

The Windows-only `process-wrap` dependency is dual licensed Apache-2.0 OR MIT, has Rust 1.87 MSRV,
and wraps established Windows Job Object APIs; it meets the FOSS gate. The target-scoped `nix`
0.31.4 kqueue API is MIT licensed and was already a transitive workspace dependency. Existing
workspace Tokio and rustix are reused. Native Windows runtime behavior must be tested on Windows: cross-compilation
alone is not runtime evidence. Unix children that deliberately create a new session can escape a
process group; no hostile-code sandbox is claimed. On Unix platforms other than OpenBSD where
rustix does not expose the safe `waitid` interface, the existing direct-child wait fallback is
preserved; that fallback does not
ensure descendants that closed their pipes are terminated after a successful leader exit. This
platform-specific correctness gap remains open and must not be reported as verified containment.

## Alternatives considered

### Detached or unconditionally joined blocking pipe readers

Detached readers retain uncontrolled resources; unconditionally joining them can hang forever when a
descendant inherits the pipe. Neither satisfies bounded admission and shutdown.

### Assign a Windows Job Object after ordinary process spawn

A child can spawn descendants before assignment, so the resulting tree is not reliably contained.

## Rationale

The existing async runtime provides cancellable pipe ownership without a thread per pipe, while a
scoped supervisor makes synchronous media callers safe even inside an existing Tokio runtime. Native
process groups and Job Objects provide the platform-specific tree boundary without adding unsafe
code to this repository.

## Consequences

### Positive

- Child cleanup, output draining, cancellation and permits share one bounded lifetime.
- Pipe inheritance cannot indefinitely block a reader thread after a timeout.

### Negative / trade-offs

- Each admitted tool has a scoped supervisor thread and an async runtime; the admission bound limits
  their aggregate count and must be included in performance measurements.
- Windows introduces one target-specific dependency and requires native qualification.
- Unix targets other than OpenBSD lacking safe `waitid` retain an unresolved descendant-cleanup
  gap; reference Linux platforms use the fenced path. Native OpenBSD execution remains unverified.

### Follow-up work

- Exercise native Windows and OpenBSD process-tree and interrupted-operation tests before claiming
  either platform's runtime behavior verified.

## Validation

Run timeout, cancellation, output saturation, inherited-pipe, successful-exit-with-descendant,
version-probe and decoded-audio verification regressions, followed by real codec tests. Run a Windows
cross-target build and the same process-tree tests on native Windows. Verify the dependency license
with the repository dependency gate.

On 2026-11-27 UTC, the OpenBSD library and test sources passed a metadata-only cross-check from
Linux using pinned Rust 1.99.0, `RUSTC_BOOTSTRAP=1`, `-Z build-std=std,panic_abort`, and
`blake3/pure`, in an isolated temporary target directory. The ordinary `rustup target add
x86_64-unknown-openbsd` attempt reported that the toolchain has no prebuilt artifacts for that
target. A macOS `--tests` cross-check of the same kqueue API also passed with `blake3/pure`. The
FreeBSD attempt instead exposed upstream nix 0.31.4/libc 0.2.190's missing `NOTE_PCTRLMASK`
constant in nix's FreeBSD event module. The OpenBSD check did not link an OpenBSD sysroot or run a
native process, and none of these cross-checks proves runtime process-tree behavior. `cargo deny
check licenses` passed with the target-scoped nix and process-wrap dependencies.

## References

- [Performance standard](../../PERFORMANCE.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [process-wrap source and license](https://github.com/watchexec/process-wrap/tree/v10.0.1)
