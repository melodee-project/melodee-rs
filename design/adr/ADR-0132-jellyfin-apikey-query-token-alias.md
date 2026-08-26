# ADR-0132: Jellyfin ApiKey query token alias

- **Date:** 2026-08-13
- **Status:** Accepted
- **Decision owners:** Phase 8 real-client smoke work package (explicitly authorized by the
  assigning session prompt)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-JF-001, PRD-API-JF-005, PRD-STREAM-002, PRD-SEC-001;
  ADR-0102 (separate adapters), ADR-0131 (Jellyfin token schema and routing preservation)

## Context

Jellyfin clients embed the access token in media/image URLs because HTML media elements cannot
send authorization headers. Upstream Jellyfin's `AuthorizationContext` reads the query parameter
`ApiKey` (case-insensitive `IQueryCollection`) as the **primary** query token source and treats
`api_key` as a legacy fallback gated behind `EnableLegacyAuthorization`.

The legacy Melodee server's `JellyfinTokenParser` read **only** `api_key`. The Phase 8
real-client smoke (Feishin v1.15.1 web build, driven by Playwright; evidence in the withdrawn
`design/evidence/phase-8/REAL-CLIENT-ACCEPTANCE.md` report) showed that Feishin builds playback URLs as
`/Items/{id}/Download?apiKey={token}` — so audio playback 401'd against the Rust adapter, exactly
as it would have against the legacy server. This is a legacy real-client defect the frozen
captures never exercised: every golden scenario authenticates media requests with `api_key`.

Changing or removing `api_key` acceptance is not in scope — that is the captured behavior and
clients (Finamp) depend on it.

## Decision

The Jellyfin adapter accepts the access token from both query parameters, matched
case-insensitively:

1. `api_key` — captured behavior, unchanged precedence position.
2. `ApiKey` — alias accepted at the same position (only when no header supplied a token, matching
   the legacy short-circuit order: `Authorization` / `X-Emby-Authorization` /
   `X-MediaBrowser-Token` / `X-Emby-Token` headers first, query parameters last).

No captured golden changes; this is strictly additive. Header precedence over query parameters
is preserved, including the legacy behavior that a present-but-invalid header token wins over a
valid query token.

## Alternatives considered

### Alternative A — capture-faithful `api_key` only

Keeps byte-for-byte parity with the legacy defect. Rejected: the Phase 8 exit criteria require
documented real-client smoke workflows to pass, and the defect blocks all browser/desktop clients
that emit `ApiKey` media URLs. The golden corpus does not pin the 401 for `ApiKey` (no capture
used it), so no compatibility evidence is violated.

### Alternative B — full upstream Jellyfin query parsing

Upstream also reads other query aliases under legacy-auth configuration. Rejected as scope creep:
only `ApiKey` is evidenced by real-client traffic in the smoke; anything more invents surface the
captures do not cover.

## Rationale

Phase 8 exists to make real clients work, not to replicate legacy bugs that broke them. The
widening is invisible to every captured scenario, aligns the adapter with upstream Jellyfin's
primary token parameter, and was proven necessary and sufficient by the shipping Feishin client
(playback assertion passes only with the alias).

## Consequences

### Positive

- Feishin (and any other client emitting `ApiKey` media URLs) streams audio from the Jellyfin
  surface.
- Documented, tested divergence-from-defect alongside ADR-0130 decision 4 (double-range bug) and
  the playlist cover-resolution 302 rollback.

### Negative / trade-offs

- One intentional divergence from the legacy server where the legacy server is demonstrably
  broken for real clients; recorded here so future compatibility audits can see it was decided,
  not accidental.
- A second accepted credential location marginally widens the token-bearing URL surface; token
  redaction in logs already covers query strings.

### Follow-up work

- If future captures from a legacy server configured differently show `ApiKey` rejecting, revisit
  this decision with that evidence.

## Validation

- `apps/melodee-server/tests/jellyfin_core_api.rs::apikey_query_param_authenticates_alongside_api_key`
  — `apiKey`, `ApiKey`, and `api_key` authenticate the captured token-gated `/Users` route;
  garbage tokens still 401.
- Real-client evidence: Feishin smoke playback assertions
  (`compatibility/captures/phase8-client-smoke/assertions.json`) pass on both surfaces.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- ADR-0131 (Jellyfin token schema and routing preservation)
- ADR-0130 (OpenSubsonic auth decisions; precedent for not-preserving legacy auth defects)
- Phase 8 real-client acceptance (withdrawn evidence report
  `design/evidence/phase-8/REAL-CLIENT-ACCEPTANCE.md`)
- Upstream: `jellyfin/jellyfin` `MediaBrowser.Controller/Net/AuthorizationContext.cs`
  (`queryString["ApiKey"]` primary, `api_key` legacy-gated fallback)

## Phase 9 status note (2026-08-13)

- **Status changed: Proposed → Accepted.** The validation evidence named above now exists:
  the Phase 8 differential replay reports zero unexpected differences and the shipping Feishin
  v1.15.1 client completed the documented Jellyfin workflows (12/12 assertions, recorded in the
  withdrawn `design/evidence/phase-8/REAL-CLIENT-ACCEPTANCE.md` report) against the shipped
  implementation.
