# ADR-0199: Administrator Artist and Release deletion from the catalog lists

- **Date:** 2026-11-15
- **Status:** Accepted
- **Decision owners:** Catalog curation, Admin API, Admin UI
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-ADMIN-002, PRD-CATALOG-001, PRD-POLICY-007,
  PRD-UI-005

## Context

Operators could merge Artists and (from ADR-0198) merge Releases in the Admin UI, but there was
no way to remove a wrong Artist or a set of duplicate/wrong Releases from the catalog lists. The
only deletion path was Release Review: one Release, one policy decision, and a work item in
`NeedsReview`. That is the correct path for a *processing* disposition, but it cannot express
"remove these five wrong imports I just found in the catalog".

The C# baseline offered an Artist delete that removed database rows and moved/deleted media
directories, scoped by filesystem naming. That shape is not acceptable here: paths never
determine identity, policy facts and public ids must survive, and a deletion must be auditable
and attributable.

Two operator expectations had to be satisfied at once:

1. Sometimes only the catalog record is wrong (a duplicate identity, a bad import shell) and the
   media must stay: **delete catalog records only**.
2. Sometimes the media itself is unwanted: **delete catalog records and media files**.

Artists are the most connected entity in the schema (identities, credits, contributions,
observations, provider evidence, user state, audit history). Physical row deletion would destroy
that graph and every public-id guarantee.

## Decision

1. Add one deletion surface under `/api/admin/v1/catalog` with two entry points and one shared
   semantic:

   ```http
   POST /api/admin/v1/catalog/artists/delete/preview
   POST /api/admin/v1/catalog/artists/delete
   POST /api/admin/v1/catalog/releases/delete/preview
   POST /api/admin/v1/catalog/releases/delete
   ```

   Preview is read-only and reports exact counts (Releases, Tracks, media files, media bytes,
   credits, contributions, user state), whether canonical storage is configured, how many
   affected Releases have durable storage evidence, and warnings. Delete requires a non-blank
   audit reason and one of two modes, `catalogOnly` or `releasesAndFiles`, and is bounded to 100
   distinct entries per request.

2. **Deletion is a tombstone, never a row delete.** Artists gain a terminal `deleted` status
   (migration 0049); Releases reuse their existing `deleted` status. Every identity, credit,
   contribution, observation, provider record, per-user state row, Track, media asset, public id,
   and audit event survives. Browse, resolver, provider, and identity predicates already filter
   `status IN ('active','verified')` for Artists and `status <> 'deleted'` for Releases, so a
   tombstone disappears from every operator and client surface without a new exclusion rule.

3. **Artists own Releases only when they are the sole primary Release artist.** An Artist
   deletion with `releasesAndFiles` retires the Releases whose primary billing is exactly the
   selected Artist set. A Release also billed as primary by another Artist is *kept*, and the
   count is reported in the preview and result, so deleting one half of a collaboration never
   destroys the other Artist's release.

4. **Files are removed before the tombstone is written, from durable evidence only.** The caller
   resolves each Release's `release_publication_plans.target_relative_path` and
   `release_work_item_staging_handoffs.staging_relative_path`, removes those trees with the
   shared no-follow deletion helper (ADR-0152), and only then commits the catalog transaction. If
   canonical storage is unconfigured the file scope fails closed with
   `503 delete_storage_unavailable`; if any affected Release lacks publication evidence the whole
   operation fails closed with `409 storage_evidence_missing` and nothing is changed.

5. **The Admin UI exposes both actions beside merge on both lists.** **Catalog → Artists** and
   **Catalog → Releases** each get a bulk **Delete** action next to **Merge**, selecting the same
   page-scoped rows. Delete opens the shared destructive dialog (impact counts, storage impact,
   mandatory reason, typed `DELETE`) with a scope choice; the catalog-only scope shows zero
   affected Releases/Tracks/files because nothing but the record is retired.

6. The API never deletes a Release that is `merged` or already `deleted`, and never deletes a
   `merged`, `deprecated`, or already-`deleted` Artist (`409 not_deletable`). A Release must be
   split before it can be deleted.

