# ADR-0136: Defer non-Last.fm external provider integrations to post-parity

- **Date:** 2026-08-14
- **Status:** Accepted
- **Decision owners:** Phase 9 work package (delegated phase decision per the
  `design/agents/PHASE-9-PLAN.md` §3 decision-bundle pattern; ADR-0133 is the precedent for an
  accepted, phase-scoped waiver/deferral ADR recorded by the executing agent)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-EXT-001, PRD-EXT-002, PRD-EXT-003, PRD-EXT-004

## Context

PRD-EXT-001 requires inventorying and preserving the legacy external metadata/search/enrichment
providers "where they are active product functionality." Phase 9 WP-07 delivered that inventory
as evidence (`design/evidence/phase-9/lastfm-provider-evidence.md` §9, from the frozen oracle at
commit `4028b58d…`): nine legacy provider integrations — Last.fm, MusicBrainz, Discogs, Spotify,
iTunes/Apple, Deezer, Wikidata, Brave Search, and Metal-API (AMG is named in PRD prose but has no
legacy implementation). WP-07 also delivered the FOSS Last.fm client (scrobble/now-playing
submission plus the per-user auth session lifecycle per legacy ADR-0004), the cache and
provenance homes (`external_provider_cache`, migration 0028, ADR-0134 §7), and the
no-provider-required catalog-entry invariant (PRD-EXT-002, tested).

The eight other inventoried providers — MusicBrainz, Discogs, Spotify, iTunes, Deezer, Wikidata,
Brave, and Metal-API — have **no captured user-visible contract** in any frozen Rust surface: the
native-v1, OpenSubsonic, and Jellyfin golden corpora contain no request/response pair whose shape
depends on their search/image/enrichment lookups, and no shipping client workflow exercised in
the Phase 7/8 real-client gates touches them. Their legacy behavior is reachable only through the
Blazor admin UI's artist-management pages (Phase 10 scope) and internal enrichment pipelines.
Last.fm's own metadata-lookup legs (`searchEngine.lastFm.Enabled` artist search and album images;
the legacy top-songs stub was never implemented) are in the same position — only the Last.fm
**write** path (scrobble/now-playing/linking) has a captured contract, and that path is wired and
differentially evidenced.

Building eight provider clients in Phase 9 would be new surface area with zero differential
evidence to validate against — the same parity-vs-feature-growth confusion ADR-0133 rejected for
Party Mode.

## Decision

Phase 9 **defers** wiring the search/image/enrichment integrations of the eight non-Last.fm
inventoried providers (and Last.fm's own unwired metadata-lookup legs) to a **post-parity phase
with its own work package**. The Phase 9 parity deliverable for the provider domain is exactly
what WP-07 shipped (per `design/agents/PHASE-9-PLAN.md` WP-07 scope):

1. The provider inventory evidence with per-provider purpose, config keys, and legacy code
   citations (`lastfm-provider-evidence.md` §9).
2. The legacy provider settings keys preserved verbatim in the layered settings service,
   including the capital-E `scrobbling.lastFm.Enabled` / `searchEngine.lastFm.Enabled` quirk.
3. The `external_provider_cache` schema and `ProviderCacheStore` as the cache/provenance home.
4. The real Last.fm write path (scrobble, now-playing, link/session lifecycle) behind
   configuration flags, with the captured "unconfigured" shapes remaining the disabled-state
   contract.
5. The tested PRD-EXT-002 invariant: no external provider is ever required for a structurally
   valid local Release to enter the catalog.

Consequences for requirement tracking: the PRD-EXT-001/003/004 legs covering unimplemented
providers are **deferred** under this ADR (recorded in
`design/requirements/status-overrides.json`); PRD-EXT-002 stands tested and is unaffected.

## Alternatives considered

### Implement all nine providers in Phase 9

Benefits: closes the entire PRD-EXT surface in one phase. Costs: eight HTTP/API client
integrations (one — MusicBrainz — backed by a local decentdb replica import pipeline, itself a
subsystem) with no captured contract to validate against, no client depending on them, and
material schedule risk to the remaining Phase 9 domains; each provider also carries its own
rate-limit/terms posture that deserves individual design. Rejected: it is feature growth, not
parity — the baseline exposes nothing on the frozen API surfaces for clients to be compatible
with.

### Drop the unimplemented providers permanently

Benefits: smallest surface. Costs: the legacy integrations are real product functionality in the
Blazor admin workflow (artist lookup, image search), and the settings keys and cache/provenance
homes are already preserved; permanent removal would strand migrated configuration and foreclose
a legitimate post-parity feature. Rejected; deferral keeps the option open at no compatibility
cost.

## Rationale

Parity is defined against observable baseline behavior (PRD §6). No frozen capture, route matrix
row, or real-client workflow observes these providers, so their absence from the Rust server
changes no external contract. Meanwhile everything Phase 9 can durably lock in without inventing
surface — the inventory, the settings keys, the cache schema, the fail-degraded posture, and the
Last.fm write path — is implemented and tested. Deferral with an accepted ADR keeps the Phase 9
exit gate ("no unapproved gaps") honest: these legs are an *approved* gap with an owning
decision, not an untracked one.

## Consequences

### Positive

- Phase 9 closes the provider domain with tested deliverables instead of eight unverifiable
  client integrations.
- The deferred legs have an explicit owner (this ADR) and a defined reactivation path (a
  post-parity work package), satisfying the exit-gate waiver mechanism.
- Preserved settings keys mean migrated installations keep their provider configuration; the
  cache schema is already in place for the future lookup work.

### Negative / trade-offs

- Artist search/image enrichment beyond the internal Melodee plugin is unavailable until the
  post-parity work package lands; this is recorded honestly in the parity matrix and the Phase 9
  report.
- The `external_provider_cache` is exercised only by tests, not by live lookup traffic, until
  then (Last.fm scrobbling is a write-only, uncacheable flow).

### Follow-up work

- A post-parity work package per provider group (search, images, enrichment), each with its own
  evidence capture or legacy-source behavioral documentation, rate-limit/cache design on
  `external_provider_cache`, and provenance recording (legacy `LastFmId`/MBID columns are the
  minimum bar per `lastfm-provider-evidence.md` §8/§9).

## Validation

- `apps/melodee-server/tests/lastfm_scrobble.rs` — Last.fm write path and the
  `catalog_entry_never_requires_a_provider` invariant stay green with zero provider
  configuration.
- `crates/melodee-persistence/tests/providers.rs` — cache/provenance store contract stays green.
- `design/evidence/phase-9/checklists/05-lastfm-scrobbling.md` — WP-07 rows green with the
  deferral recorded as the honest scope note.
- `design/requirements/REQUIREMENTS-INDEX.md` — PRD-EXT-001/003/004 show `deferred` citing this
  ADR; PRD-EXT-002 remains `tested`.

## References

- [PRD.md](../../PRD.md) §37 (PRD-EXT-001..004), §56 (Phase 9 exit gate)
- [ADR-0133](ADR-0133-phase-9-party-mode-and-jukebox-parity-waiver.md) (precedent: accepted
  phase-scoped waiver/deferral under the delegated decision bundle)
- [ADR-0134](ADR-0134-phase-9-parity-domain-schema-for-radio-device-profiles-onboarding-notifications-scripts-and-providers.md)
  §7 (provider cache/link schema authority)
- `design/evidence/phase-9/lastfm-provider-evidence.md` (WP-07 inventory and Last.fm evidence)
- `design/agents/PHASE-9-PLAN.md` §3 (decision bundle) and WP-07 scope
