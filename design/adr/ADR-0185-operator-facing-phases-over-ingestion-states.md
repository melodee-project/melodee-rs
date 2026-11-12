# ADR-0185: operator-facing phases over the ingestion state machine

- **Date:** 2026-11-11
- **Status:** Accepted
- **Decision owners:** Melodee maintainers
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-INGEST-010, PRD-UI-006, PRD-UI-010, PRD-UI-REF-001, SPEC §11.2

## Context

A Release passes through fourteen lifecycle states from inbound discovery to being playable by a
user (`crates/melodee-domain/src/ingestion.rs`):

```
discovered → settling → ready → inspecting → normalizing → validating → staged
           → publishing → published → cataloging → cataloged → indexing → indexed
(+ needs_review, failed, quarantined, ignored, deleted as branches and terminals)
```

That granularity is **structural, not accidental**. Melodee runs without a message broker, so each
state is also a *claim partition*: `staged`/`publishing` is the publication worker's inbox,
`published`/`cataloging` the catalog worker's, `indexing` the search worker's. Each partition carries
its own lease fencing, retry policy, and durable evidence (staging handoff receipts, publication
authorizations/plans/journal/receipts, catalog import receipts, projection outbox rows). Collapsing
partitions would move that fencing from `state` predicates into column predicates, which is more
complex SQL with no reduction in evidence.

The problem is therefore not the state machine — it is that the operator was asked to read it. A
25-Release live batch surfaced the consequences:

- the queue's filter chips and row labels mirrored all fourteen states, so triage required knowing
  what `staged` versus `published` means;
- two states (`staged`, `published`) dwell for seconds and are invisible in operator terms;
- the pipeline reads as one linear "get to playable" chain, although a Release is **playable** at the
  policy `Accept` point (catalog rows + canonical files + `active` status) and only **findable** at
  `indexed`;
- measured latency was dominated by claim waits (~200 s median to be claimed on a 2-per-stage
  concurrency host) while the work per stage was seconds to tens of seconds. Stage *count* was never
  the bottleneck.

## Decision

1. **Phases are a derived presentation projection.** `IngestionPhase` (eight values: `inbound`,
   `processing`, `publishing`, `catalog`, **`active`**, `review`, `disposition`, `failed`) groups the
   states; `IngestionState::phase()` is total and `IngestionPhase::states()` is its inverse, enforced
   by tests. Phases are never stored, never transitioned, and never affect policy.

2. **The API exposes the projection, not just the mapping.** `GET /api/admin/v1/ingest/work-items`
   accepts an optional `phase` filter (a whitelisted state set) alongside the existing single-value
   `state`, rejects both together (`400 invalid_filter`), rejects unknown phases
   (`400 unknown_phase`), and returns a derived `phase` on every work item so no client
   re-implements the mapping.

3. **The UI groups by phase and keeps the state as detail.** Queue filter pills are phases with
   aggregated counts; each row shows the precise state chip with its phase as a caption; the
   inspector lists both. History keeps its terminal-state scoping and gains the same caption.

4. **The state machine is retained as-is.** In particular `staged` and `published` stay: they are
   claim partitions backed by distinct durable evidence, and merging them would trade two states for
   more complex claim predicates and a weaker crash boundary.

5. **The completion phase is called `active`, not `search`, and the UI shows it as the primary
   label.** `IngestionState::is_client_visible()` names the client-availability predicate in the
   domain, the API exposes it as `clientVisible` (with the authoritative `releaseStatus` from the
   join), and the queue row shows the phase chip with the machinery state as its caption. Naming it
   after the search projection was anti-semantic: clients read PostgreSQL filtered on
   `releases.status = 'active'`, and the Tantivy projection only feeds smart playlists. "Ready" was
   rejected as the label because `Ready` already means *queued for a worker* in the same state
   machine — the exact confusion this ADR removes.

6. **Validation re-hashing is retained, and the optimization is deferred with a trigger.** Structural
   validation deliberately treats the work directory as untrusted and re-reads every planned file
   (~21–39 s per Release in the measured batch) even though normalization hashed its own output. A
   hash-of-hashes seal is the obvious optimization, but it only pays off when I/O is the measured
   bottleneck *and* the seal can be committed atomically with the files it covers. Trigger condition:
   sustained ingest throughput limited by validation I/O at target scale. Until then, correctness of
   the untrusted-input boundary wins.

## Alternatives considered

### Alternative A — keep exposing all fourteen states

No API or UI change; document each state instead. Rejected: it asks every operator to learn the
internal claim-partition layout, and the measured batch shows the cost (triage confusion, seconds-long
states presented as stages).

### Alternative B — collapse the state machine to the eight phases

Fewer states, simpler UI. Rejected: each phase would need internal sub-state columns to keep the
per-worker claim fencing, retry accounting, and evidence bindings — strictly more complex than the
current uniform `state = '...'` claim predicates, and it breaks the migration/audit surface for
every in-flight work item.

### Alternative C — hash-of-hashes in structural validation now

Adopt the optimized path immediately. Rejected for now: it weakens the "work directory is untrusted"
guarantee unless the seal is durable and verified independently, and no measurement yet shows
validation I/O limiting throughput.

## Rationale

The state machine is the correct domain model for a broker-less durable pipeline; the operator does
not need it. A derived projection keeps both properties: the durable evidence and claim fencing stay
exactly as designed, while triage, filters, and labels speak in phases, and the API carries the
projection so clients cannot drift from the domain.

## Consequences

### Positive

- Triage by phase with counts; one caption line per row teaches the precise state.
- The API can filter a whole phase in one query instead of an operator issuing several state queries.
- Playable-versus-findable is documented, ending a misleading expectation.
- No migration, no state-machine change, and no change to any compatibility surface.

### Negative / trade-offs

- Two representations of "where is this Release" (phase and state) must stay consistent; the
  partition tests and a mirrored client table are the guard.
- Adding a future state requires updating `IngestionState::phase()`, the client table, and the tests
  — a deliberate forced decision rather than a silent default.

### Follow-up work

- Reference-fidelity: the mockups imply five filter chips; eight phases is the measured compromise
  between fidelity and honest lifecycle semantics.
- Revisit the hash-of-hashes seal when validation I/O is measured as the bottleneck at scale.

## Validation

- `crates/melodee-domain`: `phase_tests` prove totality, partition, wire round-trip, and that terminal
  states never project onto an "in progress" phase.
- `apps/melodee-server/tests/admin_diagnostics_api.rs`: `work_item_phase_filter_groups_states_and_reports_the_derived_phase`
  covers phase filtering, shared-phase states, empty phases, `state` compatibility, `unknown_phase`,
  and `invalid_filter`.
- `admin-ui`: `src/screens/processing/phase.test.ts` (partition + aggregation), the processing-queue
  unit suite (phase pills, legacy `state=` deep links), and the e2e queue spec.
- Visual baselines regenerated for the queue (light/dark/RTL/mobile).

## References

- [SPEC.md](../../SPEC.md) §11.2
- [PRD.md](../../PRD.md) — PRD-INGEST-010, PRD-UI-006
- [processing.md](../../docs/src/content/docs/admin/processing.md)
- [admin-api.md](../../docs/src/content/docs/api/admin-api.md)
- [storage.md](../../docs/src/content/docs/installation/storage.md)
- ADR-0145 (staging handoff and drain), ADR-0184 (Admin UI surface system)