7. No native-v1, OpenSubsonic, or Jellyfin route, DTO, or status behavior changes.

## Alternatives considered

### Physically delete Artist rows and cascade

Rejected. `catalog_audit_events`, `release_merge_events`, policy evidence, and public-id
mappings reference Artists with `ON DELETE RESTRICT`; cascading would destroy the audit history
that makes every other correction trustworthy and would invalidate public ids that clients and
migration data still hold.

### Fold Artist deletion into Release Review's policy path

Rejected. Review deletion is a processing disposition for one Release and requires a pending
policy decision; it cannot express a multi-Release catalog cleanup and would force operators to
manufacture policy decisions for content that was already accepted.

### Always delete files, or always keep them

Rejected. Both cases are legitimate operator intents, and choosing one silently would either
strand unwanted media or destroy content an operator wanted to keep. The explicit scope choice,
shown with its exact impact, makes the destructive reach of the action unambiguous.

### Delete a Release whenever the selected Artist is any primary artist

Rejected. A collaboration Release billed to two primary Artists must not disappear because one
Artist identity was removed; the surviving Artist still owns it.

### Require a separate cleanup job

Rejected for the same reason ADR-0152 executes deletion synchronously: an authorization without
execution is misleading, and file removal is a bounded, idempotent per-Release operation.

## Rationale

Deletion is the inverse of merge, and both are catalog curation of the same identity graph.
Reusing the tombstone model keeps every guarantee the merge path established — public ids,
auditable evidence, immutable media paths — while giving operators the two scopes they actually
need. Scoping Artist ownership to sole-primary Releases prevents the one destructive surprise
that a naive cascade would produce.

## Consequences

### Positive

- Both catalog lists now offer delete beside merge with a consistent, impact-first dialog.
- Nothing is row-deleted: audit, credits, observations, provider evidence, and public ids remain
  verifiable after an operator deletion.
- File removal is fail-closed, evidence-derived, and never touches a tree outside configured
  roots.
- Artists deleted on their own leave every Release and file intact.

### Negative / trade-offs

- Deleted entries remain visible in internal evidence (audit, history) and in direct public-id
  lookups; they are hidden from browse and serving surfaces, not erased.
- Work items keep their last processing state; the catalog Release tombstone is what removes the
  content from serving. Re-ingesting the same source later creates a new identity, as with any
  other tombstoned Release.
- The `releasesAndFiles` Artist scope removes only sole-primary Releases; a collaboration Release
  must be deleted explicitly from the Releases list.

### Follow-up work

- Surface the same two scopes from the Artist detail and Release detail screens for single-entry
  deletion.
- An operator-facing report of tombstoned public ids per deletion event.

## Validation

- `crates/melodee-persistence/tests/catalog_delete.rs`: catalog-only Artist deletion keeps
  Releases/Tracks and hides the identity; releases-and-files deletion retires only owned
  Releases, retains evidence and recomputes Artist aggregates; a shared-primary Release is kept;
  bulk Release deletion retires the selection and keeps audit/credits.
- `apps/melodee-server/tests/admin_catalog_delete_api.rs`: administrator gate, preview counts,
  catalog-only apply, invalid reason/mode, `503 delete_storage_unavailable`,
  `releasesAndFiles` apply with a configured root, storage-ready reporting, and
  `409 not_deletable` on repeat deletion.
- `admin-ui/src/screens/catalog/CatalogListScreens.test.tsx` and
  `admin-ui/e2e/catalog.spec.ts`: both bulk actions, scope selection, impact counts, reason and
  typed confirmation, and the exact request payloads.
- `crates/melodee-persistence/tests/migrations.rs` and the full workspace gate keep the new
  `deleted` Artist status and rollback path honest.

## References

- [ADR-0120](ADR-0120-phase5-classification-novelty-and-policy-schema.md)
- [ADR-0152](ADR-0152-execute-confirmed-release-deletion-with-audited-tombstones.md)
- [ADR-0198](ADR-0198-multi-source-release-merge-in-the-admin-ui-and-admin-api.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
