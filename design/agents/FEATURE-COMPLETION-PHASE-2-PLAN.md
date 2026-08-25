# Feature-Completion Phase 2 Plan — Artist/Release Identity and Provider Enrichment

**Status:** Executed under the explicit Feature-Completion Phase 2 assignment on 2026-08-25.
**Scope:** complete the identity/evidence and external-provider enrichment pipeline end to end;
this plan is the implementation map, while
[`design/evidence/feature-completion/phase-2/PHASE-2-REPORT.md`](../evidence/feature-completion/phase-2/PHASE-2-REPORT.md)
is the evidence record.

## Governing context

Requirements are drawn from the `PRD-ARTIST-*`, `PRD-RESOLVE-*`, `PRD-EXT-*`, `PRD-FP-*`,
`PRD-MEDIA-*`, `PRD-CONFIG-*`, `PRD-CATALOG-*`, and `PRD-WORK-*` families in the generated
requirements index. The implementation follows ADR-0117 (authoritative catalog/evidence schema),
ADR-0153 (same-Release identity reuse), ADR-0154 (provider set and runtime), ADR-0158 (scheduled
job state), ADR-0159 (OpenSubsonic artwork sizing), ADR-0160 (cross-Release identity reuse),
ADR-0161 (MeloAmp owns listener workflows), and ADR-0162 (durable enrichment and Release evidence).

Phase 1 is a closed entry prerequisite: its terminal report covers embedded-ID extraction,
AudioIdentity proposals, RELEASE-002, and local evidence for the nine service rows. The Phase 2
entry decision superseding ADR-0155 records that the server owns APIs, ingestion, storage,
streaming, operations, and `/admin`; MeloAmp owns listener browsing, search, playback, playlists,
ratings, recommendations, and related listener workflows. A duplicate `web-ui/` SPA is not in
server scope.

## Workstreams and exit evidence

1. **Durable evidence:** retain all embedded external-ID families, persist provenance and
   confidence, add the accepted Release/provider/candidate evidence shape, and prove re-import
   idempotency and malformed-observation safety.
2. **Identity resolution:** wire LocalArtistResolver into catalog reconciliation; converge only
   on trusted strong evidence; preserve weak, conflicting, locked, and negative decisions; add
   auditable reversible Release merge/split with bounded/cancellable/idempotent reconciliation.
3. **Provider runtime:** register MusicBrainz/local-replica, Discogs, Spotify, iTunes, Deezer,
   Wikidata, Brave, Metal-API, Last.fm metadata, and Melodee self-search purposes behind one
   provider-neutral boundary. Apply layered settings, bounded HTTP permits, retry/backoff,
   timeout/cancellation, cache freshness, provenance, and disabled/unconfigured behavior.
4. **Durable enrichment:** schedule provider work from production catalog import, claim with
   leases, persist attempts and retry state, write `external_provider_cache`, retain candidate
   evidence without forcing merges, and fail-degrade when providers are unavailable.
5. **Provider APIs/artwork:** make native Artist lookup provider-backed with honest partial
   failures and runtime capabilities; validate/normalize/persist provider artwork while admin
   overrides remain first; keep native-v1, OpenSubsonic, Jellyfin, and MeloAmp contracts separate.
6. **Closeout:** run deterministic provider fixtures and fault tests, PostgreSQL migration and
   concurrency tests, ingestion and API journeys, compatibility smoke tests, performance checks,
   docs checks, full repository gates, conservative parity updates, and this report.

## Safety and non-goals

Providers are evidence sources, never Melodee identity authorities. Names are never global identity
keys. The work is additive and rollback is restore/forward migration based; no source media is
deleted or rewritten. No proprietary SDK, broker, Redis, search service, GraphQL, microservice, or
listener-facing server SPA is introduced. Real credentials, production libraries, and cutover
remain operator-authorized external legs and are reported separately from local evidence.

## Required validation

Focused Rust/provider/UI tests run during implementation. Closeout requires `cargo fmt --all`,
`make check-fast`, `make test-integration`, provider fault suites, relevant admin UI checks,
MeloAmp compatibility smoke that does not modify MeloAmp, repository validation, requirements
extraction check, applicable docs checks, and `make check`. A red required gate blocks the phase
exit decision.
