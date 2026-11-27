# Disposable stage-file fs-verity feasibility, 2026-11-26

This is an isolated kernel/owned-file experiment, not production ingestion,
a benchmark, a custody protocol, or evidence of final acceptance. The test
created one unique temporary directory and a 4,096-byte test file under the
workspace filesystem, enabled verity only on that file, and removed the
directory. No producer file, filesystem feature, host security setting or
service was modified. A post-run directory listing found no matching probe
directories. The exact time of the run was not recorded.

The host exposed Linux `7.3.4-300.fc45.x86_64`, 4 KiB pages, Btrfs for the
workspace, `CONFIG_BTRFS_FS=y`, and `CONFIG_FS_VERITY=y`; the `fsverity`
command was absent. The Python 3.15.2 probe used standard-library `fcntl` and
an x86_64-only ioctl encoding with the 128-byte struct layout checked against
the installed Linux UAPI header. The probe SHA-256 at execution was
`9724d5d078403c149dddb7c00136ee804850630d34bf2909f5ae99fea4500aca`.
Five Python tests passed, including safe help, mocked unsupported behavior,
ABI size/opcode checks, and cleanup while preserving an unrelated sibling.

| Test | Observed result |
| --- | --- |
| ioctl on a pipe | `ENOTTY` (25) |
| enable with an open writable descriptor | `ETXTBSY` (26) |
| enable with a writable shared mapping after its descriptor was closed | `ETXTBSY` (26) |
| enable after closing writers and mapping | Success (0) |
| new write after enablement | `EPERM` (1) |
| read after rejected write | Original test bytes unchanged |

`ENOTTY` on the pipe tests an unsupported **object type**. A separate run
created a unique owned child in the existing `/dev/shm` tmpfs; enabling verity
on that file returned `ENOTTY` (25) and the probe reported unsupported. The
owned child was removed. This tests one unsupported filesystem, not every
unsupported mode. No crash, rename, path substitution,
concurrent external process, source reobservation, recovery, disk exhaustion,
throughput or larger file was tested. The kernel operation protects the file
inode's contents after successful enablement, not names or directory structure.
The result does not authorize mutating producer-owned files or assuming a
configured staging path supports fs-verity.

Reproduce only on an explicitly owned disposable parent:

```text
python3 -m unittest tests.python.test_perf_fsverity_probe
python3 scripts/perf_fsverity_probe.py --temp-parent <owned-existing-parent>
```

The probe supports only Linux x86_64 with 4 KiB pages and removes its unique
temporary child on normal exit or handled SIGINT/SIGTERM. SIGKILL or host
failure may leave that child for operator inspection. The proposed integration
and its unresolved fallback and recovery conditions are documented in
[ADR-0221](../../design/adr/ADR-0221-freeze-staged-custody-file-contents-with-fs-verity-where-supported.md).

## Test-only protected-parent adapter

A later test-only adapter requires the opaque `PrivateStageParent` capability
and revalidates it before and after file creation, before the ioctl, and at
verification. The first run failed because the capability attempted ACL
inspection through a non-readable directory descriptor; the descriptor fix was
validated separately. The subsequent serialized `stage_verity::tests` run
passed 7/7 test functions on an ACL-free tmpfs runtime parent. It exercised
replacement before create, replacement before seal with the ioctl not invoked,
synthetic verification ordering after replacement, no-follow/no-overwrite,
stage-file replacement, and deterministic unsupported/open-writer errors.

The real retained-writer denial, successful-seal/write-denial and post-seal
parent-replacement branches printed `fs-verity unsupported` and did not run on
that tmpfs. They must not be counted as a protected-parent positive test. The
standalone Btrfs probe above established the kernel behavior on a different
owned file whose workspace ancestry has an ACL; it did not establish the
combined protected-parent-plus-verity path. At that point an actual positive test on a suitable protected filesystem
remained outstanding; see the subsequent isolated result below. Durable source
reobservation, recovery and production integration remain open. The adapter
and dependency remain test-only.

## Subsequent protected Btrfs test in private namespaces

A source-fingerprinted debug Rust test binary was run with a fresh owned Btrfs
fixture bind-mounted inside private user, mount and PID namespaces. The child
used a new read-only proc mount, read-only bind mounts for its test executable
and runtime libraries, and a child-only chroot into the fixture. It validated
namespace IDs, the unprivileged one-UID mapping, private mount propagation and
owned directory identities before mounting; it closed the passed fixture
file descriptors and checked for other inheritable descriptors before executing
the test. Independent static safety review cleared that version of the harness.

All seven stage-adapter test functions passed, with **zero** unsupported
fs-verity branch messages. The real kernel paths therefore exercised an open
writer refusal, successful seal and subsequent write rejection, and replacement
of the protected parent after sealing, alongside the earlier before-create,
before-seal and synthetic ordering cases. The parent user, mount and PID
namespace IDs remained unchanged, and both owned fixture directories were
absent after cleanup. See [the sanitized result](stage-verity-private-pid-2026-11-26.json).

An earlier attempt that tried to bind the host `/proc` failed before running
any Rust tests: its private-namespace `proc_bind` step exited 32 and reported
`wrong fs type`. Its owned directories were also removed and parent namespace
IDs were unchanged. The successful run instead mounted a new proc instance in
the isolated PID namespace; no host mount or security setting was changed.

The tested binary SHA-256 was
`55492821222007635162f8766d4f6108ea2ed20b88478291385212356a9517aa`.
The [build record](stage-verity-namespace-build-2026-11-26.json) reports
matching before/after workspace source fingerprints and HEAD
`37f29889fb05d1825faecb8b597387f0eb971b5e`. A
[post-run comparison](stage-verity-private-pid-postrun-source-2026-11-26.json)
found six changed source files, including `custody.rs`; it is not a snapshot of
the workspace during the test. Thus the success applies to the recorded test
binary and its earlier source, not necessarily the final integrated version.
The current code needs a final-source rerun. This remains a test-only operation:
source mutation/reobservation, real receipt binding, cancellation/lease loss,
crash recovery, unsupported-filesystem fallback and performance are not proven.
