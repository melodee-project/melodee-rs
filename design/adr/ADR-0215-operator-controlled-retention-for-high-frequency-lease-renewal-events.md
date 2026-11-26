# ADR-0215: Operator-controlled retention for high-frequency lease renewal events

- **Date:** 2026-11-26
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-010, PERF-011

## Context

`release_work_item_events` is the append-only operator history for a Release work item. It holds
low-volume, high-value transition evidence (`created`, `transitioned`, `failed`, `completed`,
`retry_scheduled`, lease acquisition/release) and high-volume `lease_renewed` rows written on every
heartbeat. A busy worker renews a lease every few seconds per active item, so the table grows with
uptime and active work, not with the number of items.

The existing per-item lease-event history deliberately keeps per-item lease outcomes for
diagnostics. There is currently no
retention requirement or configured window for this table in the product. Deleting history by
default, or inventing a retention period, would discard audit evidence operators rely on.

## Decision

Provide an **opt-in, bounded, resumable operator command** instead of default pruning:

```text
melodee-cli work prune-lease-events --older-than-days <N> [--batch-size <B>] [--apply]
```

- Only `event_type = 'lease_renewed'` rows older than the explicit cutoff are eligible.
  Transition, failure, completion, retry, and lease acquire/release rows are never eligible, so
  "what happened to this Release and why" remains intact.
- Default mode is read-only and reports the eligible count. `--apply` performs the deletion.
- Deletion runs in bounded batches with a printed progress line, so it is resumable and safe to
  interrupt. It is idempotent (rerunning deletes only rows still eligible).
- No default window is configured or enabled. An operator must choose the cutoff explicitly.
- The command takes a transaction-scoped advisory lock so two prunes cannot interleave batches for
  the same table.

## Alternatives considered

### Alternative A — prune by default with a fixed window

Rejected: no product retention requirement exists, and silently deleting that evidence would
be an unapproved audit change.

### Alternative B — remove lease-renewal events entirely and rely on the current expiry column

Rejected: the existing contract deliberately retains per-item lease outcomes, and existing diagnostics/tests
assert renewal evidence.

## Rationale

High-frequency renewal noise is the only unbounded growth in this table that has no independent
audit value beyond a bounded recent window. Making the cutoff explicit keeps the decision with the
operator, preserves all transition history, and gives a tested path for installations that need to
control table growth without weakening recovery or diagnostics.

## Consequences

### Positive

- Operators can bound the table without losing transition history.
- No silent destructive change and no invented retention default.
- Bounded, resumable, idempotent, and interruptible.

### Negative / trade-offs

- The operator must choose and document a cutoff.
- Very old renewal evidence is lost when the command is applied; that is the operator's explicit
  choice.

### Follow-up work

- If a product retention requirement is later ratified, wire the same bounded prune into the
  scheduled maintenance path with that window.

## Validation

- Unit/integration test: only `lease_renewed` rows older than the cutoff are removed; transition,
  failure, completion, and lease acquire/release rows remain; a second apply is a no-op.
- Interrupted prune resumes correctly and never removes ineligible rows.

## References

- [PERFORMANCE.md](../../PERFORMANCE.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
