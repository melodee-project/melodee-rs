# ADR-0211: Hard delete catalog entities and files on operator confirmation

- **Date:** 2026-11-30
- **Status:** Accepted
- **Decision owners:** Catalog curation, Admin API, Admin UI
- **Supersedes:** ADR-0199, ADR-0197
- **Superseded by:** None
- **Related requirements:** PRD-API-ADMIN-002, PRD-CATALOG-001, PRD-POLICY-007,
  PRD-UI-005, PRD-UI-008

## Context

ADR-0199 made catalog Artist and Release deletion a tombstone: public ids, Tracks, credits,
observations, policy evidence, and per-user state survived, and the operator chose between
`catalogOnly` and `releasesAndFiles` after supplying an audit reason and typing `DELETE`. The
operator has now asked for the opposite semantics:

- an administrator or editor can delete any Artist, Release, or Track at any time, for any
  reason or no reason;
- a single confirmation dialog is the whole workflow;
- the confirmed action is a hard delete of the database rows and every file the entity owns;
- the Processing Queue bulk action deletes the selected Releases directly, with no
  failed-only restriction, purge stage, or additional review decision.

The previous semantics also contradicted the operator's expectation on the two screens they use
for cleanup: the Release detail page had no Delete action, and the Processing Queue bulk action
only accepted `failed` pre-catalog work items.

## Decision

1. **Hard deletion replaces catalog tombstones for Artists, Releases, and Tracks.** The Admin
   UI exposes Delete on the Artist, Release, and Track detail screens, in both catalog list
   bulk actions, and as a new Track list bulk action. Each action opens one confirmation dialog
   showing the exact affected counts. Confirmation is the only additional input: the reason is
   optional (a fixed audit note is recorded when blank), there is no scope choice, typed phrase,
   or second stage.

2. **The delete surface is one Admin API family**, available to administrators and editors:

   ```http
   POST /api/admin/v1/catalog/artists/delete/preview
   POST /api/admin/v1/catalog/artists/delete
   POST /api/admin/v1/catalog/releases/delete/preview
   POST /api/admin/v1/catalog/releases/delete
   POST /api/admin/v1/catalog/tracks/delete/preview
   POST /api/admin/v1/catalog/tracks/delete
   POST /api/admin/v1/ingest/work-items/delete
   ```

   Preview is read-only and reports the exact rows and files that apply will remove. Delete
   accepts one to 100 distinct public ids, verifies the current rows under lock, and returns the
   deleted public ids. The existing `purge` route remains as a pre-catalog compatibility surface.

3. **Files are removed before the database commit, from durable evidence only.** Release
   deletion removes the publication bundle and retained staging tree when publication evidence
   exists, otherwise the canonical media files recorded for its Tracks. Track deletion removes
   only media files that no other Track still references. Artist deletion applies the Release
   rules to every Release the selected Artists own alone (primary billing with no other
   primary Artist outside the selection). Artwork is content-addressed and is unlinked after the
   commit only when no other row references its asset key. If required storage is not configured,
   or a no-follow removal fails, the operation fails closed and changes no catalog row.

4. **A hard deletion removes the processing work item and its evidence.** Tracks, media links,
   artist billing, observations, contributions, classification/novelty/policy evidence, provider
   evidence, per-user state, playlist/playback rows, projection outbox rows, merge/split history,
   and the originating work item are deleted in one transaction. Catalog evidence tables keep
   their append-only triggers for every ordinary path; the hard-delete transaction sets the
   local `melodee.hard_delete` flag that migration 0056 recognizes. One new
   `catalog_audit_events` row per deleted entity records the actor, optional reason, and that the
   deletion was a hard delete, so accountability outlives the removed rows. A delete outbox entry
   for each removed Release, Track, and Artist keeps the rebuildable search projection from
   retaining a deleted document until the next full rebuild.

5. **The Processing Queue bulk action deletes the selection directly.** Selecting any rows and
   choosing **Bulk actions → Delete → Delete** confirms the selected count. A work item bound to
   a catalog Release is routed through the shared Release hard deletion; a pre-catalog work item
   is deleted at any lifecycle state, including `needs_review`, `quarantined`, or `failed`.
   Storage cleanup still uses only paths derived from durable handoff/plan evidence beneath
   configured roots.

