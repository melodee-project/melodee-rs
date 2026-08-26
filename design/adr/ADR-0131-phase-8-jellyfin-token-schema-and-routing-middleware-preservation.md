# ADR-0131: Phase 8 Jellyfin token schema and routing middleware preservation

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Phase 8 Jellyfin adapter work package (explicitly authorized by the
  assigning session prompt)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-JF-001 through PRD-API-JF-006, PRD-STREAM-002, PRD-SEC-001;
  ADR-0102 (separate adapters), ADR-0105 (PostgreSQL authoritative store),
  ADR-0110 (sqlx and explicit SQL), ADR-0123 (scope-overlap governance)

## Context

The Jellyfin compatibility surface is a music-focused subset with its own stateful token model,
captured in Phase 0 (historical artifact removed,
`compatibility/jellyfin/contracts/route-inventory.json`):

- **Issuance:** `POST /Users/AuthenticateByName {Username, Pw}` (the DTO reads only `Pw`; a body
  with `Password` and no `Pw` is a captured 400) returns `{User, SessionInfo, AccessToken,
  ServerId}`. AccessToken is 32 random bytes hex-encoded (64 lowercase hex chars).
- **Storage:** tokens are stateful. The legacy server stores `HMAC-SHA256(pepper, salt + token)`
  with an 8-character cleartext prefix for lookup; expiry is
  `jellyfin.token.expiresAfterHours` (default 168h); at most 10 active tokens per user, with the
  oldest soft-revoked on overflow.
- **Transports:** `Authorization: MediaBrowser ... Token="..."`, `X-Emby-Authorization`,
  `X-MediaBrowser-Token`, `X-Emby-Token`, and the `api_key` query parameter.
- **Failures:** missing/garbage/expired/revoked tokens are indistinguishable 401
  JellyfinProblemDetails responses.
- **Logout:** `POST /Sessions/Logout` revokes the presented token (204, idempotent — even for
  garbage tokens); revoked tokens die immediately, unlike the stateless native JWT surface.
- **Routing:** `JellyfinRoutingMiddleware` rewrites detected requests to `/api/jf{path}` before
  endpoint selection. Four allowlisted pre-auth paths rewrite regardless of headers; marker
  headers (`MediaBrowser` authorization schemes, token headers, `api_key` under a Jellyfin path
  prefix) trigger rewrites elsewhere; `GET/HEAD /` rewrites only with a marker; non-music surfaces
  are plain empty 404s; `jellyfin.enabled=false` hard-404s explicit `/api/jf/*` while detected
  root-form requests fall through.

This state needs a PostgreSQL home. No schema may land without an accepted schema ADR, and the
routing middleware behavior is a frozen external contract (PRD-API-JF-002).

## Decision

1. **Token storage schema (migration `0021_phase8_jellyfin.sql`).** A dedicated adapter-owned
   table `jellyfin_access_tokens` keyed by v7 UUID with: `user_id` FK to `native_users`,
   `token_prefix` (exactly 8 cleartext hex chars, indexed, lookup only), `token_digest`
   (32 bytes, `HMAC-SHA256(pepper, salt + token)`; unique), per-row random 16-byte
   `token_salt`, `client`/`device_name`/`device_id` text captured from the MediaBrowser
   authorization header (bounded, nullable), `issued_at`, `expires_at`, `revoked_at`, and audit
   timestamps. Clear tokens are never stored, matching the legacy storage contract.
2. **Pepper and expiry are server configuration** (`MELODEE__JELLYFIN__TOKEN_PEPPER` required
   when the adapter is enabled, minimum 32 bytes; `MELODEE__JELLYFIN__TOKEN_EXPIRES_AFTER_HOURS`,
   default 168, bounded 1..=8760). The pepper never appears in logs, DTOs, or captures.
3. **Max-active enforcement:** on issuance, if a user already has 10 unrevoked unexpired tokens,
   the oldest (by `issued_at`, tiebreak `id`) is soft-revoked. Lookup is by `token_prefix` then
   digest verification; expired and revoked tokens are indistinguishable from invalid ones in the
   captured 401 envelope.
4. **Logout revocation:** `POST /Sessions/Logout` sets `revoked_at` for the presented token when
   it resolves; the response is 204 in all cases (idempotent, garbage tokens included).
