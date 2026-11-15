# ADR-0197: Purge failed pre-catalog work items and their owned storage

- **Date:** 2026-11-14
- **Status:** Accepted
- **Decision owners:** Ingestion operations, Admin API
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-ADMIN-002, PRD-INGEST-010

## Context

Operators can quarantine or ignore a failed work item, and an accepted catalog Release has a
durable, reversible deletion path through Release Review. Neither path removes a failed
**pre-catalog** import: the work item, its append-only processing log, its staging tree, its
normalization work area, and any still-present inbound candidate remain visible indefinitely.
That is useful evidence during triage but becomes permanent clutter once the operator has decided
the source is corrupt or unusable.

The requested operation is intentionally stronger than the existing `deleted` disposition:
"delete the release and job log like it never existed." A normal state transition cannot satisfy
that because the event history is the point of the queue.

Hard deletion is security- and storage-sensitive. It must never delete source media because a
classification is uncertain, never touch a cataloged Release, and never follow an unvalidated
path. The decision therefore needs an explicit, restricted purge contract rather than an
unbounded filesystem delete.

## Decision

1. Add an administrator-only bulk endpoint:
   `POST /api/admin/v1/ingest/work-items/purge`.
   It accepts 1–100 internal work-item ids, a non-blank reason, and `confirm: true`. The Admin UI
   additionally requires a typed `DELETE` confirmation.
2. Only work items in the `failed` state are eligible. Any other state is reported per item as
   `invalid_state` and left untouched.
3. An item with a `catalog_imports` binding is refused as `cataloged`. Catalog identity uses the
   Release Review deletion path, with its policy decision, tombstones, and audit rules. The purge
   endpoint must never orphan a catalog Release or regenerate public identity.
4. Storage deletion happens before the database transaction and touches only relative paths
   derived from durable work-item evidence below configured roots:
   - publication plan `target_relative_path` below the canonical root;
   - staging handoff `staging_relative_path` below the staging root;
   - `<work_item_id>` below the normalization work-area base;
   - the item's `current_path` below the matching inbound source root, only when no staging
     handoff exists (a released inbound path may now belong to a new arrival; the staging tree is
     then authoritative).
   Symlinks, special files, traversal components, empty/absolute paths, and ambiguous inbound
   roots fail closed. Missing trees are idempotent success so a retry can finish the durable
   delete after a storage or process failure.
5. The database purge runs in one transaction: it re-checks `failed` and the absence of a catalog
   import, deletes publication journal, receipts, plans, and authorizations in dependency order,
   then deletes the work item. Cascades remove the processing event log, manifests, settling
   snapshots, and staging handoff.
6. One independent `catalog_audit_events` row records `work_item_purged` with the acting
   administrator, reason, and removed relative paths. It deliberately survives the deleted
   processing history so the destructive operation remains attributable.
7. The Admin UI exposes the operation only when every selected row is `failed`, through
   **Bulk actions → Delete selected**, with an impact summary, required reason, and typed
   confirmation. Partial batches report each refusal and leave those rows in the queue.

## Alternatives considered

### Mark the item `deleted` like a catalog Release

Rejected: it keeps the processing log and staging/work trees, which is exactly what the operator
asked to remove. The catalog Release deletion path also assumes a policy decision that pre-catalog
failures do not have.

### Delete directly from the Admin UI or CLI without a server contract

Rejected: it bypasses root validation, the administrator gate, the catalog-binding check, and
audit accountability. The server must be the authority for which paths are owned.

### Delete source/inbound directories unconditionally

Rejected: an operator may select the wrong item or an item whose path changed. The purge validates
configured roots, relative components, and unambiguous source-root selection, and refuses rather
than guessing.

### Keep a tombstone work item with an empty log

Rejected: it cannot satisfy "like it never existed" and still leaves a row an operator must
filter. The independent audit row preserves the accountability that a tombstone would provide.

## Rationale

This is the smallest operation that gives operators a real cleanup path without weakening the
catalog identity model. It is limited to the one state where no catalog binding can exist or be
created, removes exactly the configured/owned trees the pipeline created, and records the
destructive action outside the deleted evidence.

## Consequences

### Positive

- Failed pre-catalog garbage can be removed completely, including files and processing history.
- Cataloged Releases remain protected by the existing policy-deletion contract.
- Path traversal and ambiguous roots fail closed.
- The purge is attributable after all processing rows are gone.

### Negative / trade-offs

- The purge is irreversible; the operator loses work-item evidence after confirming.
- Storage and database deletion are two phases. A storage failure leaves the item recoverable; a
  database failure after storage deletion leaves an item whose trees are already gone, which is
  safe to retry.
- Bulk requests report partial success rather than all-or-nothing. The UI explains and refreshes.

### Follow-up work

- If operators need purge evidence in the Admin UI, add a read-only purge-audit view backed by
  `catalog_audit_events`; the current record is available through diagnostics/database tooling.

## Validation

- `apps/melodee-server/tests/admin_purge_api.rs` covers the administrator gate, confirmation
  requirement, storage and event-log deletion, surviving audit evidence, per-item invalid-state
  refusal, and refusing a cataloged Release even when its work item was marked failed.
- `admin-ui/src/screens/processing/ProcessingQueueScreen.test.tsx` covers selection, the two-step
  bulk action, typed confirmation, and the exact API call.
- `admin-ui/e2e/processing-queue.spec.ts` drives the real UI at the canonical viewport.
- The repository full gate (`make check`) remains required.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [Admin API — Purge failed work items](../../docs/src/content/docs/api/admin-api.md#purge-failed-work-items)
- [Processing — Bulk purge of failed items](../../docs/src/content/docs/admin/processing.md#bulk-purge-of-failed-items)
- [ADR-0152](ADR-0152-execute-confirmed-release-deletion-with-audited-tombstones.md)
