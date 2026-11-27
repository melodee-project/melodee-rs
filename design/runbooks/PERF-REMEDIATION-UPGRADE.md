# Performance Remediation — Upgrade, Repair, and Rollback Runbook

Applies to the forward migrations `0060`–`0071` and the derived-state repair delivered with the
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
| 0067 | Adds fenced inbound scan leases, scan markers, path-format versioning, exact-equality hash-assisted occurrence lookup and operator-review evidence; replaces the old full-path unique index with serialized exact identity checks | transactional DDL and ordinary index build under the maintenance fence |
| 0068 | Adds incomplete-capture markers and exact-nanosecond metadata to settling evidence, plus indexes for per-scope recovery and ordered file pages | additive transactional DDL; ordinary index builds require the maintenance fence |
| 0069 | Adds a Jellyfin Track-number/public-ID ordering index | ordinary transactional index build under the maintenance fence |
| 0070 | Adds durable inbound custody intent/evidence rows for the proposed quarantine protocol, including source-root generation and distinct native-path and legacy-receipt digests | additive transactional DDL; filesystem handoff and recovery remain in progress |
| 0071 | Adds a quota-controlled provisional custody-hash metadata spool, retained-source obligations, and pre-catalog purge guards | additive transactional DDL; custody traversal, receipt binding and independently scheduled orphan cleanup remain in progress |

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

No column is removed. Migration 0066 removes the two foreign-key constraints described above.
The three dropped indexes are exact duplicates of an existing constraint index; recreating them
with their original `CREATE INDEX` text restores the prior index shape if a forward fix is needed.

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
- **Discovery interrupted:** discovery maintenance cursors advance with their batch effects. Inbound
  source scans leave a temporary leased generation; after lease expiry the next scan rescans the
  source and fences the prior owner. Recovery clears prior observation bits in bounded pages and
  rescans the root; completed work markers are retained and revalidated. A one-shot scan reports a
  busy error after 30 seconds if another owner remains active; retry it. Do not manually delete an
  active scan. Work in progress: migration 0068 stages snapshot metadata directly in the evidence
  tables and publishes it only after verification; interrupted captures are invisible to normal
  snapshot readers. Recovery removes no more than 128 file rows per transaction in bounded cleanup turns;
  expired, removed source scopes require the scheduler cleanup path. This remains subject to final
  integration and recovery tests. Database disk-full errors roll back the affected page; restore
  capacity and retry. Incomplete metadata is limited globally by the singleton
  `ingestion_settling_capture_budget` row (default 4,096 captures, 1,000,000 entries and 1 GiB of
  logical metadata, including fixed per-capture and per-row allowances). This is not a PostgreSQL heap, index, or WAL disk quota.
  A Release that exceeds either limit remains invisible and retryable: after assessing available
  PostgreSQL disk capacity, an operator can raise `max_captures`, `max_rows`, and/or `max_bytes` in that row and
  retry the scan; raising limits does not discard or alter evidence. Completed authoritative
  snapshot evidence is retained separately and may grow with the Release's file count. Limits,
  counters, and cleanup accounting remain subject to final integration tests.
- **Historical non-Unicode inbound paths:** work in progress under ADR-0220. Previous lossy text
  can map multiple physical paths to one work item. Preserve existing rows, media, and receipts;
  do not infer a raw path or delete history from the display value. `melodee-cli ingest legacy-paths`
  lists up to 100 unresolved rows; pass `--after` with the last ID for another page. For a reviewed
  version-1 occurrence, run `melodee-cli ingest verify-staged <work-item-id>` to reread its staged
  tree and compare the printed BLAKE3 tree hash with independently reviewed original-media evidence.
  After confirming the source, occurrence and receipt, run `melodee-cli ingest resolve-legacy-path
  --legacy <old-id> --reviewed <new-id> --receipt-tree-hash <hash> --actor <operator> --reason
  <evidence-summary>`. This local administrative command rechecks the receipt and records the
  supplied actor and reason in durable audit evidence; the actor is an operator assertion, not an
  authenticated identity. It retires only an unleased, un-staged legacy duplicate, and never
  deletes media. A conflicting review is rejected and an identical retry is idempotent. Existing
  legacy rows with verified staging receipts recover from that staging without deleting the
  ambiguous inbound path. These changes remain under validation and security review.
  Resolution audit retains non-owning historical work-item UUIDs after legitimate catalog
  deletion. Until a dedicated audited path-purge protocol is implemented, the administrative
  pre-catalog purge refuses both lossy and encoded occurrence paths before removing any files;
  do not work around this by deleting storage or audit records manually.
