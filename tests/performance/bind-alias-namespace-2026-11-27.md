# Bind-alias private-namespace test attempts, 2026-11-27

The exact ignored test `custody::tests::private_namespace_bind_aliases_are_rejected`
remains **unverified** by these attempts. The launcher uses a private user,
mount and PID namespace, checks the one-UID map and private propagation,
constructs a child-only chroot through a pinned fixture directory, and attempts
to expose only read-only proc, runtime libraries, the attested test executable,
and trusted mount utilities. It records only fixed setup labels and hashes;
no raw output or fixture contents are committed.

The [first attempt](bind-alias-2026-11-27.json) failed before the exact test
passed and did not retain a specific failure stage. The
[bounded diagnostic attempt](bind-alias-diagnostic-2026-11-27.json) identified
failure at `proc_mount`. A further [mount diagnostic](bind-alias-mount-diagnostic-2026-11-27.json)
reported util-linux's generic `wrong_fs_type` text at the same step; this is
not by itself a determination of the underlying kernel error. The matching
[build proof](bind-alias-mount-diagnostic-build-2026-11-27.json) records a
source-fingerprinted debug binary from an intermediate dirty snapshot based on
`37f29889fb05d1825faecb8b597387f0eb971b5e`; three provisional dead-code
warnings were emitted. None of these attempts establishes a passing test.

The parent user, mount and PID namespace identities were unchanged in all
recorded attempts. Fixtures were deliberately retained, not automatically
removed through replaceable directory paths. The exact registered snapshot and
its retained fixtures are in the owner-only same-filesystem sibling quarantine
created for this work; a private quarantine manifest records its worktree and
source identities. An operator must inspect identity and contents before any
manual cleanup. No host mount or security setting was modified. Final-source
verification and the production custody protocol remain open.
