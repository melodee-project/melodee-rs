# ADR-0208: Identify and correct unpublished Releases in review

- **Date:** 2026-11-25
- **Status:** Proposed
- **Decision owners:** Melodee maintainers
- **Related requirements:** PRD-UI-007, PRD-UI-008, PRD-API-ADMIN-002

## Context

A pre-catalog Release can have normalized audio but lack a title, an artist, or Track ordering. The C# Identify dialog searches by artist and title and lets the operator pick a result. Rust has a validation queue, provider adapters, and a reversible catalog merge, but each operates at a different stage of ingestion.

## Decision

An editor or administrator can revise the presentation fields of a normalized, unpublished Release in review. Save the corrected manifest, its semantic hash, the actor, the field changes, and the transition back to validation atomically. Preserve the byte hashes, file inventory, original file tags, and normalized outputs. Reject a stale hash, pending deletion, active lease, catalog binding, or publication authorization.

Identify requires an explicit choice. Melodee results carry catalog IDs and external results are verified against the provider's Release search cache. Persist the choice outside the manifest so media tags cannot authorize a merge. Apply the selection in the same transaction that accepts the Release and starts indexing. An external selection records its provider identity on the new Release. An internal selection uses the catalog's reversible merge, retains the selected Release and billing, and waits for the affected search projections. If the target becomes unavailable, park the item for reselection.

## Alternatives

Re-inspecting the source would discard operator edits and repeat normalization. Editing embedded tags would change the original evidence and require rewriting media. Matching by title alone could merge distinct editions without operator authorization. Applying a merge before the policy decision would allow a later policy update to reactivate the merged source.

## Migration and recovery

Migration 0052 adds a per-work-item identification table and a partial index for pending choices. Existing work items require no backfill. To roll back application binaries, first drain pending identifications or pause the acceptance worker and retain its pending records until the correction-capable version is restored. An older policy worker does not enforce pending choices. Retain the table and audit history on rollback. The catalog merge history supports splitting an incorrect local merge.

## Validation

PostgreSQL API tests exercise editor authorization, correction, stale revisions, verified selection, identity retention and replacement. Indexing tests exercise a stale target, reselection and exactly-once merge. Admin UI tests cover both dialog submissions.

## References

- [Product requirements](../../PRD.md)
- [Technical specification](../../SPEC.md)
- [Convergent canonical publication](ADR-0116-use-immutable-release-ids-and-convergent-canonical-publication.md)
- [Administrative Release merge](ADR-0198-multi-source-release-merge-in-the-admin-ui-and-admin-api.md)
