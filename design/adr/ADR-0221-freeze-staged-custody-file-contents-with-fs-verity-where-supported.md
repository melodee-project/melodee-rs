# ADR-0221: Freeze staged custody file contents with fs-verity where supported

- **Date:** 2026-11-26
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PERF-009, RM-11, custody and recovery invariants in `PERF-REMEDIATION-SPEC.md`

## Context

A full-content verification observes bytes at read time; it does not stop a
writer changing the same inode immediately afterward. Moving a source into a
protected directory does not revoke file descriptors or shared writable maps
already held by a producer. A subsequent database receipt must not imply that
such a source or staged copy is immutable merely because identity, size,
timestamps or a previous hash match.

A local disposable-file experiment and earlier raw-parent helper tests
established that this host's workspace Btrfs accepts Linux fs-verity, rejects enablement
while a writable descriptor or writable shared mapping remains, and rejects new
writes after successful enablement. Protected-parent adapter tests first ran
on tmpfs, where fs-verity was unavailable. A subsequent isolated private
user/mount/PID namespace run exercised the combined protected-parent and
fs-verity path on an owned Btrfs fixture; it used an earlier source-fingerprinted
test binary and does not prove the final integrated implementation. See
[the scoped evidence](../../tests/performance/fsverity-stage-probe-2026-11-26.md).
This is not evidence for arbitrary staging filesystems, full custody recovery or
the final production workflow.

## Proposed decision

Explore a Linux-specific custody mode that **only** enables fs-verity on newly
created, service-owned staging files; never enable it on a producer-owned source
or silently alter an existing retained file. The mode must remain disconnected
until every requirement below has been implemented and validated:

1. Create files without following links or replacing existing names inside a
   separately protected staging namespace, on a validated filesystem and mount
   topology. Finish and sync each copy, close every service writer, and keep a
   verified read-only descriptor. The kernel operation must succeed for every
   required regular file before accepting a frozen stage proof. A preexisting
   writable descriptor or mapping is a retryable failure, not grounds to ignore
   sealing or assume the writer will stop.
2. Bind the **sealed** file bytes, native relative names, directory structure,
   and stage-generation identity to the existing versioned cryptographic tree
   and legacy receipt proofs. The fs-verity Merkle digest is a different
   algorithm and format and must not be substituted for either existing digest.
   Revalidate the no-follow namespace, source occurrence, work item and current
   lease before binding; protect against file or directory replacement through
   every later consuming boundary. fs-verity protects file contents, not names,
   directory membership, symlinks, metadata, mounts or a privileged actor.
3. Sync file data and required parent directories before committing a receipt.
   Stage creation, per-file sealing, proof state, database binding and recovery
   must form an idempotent fenced protocol across interruption. An already
   enabled file can only be adopted after verifying that it is the expected
   owned object and its measured verity state and content match the durable
   attempt; `EEXIST` alone is not proof.
4. Preserve the original source bytes and occurrence. A producer may change
   the source after any verification, including after receipt commit. An
   independently scheduled, bounded, durable source reobservation obligation
   must persist atomically with the receipt, recover after crashes, and create a
   distinct occurrence for a later observed content revision. It must not rely
   solely on watcher events or inode, size and timestamps. This provides eventual
   detection of a persistent change, not a guarantee of observing every transient
   A→B→A change. Define an explicit operator or producer handoff protocol before
   any terminal cleanup; an uncooperative producer cannot be proven quiescent
   by a finite number of content reads.
5. Feature-detect without changing host configuration. Unsupported kernels,
   filesystems, required-signature policies, resource exhaustion and unexpected
   errors must never be reported as successful enforcement. The safe,
   compatibility-preserving fallback is not established by this proposal: it
   must independently solve both post-check windows or remain fail-closed with
   data retained and an actionable operator disposition. No blanket switch to
   fs-verity or source deletion is authorized by this proposed decision.
6. Admit potentially long ioctl work before spawning blocking work, retain the
   permit until it has actually terminated, and preserve lease fencing and
   cleanup under cancellation and shutdown. Kernel fs-verity construction
   performs its own full-file Merkle-tree work; measure its reads, time, space
   and concurrency impact rather than claiming a performance reduction.

