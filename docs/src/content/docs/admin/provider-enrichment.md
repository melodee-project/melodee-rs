---
title: Provider enrichment operations
description: Monitor, troubleshoot, and recover asynchronous external-provider enrichment.
sidebar:
  order: 7
---

Provider enrichment is a durable background workflow. Catalog import schedules idempotent Artist
and Release metadata/artwork tasks and does not wait for the network. PostgreSQL is authoritative
for task state; the response cache is rebuildable evidence.

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
gate per provider. HTTP responses are capped, timeouts are configured per provider, and 429/5xx or
transient transport failures use a bounded retry count with capped backoff. Cancellation and
shutdown drop in-flight request futures; claimed database work is recovered by lease expiry.

## Diagnosing failures

Structured warning events include the task ID, entity, provider, purpose, attempt, error category,
and next retry time. Error categories distinguish configuration, timeout, transport,
rate-limited, malformed response, invalid image, cache, and persistence failures. No credential or
raw authorization header is included.

For direct database inspection, query `provider_enrichment_tasks` for the current state and
`provider_enrichment_attempts` for the full attempt history. A task with `status = 'retry'` is
waiting for `next_attempt_at`; `failed` means its bounded attempt budget was exhausted. A disabled
provider is recorded as a successful `disabled` attempt rather than treated as a catalog failure.

Provider failures do not quarantine or delete media. To recover after correcting a URL or
credential, leave the task queued for its next retry or re-run the bounded worker pass; repeating
the same import is safe because scheduling is unique and idempotent. An administrator artwork
override intentionally causes artwork work to complete without contacting the provider.

## Local MusicBrainz replica

`searchEngine.musicbrainz.storagePath` can point at the legacy materialized DecentDB file. The Rust
server reads the `Artist`/`ArtistAlias` search projection without writing it. If no local replica is
present, the configured MusicBrainz HTTP endpoint is used. A malformed or unavailable local file is
reported as provider evidence and does not block catalog import.
