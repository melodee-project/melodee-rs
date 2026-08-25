# Feature Completion Program — Phase 2 Report

**Status:** COMPLETE — local implementation and evidence closed 2026-08-25.

## Phase objective and exit decision

Phase 2 delivered the production-wired Artist/Release identity and external-provider pipeline.
Embedded identifiers remain observations with provenance; trusted strong evidence can converge
identities through the catalog importer, while provider search results remain candidate evidence
and never allocate or own a Melodee identity. Provider work is durable, bounded, cache-aware,
restart-safe, and fail-degraded. Native Artist lookup is provider-backed and provider artwork is
validated, persisted, and served without overriding administrator artwork.

The local phase exit criteria are accepted. Real provider credentials, provider quota behavior,
current-C# differential captures, a production MusicBrainz replica dump/warmup rehearsal, and
production-library/cutover operations remain explicitly operator-authorized external legs; they
are not represented as local passes.

## Phase 1 reconciliation

Phase 1 was closed before dependent Phase 2 behavior was evaluated. Its report is terminal at
[`phase-1/PHASE-1-REPORT.md`](../phase-1/PHASE-1-REPORT.md). The closeout verified MP3/ID3v2,
FLAC/Vorbis, and M4A/MP4 MusicBrainz extraction; all six identifier families through inspection
and manifest publication; PostgreSQL AudioIdentity evidence/proposals; bounded and idempotent
proposal behavior; RELEASE-002 edit/save/reload/validation/audit/search-revision evidence; the
nine previously local-but-unproven service rows; matrix/traceability reconciliation; and the
focused Phase 1 gates. No media was rewritten, deleted, or quarantined.

## Product decision and ADRs

ADR-0161 supersedes ADR-0155 while preserving ADR-0155's historical body. MeloAmp owns listener
browsing, search, playback, playlists, ratings, recommendations, and related listener workflows.
The Rust server owns APIs, ingestion, storage, streaming, operations, and `/admin`; only narrow
HTML surfaces required by a public-link or security flow remain in scope. `WEBUI-001` is recorded
as `DIFFERENT — APPROVED`, and the duplicate server listener SPA was removed from the active
backlog.

ADR-0162 defines the additive provider task, attempt, Release evidence, candidate evidence,
provider artwork, and reversible Release merge/split schema. ADR-0163 accepts the pinned,
Apache-2.0 Rust DecentDB read path for configured MusicBrainz replicas. Existing ADR-0117,
ADR-0153, ADR-0154, ADR-0158, ADR-0159, and ADR-0160 govern the catalog, identity, provider,
job, artwork-sizing, and convergence boundaries.

## Workstream evidence

### A — Durable external-ID evidence

Catalog import persists all six embedded MusicBrainz families with source, provider, timestamp,
confidence, and trusted/untrusted treatment. Valid UUID-like values can enter trusted evidence;
malformed values remain observations and cannot authorize convergence. Release external IDs and
provider candidate observations have separate additive tables. Re-import uses conflict-safe
upserts and schedules provider work idempotently; the representative import fixture produced the
same durable evidence and 36 provider tasks on repeat import.

### B — Production identity resolution

The production catalog importer now calls the bounded resolver/reconciliation path. Cross-Release
Artist reuse requires accepted strong, non-conflicting evidence; normalized names alone never
merge identities. Release convergence uses trusted provider evidence and records auditable,
reversible merge/split events. Locked or negative decisions suppress automation, conflicting or
weak evidence remains provisional, and provider candidates are never read as trusted identity
predicates. Merge/split preserves observations, external evidence, Track ordinals, media paths,
public IDs, audit rows, and projection/outbox revisions. Repeated reconciliation is idempotent.
Recording IDs and fingerprints feed bounded Proposed AudioIdentity work without ambiguous
automatic confirmation.

### C — Provider implementations

The reachable provider set is MusicBrainz (including configured DecentDB/JSON local-replica
behavior), Discogs, Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API, Last.fm metadata lookup,
and Melodee self-search. Each external provider has runtime descriptors, enablement/credential
settings, artist and Release parser paths, cache keys/freshness, provenance, bounded global and
per-provider concurrency (8 global, 2 per provider), timeouts, cancellation boundaries, bounded
retry/backoff, disabled/unconfigured behavior, and deterministic local fixtures. Melodee
self-search remains the local Artist lookup purpose; it is not scheduled as an external Release
provider.

