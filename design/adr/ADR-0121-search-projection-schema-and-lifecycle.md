# ADR-0121: Search projection schema and lifecycle

- **Date:** 2026-08-11
- **Status:** Accepted
- **Decision owners:** User-directed Phase 6 authority work
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-ARCH-003, PRD-ARCH-004, PRD-ARCH-006, PRD-CATALOG-005,
  PRD-SEARCH-001, PRD-SEARCH-002, PRD-SEARCH-003, PRD-SEARCH-006, PRD-SEARCH-007, PRD-SCALE-001,
  PRD-SCALE-002, PRD-SCALE-003, PRD-OBS-001 through PRD-OBS-005, PRD-PERF-003, PRD-PERF-005,
  PRD-PERF-006, PRD-TEST-006, PRD-MIG-006

## Context

Phase 4 (`0007_phase4_catalog.sql`) created `catalog_projection_outbox` with a `(projection_name,
entity_type, entity_id, entity_revision)` unique key, an `operation IN ('upsert','delete')` column, a
lease owner/expiry pair, `available_at`, `attempt_count`, `processed_at`, and a partial pending index.
PostgreSQL is authoritative; Tantivy is a disposable, fully rebuildable projection (ADR-0105,
ADR-0107, PRD-ARCH-003/004).

Phase 6 must turn that outbox into a durable, crash-safe, multi-process coordinated Tantivy index with:

- three document kinds (Artist, Release, Track) carrying internal + public IDs, display/sort names,
  aliases, credited/participant Artist IDs (ID-backed, not string-reconstructed), genres/traits, dates,
  and a per-document revision for safe replacement;
- a bounded, SKIP-LOCKED outbox consumer with lease fencing;
- a generational rebuild that never disturbs the live generation and atomically activates only after
  validation and concurrent-change catch-up;
- durable projection checkpoints, generation metadata, and rebuild-job status so a running server never
  serves a partially built index and so any process can fence a stale writer.

Out of scope for this ADR: the MQL grammar/semantics (ADR-0122), the public HTTP API (Phase 7+), and
the Tantivy tokenizer choice beyond the Unicode/diacritic requirements below.

## Decision

### Projection storage root and generations

- All Tantivy data lives under one configured search root (`MELODEE__SEARCH__ROOT`, required when
  `MELODEE__SEARCH__ENABLED=true`). The root is validated at startup: it must be an absolute,
  canonical path; it must not escape an owning parent; it must not overlap another configured root;
  symlink components are rejected; the directory is created if missing with restrictive permissions.
- Each built index is a **generation**: a directory `gen_<uuid7>/` containing a complete Tantivy index
  plus a `generation.json` with `{ generation_id, kind, created_at, document_counts, schema_version,
  source_high_water_mark }`.
- Exactly one generation is **active**, recorded durably in PostgreSQL
  (`search_active_generation`) keyed by `search_root` so multiple server processes coordinate through
  the database, not the filesystem.
- Readers open the active generation once and periodically refresh after a confirmed activation; a
  reader never opens a generation that is not yet marked active.

### Schema (Tantivy 0.26.1, exact centralized workspace dependency)

Document kinds share a `kind` enum field (`artist`/`release`/`track`) and a `doc_id` (`kind + ':' +
internal_id`). Fields per kind:

- **Artist**: `internal_id` (string, fast/term), `public_id` (string, fast/term), `display_name`
  (text, tokenized + stored), `sort_name` (string, fast, for deterministic sort), `aliases`
  (text, multi, tokenized), `type` (string, term), `status` (string, term), external/display facts
  (e.g. `disambiguation` stored, `begin_year`/`end_year` as `i64` fast where present), `revision`
  (`i64` fast, the authoritative `entity_revision` from the catalog), `updated_at` (date, fast).
- **Release**: `internal_id`, `public_id`, `title` (text tokenized + stored), `sort_title` (string
  fast), `artist_ids` (multi string term, credited ReleaseArtists), `artist_display` (stored text for
  display), `genre_ids`/`tags` (multi string term), `release_type`, `release_year` (`i64` fast from
  authoritative date), `release_date` (date fast), `revision`, `updated_at`.
- **Track**: `internal_id`, `public_id`, `release_id` (string term), `title` (text tokenized +
  stored), `sort_title` (string fast), `primary_artist_ids` (multi string term),
  `participant_artist_ids` (multi string term — all credited/performing Artists, per PRD-SEARCH-003),
  `artist_ids_by_role` — structured multi fields for accepted MQL role concepts
  `primaryArtist`, `featuredArtist`, `performer`, and `other` (each a multi string term of Artist
  internal IDs), `display_credits` (stored text), `genre_ids`/`tags` (multi string term), `track_number`
  (`i64` fast), `duration_ms` (`i64` fast), `release_year`/`release_date` (fast), `revision`,
  `updated_at`.