5. **Routing middleware is preserved exactly** as the captured `JellyfinRoutingMiddleware`
   contract: the four-path allowlist, the marker detection rules, the marker-only root rewrite
   (200 empty, HEAD supported), pass-through of `/api/`, `/rest/`, `/song/` prefixes, the
   `jellyfin.enabled=false` behavior, and plain empty 404s for non-music surfaces. The adapter
   mounts canonical `/api/jf/*` routes; a tower middleware in the server composition root performs
   detection and rewriting before routing, so Jellyfin handlers always see canonical paths.
6. **Unknown-item and unsupported-scope behavior** (resolving coverage-matrix discrepancy D3 for
   this surface): unknown item ids are the captured clean 404 JellyfinProblemDetails; non-music
   paths are empty-body 404; non-music `includeItemTypes` are silently dropped from queries.
7. **Deterministic ordering** (resolving D10 for this surface): item listings append a stable
   tiebreak (item id) to every captured sort, removing the legacy nondeterministic
   `SongNumber`-only ordering.

## Alternatives considered

### Reuse native refresh-token tables for Jellyfin tokens

Rejected: the native model is stateless-JWT plus rotating refresh families with replay detection;
the Jellyfin model is a simple stateful access token with immediate logout revocation and
different failure envelopes. Sharing tables would couple two adapters' security semantics in
violation of ADR-0102 and SPEC §20 ("token conventions remain isolated").

### Implement the routing at the axum router level instead of middleware

Rejected: the detection contract inspects headers and query parameters across arbitrary paths
(including `/`) and must run before endpoint selection with pass-through fallbacks; that is
middleware semantics. Encoding it as routes would duplicate the allowlist/marker rules per route
and could not reproduce the root rewrite or the `jellyfin.enabled=false` fallthrough.

### Store tokens with a server-side symmetric encryption instead of HMAC digests

Rejected: the server only needs equality verification; HMAC-SHA256(pepper, salt + token) with an
indexed cleartext prefix reproduces the captured legacy storage contract and keeps clear tokens
out of the database.

## Rationale

The captured token model is small, stateful, and security-relevant; giving it a dedicated,
constraint-checked table preserves the exact client-visible lifecycle (issue → transport →
expiry/revocation → indistinguishable 401) while inheriting Phase 7 schema conventions (v7 ids,
timestamptz audit columns, CHECK constraints, no cleartext secrets). Preserving the middleware
contract verbatim is required by PRD-API-JF-002: Jellyfin clients depend on discovery at the
server root and on the marker-triggered rewriting, and the captured no-marker fallthrough to the
admin app is itself part of the contract.

## Consequences

### Positive

- Jellyfin tokens are isolated from native auth state; logout revocation is immediate and
  testable; expiry and max-active are enforced by a single issuance path.
- The routing middleware is one reviewed component with golden-backed tests for every detection
  branch, instead of logic scattered across handlers.
- All new schema is additive with CHECK constraints; binary downgrade leaves the table intact.

### Negative / trade-offs

- A second token store must be operated and backed up; token lookup is an indexed prefix scan
  plus HMAC verify per request.
- The middleware must run for every request at the server root, adding a cheap header inspection
  to non-Jellyfin traffic.

### Follow-up work

- `compatibility/COVERAGE-MATRIX.md` discrepancies D3 and D10 (Jellyfin parts) reference this ADR.
- Real-client smoke (Finamp, Feishin, Streamyfin) validates the token transports and discovery
  flow end to end (PRD-API-JF-006, Gate E).

## Validation

- Migration tests: constraint enforcement (digest length, prefix length, expiry ordering),
  max-10 soft-revocation, idempotent logout revocation, indistinguishable 401s.
- Golden differential replay of `compatibility/scenarios/jellyfin-discovery.json`,
  `jellyfin-auth.json`, and `jellyfin-playback.json` against the Rust adapter.
- Middleware unit/integration tests for every captured detection branch, the disabled-flag
  behavior, and the root rewrite.

## References

- [PRD.md](../../PRD.md) (PRD-API-JF-001..006, PRD-STREAM-002)
- [SPEC.md](../../SPEC.md) §19.3, §20, §24
- Jellyfin capture evidence (historical artifact removed) §1–§2
- `compatibility/COVERAGE-MATRIX.md` discrepancies D3, D10