MusicBrainz DecentDB access is read-only, bounded through `spawn_blocking`, and uses the pinned
Rust engine from ADR-0163. No provider can write catalog identity or the replica source.

### D — Durable asynchronous enrichment

Catalog import schedules metadata and artwork work in the same PostgreSQL transaction as catalog
evidence. The worker claims bounded batches with `FOR UPDATE SKIP LOCKED` leases, records every
attempt, reclaims expired leases, applies bounded retries, cleans expired cache entries, and
continues after provider failures. Successful metadata traffic writes `external_provider_cache`;
fresh entries suppress calls and expired entries refresh through the same idempotent cache key.
Artist provider responses become candidate evidence and schedule local review/re-resolution; they
do not force merges. Operator diagnostics contain task/entity/provider/purpose/attempt/error
category/next retry information without credentials or secrets.

### E — Provider APIs and artwork

Native `/api/v1/artist-lookup` invokes the runtime provider boundary, consumes configured runtime
capabilities, and reports failed providers separately from successful candidates. Provider images
are downloaded with response-byte and pixel bounds, MIME validation, decode/normalization, and
failure isolation. Provider artwork is stored with source/provenance and served behind admin
catalog-image precedence across native-v1, OpenSubsonic, and Jellyfin. Administrator overrides
remain first and OpenSubsonic stored-artwork sizing remains governed by ADR-0159. Existing
compatibility DTOs, routes, status codes, auth, and intentional redirect behavior were not
changed.

## Requirements and parity disposition

The generated requirements index and overrides were regenerated and checked. Evidence promotes
the applicable `PRD-ARTIST-*`, `PRD-RESOLVE-*`, `PRD-EXT-*`, `PRD-FP-*`, `PRD-MEDIA-*`,
`PRD-CONFIG-*`, `PRD-CATALOG-*`, and provider portion of `PRD-WORK-*` rows only where local
production-path evidence exists. `PRD-ARTIST-011`, `PRD-RESOLVE-005`, `PRD-EXT-001..004`,
`PRD-CONFIG-001`, `PRD-CATALOG-001/005`, and the provider portion of `PRD-WORK-001` are covered
by the Phase 2 overrides and tests. Remaining rows stay conservative where the current-C#
differential or operator leg was not executed. `WEBUI-001` is an approved relocation to MeloAmp.

The detailed parity audit retains historical pre-Phase-2 observations for traceability; its
current addendum and feature matrix record the implemented local evidence and external boundary.
The historical 28-blocker count remains a baseline, with `WEBUI-001` now an approved difference;
no historical evidence was erased.

## Tests and validation

Focused Phase 2 evidence executed successfully:

- `cargo test -p melodee-persistence --test migrations` — 8 passed.
- `cargo test -p melodee-persistence --test catalog_import -- --test-threads=1` — 7 passed.
- `cargo test -p melodee-persistence --test catalog_merge_split -- --test-threads=1` — 2 passed.
- `cargo test -p melodee-persistence --test provider_enrichment -- --test-threads=1` — 3 passed.
- `cargo test -p melodee-server --lib providers::tests -- --test-threads=1` — 5 passed.
- `cargo test -p melodee-server --test provider_enrichment_worker -- --test-threads=1` — 3 passed.
- Native discovery/provider lookup, native image, OpenSubsonic image, Jellyfin image, and admin
  artwork suites — passed in the final focused compatibility run.
- Phase 1 focused suites and browser evidence — recorded in the terminal Phase 1 report.

The full repository gate initially exposed one repository-policy omission: the newly accepted
ADR-0163 git source was not present in `deny.toml`. The exact source was added to the allowlist;
the focused `cargo deny check` then passed and the complete gate was rerun.

## Compatibility and client impact

Native-v1 gained provider-backed Artist lookup and provider artwork behind its existing contract;
its DTOs, IDs, errors, auth, and pagination semantics remain independent. OpenSubsonic and
Jellyfin retain their separate wire adapters and image-serving contracts, including ADR-0159
sizing and the captured intentional redirect behavior for unavailable provider search routes.
MeloAmp remains read-only in this phase and is the approved listener application; its native-v1
browse/search/artwork journeys are the client acceptance surface for the server.