- **Retained-source reobservation:** work in progress under migration 0071. A retained obligation
  is not permission to delete source media. The scheduler uses a separate leased, oldest-due
  queue and leaves failed, missing, or ambiguous sources for retry. Its read budget is controlled
  by `MELODEE__INGEST__RETAINED_RECHECK_MIB_PER_SECOND` (default 8 MiB/s per process), and
  `MELODEE__INGEST__RETAINED_RECHECK_INTERVAL_SECONDS` controls the delay after a stable
  observation (default 300 seconds, maximum 30 days). Multiple configurations in one process
  use the strictest read rate until restart. This paces application reads, not filesystem readahead.
  The defaults remain provisional until representative mixed-load measurement; receipt activation,
  interrupted-copy recovery, and fixed-crash traversal fairness remain incomplete. Do not manually
  remove source media or unfinished custody evidence to clear the queue.
  For read-only review, `melodee-cli ingest retained-obligations --limit 100`
  lists active rows ordered by work-item ID, with source-library identity,
  exact stored source path and encoding version, baseline/change state,
  retry/unreadable reasons and counts, unsupported-directory reason and retry
  time, lease state, and due information. Use the returned
  `nextAfter` as `--after` to continue; restart at the beginning to capture rows
  inserted ahead of the cursor. JSON paths may be sensitive. Preserve encoded
  `//melodee-unix-hex:` and `//melodee-windows-u16:` strings exactly; a
  format-0 path containing U+FFFD is ambiguous and must never be decoded by
  guessing. This listing does not verify or change the source, create a
  successor, or implement safe copy-only retained intake. An active lease is
  not proof of process liveness, and a missing source is not permission to
  remove its obligation.
- **Interrupted pre-catalog purge:** under the stop-all-writers maintenance fence, preserve and
  independently back up affected source, staging, work, and canonical media plus PostgreSQL.
  Inspect those locations and the work-item evidence before deciding whether to release a guard;
  the guard records intent, not the outcome of filesystem operations. `melodee-cli ingest
  purge-guards` lists at most 100 guards in work-item-ID order; use `--after <last-id>` for the
  next page. After recording the exact ID, fence token, and owner, use `melodee-cli ingest
  recover-purge-guard --work-item-id <id> --fence-token <token> --guard-owner <owner> --actor
  <operator> --reason '<inspection and decision>' --stopped-all-writers --inspected-media --backup-verified
  --confirm`. The command requires these operator assertions, transactionally checks the exact
  guard and rejects live occurrence leases, audits the decision, and removes only that guard. It
  does not delete media or work-item rows or retry the purge. The database cannot prove an old
  external process stopped; if a writer, filesystem outcome, or identity is uncertain, keep the
  guard and investigate. Fence tokens and actor names are operator evidence, not authentication.
  A pre-catalog purge with an existing inbound source and no staging handoff currently fails
  before filesystem mutation with `source_custody_required`; occurrence-bound source deletion
  remains unfinished. This is a temporary compatibility restriction, not a completed remediation.

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

### Planned lifecycle-lock upgrade

The lifecycle-lock upgrade and its repair tooling are **not complete**. Do not enable protected
intake or treat a reservation, a database lease expiry, or a lock-registry row as proof that an
older writer has stopped. The intended upgrade requires the operator to stop every server, worker,
script process and older binary that can access the affected media, verify that they have actually
terminated, and preserve verified database and media backups before provisioning begins. The
maintenance fence must remain in effect across interruption and until all upgraded writers are
known to honor the same per-work-item lock; old binaries must not be restarted afterward.

Provisioning is intended to reserve each identity before exclusively creating its lock in a
validated private namespace, record the exact filesystem identities only after durable creation,
and progress in bounded replayable pages. An existing unacknowledged lock name is ambiguous: keep
it and the associated media intact for operator review instead of adopting, replacing, or deleting
it. Missing roots, uncertain ownership, unsupported filesystem behavior, and unverified stage
identity must likewise remain quarantined. A later completed runbook will provide the actual
commands, explicit fence/recovery procedure and verification evidence; this text does not authorize
or implement the upgrade.

The lifecycle-lock prototype includes a candidate for distinct supported local filesystem devices
with no relevant nested mounts. One owned different-device lock test has passed; full provisioning,
recovery, worker and deletion integration across filesystems remain unverified. Same-device
different-mount aliases and unsupported or ambiguous topologies fail closed. Cross-filesystem
support is unfinished and remains part of the remediation.
