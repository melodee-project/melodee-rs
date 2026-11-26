---
title: Provider enrichment operations
description: Monitor, troubleshoot, and recover asynchronous external-provider enrichment.
sidebar:
  order: 7
---

Provider enrichment is a durable background workflow. Catalog import schedules idempotent Artist
and Release metadata/artwork tasks and does not wait for the network. PostgreSQL is authoritative
for task state; the response cache is rebuildable evidence.

## Which providers are scheduled

An external provider is **eligible** when it is enabled in effective settings *and* every required
credential resolves to a non-blank value. A blank or whitespace-only secret counts as "not
configured". MusicBrainz is the local option and needs no credential. Built-in Melodee search stays
available in interactive search but never creates background work.

Eligibility is recorded once per installation as booleans plus a revision — never credentials —
and refreshed at startup, on settings changes and at least once per minute. The capability
endpoint, the catalog importer, the backfill and the worker all use that one record.

- An import schedules work only for eligible providers. With three of nine providers eligible,
  one new Artist and one new Release create **12** tasks; with none eligible the import creates
  zero tasks and still reaches playable, cataloged media.
- Repeating an import creates no additional tasks.
- Enabling a provider later starts one durable backfill per revision. It walks live Artists and
  Releases by stable keyset in pages of 250 and creates only the missing purposes, so the existing
  catalog is covered without a re-import. Disabling fences a running scan.
- Disabling a provider reconciles the installed backlog: never-attempted queued rows are removed,
  historically attempted unfinished work and `disabled`-only successes become `dormant`, genuine
  successes and exhausted genuine failures are preserved. Dormant rows keep their task ID,
  attempt history and retry budget, are not claimed, and become runnable exactly once when the
  provider is enabled again.
- A disablement observed before any request releases the claim without burning an attempt or
  recording a `disabled` attempt. Requests already in flight finish under their lease.

Provider eligibility, runnable/dormant/retry/failed counts, oldest runnable age and backfill
progress are available at `GET /api/admin/v1/processing/enrichment` and in the Admin
System → Search panel. Enrichment lag is reported separately from time to playable: a Release is
playable as soon as it is cataloged, whether or not a provider has answered.

## Task lifecycle

Each task records its provider, purpose, request key, attempt count, status, lease, next attempt,
last error category, and last error message. Attempt history is append-only. Claims use bounded
`FOR UPDATE SKIP LOCKED` leases, so a process crash leaves work reclaimable after lease expiry.

Successful work writes `external_provider_cache` with a provider/purpose cache key and freshness
timestamp. A fresh cache entry suppresses a provider request. Expired entries are refreshed by the
worker and old cache rows are removed by the worker's bounded cleanup pass.

## Worker controls

The worker is enabled by default and can be bounded with:

| Environment variable | Default | Effect |
| --- | --- | --- |
| `MELODEE__ENRICHMENT__WORKER__ENABLED` | `true` | Enable or disable provider enrichment without disabling catalog import. |
| `MELODEE__ENRICHMENT__WORKER__POLL_INTERVAL_SECONDS` | `15` | Delay between claim passes. |
| `MELODEE__ENRICHMENT__WORKER__BATCH_SIZE` | `8` | Maximum tasks claimed per pass, capped at 32. |
| `MELODEE__ENRICHMENT__WORKER__LEASE_SECONDS` | `120` | Lease duration, bounded from 5 to 900 seconds. |

The runtime enforces a process-wide limit of eight provider operations and a separate two-operation
gate per provider. Metadata responses are limited to 256 KiB and downloaded images to 10 MiB,
including responses without a Content-Length header. Oversized streams are stopped as soon as
they exceed the limit. Images must also fit 16,777,216 pixels and a 64 MiB decode allocation
budget. Timeouts are configured per provider, and 429/5xx or
transient transport failures use a bounded retry count with capped backoff. Cancellation and
shutdown drop in-flight request futures; claimed database work is recovered by lease expiry.

## Diagnosing failures

Structured warning events include the task ID, entity, provider, purpose, attempt, error category,
and next retry time. Error categories distinguish configuration, timeout, transport,
rate-limited, malformed response, invalid image, cache, and persistence failures. No credential or
raw authorization header is included.

For direct database inspection, query `provider_enrichment_tasks` for the current state and
`provider_enrichment_attempts` for the full attempt history. A task with `status = 'retry'` is
waiting for `next_attempt_at`; `failed` means its bounded attempt budget was exhausted;
`dormant` means the provider is not eligible and the work is suspended with its history intact.
Rows written before this behavior (a successful `disabled` attempt) are reconciled to `dormant`
the first time eligibility is resolved.

Provider failures do not quarantine or delete media. To recover after correcting a URL or
credential, leave the task queued for its next retry or re-run the bounded worker pass; repeating
the same import is safe because scheduling is unique and idempotent. An administrator artwork
override intentionally causes artwork work to complete without contacting the provider.

## Local MusicBrainz replica

`searchEngine.musicbrainz.storagePath` can point at the legacy materialized DecentDB file. The Rust
server reads the `Artist`/`ArtistAlias` search projection without writing it. If no local replica is
present, the configured MusicBrainz HTTP endpoint is used. A malformed or unavailable local file is
reported as provider evidence and does not block catalog import.
