# ADR-0129: Secure native public-share bearers and resolvable links

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (Phase 7 security remediation)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-SHARE-001, PRD-SHARE-002, PRD-SHARE-003, PRD-SEC-004, PRD-API-M1-004

## Context

Native public shares are anonymous bearer capabilities. The frozen API returns a `shareUrl`, and
existing share identifiers are migration data. The first Phase 7 independent review found that the
initial Rust implementation derived a 16-hex-character value from the beginning of a UUIDv7. Given
the creation millisecond, that value contains only about 12 unpredictable bits and can disclose a
private playlist through enumeration. The returned `/share/{token}` URL was also not mounted.

Security of an anonymous bearer takes precedence over retaining a weak identifier. Compatibility
still requires preserving existing share URLs when their imported tokens meet the security floor,
and requires an explicit rotation result when they do not.

## Decision

1. New public-share tokens contain at least 128 bits from the operating system CSPRNG. Encoding is
   URL-safe and unpadded. Token creation uses a bounded collision retry against a unique database
   constraint and never derives the bearer from a timestamp or internal/public resource UUID.
2. The token is treated as a secret. It is never logged, placed in tracing fields, returned by list
   APIs except as the captured `shareUrl`, or accepted through a query string.
3. `/share/{token}` is a compatibility alias for the anonymous native public-share representation.
   Until a future web UI owns that path, it returns the same bounded JSON/status contract as
   `/api/v1/shares/public/{token}`. This makes the generated `shareUrl` usable verbatim without
   inventing an HTML redirect or Phase 10 UI.
4. Share import is transactional and replay-safe. It records the legacy identity binding and
   preserves an existing token only when that token represents at least 128 bits of unpredictable
   bearer material in an accepted encoding. Weak legacy tokens are rotated during import; the
   import result records the old identity and new URL so the cutover report can notify the owner.
   Weak old bearer URLs are not left active.
5. Import exact retry adopts an identical mapping. A conflicting retry, duplicate token, cross-user
   target, missing target, or unsupported token representation fails closed.
6. Public resolution is rate-limited and returns the same not-found result for unknown, expired,
   revoked, unauthorized-target, and malformed tokens. It never reveals whether a candidate was a
   former weak legacy token.

## Alternatives considered

### Preserve the UUIDv7 prefix

Rejected. A known timestamp reduces the anonymous bearer to an enumerable space and violates
PRD-SEC-004.

### Preserve every legacy token indefinitely

Rejected. Compatibility does not authorize retaining a known weak anonymous capability. Secure
tokens are preserved; weak tokens receive an explicit migration rotation result.

### Return a web page from `/share/{token}` now

Rejected. Phase 10 owns the administrative/web UI. Serving the captured bounded JSON contract keeps
the link functional without inventing a UI contract.

## Consequences

New and qualifying migrated links remain stable. Weak legacy links require owner-visible rotation
at cutover. The server has two paths to the same anonymous representation, both protected by the
same token, expiry, target-authorization, and rate-limit checks.

## Validation

- Deterministic injected-RNG tests prove full entropy consumption, collision retry, and exhaustion.
- PostgreSQL import tests prove exact adoption, conflict rejection, secure preservation, weak-token
  rotation, identity binding, and cross-owner/target fencing.
- HTTP tests follow the returned `shareUrl` verbatim and prove indistinguishable unknown/expired/
  revoked/malformed behavior without logging the bearer.
- Route evidence identifies this ADR for the alias and the intentional weak-token migration
  normalization.

## References

- [PRD](../../PRD.md)
- [SPEC](../../SPEC.md)
- [ADR-0127](ADR-0127-native-public-id-migration-and-id-preserving-schema-for-api-v1.md)
