---
title: Search maintenance and outbox retention
description: How indexed-track history, exact hourly statistics, pruning and rebuilds interact.
sidebar:
  order: 9
---

The search projection records every catalog change in `catalog_projection_outbox`. A row is marked
`processed_at` only after the Tantivy commit that made the document queryable, so the row is the
durable evidence of real index work. That evidence is useful for a short operational window; the
Admin activity chart only needs one number per hour.

## What is retained

| Data | Retention | Notes |
| --- | --- | --- |
| Pending, leased, retryable and permanently failed outbox rows | Until processed or resolved | Never pruned. |
| Successful (`processed_at`) outbox detail | 7 days by default, measured from `processed_at` in UTC | Configurable 2–90 days. |
| Exact hourly totals | 400 days | One row per projection and UTC hour, `count(DISTINCT Track)`. |

Configure the detail window with the ordinary Admin-editable setting
`search.processedOutboxRetentionDays` or
`MELODEE__SEARCH__PROCESSED_OUTBOX_RETENTION_DAYS` (range 2–90, default 7).

The "Tracks indexed per hour" chart counts distinct Track IDs successfully indexed. Two revisions
of one Track in an hour count once; the same Track in two hours counts once in each. The live partial
hour, the rolling 24-hour window and zero-filled hours keep working after pruning because a closed
hour is finalized from its full raw data before any of its rows can be deleted.

## How maintenance runs

The search worker runs one bounded maintenance pass per hour (at most 1,000 ordered rows per
transaction, yielding between batches). It:

1. finalizes every fully closed hour that is not finalized yet;
2. pauses without deleting while a projection has a requested, running or validated rebuild;
3. deletes eligible rows in ordered batches, recording the highest pruned ID in the same
   transaction;
4. ages out hourly totals older than 400 days while keeping the finalized-through watermark.

Deleting is safe because acknowledgements and hour finalization are serialized in PostgreSQL: an
acknowledgement that is in flight when an hour closes is counted, and a late commit cannot lose a
count. A durable per-entity tombstone suppresses a stale producer replay of an already-pruned
revision; a newer revision still enqueues.

## Operator preview and apply

A bare invocation or `--dry-run` shows the preview and changes nothing:

```bash
docker compose exec melodee melodee-cli search prune-outbox
docker compose exec melodee melodee-cli search prune-outbox --dry-run
```

`--apply` uses exactly the same selection, fencing and audit logic as the scheduled pass:

```bash
docker compose exec melodee melodee-cli search prune-outbox --apply
```

Both print JSON with `retentionDays`, `eligible`, `protected`, `failed`, `pending`, `leased`,
`oldestRetainedCompletion`, `lastFinalizedHour`, `lastPruneAt`, `lastPruneResult` and
`blockingRebuild`. `eligible` rows are deleted by `--apply`; `protected` rows are retained because of
age, checkpoint or hour-finalization rules; `failed` rows are preserved until an operator resolves
them.

If a rebuild is requested or resumable, pruning stops and `blockingRebuild` names the status. It
resumes after the rebuild reaches `activated` (or fails terminally and is replaced).

## Admin visibility

System → Search shows the retained days, eligible/protected counts, finalized-through hour, the last
retention pass and any blocking rebuild. `GET /api/admin/v1/search/status` returns the same values.
`GET /api/admin/v1/processing/enrichment` reports provider eligibility, runnable/dormant/failed
counts, oldest runnable job, backfill progress and manifest evidence bytes.

## Rebuilds

Pruning never blocks a rebuild and a rebuild never loses the chart. A rebuild snapshots the current
high-water mark, which accounts for both existing rows and the highest pruned ID. If a protected
rebuild leaves detail older than the retention window, the same acknowledgement and replay checks
clean it up without resurrecting an expired chart bucket.

## Troubleshooting

| Symptom | Meaning | Action |
| --- | --- | --- |
| `eligible` stays above zero after `--apply` | More than one bounded pass is required | Run `--apply` again; each pass is bounded by design. |
| `blockingRebuild` is set | A rebuild is requested/running/validated | Wait for it to activate, or inspect System → Search. |
| `failed` is nonzero | Permanently failed projection rows | Inspect `latestFailureMessage` in System → Search, correct the cause, then request a rebuild. |
| Chart hour is zero but work happened | The hour has not closed yet | The live partial hour comes from raw detail; it appears after the hour closes. |
