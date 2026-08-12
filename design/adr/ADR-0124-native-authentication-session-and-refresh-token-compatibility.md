# ADR-0124: Native authentication, session, and refresh token compatibility

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Repository maintainers (phase governance)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-AUTH-001..007, PRD-API-M1-001..007

## Context

Phase 7 requires native auth/session behavior to be compatible with legacy clients without forcing client updates. This includes JWT/login/refresh/logout/revoke flows, provider sessions where enabled, and token-failure semantics.

## Decision

- Preserve the captured legacy bcrypt password hashes on migration. Verification uses the encoded
  bcrypt parameters and a constant-time verifier; successful login may create a stronger replacement
  hash only after the old hash has verified.
- Preserve the native auth API request/response shapes where observed.
- Preserve access-token behavior without weakening security posture: refresh-token families are replay-protected and atomically rotated; secrets and hashes are never logged.
- Preserve account state (disabled/locked/blacklisted) response behavior where exposed.
- Preserve provider and social-linking shape where legacy native-v1 behavior exists. Phase 9 owns
  only non-native/full-domain breadth under ADR-0123.

The additive PostgreSQL authority consists of:

- `native_users`: immutable UUIDv7 internal ID, immutable unique legacy/public API UUID, normalized
  user name used only for login lookup, display/user fields, password hash and algorithm tag,
  enabled/locked/blacklisted state, locale, roles and monotonically increasing auth revision.
- `native_refresh_families`: UUIDv7 family ID, user/device binding, creation/expiry/revocation and
  revocation reason. One family is locked for any rotation or revocation.
- `native_refresh_tokens`: UUIDv7 token ID, family ID, SHA-256 token digest only, issued/expiry/
  consumed/revoked timestamps and replacement-token lineage. Clear tokens are returned exactly
  once and never persisted.
- `native_password_reset_tokens`: UUIDv7 token ID, User binding, SHA-256 token digest only,
  issue/expiry/consumption timestamps, and one-use lineage. Creating a token invalidates any
  earlier unconsumed reset token for that User in the same transaction. Clear reset tokens are
  returned only by the captured development-mode response and are never logged or persisted.
- `native_user_social_logins`: User, normalized provider name, immutable provider subject,
  verified provider email/display/hosted-domain facts, linked/last-login timestamps, and unique
  provider/subject ownership. Linking, unlinking, and first-login creation are transactional and
  cannot silently move a provider identity between Users.

JWT v1 remains HS256 with issuer `MelodeeApi`, audience `MelodeeClient`, zero lifetime clock skew,
the captured claims, a 15-minute access lifetime, 30-day refresh lifetime, and configured absolute
session cap. The legacy `/api/v1/auth/refresh` requires a valid unexpired access JWT. The rotating
`/api/v1/auth/refresh-token` uses a SHA-256 digest encoded consistently with the migration format.

All state transitions use one PostgreSQL transaction and row locks. Refresh rotation consumes exactly one current
token and inserts its successor. Reuse of a consumed/revoked token atomically revokes the family.
Logout revokes all refresh families for the authenticated user while already-issued access JWTs
remain valid until expiry, matching the captured stateless behavior. Account disabled/locked/
blacklisted checks still fail closed at new authentication/refresh operations.
No per-token access-revocation table participates in captured logout/revoke: access JWTs remain
stateless and valid until expiry. IDs, digests, state tuples, and timestamp relationships are database constrained. Migration is
additive; binary downgrade retains these tables, and full schema rollback uses a verified pre-Phase-7
backup restored into a new database.

Password-reset request responses remain non-enumerating. Reset tokens are cryptographically random,
stored only as SHA-256 digests, expire after the configured positive bounded lifetime (60 minutes by
default), and are consumed atomically with the bcrypt password replacement and refresh-family
revocation. The captured development response may include the one-time clear token and reset URL;
production preserves the generic response without exposing whether the email exists.

Google authentication is disabled unless explicitly configured. When enabled, Melodee validates the
Google ID token's asymmetric signature against Google's HTTPS JWKS, exact issuer, configured audience,
expiry/not-before with bounded configured skew, verified-email flag, and optional hosted-domain
allowlist before any account lookup/link/write. JWKS responses use a bounded in-memory cache honoring a
capped freshness interval; unknown key IDs may trigger one refresh, never unbounded retries. Client IDs,
provider tokens, subjects, and provider secrets are redacted from logs. Self-registration and automatic
email linking are separate fail-closed settings; otherwise a new provider subject must be linked by an
already authenticated User. Provider-disabled behavior preserves the captured typed rejection.

The exact-pinned `jsonwebtoken` 11.0.0 crate (MIT) with its `aws_lc_rs` backend owns RS256 and JWK
parsing/verification. That backend is already the repository's rustls cryptographic provider and,
unlike the crate's all-algorithm RustCrypto feature, does not pull the RSA 0.9 line affected by
RUSTSEC-2023-0071. Melodee does not implement RSA or JWK parsing itself. The existing rustls-only
`reqwest` client retrieves JWKS with redirects disabled, response/time/key-count bounds, and a
singleflight capped cache. Reqwest's platform-verifier certificate bundle is distributed under the
FOSS `CDLA-Permissive-2.0` license, which is explicitly admitted by the repository license gate.

## Alternatives considered

### Alternative A

Redesign auth to a new provider-first OAuth-only flow.

- Rejected: violates preserved native client compatibility.

### Alternative B

Retain access tokens forever and rotate refresh tokens lazily.

- Rejected: increases replay risk and weakens security.

## Rationale

The decision maximizes compatibility while preserving explicit security invariants and replay safety.

## Consequences

### Positive

- Existing users and clients can be migrated without forced mass password reset.
- Replay/revocation behavior becomes testable and auditable.

### Negative / trade-offs

- More schema and worker complexity for family tracking and token lineage.
- More tests for concurrency/replay paths.
- Enabled Google authentication requires outbound HTTPS to Google's signing-key endpoint and a
  small FOSS HTTP/JWT verification dependency surface; deployments that do not enable it make no
  provider request.

### Follow-up work

- Add session and token schema + migration in persistence layer.
- Add adversarial tests for replay, replay after logout, malformed credentials, and family reuse.
- Add reset-token one-use/race tests and Google issuer/audience/key-rotation/hosted-domain tests
  against a local deterministic JWKS fixture; tests never depend on the live Google service.

## Validation

- Golden and adversarial auth tests against legacy oracle capture.
- Fault/replay tests in integration and property-based suites.
- Real-client auth flows in scoped environments.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
