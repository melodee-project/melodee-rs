# ADR-0212: Revisioned provider eligibility and bounded enrichment scheduling

- **Date:** 2026-11-25
- **Status:** Accepted
- **Decision owners:** catalog/enrichment maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-EXT-001, PRD-EXT-002, PRD-EXT-003, PRD-EXT-004,
  PRD-WORK-003, PRD-INGEST-001, PRD-INGEST-002, PRD-INGEST-008, PRD-INGEST-009,
  PRD-INGEST-010, PRD-PERF-001, PRD-PERF-006
- **Amends:** ADR-0162 (provider attempt history and lease semantics remain)

## Context

Production on 2026-11-25 held 153,320 unattempted enrichment jobs whose oldest
was 15.7 hours old. In one hour 23,886 were created and only 1,594 completed.
Six providers accounted for 39,674 historical attempts whose outcome was
`disabled`, i.e. work that could never have produced evidence.

The cause was structural, not a tuning problem:

1. Every catalog import scheduled metadata and artwork work for **all nine**
   providers, whether or not the provider was enabled or had credentials.
2. Eligibility was resolved per task, per attempt, in the worker, so nothing
   could join against it when scheduling.
3. "Disabled" was a terminal success outcome, so a provider that was off
   burned the whole queue and recorded history that looked like work.
4. There was no durable, bounded way to cover the catalog when a provider was
   enabled later, so operators either re-imported or lived with gaps.

## Decision

1. **Eligibility is an installation fact, not a per-task lookup.** A small
   table records, per external provider, `metadata_eligible`, `artwork_eligible`
   and the revision at which the values last changed. A singleton row holds the
   installation revision and the last resolver diagnostic. Only booleans are
   stored; credentials never leave the settings resolver.
2. **One shared predicate.** `resolve_provider_eligibility` is defined once, in
   `melodee-persistence/src/providers.rs`, and used by the capability endpoint,
   the importer, the backfill and the worker. A provider is eligible when its
   enabled flag resolves true through the ordinary precedence (admin override →
   environment → config file → database → default) and every credential resolves
   to a non-blank value. Blank and whitespace-only secrets are "not configured".
   MusicBrainz is the documented local option and needs no credential. Built-in
   Melodee search stays interactively available and never creates background
   work.
3. **Scheduling is two set-based statements inside the catalog transaction.**
   The touched Artist IDs and the Release ID are deduplicated and joined to
   `provider_eligibility`; the existing metadata and artwork purposes are
   inserted with `ON CONFLICT DO NOTHING` against the existing scheduling key.
   Statement count is independent of entity and provider count. No network
   I/O, capability resolution or catalog scan runs on this path. When the
   eligibility table is empty (a new installation, or a schema that predates
   this ADR), scheduling keeps every known provider so media becomes playable;
   the first reconciliation catches up.
4. **Unknown eligibility is explicit.** An empty table means "not resolved
   yet". A resolved table is authoritative. A resolver failure records a
   diagnostic and retains the last known state; it cannot silently disable
   enrichment.
5. **Backfill is durable and revisioned.** Transition to eligible creates one
   `provider_enrichment_backfills` row per provider revision and entity type.
   Each step scans live Artists/Releases by stable UUID keyset in pages of 250
   and advances the cursor in the same transaction that enqueues the page.
   Uniqueness plus `ON CONFLICT DO NOTHING` makes a crash-replayed page a
   no-op, and a completed scan is never restarted. Disabling fences an
   in-progress scan. Concurrent reconcilers serialize on the installation row;
   backfill pages use `FOR UPDATE SKIP LOCKED`.
6. **Disablement reconciles the installed backlog in bounded transactions.**
   - A never-attempted `queued` row is deleted only after proving
     `attempt_count = 0`, no lease and no attempt rows.
   - Historically attempted unfinished work, and a `succeeded` row whose only
     outcome was `disabled`, becomes `dormant`.
   - `dormant` rows keep their task ID, attempt history, attempt numbering and
     retry budget; they are excluded from runnable counts and claims.
   - Re-enabling revives dormant work to `queued` exactly once and starts the
     revisioned backfill that covers entities created while the provider was
     off.
   - Genuine successes and exhausted genuine failures keep their state.
