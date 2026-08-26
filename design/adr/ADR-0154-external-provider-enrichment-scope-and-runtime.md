# ADR-0154: External-provider enrichment scope and runtime architecture

- **Date:** 2026-08-24
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** [ADR-0136](ADR-0136-defer-non-last-fm-external-provider-integrations-to-post-parity.md)
- **Superseded by:** None
- **Related requirements:** PRD-EXT-001, PRD-EXT-002, PRD-EXT-003, PRD-EXT-004, PRD-ARTIST-005,
  PRD-ARTIST-012, PRD-RESOLVE-002, PRD-RESOLVE-005, PRD-MEDIA-005, PRD-WORK-003

## Context

ADR-0136 deferred the search/image/enrichment integrations of the eight non-Last.fm inventoried
providers (and Last.fm's own metadata-lookup legs) to a post-parity phase because no frozen capture
observed them. That deferral is now resolved: the Feature Completion Program makes external provider
enrichment a **v1 obligation**. The user-assigned product direction for this phase states that the
*active legacy provider capabilities* are in scope — MusicBrainz (including the local DecentDB
replica), Discogs, Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API, and the applicable Last.fm
metadata-lookup legs — while providers remain **optional and fail-degraded**.

The legacy composition root is `src/Melodee.Common/Plugins/SearchEngine/` (9 external provider
folders plus the `MelodeeArtistSearchEnginePlugin` self-search, plus the `MusicBrainz/Data/` DecentDB
local replica importer/warmup) consumed by `src/Melodee.Common/Services/SearchEngines/`
(`ArtistSearchEngineService.cs:36`, `AlbumSearchEngineService.cs:14`,
`ArtistImageSearchEngineService.cs:22`, `AlbumImageSearchEngineService.cs:28`), the
`ArtistLookupController.cs` native route, the OpenSubsonic `SearchEngineController`, and the
`MusicBrainzDecentDbWarmupHostedService` registered in `Program.cs:172`. C# calls these during
staging (`DirectoryProcessorToStagingService`) and from admin artist/album pages; failures degrade
enrichment and never block ingestion.

The Rust seams already exist but have no production wiring: `ProviderCacheStore`
(`crates/melodee-persistence/src/providers.rs:134`, `get_if_fresh`/`upsert`/`purge_expired`) is the
cache/provenance home with test-only callers; `native/discovery.rs:527-591` (`artist_lookup`)
projects only stored `artist_external_identities` rows and hardcodes the capability list;
`lastfm.rs` implements the write path only. `external_provider_cache` (migration 0028, ADR-0134 §7)
has no production writer/reader/cleaner. Provider settings keys are preserved but inert.

## Decision

1. **Scope.** All active legacy external provider *metadata/search/enrichment* capabilities are in
   v1 scope: MusicBrainz (search, images, ID lookup, including the local DecentDB replica), Discogs,
   Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API, and the Last.fm metadata-lookup legs
   (artist/album search and images). The self-search plugin (Melodee's own catalog) is in scope as
   the always-available provider. AMG has no legacy implementation and remains out of scope.
2. **Optional + fail-degraded (PRD-EXT-002).** No provider is ever required for a structurally valid
   local Release to enter the catalog. Every provider call is wrapped so a failure, timeout, or
   misconfiguration degrades enrichment (skip, fall back to the next configured provider, or
   leave the field unenriched) and never fails ingestion or an API request.
3. **Runtime architecture.** A single Rust-native provider boundary, `Provider` trait in
   `melodee-domain`-adjacent application code, with per-provider adapters that are plain HTTP/JSON
   integrations (no proprietary SDK, no hosted-only dependency; FOSS gate per
   `TECHNOLOGY-BASELINE.md`). Each adapter declares capabilities (artist-search, album/release-search,
   artist-image, album-image, top-songs, id-lookup), credential requirements, and rate limits.
4. **Credential handling.** Provider API keys/tokens are read from the layered settings service
   (preserved keys) and the existing secret-store mechanism; they are never logged, never written to
   `external_provider_cache`, and are never sent to more than one provider.
5. **Rate limits / bounded concurrency / retry-backoff.** Per-provider token buckets from the
   provider's documented limits; a bounded worker pool per enrichment batch (explicit backpressure,
   bounded concurrency) rather than unbounded fan-out; exponential backoff with jitter and a hard
   retry ceiling per call.
6. **Caching / provenance (PRD-EXT-003/004).** Provider responses are cached in
   `external_provider_cache` (already schema'd) with per-provider TTL and a purge job; every
   enrichment writes provenance/evidence rows describing which provider supplied which field
   (provider-scoped identity rows per PRD-ARTIST-005).
7. **Cancellation / operator-visible failures.** Every provider call takes a `CancellationToken`;
   enrichment work is cancellable. Failures are recorded in a bounded, operator-visible location
   (admin diagnostics + `external_provider_cache`/audit rows) and surface through the native
   `artist_lookup` `has_partial_failures`/capability contract without breaking the response shape.
8. **Local DecentDB replica.** The MusicBrainz local replica (importer + warmup) is in scope as an
   optional, configuration-driven enrichment source that improves offline/low-latency lookups; per
   PRD-ARTIST-012 it is evidence/projection data only and never becomes the canonical identity
   mechanism.
9. **Wiring.** The native `/api/v1/artist-lookup` and the OpenSubsonic `searchForArtist*` /
   `searchForAlbum*` routes call the provider boundary; ingestion-time enrichment is a durable work
   item that degrades per (2). The cached "unconfigured" response shapes remain the disabled-state
   contract (ADR-0136 deliverable preserved).

## Alternatives considered

### Keep ADR-0136 deferral (post-parity)

Rejected by the user-assigned product direction: provider enrichment is a v1 obligation, and the
admin/ingestion workflows that consume it are v1 surfaces.

### Minimal "better" default (MusicBrainz + Last.fm + one image source first)

Benefits: smaller first slice. Costs: strands seven inventoried providers whose settings keys, cache
schema, and admin workflows already exist in the Rust codebase, and forces a second ADR/phase to
re-decide scope. Rejected — the adapter boundary makes breadth cheap; a partial default would be
re-litigated later.

### Port the C# plugin class architecture

Rejected per the ground-up rewrite rule and PRD-SCRIPT-005: we preserve user-visible provider
*behaviors*, not the C# `plugin`/`SearchEngineService` class boundaries. The Rust boundary is a
trait + per-provider HTTP adapters.

## Rationale

The seams (cache store, capability list, settings keys, native lookup route) were deliberately built
in Phase 9 so this work is wiring, not redesign. Making the full active legacy set in scope — with a
strict fail-degraded posture — satisfies parity (the C# Blazor workflows that call providers during
staging and from artist/album pages) without inventing new external contracts, because the frozen
API surfaces already define the disabled/cached response shapes.

## Consequences

### Positive

- Closes the provider row of the parity matrix (PROVIDER-001, ARTIST-002) with a single accepted
  decision instead of a deferred gap.
- One provider boundary serves native lookup, OpenSubsonic search, and ingestion enrichment.
- Preserved settings keys and cache schema become live rather than inert.

### Negative / trade-offs

- Eight external HTTP integrations add supply-chain and third-party-terms surface; each must pass
  the FOSS gate and, where licensing/terms make exact parity infeasible, may receive an explicit
  approved difference (recorded per provider).
- Rate limits and third-party outages are operational risk; mitigated by fail-degraded + caching +
  bounded concurrency.

### Follow-up work

- Per-provider adapter, credential/rate-limit config, cache TTL, and provenance writer in the
  Feature Completion Phase 2 workstream (ARTIST / PROVIDER), each with a capture or
  legacy-source behavioral document and fail-degraded tests.
- Real-client / real-provider acceptance legs require documented operator authorization
  (credentials, external calls) and are deferred to that phase.

## Validation

- Provider boundary unit tests: a structurally valid Release enters the catalog with zero provider
  configuration (PRD-EXT-002); each provider adapter is independently fail-degraded (timeout/error/
  429 → skip + provenance row + `has_partial_failures=true`, no ingestion failure).
- Cache/provenance tests: `external_provider_cache` round-trips a provider response with TTL and a
  purge; provenance rows identify the supplying provider.
- Native `artist_lookup` golden diff: cached "unconfigured" shape unchanged; configured shape
  additive.
- Bounded-concurrency/backoff property tests.
- `cargo deny check` and the FOSS gate pass (no proprietary SDK / hosted-only dependency added).

## References

- [PRD.md](../../PRD.md) §37 (PRD-EXT-001..004), §13 (PRD-ARTIST-005/012), §14 (PRD-RESOLVE)
- [ADR-0136](ADR-0136-defer-non-last-fm-external-provider-integrations-to-post-parity.md) (superseded)
- [ADR-0134](ADR-0134-phase-9-parity-domain-schema-for-radio-device-profiles-onboarding-notifications-scripts-and-providers.md)
  §7 (provider cache/link schema)
- Withdrawn `design/evidence/phase-9/lastfm-provider-evidence.md` report §8/§9 (provider inventory)
- Phase 9 report (withdrawn evidence report `design/evidence/phase-9/PHASE-9-REPORT.md`)
