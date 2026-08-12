# ADR-0126: Rate limiting and client-address semantics for native API v1

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (phase governance)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-AUTH-005, PRD-AUTH-006, PRD-API-M1-005, PRD-API-M1-006

## Context

Legacy compatibility requires preserved native response semantics, but phase-7 rollout also needs bounded resource behavior that does not weaken client behavior.

## Decision

- Preserve legacy-compatible rate throttling behavior where it exists in captured baselines.
- Derive request actor identity from the authenticated principal where available. Unauthenticated
  auth attempts use the normalized user-name plus a trusted-proxy-safe client-address key.
- Accept `X-Forwarded-For`/`Forwarded` only from configured trusted proxies; otherwise use direct remote address.
- Ensure rate-limit errors are typed and non-secretive, include `Retry-After`, and preserve the
  captured general error envelope.
- Native browser/Electron clients use cross-origin requests. CORS is therefore an explicit,
  bounded allowlist of exact configured origins with the captured native methods and
  `Authorization`/`Content-Type` request headers. Wildcard origins are forbidden when credentials
  are enabled; an absent production allowlist does not silently expose the API. Preflight handling
  is covered by route-level tests and the public configuration reference.

Rate-limit counters are bounded by configured window, per-key attempt ceiling, total retained key
ceiling, and expiry. Configuration rejects zero/unbounded values and proxy entries that are not
explicit CIDR/address values. When the key ceiling is reached, new unauthenticated keys fail closed
without evicting an active key. A multi-process deployment uses PostgreSQL-backed atomic counters;
an in-process-only limiter must not be presented as cluster-wide protection.

## Alternatives considered

### Alternative A

Disable rate limiting to avoid legacy behavior divergence.

- Rejected: allows abuse and removes an operational control required by PRD-AUTH-006.

### Alternative B

Always trust `X-Forwarded-For`.

- Rejected: unsafe against spoofed headers on untrusted ingress.

## Rationale

This model is minimally invasive and preserves compatibility while preventing address spoofing and abuse.

## Consequences

### Positive

- Better operational safety without forced client breaks.
- Clear audit surface for address-binding failures.

### Negative / trade-offs

- Adds trusted-proxy configuration complexity.

### Follow-up work

- Add config schema validation and tests for trusted proxy and header precedence.

## Validation

- Integration tests: header precedence and spoofing rejection.
- Load tests for bounded rejection under high request volume.
- Security reviews of secret-safe logs and observability.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