Tokenization uses a Unicode-aware analyzer: `UnicodeSegmenter` + lowercasing + ASCII-folding so
diacritics (é→e) and multilingual text tokenize consistently and are testable. `kind` + ID fields are
un-tokenized (term) for exact ID queries. Stored fields carry only what the result DTO needs; large
text is not duplicated. Document versioning uses the catalog `entity_revision`; the index writer never
applies an outbox row whose revision is <= the document's current revision (revision fencing).

### Outbox consumer and lifecycle (PostgreSQL)

- A new migration `0009_phase6_search.sql` adds (additive, no breaking change to 0007):
  - `search_projection_checkpoint(projection_name PK, last_processed_outbox_id, lease_owner,
    lease_expires_at, updated_at)` — durable high-water mark per projection;
  - `search_generations(generation_id PK, search_root, schema_version, status
    ('building'|'validated'|'active'|'retired'|'failed'), created_at, activated_at, document_counts
    jsonb, current_document_counts jsonb, source_high_water_mark, owner_lease,
    owner_lease_expires_at)`; `document_counts` is the sealed snapshot baseline, while
    `current_document_counts` advances atomically with catch-up/outbox acknowledgement for live
    diagnostics;
  - `search_rebuild_jobs(job_id PK, requested_at, requested_by, status
    ('requested'|'running'|'validated'|'activated'|'failed'), generation_id, requested_high_water_mark,
    caught_up_at, error, finished_at)`;
  - `search_active_generation(search_root PK, generation_id, activated_at, updated_at)`.
- **Claim**: `SELECT ... FROM catalog_projection_outbox WHERE projection_name=$1 AND processed_at IS
  NULL AND available_at <= now() ORDER BY id ASC LIMIT $batch FOR UPDATE SKIP LOCKED`. Work is claimed
  by setting `lease_owner`/`lease_expires_at` within a short bounded lease.
- **Fencing**: a worker may only acknowledge/delete a row if its `lease_owner`/`lease_expires_at`
  still match. Lease expiry makes the row claimable again by another worker; a stale worker's write is
  rejected because the lease no longer matches.
- **Ack**: after the Tantivy commit is durable, the outbox row is marked `processed_at = now()`. We
  **never** acknowledge PostgreSQL before the Tantivy commit is durable (SPEC-INV-007). A crash after
  Tantivy commit but before DB ack leaves the row unacked; on reclaim it replays — the index uses
  revision fencing so replay is idempotent (no duplicate/regressed document).
- **Retry / failure**: transient failures increment `attempt_count` and set `available_at` to a bounded
  backoff. At the configured positive, bounded attempt limit they become permanent failure evidence
  instead of retrying forever. A document-level indexing failure is settled only for that row; the
  writer discards its uncommitted partial replacement, replays proven siblings, commits/acknowledges
  those siblings, and records the poison row independently. Permanent failures record `last_error`
  with actionable diagnostics. Infrastructure failures are recorded as projection failure evidence
  only — never as catalog/media corruption.
- **Status/backlog/age**: queries over the outbox (`processed_at IS NULL` count, `min(available_at)`),
  checkpoint, active generation, and latest job are exposed as diagnostics (PRD-OBS-002/003).

### Generational rebuild (safe, no media reads)

1. Durable `search_rebuild_jobs` row `requested`; allocate a new `gen_<uuid7>` with status `building`
   and take an ownership lease.
2. Stream a consistent PostgreSQL snapshot in bounded pages (keyset over `artists`/`releases`/`tracks`
   by internal id) — **never** reading canonical media or audio bytes (rules 10–11).
3. Record `source_high_water_mark` = current max `catalog_projection_outbox.id` at snapshot start.
4. Build the new generation's Tantivy index from the snapshot only.
5. Validate: schema version matches reader, document counts are non-negative and match the snapshot
   counts, and a set of representative queries returns expected results.
6. Catch up concurrent outbox rows with `id > source_high_water_mark` against the new generation.
7. Atomically activate: in one transaction set the new generation `status='active'` and update
   `search_active_generation`; readers refresh and switch to the new generation.
8. Retire old generations only within the ownership-validated search root (no `rm -rf`; explicit,
   path-validated removal of directories under the validated root).

