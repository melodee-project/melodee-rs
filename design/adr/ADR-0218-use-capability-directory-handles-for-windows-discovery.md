# ADR-0218: Use capability directory handles for Windows discovery

- **Date:** 2026-11-26
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-002, PRD-PERF-003, PRD-SCALE-002

## Context

The stable Rust standard library does not expose Windows directory-relative
filesystem operations or stable Windows volume/file-index accessors. Windows
source discovery must avoid pathname replacement races without adding unsafe
code to this repository or silently disabling the platform. Existing custody
code has separate platform limitations; this decision does not waive them.

## Decision

Evaluate `cap-std` and `cap-fs-ext` 4.0.3, using their safe capability directory
APIs and no-follow child opens for Windows discovery and bounded streaming
snapshot traversal on supported platforms. The upstream projects are
dually licensed Apache-2.0 or MIT, both allowed by the repository FOSS gate.
Directory handles must be opened without `FILE_SHARE_DELETE`; capability
operations and snapshots must remain relative to pinned handles. An identity
value, including a truncated ReFS file ID, is not by itself an adequate fence.
Preserve in-root symlink semantics and fail closed when a safe traversal cannot
be established. Existing cryptographic custody verification is unchanged.

## Alternatives considered

Raw Win32 bindings would require unsafe code in this repository. Pathname-only
pre/post metadata checks permit replacement races. Disabling Windows source
discovery changes prior behavior. A capability implementation confines those
unsafe platform details to audited dependencies while exposing safe Rust APIs.

## Consequences

The dependencies introduce a FOSS and transitive-dependency audit obligation.
The direct dependencies are shared across supported targets; the transitive Windows helper `winx`
0.36.4 uses Apache-2.0 WITH LLVM-exception alone. The exception relaxes certain
Apache redistribution conditions for embedded compiled portions and provides
a GPLv2 compatibility waiver; it does not add proprietary conditions. This is
compatible with the FOSS-only requirement and the existing Apache-2.0 baseline.
`deny.toml` grants that license only to `winx`, not globally. Review the license
and dependency again on upgrade. Cross-compilation does not establish
Windows runtime behavior: release qualification requires native Windows tests,
including reparse points, in-root and escaping symlinks, root replacement,
share-mode behavior and recovery. This decision is proposed while the fallback
and its validation are unfinished.
The cross-platform streaming snapshot integration and its resource and
cancellation tests are also in progress; this decision does not assert that
those paths have completed verification.


## Validation

Run target-specific compilation and dependency/license audits. Exercise native
Windows adversarial filesystem tests and all source/recovery compatibility
checks before marking the implementation verified.

## References

- [cap-std directory API](https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html)
- [cap-std license](https://github.com/bytecodealliance/cap-std/blob/main/Cargo.toml)
- [cap-fs-ext directory API](https://docs.rs/cap-fs-ext/4.0.3/cap_fs_ext/trait.DirExt.html)
- [ADR-0217](ADR-0217-fence-and-bound-inbound-discovery-scans.md)
