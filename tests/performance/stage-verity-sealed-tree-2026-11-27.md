# Test-only protected tree sealing in private namespaces, 2026-11-27

An isolated debug libtest build from an intermediate source snapshot exercised the
protected tree-copy path on an owned Btrfs fixture. The harness used private
user, mount and PID namespaces, a fresh read-only proc mount, read-only test
binary and library bind mounts, and a child-only chroot. It did not change host
mounts or security settings. The fixture and mounts were removed by the harness;
the parent user, mount and PID namespace identities were unchanged.

The [successful sanitized result](stage-verity-sealed-tree-inherit-2026-11-27.json)
reports one exact test passed, the explicit real sealed-copy marker, no
unsupported marker and no panic. The test also performs locked-child,
promotion, remeasurement and source-retention assertions before completing.
The [build proof](stage-verity-sealed-tree-inherit-build-2026-11-27.json)
identifies the debug binary and fingerprints 577 source inputs and four build
and runner helpers. The checkout was a detached, dirty intermediate snapshot
at HEAD `37f29889fb05d1825faecb8b597387f0eb971b5e`; subsequent shared
source edits, including directory-page checks, were excluded. The build emitted
three dead-code warnings. The build and its target directory were private to
this invocation, and the proof records hashes rather than claiming a clean HEAD.
A separately reviewed build helper was updated in this snapshot before the
build; the snapshot's source hashes were verified unchanged. Raw output remains
in a private owner-only invocation directory and is not committed.

This user namespace maps one UID. Its named-user ACL rejection uses the mapped
service UID, so it does not by itself prove the distinct-producer-UID ACL case.
The passing test is not evidence of production intake, an unsupported-filesystem
fallback, crash recovery, final integrated source, or performance. The
implementation remains test-only pending its broader custody protocol and ADR
requirements. The distinct-UID ACL behavior was tested separately outside this
one-UID namespace; see the existing protected-parent evidence.

Earlier attempts are retained for traceability:

- [Initial result](stage-verity-sealed-tree-2026-11-27.json) exited 101 after
  the sealed-copy marker. Its original runner did not record panic locations;
  the result is a failure, not a successful test.
- A [diagnostic rerun](stage-verity-sealed-tree-diagnostic-2026-11-27.json)
  reached the sealed-copy marker but failed the test's named-UID ACL setup with
  EINVAL because UID 1 was not mapped in the private namespace. Its
  [separate build proof](stage-verity-sealed-tree-diagnostic-build-2026-11-27.json)
  identifies the exact later binary. The corrected test preserves the ACL
  rejection assertion while using a mapped named UID when necessary.
- The [first corrected snapshot](stage-verity-sealed-tree-corrected-2026-11-27.json)
  and [fixed-path re-exec snapshot](stage-verity-sealed-tree-reexec-2026-11-27.json)
  failed before sealing when the lock child could not start in the minimal
  chroot. The latter retained an exact read-only `/test-binary` executable;
  inheriting the already-open stdin instead of reopening absent `/dev/null`
  resolved the observed test failure in the successful run.
- An additional inherited-stdin snapshot did not compile due to a moved test
  proof; it produced no successful build record or namespace result. The test
  ownership error was fixed before the successful snapshot.

A final integrated-source rerun remains required; no source mutation or
receipt-binding correctness is inferred from this isolated experiment.
