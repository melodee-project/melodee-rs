# ADR-0106: Use Release-centric canonical storage

- **Date:** 2026-08-07
- **Status:** Proposed
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-STORAGE-001 through PRD-STORAGE-004

## Context

Artist-folder ownership makes collaborations, same-name Artists, merges, splits, and identity
corrections into filesystem mutations. The existing library is nearly 5 TB, so avoidable moves are
high risk.

## Decision

Proposed: shard canonical storage by stable Release ID. Artist/release relationships live in the
catalog. Optional human-readable by-Artist views are generated and disposable.

## Alternatives considered

- Preserve Artist/Release physical hierarchy.
- Content-address every MediaAsset and make Releases purely virtual.

## Validation

Prototype migration/reference behavior against the existing storage tree and verify backup,
human-operability, and cross-filesystem publication behavior before acceptance.
