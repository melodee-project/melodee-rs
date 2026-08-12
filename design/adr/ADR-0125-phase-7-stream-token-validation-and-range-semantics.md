# ADR-0125: Phase 7 stream token validation and range semantics

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (phase governance)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-STREAM-001, PRD-STREAM-002, PRD-AUTH-001..007, PRD-API-M1-001..007

## Context

Streaming is part of first-party client core experience. Native stream URLs and range behavior must remain safe and compatible while preventing malformed token abuse and unsafe file access.

## Decision

- Preserve native route shape and media authorization semantics where captured.
- Preserve the captured non-versioned route and token encoding:
  URL-safe unpadded base64 of
  `"{userGuid}:{trackGuid}:{clientIp}|{expiryUnixSeconds}|{standard-padded-base64(HMAC-SHA256)}"`,
  URL-encoded in the path. HMAC-SHA256 covers the exact `data|expiry` prefix. The HMAC secret is the User's preserved `PublicKey`; the default expiry
  is seven days; User/Track path IDs, bound client IP, and expiry are validated. HMAC comparison is
  constant-time. Any future different token uses a versioned contract plus a client migration plan.
- Reject malformed tokens, an empty/unknown User ID, a wrong-Track token, and tampering with the
  same non-enumerating typed 401 response. The captured legacy login redirects for malformed stream
  credentials are approved safety normalizations under ADR-0128, not preserved.
- Support HEAD, full GET, initial/middle/suffix/open-ended ranges, and concurrent playback with bounded concurrency and cancellation safety.
- Trust proxy-derived client addresses only under ADR-0126; the address used when issuing and
  consuming the token is resolved by the same function. The tolerated MeloAmp `?jwt=` query value
  is ignored by this allow-anonymous HMAC route and is never logged.
- Resolve only the catalog-authoritative playable MediaAsset and open it through the already-owned
  canonical storage root using descriptor-relative no-follow traversal through the validated
  canonical Release directory plus regular-file and containment rules. A path
  supplied by the request is never used. Cancellation stops body streaming promptly.

Only one byte range is accepted. Full GET and HEAD return 200; satisfiable initial, middle, suffix,
and open-ended ranges return 206 with exact `Content-Range`, `Content-Length`, `Content-Type`, and
`Accept-Ranges`. Multipart, malformed, inverted, and unsatisfiable ranges preserve the captured 400
`BAD_REQUEST` envelope rather than silently serving the full asset.

## Alternatives considered

### Alternative A

Use a fully static pre-signed public URL scheme without server binding checks.

- Rejected: increases token leakage and weakens cancellation/revocation safety.

### Alternative B

Drop range support and return full bodies only.

- Rejected: breaks seek/resume workflows and known native clients.

## Rationale

This preserves behavior needed for Android/MeloAmp playback while enforcing modern safety constraints.

## Consequences

### Positive

- Seek/resume and native playback behavior stay functional.
- Security posture improves around token and path validation.

### Negative / trade-offs

- Requires strict edge-case test matrix (malformed/overlapping ranges, discontinuous requests).
- Slightly stricter failure handling may expose legacy baseline defects for explicit repair.

### Follow-up work

- Implement the captured signed stream token and route in the native adapter.
- Add stream-range property matrix with malformed/inverted/unsatisfiable cases.

## Validation

- Golden stream matrix and real-client playback checks (seek/head/full GET/range).
- Path traversal and non-regular file attempts are explicitly denied in integration tests.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