7. **The claim re-checks eligibility.** The claim statement joins
   `provider_eligibility` and only scans runnable/recoverable shapes
   (`queued`/`retry` ready, or `running` with an expired lease). A disablement
   observed before provider I/O releases the claim, restores the attempt
   counter and writes no attempt, so no `disabled` history is fabricated.
   In-flight requests finish under existing lease and override checks.
8. **One capability snapshot per batch.** The worker resolves the nine
   providers once per pass, reuses process-wide and per-provider semaphores,
   refills a full batch promptly (bounded drains per tick), advances a bounded
   number of backfill pages, and retains backoff, rate-limit, cancellation and
   idle polling behavior.
9. **Long-term invariant:** at most one scheduling key per distinct eligible
   entity/provider/purpose, regardless of repeated imports and settings
   refreshes. Attempt history remains append-only under ADR-0162.

## Alternatives considered

### Resolve eligibility in the worker only (status quo)

Cheap to keep, but it cannot prevent queue growth, and it produced 39,674
`disabled` attempts that were indistinguishable from real work.

### Delete all work for an ineligible provider on disable

Simple and bounded, but destroys attempt history and retry budgets that
ADR-0162 requires for diagnosis, and loses the queue when the provider is
re-enabled.

### Backfill with a full catalog scan per enablement

Rejected: it contradicts PRD-PERF-006 and cannot be bounded, restarted or
fenced.

## Rationale

Eligibility is a property of the installation and the provider, not of a task.
Recording it once lets scheduling, claiming, backfill and operator views agree
without a catalog scan, and it makes "no eligible provider" a genuinely free
path. Dormant work is the smallest state that preserves diagnosis and retry
semantics across disablement.

## Consequences

### Positive

- Imports cost two bounded statements regardless of catalog and provider count.
- An installation with three of nine providers eligible schedules a third of
  the work (measured acceptance: 12 jobs for one Artist and one Release).
- Enabling a provider later covers the existing catalog automatically.
- The runnable queue no longer fills with unreachable-provider work.

### Negative / trade-offs

- One more table and two revision columns; eligibility must be refreshed
  (throttled to once per minute) by a running worker.
- Disablement deletes never-attempted rows, so a certificate of "what might
  have run" is not kept for work that never started.
- Historical `disabled` attempts remain in the append-only attempt log as
  evidence of the old behavior.

### Follow-up work

- Materialized per-provider dashboards in the Admin UI beyond the JSON
  endpoint (the API fields are implemented).
- Provider-specific quotas beyond the existing per-provider semaphore.

## Validation

- `crates/melodee-persistence/tests/provider_eligibility.rs`: 12/0/24-hour
  acceptance counts, idempotent repeated import, zero-eligible import reaching
  `cataloged` with playable media, fourth-provider backfill, disable/enable
  races, dormant revival exactly once, disabled-only history, retry budgets,
  never-attempted removal, terminal failures, concurrent reconcilers, resolver
  failure retention, bounded/resumable pages, and shared settings resolution.
- `apps/melodee-server/tests/provider_enrichment_worker.rs`: an ineligible
  provider is never claimed and writes no attempt; 429 retries stay visible;
  fixture provider completion and cache hits.
- Catalog import assertions continue to pin the job count for an unknown
  eligibility state (36 for one Artist and one Release across nine providers).

## References

- [PRD.md](../../PRD.md), sections 37 (external metadata), 46 (CLI), 48
  (performance)
- [SPEC.md](../../SPEC.md)
- [ADR-0162](./ADR-0162-durable-provider-enrichment-tasks-and-release-external-evidence.md)
- [ADR-0213](./ADR-0213-store-full-manifest-evidence-on-persistent-file-storage.md)
