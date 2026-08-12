# ADR-0128: Capture-required legacy bug fixes in native API v1

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (phase governance)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-M1-005, PRD-STREAM-001, PRD-USER-001, PRD-PLAYLIST-001

## Context

Route evidence captures several defects that would break modern clients if carried forward unchanged.

Known legacy defects include unknown-resource 500s, playlist add-song corruption, unstable recent-list ordering, stale scrobble counts, malformed stream tokens resolving to redirects, and access-token behavior after logout expectations.

## Decision

Phase 7 may apply explicit compatibility-preserving fixes for these defects only when the defect is either:

- required by native client behavior, and/or
- recorded as approved compatibility divergence with accepted ADR/traceability.

Default behavior remains captured for all other legacy defects.

The following captured defects are approved normalizations in Phase 7 and require a golden test for
both the legacy result and the Rust result:

1. Unknown Artist/Release/Track/Playlist IDs return the documented typed 404 envelope, never 500.
2. Both accepted playlist add-item bodies from ADR-0114 append/reconcile atomically, preserve order,
   and never reproduce the legacy null-navigation 500.
3. Recent lists use deterministic `created_at DESC, public_id ASC` ordering.
4. Successful scrobble writes and derived user/Track counters are one transaction; subsequent
   detail/stat reads observe the committed values without the legacy stale-cache defect. The
   captured scrobble response remains empty.
5. Malformed, tampered, empty-User, wrong-Track, and unknown-User stream credentials return the
   non-enumerating typed 401 behavior specified by ADR-0125, never a login redirect.
6. Logout/revoke prevents refresh-family reuse. Already-issued access-token behavior remains the
   captured behavior documented by ADR-0124 rather than being silently changed.
7. The four endpoint-registry routes authorize from the validated native principal and enforce
   session ownership/role checks. They do not preserve the legacy `ClaimTypes.Sid` mismatch that
   returned 401 for a bearer token accepted by every adjacent protected native route.
8. Analytics and recommendation results derive from committed scrobbles and catalog facts. They
   do not preserve the legacy hard-coded empty top Artist/Release values, zero play-time values,
   or newest-Track-as-popular placeholder. Empty inputs still return the captured empty shapes;
   positive facts produce deterministic, bounded, user-scoped results. Recommendation requests
   inspect a fixed, index-ranked candidate window. If that window cannot prove a complete result
   while ranked candidates remain, the route returns the existing typed
   `SERVICE_UNAVAILABLE`/503 envelope rather than scan an unbounded catalog tail or return a
   falsely empty/partial success. Client retries are safe because the route is read-only.
9. Chart detail and generated-playlist Track artwork uses the compiled anonymous image contract
   `/images/album_{public_id}/80`. It does not preserve the legacy dangling
   `/api/v1/images/album/{public_id}` value whose controller was absent from the frozen build.
   The chart-list image URL remains captured because it is a distinct chart-image contract.
10. Smart-playlist routes use the durable, public-ID-preserving Phase 7 schema and bounded MQL
    evaluator. They do not preserve the frozen deployment's missing-table 500 for create, list,
    detail, update, evaluate, and delete. Legacy failure captures and positive Rust lifecycle tests
    are both required for these normalized routes.

No other observed difference is normalized merely because it appears undesirable.

## Alternatives considered

### Alternative A

Preserve every legacy defect exactly as observed.

- Rejected: keeps unsafe or broken client-impacting paths.

### Alternative B

Rewrite these behaviors without explicit tests.

- Rejected: unverifiable changes and compatibility regressions.

## Rationale

Explicitly scoping defect repairs keeps correctness and compatibility claims defensible.

## Consequences

### Positive

- Reduced client-facing errors in known high-impact paths.
- Better correctness for playlists, unknown IDs, recents, scrobble, and stream safety.

### Negative / trade-offs

- More ADR-linked differential cases and compatibility evidence required.
- A highly selective principal/category can receive a retryable 503 when its eligible facts fall
  beyond the fixed candidate window; operators receive a bounded-window exhaustion warning.

### Follow-up work

- Add per-defect differential cases and map each to a route disposition entry.
- Include each accepted fix only after ADR acceptance and test evidence.

## Validation

- Golden/fuzzer differential tests over `compatibility/melodee-api-v1/golden/*` scenarios.
- Regression evidence in route matrix and Phase 7 phase report.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- compatibility/COVERAGE-MATRIX.md