### Candidate dependency

Evaluate the Linux-only `composefs-ioctls` **0.10.0** safe `AsFd` wrapper as a
narrow dependency with default features disabled. The published archive's
SHA-256 is
`1d1e1dbf64a4a2dda8db3cbdb3109d29274b0cbeedc1ee68fdc8ccfdefc17cb9`;
its `src/fsverity.rs` matches the upstream v0.10.0 tag and uses the kernel's
128-byte C layout and rustix's architecture-aware ioctl opcode. The wrapper
contains a small unsafe ioctl boundary inside the dependency; review its exact
published code and platform behavior independently before adoption. Call only
its stage-file API from safe Melodee code.

The archive declares `MIT OR Apache-2.0`, minimum Rust 1.88 and only `rustix`
and `thiserror` normal dependencies. The current workspace already pins
rustix 1.1.6 and thiserror 2.0.21; their existing locked dependency license
expressions have choices on the `deny.toml` allowlist. The exact workspace
dependency and lock entry have been added for a disconnected stage-only helper; production custody does not call it.
`cargo deny check` passed advisories, bans, licenses and sources against that
lock on 2026-11-26. The decision remains Proposed pending the integration,
compatibility, recovery, operational and performance gates below.

## Alternatives considered

- **Ordinary hash before and after copy:** preserves useful integrity checks,
  but cannot close an in-place change after the last read. Multiple checks do
  not enforce immutability.
- **Rename, chmod or advisory lock on the source:** cannot revoke an existing
  writable descriptor or writable mapping, and advisory locks require producer
  cooperation. Do not change producer-owned files to make them immutable.
- **`FICLONE` into a protected fresh inode:** Linux supports atomic per-file
  copy-on-write cloning against concurrent writes on supported same-filesystem
  storage. It does not atomically snapshot the whole directory or prevent later
  writes to the new destination by an authorized writer; source reobservation
  and protected names remain necessary. It may be an optional copy optimization
  only after independent content verification and fault tests.
- **Btrfs read-only subvolume snapshot:** can give a stable filesystem view on
  an appropriately provisioned Btrfs subvolume, but snapshotting is not
  recursive across nested subvolumes and can require privileges. A snapshot
  may capture an application mid-write; it does not account for later source
  changes or other supported filesystems.
- **memfd sealing:** kernel-enforced file seals can prevent writes to a memfd,
  but those anonymous files are not a durable, restart-safe replacement for
  persistent staged media.
- **Producer-cooperative handoff:** a contract requiring producers to close all
  writers and publish new immutable occurrences can solve the producer side
  when the contract is enforceable, but cannot be inferred for existing
  producers. Durable retained-source reobservation remains required for the
  non-cooperative case.

## Consequences and validation

This mode adds persistent per-file filesystem metadata, potentially another
full-file read and Merkle-tree storage, filesystem and kernel constraints, and
an immutable stage-file lifecycle. It does not establish source immutability or
prove that a finite polling pass catches transient writes. Deployments without
support require an independently verified alternative or an explicit safe
failure; do not modify filesystem features or host security settings.

Before acceptance, test actual production-path same-inode same-size writes
before and after every proof, retained writable file descriptors and mappings,
file and directory replacement, namespace/mount aliases, unsupported and
signature-required filesystems, duplicate/partial seals, disk exhaustion,
lease loss, cancellation, crashes around each sync/bind boundary, idempotent
recovery, legacy receipt compatibility, retained-source revision detection and
fair reobservation. Measure release-build end-to-end operations, storage and
resource admission on supported and unsupported filesystems. Run fresh and
populated migrations, dependency and license gates, platform tests and
independent security review before wiring or acceptance.

## References

- [Linux fs-verity API](https://docs.kernel.org/filesystems/fsverity.html)
- [Linux FICLONE semantics](https://www.man7.org/linux/man-pages/man2/FICLONE.2const.html)
- [Btrfs nested subvolume snapshot behavior](https://btrfs.readthedocs.io/en/latest/Subvolumes.html)
- [composefs-ioctls 0.10.0 source](https://raw.githubusercontent.com/composefs/composefs-rs/v0.10.0/crates/composefs-ioctls/src/fsverity.rs)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