## Database, migration, rollback, and data safety

Migration `0035_provider_enrichment_and_release_evidence.sql` is additive and covered by
PostgreSQL migration/integrity tests. It adds durable task/attempt leases, Release external
evidence, candidate evidence, provider images, and reversible Release merge/split event data.
Forward migration is the normal deployment path. Rollback is restore/forward migration using the
documented backup/restore procedure; no destructive down-migration or source-media operation is
authorized. Provider failure cannot delete, quarantine, or invalidate otherwise valid media.

## Security, failure, and performance

Credentials are read through layered runtime settings and never enter cache payloads, attempt
messages, candidate evidence, or logs. Requests have bounded timeouts, global/per-provider
semaphores, capped response/image sizes, MIME/decode checks, bounded retries, Retry-After caps,
and cancellation at the async request boundary. Invalid credentials, disabled providers,
timeouts, malformed responses, rate limits, and partial outages become honest diagnostics and
retry/failure state. The worker has bounded batch size and lease durations; the DecentDB blocking
reader is additionally protected by the provider semaphore. Local fixture evidence verifies the
configured/cache-hit, disabled, rate-limited, partial-failure, restart/lease, and malformed-image
paths.

## Remaining external legs and risks

- Real credentials and live quota/rate-limit behavior for the nine external providers.
- Differential request/response comparison against the current C# provider deployments.
- Operator preparation and rehearsal of a full production MusicBrainz DecentDB dump/warmup.
- Production-library identity review, backup/restore rehearsal, cutover, and rollback, under the
  existing migration/operator authorization boundary.
- Any unrelated scheduled-job gaps not part of provider maintenance, and the previously recorded
  current-C# configured Last.fm response capture gap.

These are visible follow-up legs, not silently completed features. The principal residual risk is
provider-specific live API drift; local parsers and runtime behavior are deterministic and
fail-degraded, but live credentials and current payload differential evidence are still required
before claiming external parity.

## Final gate evidence

- `cargo fmt --all` — passed.
- `make check-fast` — passed: repository/document/requirements checks, formatting, workspace
  Clippy, and Rust library/binary unit tests were green.
- `make test-integration` — passed: PostgreSQL-backed workspace integration suites were green,
  including migration (8), catalog import (7), provider enrichment persistence (3), provider
  worker (3), native-v1, OpenSubsonic, Jellyfin, artwork, and fault/restart suites.
- `python3 scripts/validate_repository.py` — passed: 64 ADRs, 8 mockups, and 239 requirements.
- `python3 scripts/extract_requirements.py --check` — passed: 239 requirements current.
- Admin UI gate — passed: typecheck, lint (0 errors; 2 existing fast-refresh warnings), locale
  structure, 44 Vitest files / 248 tests, and production build.
- Docs gate — passed: `pnpm check`, `pnpm check:links`, and `pnpm build`; 45 pages built with
  zero errors, warnings, or hints.
- `cargo deny check` — passed: advisories, bans, licenses, and sources all passed. Existing
  duplicate-version warnings and the unmatched `PostgreSQL` license allowance are non-fatal.
- `make check` — passed on the final rerun after the source allowlist fix; all repository,
  Python, admin UI, formatting, Clippy, workspace all-feature tests, and dependency checks were
  green.
- The read-only `python3 scripts/run_phase8_differential.py --only opensubsonic --skip-build`
  replay remains an external unresolved leg: this environment produced 158 unexpected fixture
  differences. It is recorded as a failed external comparison, not as a local compatibility
  pass; the accepted Phase 8 artifact remains the historical zero-diff evidence for the prior
  contract.

## Documentation impact:
- Updated - `design/evidence/feature-completion/phase-1/PHASE-1-REPORT.md`,
  `design/evidence/feature-completion/phase-2/PHASE-2-REPORT.md`,
  `design/agents/FEATURE-COMPLETION-PHASE-2-PLAN.md`,
  `design/ROAD-TO-FEATURE-COMPLETION.md`, `design/ROAD-TO-FEATURE-PARITY.md`,
  `design/requirements/FEATURE-PARITY-MATRIX.md`,
  `design/requirements/status-overrides.json`, generated requirements indexes,
  ADR index/ADR-0155 metadata, and public provider, operator, configuration, and MeloAmp docs.
