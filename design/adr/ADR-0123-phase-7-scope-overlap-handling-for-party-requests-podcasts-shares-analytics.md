# ADR-0123: Native-v1 routes stay in Phase 7 while full parity domains remain in Phase 9

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (phase governance)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-M1-001..007, PRD-REQUEST-001..002, PRD-SHARE-001..003, PRD-POD-001..003, PRD-DISCOVERY-001..003, PRD-PARTY-001..004

## Context

Phase 0 route capture and manifests show overlapping domains across the native-v1 surface
(`podcasts`, `requests`, `shares`, `analytics`, `party`) and the broader Phase 9 parity domains.
The phase boundary must not silently defer compiled native-v1 routes or claim a legacy route is
implemented in Rust merely because it was captured.

## Decision

For this Phase 7 implementation slice:

1. Phase 7 owns every route compiled into the frozen native Melodee API v1 inventory, including
   native routes for requests, shares, podcasts, analytics, charts/recommendations, playback
   backends, and related domain slices.
2. Phase 9 owns completion of those domains outside the native-v1 contract: non-native adapters,
   operator/UI/worker breadth, migration utilities, and behaviors not exposed by compiled v1 routes.
3. The 35 controllers/routes excluded from the legacy build remain negative compatibility
   obligations. Phase 7 must prove the observed absence/negative routing in Rust; source exclusion
   alone is not executable evidence. If a route is later activated, a superseding ADR and client
   compatibility plan are required.
4. The route matrix distinguishes `legacy-captured`, `rust-implemented`, `pending-implementation`,
   and `preserved-negative`; capture/scenario evidence can never imply Rust implementation.
5. No live legacy route may be labelled deferred merely because its differential fixture has not
   been captured yet. It remains a Phase 7 implementation-and-capture obligation.
6. Native podcast discovery uses the public iTunes Search API through exact-pinned reqwest 0.13.4
   with rustls and no native-TLS or system-proxy feature. Requests have explicit connect and total
   timeouts, an eight-request semaphore with an eight-second acquisition timeout, automatic
   redirects are disabled, parameters are URL-encoded by the client, results are capped at 100,
   and response bodies are streamed with a one-MiB hard limit before JSON
   parsing. The configured endpoint must use HTTPS except for a loopback HTTP endpoint used by
   deterministic integration tests. Non-success responses, redirects, invalid response shapes,
   and limit violations fail closed with the captured generic native error envelope. The endpoint
   is configurable through `MELODEE__PODCAST__DISCOVERY_ENDPOINT`; its default is
   `https://itunes.apple.com/` and it never receives Melodee credentials or catalog data.
7. Podcast RSS/Atom metadata is parsed with exact-pinned quick-xml 0.41.0 without optional
   features. Feed bodies are capped before parsing; entries are capped per refresh and upserted by
   `(channel_id, feed_item_id)`. Enclosure downloads use PostgreSQL `SKIP LOCKED` leases, bounded
   attempts/backoff/body size, and atomic publication beneath one validated dedicated podcast
   storage root. Recovery may reclaim expired leases and remove only verified direct temporary
   files created inside that root. It never follows symlinks, deletes catalog media, or treats a
   path as semantic identity. User-supplied feed and enclosure hosts are resolved with a bounded
   DNS lookup, reject non-public addresses in production, and are pinned into the no-redirect
   request client so DNS rebinding cannot redirect a validated request into a private network.
8. Native theme-pack import/export uses exact-pinned `zip` 6.0.0 (MIT), with only its FOSS
   DEFLATE implementation enabled. Imports remain in memory, reject encrypted or unsupported
   compression, traversal/absolute/backslash paths, links, duplicate names, excessive entry
   counts, and excessive compressed or expanded sizes, then insert theme metadata/assets in one
   PostgreSQL transaction. An existing theme ID fails closed, matching the frozen behavior. No
   archive entry is extracted to a filesystem path. Exports are
   deterministically ordered and bounded by the same limits.
9. Native image compatibility uses exact-pinned `image` 0.25.10 (MIT/Apache-2.0), with only the
   codecs required by captured native artwork fixtures enabled. Decoding is bounded by input byte
   size and decoded pixel count before allocation, requested dimensions are limited by the native
   route contract, and output encoding is deterministic. Catalog artwork is opened only through
   the accepted no-follow canonical-media boundary; image processing never derives identity or
   mutates canonical media.

## Alternatives considered

### Alternative A: implement full non-native domain breadth in Phase 7

Implement every adapter, background worker, operator flow, and non-native surface for the overlap
domains in Phase 7 in addition to their native-v1 routes.

- Rejected: expands compatibility blast radius before auth/session/stream/security hardening and testing is complete.

### Alternative B

Drop overlap domains permanently.

- Rejected: introduces silent contract loss against PRD requirements and known native client behavior.

## Rationale

This decision preserves a stable, testable contract boundary while preventing unreviewed expansion. It also keeps route scope explicit in machine-readable form.

## Consequences

### Positive

- Explicit ownership without silently shrinking the native-v1 compatibility surface.
- No silent route removal or hidden behavior changes.
- Reviewable migration of overlap domains in a later ADR-gated slice.
- Real podcast discovery parity without adding a hosted-only SDK or native TLS dependency.
- Functional theme-pack migration without exposing a filesystem extraction boundary.

### Negative / trade-offs

- Phase 7 remains broad: every compiled native-v1 route needs implementation and evidence.
- Phase 9 must avoid duplicating or contradicting the accepted native adapter behavior.
- Podcast discovery depends on an external public service and therefore fails closed during
  outages; timeout/size/result bounds trade completeness for predictable resource use.
- Theme archives using encrypted entries or codecs other than Stored/DEFLATE are rejected.
- Image resizing adds bounded CPU and memory work to anonymous image requests; concurrency and
  decoded-pixel caps keep that work from becoming an unbounded resource surface.

### Follow-up work

- Phase 9 work packages must consume Phase 7 native-v1 contracts as already-established adapters.
- Advance route matrix rows only after Rust handler and differential/negative evidence are green.

## Validation

- `python3 scripts/build_native_inventory.py --check`
- Route matrix keeps every compiled overlap route as a Phase 7 implementation obligation.
- `PHASE-7-WORK-PACKAGE.md` references this ADR and route blockers.
- `apps/melodee-server/tests/native_podcasts_api.rs::all_native_podcast_routes_are_tenant_scoped_bounded_and_authorized`
  covers the bounded local discovery/feed/enclosure fixtures, lease theft, retry recovery,
  bookmarked retention, safe deletion, and orphan cleanup.
- `apps/melodee-server/tests/native_community_api.rs::native_community_routes_are_owner_fenced_bounded_and_replay_safe`
  covers request/share authorization and anonymous resolution/expiry for every share resource kind.
- `crates/melodee-persistence/tests/native_community_phase7.rs::community_schema_preserves_ids_parent_scope_and_replay_safe_preferences`
  covers atomic, catalog-ID-backed AddRelease/AddTrack request completion and idempotent system
  activity.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- compatibility/melodee-api-v1/contracts/route-inventory.json
- design/agents/PHASE-7-ROUTE-MATRIX.json
