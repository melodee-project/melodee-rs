# ADR-0202: Publish sealed Release files on NFS using exclusive hard links

- **Date:** 2026-11-23
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** The directory installation step of ADR-0116 on Linux NFS
- **Superseded by:** None
- **Related requirements:** PRD-STORAGE-001, PRD-STORAGE-002, PRD-STORAGE-004, PRD-INGEST-009

## Context

A Linux NFS client can reject a no-replace directory rename with EINVAL even when the destination is absent. Ordinary rename can replace an existing empty directory. Checking a pathname before an ordinary rename leaves a race with other writers.

## Decision

The publisher identifies Linux NFS using the filesystem type of the open per-Release lock. Under that lock it exclusively creates the planned Release directory. A hard link to the persistent lock inode inside that directory authenticates its reservation across restarts. The marker name includes the immutable plan hash. A matching filename alone does not prove ownership.

The worker copies each asset to an exclusively created, attempt-specific scratch file on the same filesystem, verifies its size and BLAKE3 hash, syncs it, and installs the verified inode with a no-replace hard link. After verifying the inventory and syncing the asset directories, the worker installs the sealed manifest by the same mechanism. It removes the reservation marker, verifies the final bundle, and then allows the existing database receipt transaction to mark the Release published. The canonical paths, public IDs, manifest format and staged-source retention follow [ADR-0116](ADR-0116-use-immutable-release-ids-and-convergent-canonical-publication.md).

## Recovery and tradeoffs

- Directory browsers can see a partial Release during the copy. Catalog import requires a verified publication receipt. A manifest is installed only after the asset files are complete.
- A retry under the lock resumes a directory carrying the same plan marker and lock inode. It rehashes existing assets. A committed manifest causes verification and adoption of the complete bundle. Conflicts preserve the files for operator inspection.
- A crash between exclusive directory creation and linking its ownership marker can leave an empty unmarked directory. No process can prove that it created that directory after a restart. Publication reports a target conflict. An operator can inspect and move the directory aside before retrying.
- A killed process can leave a hidden scratch file in the shard. Future attempts use fresh names and never truncate it. The lock file persists across attempts and must remain on the export.
- The export must support hard links, exclusive directory and file creation, and inter-client file locking. Unsupported operations fail the attempt. Other filesystems retain the directory rename protocol.

## Validation

Publication tests exercise locking, interrupted asset installation, a crash after manifest installation, identical adoption, foreign directories, forged markers and conflicting media. These fixtures can also run on a disposable directory on an NFS export.
