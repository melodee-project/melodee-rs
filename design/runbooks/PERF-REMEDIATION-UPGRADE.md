# Performance Remediation — Upgrade, Repair, and Rollback Runbook

Applies to the forward migrations `0060`–`0066` and the derived-state repair delivered with the
PERF-001…PERF-015 / V001 / V002 remediation. This is an internal operator runbook; the public
summary lives in `docs/src/content/docs/installation/upgrading.md`.

## What changes on disk and in PostgreSQL

| Migration | Change | Nature |
|---|---|---|
| 0060 | `tracks(playable_media_asset_id)`, `catalog_genre_terms(release_id, surface, display_value)`, `artist_match_decisions(artist_id, observed_artist_id)`, partial `native_artist_user_states(rating > 0)` indexes; drops three exact duplicate indexes | ordinary transactional DDL |
| 0061 | Replaces the genre aggregate refresh functions with an advisory-lock + fresh-recount protocol; adds an actual-status-change guard; adds a Track-release-move term-maintenance trigger; adds `catalog_genre_aggregate_discrepancies()` and `repair_catalog_genre_aggregates(apply, limit)` | functions + triggers |
| 0062 | `native_discovery_progress` with one durable cursor per discovery work class | additive table |
| 0063 | `search_rebuild_snapshot_outbox`, `search_rebuild_applied_outbox` | additive tables |
| 0064 | `catalog_identity_reconciliation_seed`; `lock_artist_identity_names(text[])` | additive table + function |
| 0065 | Adds a primary-credit ordering index; orders Track and recommendation candidate locks and replaces popularity maintenance triggers with statement-level grouped changes | ordinary transactional index + replacement functions and triggers |
| 0066 | Makes reconciliation Artist and Observation UUIDs non-owning hints; removes their FK checks without deleting pending seeds | transactional constraint removal |

Identity hints are resolved by indexed joins during reconciliation; deleted identities can leave
stale hints, which are ignored and removed when their seed is acknowledged. Hard deletion records
affected names and external keys in the same transaction before removing identity evidence. The
Release FK remains in place. Existing seeds survive the upgrade. Reverting this schema requires a
validated pre-upgrade restore: restoring the constraints alone can fail on stale hints and would
reintroduce the lock inversion this migration removes.

Migration 0065 aligns Artist popularity for repeated primary credits with the
existing discovery recount (one contribution per credit); previously the play
trigger counted that Artist once. Existing drift converges through scheduled,
bounded discovery maintenance; the migration itself does not recount. User Artist
play state still counts one play per distinct Artist.

No column or constraint is removed. The three dropped indexes are exact
duplicates of an existing constraint index; recreating them with their original `CREATE INDEX`
text restores the prior shape if a forward fix is ever needed.

## Required sequence

1. **Verify versions and backup readiness.** Record the current image digest and migration
   version. Take a PostgreSQL dump and a copy of the asset directory; verify with
   `pg_restore --list` and an isolated restore. A backup that has not been test-restored is not a
   rollback asset.
2. **Enter the ADR-0166 maintenance fence.** Stop **every** Melodee server and worker that can
   write to this database. Do not run `CREATE INDEX` while any writer is live. Leave one shell
   with the production configuration available.
3. **Apply forward migrations** with a single invocation:
   `melodee-cli migrate` (or start exactly one candidate server while traffic is fenced). The
   new indexes are built with ordinary `CREATE INDEX` inside the migration transaction, so write
   availability is lost for their duration; this is expected and is why the fence is required.
4. **Repair genre aggregates.**
   - Read-only check: `melodee-cli catalog repair-genres`.
   - Apply: `melodee-cli catalog repair-genres --apply`. The command takes the same advisory keys
     live maintenance uses, works in bounded pages, is idempotent, removes keys with no active
     contributor, and prints progress until it reports `checked=0`.
   - `melodee-cli catalog repair-genres` must report `genre aggregates are consistent` afterwards.
5. **Initialize durable discovery progress and identity reconciliation.** Migration 0062 seeds one
   row per discovery work class with a `nil` cursor, so the first pass starts at the beginning of
   each class. Migration 0064 creates the affected-identity seed queue; imports populate it in their
   authoritative transaction and the catalog scheduler drains it in coalesced closure passes, with a
   30-minute catalog-wide backstop. No manual action is required. If you upgrade from a binary that
   predates the queue, the backstop converges historical identity work even with an empty queue.
6. **Rebuild the search projection.** A normal rebuild now records the snapshot's visible outbox
   rows and replays every late/acknowledged obligation before activation. Trigger it as usual
   (`melodee-cli search rebuild --actor <operator>`, or the Admin action). Do **not** repair by replaying only
   currently pending outbox rows: previously acknowledged rows may already be missing from the old
   active generation, and the new protocol is what replays them.
   - The last valid generation stays active until the corrected replacement validates and
     activates.
   - If activation reports contention, retry; the fence is bounded and never activates stale.
7. **Verify.** Check `melodee-cli catalog status`, `melodee-cli search status`,
   `melodee-cli work list`, the health/ready endpoints, and an ordinary import of one known
   Release. Confirm genre counts match authoritative terms (the repair check) and that discovery
   cursors advance across two intervals.
8. **Resume workloads** in the documented order: PostgreSQL, then one server, then workers/replicas
   after the first reports healthy.

## Optional retention maintenance

Lease renewal events are high-frequency. Retention is opt-in and preserves all transition/failure/
completion history (ADR-0215):

```bash
melodee-cli work prune-lease-events --older-than-days 30            # read-only count
melodee-cli work prune-lease-events --older-than-days 30 --apply    # bounded, resumable
```

## Concurrency and maintenance requirements

| Command | Requires exclusive maintenance? | Safe with live writers? |
|---|---|---|
| `melodee-cli migrate` | Yes (ADR-0166) | No |
| `melodee-cli catalog repair-genres` (check) | No | Yes, read-only |
| `melodee-cli catalog repair-genres --apply` | Recommended | Yes — takes the live advisory keys, but a maintenance window keeps counts stable for verification |
| Search rebuild | No | Yes — snapshot + commit-safe catch-up + activation |
| Discovery maintenance | No | Yes — durable cursors, transaction-fenced |
| Identity reconciliation drain | No | Yes — durable seeds, session-fenced, bounded iterations |
| `melodee-cli work prune-lease-events --apply` | No | Yes — only lease_renewed rows, advisory-fenced batches |

## Interrupted operation

- **Migration interrupted:** SQLx rolls back the migration transaction; restart with the fence
  held. A failed ordinary `CREATE INDEX` leaves no invalid index because it is transactional.
- **Repair interrupted:** rerun `--apply`; completed pages are already correct and the command
  resumes from the remaining discrepancies.
- **Rebuild interrupted:** the generation remains `building`/`validated`; the next claim resets a
  `building` generation or resumes catch-up for a `validated` one. Applied replay rows are durable,
  so resume does not re-apply them.
- **Discovery interrupted:** the cursor is advanced in the same transaction as the batch effect; a
  crash replays an idempotent batch at worst.

## Rollback

There is no automatic down migration. Rolling back requires restoring the **pre-upgrade database**
and the matching old server image. The additive tables from 0062/0063 are inert under an older
binary. The replaced functions in 0061 would be restored by the pre-upgrade database. Never edit an
applied migration; if a forward correction is needed, add a new migration.

## Mixed-version operation

Do **not** run an old worker against a database migrated to 0065. The old binary does not record
snapshot-visible outbox rows and would activate a rebuilt generation under the old (unsafe)
watermark check. Stop all writers, upgrade, verify, then resume. This is a stop-all-writers
maintenance upgrade.