6. **Editors can reach the catalog surfaces needed for this action.** The Admin UI lets an
   editor open the Artist, Release, and Track catalog lists and detail pages and confirm
   deletion. Catalog reads use an administrator-or-editor extractor; metadata editing, artwork
   upload/removal, merge, identity operations, and every other administration surface remain
   administrator-only.

7. No native-v1, OpenSubsonic, or Jellyfin route, DTO, status code, or identifier behavior
   changes.

## Alternatives considered

### Keep tombstones and add a separate hard-delete endpoint

Rejected. The operator explicitly asked that confirmation remove the rows and files. Keeping
two destructive meanings on the same screen would make the weaker tombstone path easy to choose
by mistake and would leave cleanup work unresolved.

### Delete rows but retain files

Rejected by the requested behavior. A catalog row without its media is a broken identity, and
the operator asked for the files to be removed as part of deletion.

### Require a reason or typed phrase

Rejected. The confirmation dialog already identifies the exact entity and count; the authenticated
actor and durable `catalog_audit_events` row preserve accountability. The reason field remains
available in the API for scripts that want to add context.

### Delete only a merged or non-published entity

Rejected. The operator asked for deletion at any time. The transaction re-locks the rows and
fails closed only when the selection changed concurrently.

### Use per-table cascades instead of ordered statements

Rejected. Several catalog relations are `RESTRICT` by design, including the mandatory playable
asset cycle between `tracks` and `track_media_assets`. An explicit ordered transaction documents
the full reach and keeps accidental cascades out of unrelated write paths.

## Consequences

### Positive

- One confirmation clears an unwanted Artist, Release, or Track from the database and storage.
- The Processing Queue can discard any selected work item or cataloged Release without a
  failed-only precondition or a second review decision.
- The database no longer accumulates unreachable tombstones after an operator cleanup.
- Audio, staging, canonical bundles, and artwork are removed only below configured roots and
  only from durable path evidence.

### Negative / trade-offs

- **There is no undo.** Public ids, Tracks, credits, observations, per-user state, and audit
  history for the deleted entity are gone. The only surviving record is the deletion audit row.
  Operators should take a coordinated database and media backup before bulk cleanup.
- A hard-deleted Release cannot be re-ingested under its former public id; a later import creates
  a new identity.
- Deleting an Artist also deletes every Release it owns alone. Releases billed as primary by
  another Artist are kept; the confirmation reports that retained count.
- Hard deletion of catalog evidence requires the local `melodee.hard_delete` flag and migration
  0056. Rolling back the migration restores the append-only trigger behavior but cannot restore
  rows or files removed by an authorized hard delete.

### Follow-up work

- Operator-facing export/backup guidance for pre-deletion protection.
- A report of hard-deleted public ids and asset paths per audit event.

## Validation

- `crates/melodee-persistence/tests/catalog_hard_delete.rs`: Release hard deletion removes its
  rows, work item, media link, and observations; Track hard deletion keeps the Release and
  recomputes its aggregate; Artist hard deletion removes owned Releases and Tracks.
- `apps/melodee-server/tests/admin_catalog_delete_api.rs`: non-operator denial, editor acceptance,
  optional-reason deletion, storage fail-closed behavior, release/track bulk deletion, and
  repeat-selection `404`.
- `apps/melodee-server/tests/admin_purge_api.rs`: Processing Queue deletion of a cataloged Release
  and pre-catalog item deletion in a non-failed state.
- `admin-ui/src/screens/catalog/CatalogListScreens.test.tsx`,
  `admin-ui/src/screens/processing/ProcessingQueueScreen.test.tsx`, and detail-screen tests:
  the confirmation dialog contains no reason/typed-phrase/scope step, the exact request payloads
  are sent, and the queue offers deletion for any selected phase.

## References

- [ADR-0152: Confirmed Release deletion with audited tombstones](ADR-0152-execute-confirmed-release-deletion-with-audited-tombstones.md)
- [ADR-0197: Purge failed pre-catalog work items and owned storage](ADR-0197-purge-failed-pre-catalog-work-items-and-owned-storage.md)
- [ADR-0199: Administrator Artist and Release deletion from the catalog lists](ADR-0199-administrator-artist-and-release-deletion-from-the-catalog-lists.md)
- [ADR-0206: Allow confirmed administrator and editor deletion in Release Review](ADR-0206-allow-confirmed-administrator-and-editor-deletion-in-release-review.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
