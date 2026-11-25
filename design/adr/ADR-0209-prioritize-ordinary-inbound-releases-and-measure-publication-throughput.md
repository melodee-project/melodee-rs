# ADR-0209: Prioritize ordinary inbound Releases and measure publication throughput

- **Date:** 2026-11-25
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-009, PRD-INGEST-010, PRD-UI-006, PRD-PERF-001, PRD-STORAGE-004

## Context

An archive migration can keep every worker occupied for days. Arrival order alone gives new music the latency of the entire archive. Publication receipts also incur significant shared NAS traffic. Production qualification needs the time until listeners can play a Release.

## Decision

Operators register the exact inbound directory names of a bulk submission before transferring them. The next discovery consumes each registration once in the same transaction as work creation. The optional inbound library UUID scopes the registration. Directory names identify an intake occurrence only. The catalog identity rules still apply.

Inspection, normalization, validation, publication, catalog import and policy each keep a four-slot cursor in PostgreSQL. Ordinary arrivals get three claims and bulk arrivals one when both are eligible. An empty lane yields all its capacity. Workers lock only bounded candidates and commit the cursor with their leases. A worker finishes its current claim before another Release takes that capacity. A stage reports a stall when eligible work has spent 15 minutes there without a completed handoff.

On NFS, hashing during a fresh write verifies the content supplied to the synced inode. The publisher attaches that inode with an exclusive hard link, checks the inventory and rereads all canonical assets after sealing, before the database receipt. The redundant intermediate reread of freshly installed inodes is removed. Existing files, resumed work and adoption retain their verification checks. The protocol and ownership proofs of [ADR-0202](ADR-0202-publish-sealed-release-files-on-nfs-using-exclusive-hard-links.md) still apply.

The first policy acceptance that activates the catalog Release is its playable milestone. Report the first automatic outcome and elapsed time from discovery. Include ordinary arrivals discovered during the bulk window, even if they finish later. Manual holds, failures, excluded work and outstanding arrivals keep separate counts. Aliveness of the server and RAID health require independent operating evidence.

## Trade-offs and recovery

A single strict-priority queue could starve the archive. Separate inbound roots would change the operator's storage layout. Weighted admission adds brief database serialization within each stage, bounded by the claim transaction. The share measures claims, so differences in Release size and codec still affect elapsed time.

Migration 0053 adds tables, triggers, columns, views and indexes. Existing work follows the ordinary lane. Deploy after a verified backup. Roll forward to repair an interrupted application upgrade. If restoring an earlier schema, stop workers and use the matching pre-upgrade backup and binary, preserving media for reconciliation. Reports apply to submissions registered after upgrade.

## Validation

PostgreSQL tests exercise one-slot fairness across claims, distinct sources, duplicate registration, stall and recovery thresholds, and concurrent ordinary arrivals. Publisher fault tests alter a sealed asset and interrupt NFS installation and adoption. Production acceptance uses the operator's next real bulk submission and concurrent ordinary arrivals.