The activation transaction holds a PostgreSQL `SHARE` lock on the outbox through the final
high-water check, bounded generation status updates, active-pointer upsert, and commit. Releasing it
after the max-ID read would reopen the catch-up race. No catalog scan, index I/O, or filesystem I/O
occurs under this lock; operational monitoring must treat an unexpectedly long activation
transaction as a fault.

### Multi-process coordination and crash recovery

- Exactly one writer/rebuild owns a generation via the PostgreSQL lease columns. A second process that
  loses the lease is fenced from further writes to that generation.
- Readers and writers coordinate through `search_active_generation` + `search_generations`; filesystem
  handoff is never the source of truth.
- Recovery at every boundary is replayable: unacked outbox rows, `building` generations with no
  activation, and `requested`/`running` jobs with expired leases are all reclaimable and resume from
  durable state.
- A corrupt or missing active index is detected at startup (open failure / schema-version mismatch /
  count mismatch) and triggers a diagnostic `failed` status plus an operator-guided rebuild; it never
  affects PostgreSQL or media.

### Dependency consequences

- `tantivy = "=0.26.1"` is added as an exact centralized workspace dependency (ADR-0107 already accepts
  Tantivy in principle; this pins the version the Phase 0 revalidation recorded). FOSS status: MIT,
  quickwit-oss, no commercial feature boundary (revalidated 2026-08-09). No other production dependency
  is added; in particular no broker, Redis, Elasticsearch, or OpenSearch (ADR-0111).

## Alternatives considered

### Single mutable index with in-place updates only

Rejected. In-place rebuild cannot serve reads during a full rebuild, cannot validate before exposure,
and cannot recover cleanly from a half-written index. Generations are required by PRD-SEARCH-007 and
PRD-MIG-006.

### Filesystem lockfile coordination

Rejected. A lockfile is not crash-safe across processes and does not fence a stale writer after a
network/process split. PostgreSQL leases are already the project's fencing mechanism (ADR-0105,
ADR-0110) and are reused here.

### Elasticsearch/OpenSearch

Rejected by ADR-0111 / PRD-ARCH-006 (unnecessary infrastructure) and PRD-SEARCH-001 (embedded FOSS
projection preferred).

## Rationale

The outbox already carries revision and lease columns; the lifecycle design reuses them so catalog
mutation and projection work stay atomic (PRD-CATALOG-005) and replay is idempotent by revision.
Generations make rebuild and crash recovery explicit and keep PostgreSQL as the only authoritative
state.

## Consequences

### Positive

- Index loss/corruption never touches PostgreSQL or media; rebuild needs no re-ingestion or media reads.
- A running server never serves a partially built index.
- Multi-process deployments need no extra coordination service.
- Out-of-order/stale outbox rows cannot regress a newer document revision.

### Negative / trade-offs

- Two generations briefly double disk for the search root during rebuild (bounded and retired explicitly).
- Additional PostgreSQL tables and leases to operate; smaller ops surface but well within existing
  tooling.

### Follow-up work

- Wire the worker into the server runtime (CLI `search status/rebuild/query`, config, tracing) per the
  Phase 6 work package.
- MQL translator (ADR-0122) consumes this schema's fields.

## Validation

- Outbox tests: exclusive claim + lease expiry, stale-worker fencing, duplicate/out-of-order revisions,
  retryable failure, crash-after-commit-before-ack idempotent replay, delete/tombstone.
- Rebuild tests: empty + populated catalog, concurrent writes during rebuild, crash before/after
  activation, corrupt/missing active index, rebuild with zero media reads, identical authoritative
  results after rebuild.
- Path-safety tests: symlink components, root overlap, unowned generation, traversal, conflicting
  generation/final path.
- 10-million-Track benchmark: warm p95 < 50 ms target (PRD-SEARCH-006); if missed, propose an ADR
  revising the target with evidence rather than weakening the benchmark.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0105](ADR-0105-postgresql-is-authoritative-state-store.md)
- [ADR-0107](ADR-0107-use-tantivy-as-search-projection.md)
- [ADR-0110](ADR-0110-use-sqlx-and-explicit-sql.md)
- [ADR-0111](ADR-0111-do-not-require-broker-or-redis-initially.md)
- [ADR-0117](ADR-0117-use-a-postgresql-catalog-identity-and-contribution-schema-with-set-based-manifest-import.md)
- [Phase 5 work package](../ROAD-TO-FEATURE-COMPLETION.md)
