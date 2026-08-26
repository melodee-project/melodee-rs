# ADR-0130: Phase 8 OpenSubsonic authentication, extension advertisement, and streaming corrections

- **Date:** 2026-08-12
- **Status:** Accepted
- **Decision owners:** Phase 8 OpenSubsonic adapter work package (explicitly authorized by the
  assigning session prompt to resolve the open Phase 8 product decisions)
- **Supersedes:** None
- **Superseded by:** ADR-0147 (partially — only the "transcode parameters remain silently
  ignored" clause of decision 2 and the "There is no transcoding" sentence of decision 4; all
  other decisions stand)
- **Related requirements:** PRD-API-OS-001 through PRD-API-OS-007, PRD-STREAM-002, PRD-SEC-001,
  PRD-TEST-001/002; ADR-0102 (separate adapters), ADR-0113 (no unsigned JWT),
  ADR-0123 (scope-overlap governance), ADR-0124 (native auth precedent)

## Context

Phase 0 capture (historical artifact removed,
`compatibility/COVERAGE-MATRIX.md`) left four OpenSubsonic discrepancies that require an explicit
product decision before the Phase 8 adapter can be implemented:

- **D1 (auth):** the legacy `jwt` parameter branch accepts forged unsigned JWTs and takes
  precedence over password validation. ADR-0113 forbids preserving the bypass and defers to
  Phase 8 the decision whether JWT authentication exists at all on the Rust surface.
- **D8 (extensions):** `getOpenSubsonicExtensions` advertises `apiKeyAuthentication[1]` and
  `transcodeOffset[1]`, but the legacy server never consults the `apiKey` parameter and silently
  ignores transcode parameters. The advertisement is a captured false claim.
- **D3 (unknown ApiKey class):** unknown-API-key behavior differs per surface (native 500,
  OpenSubsonic ok+error envelope / 302 class, Jellyfin 404). Each surface's behavior is part of
  its frozen contract.
- **D2 (streaming):** the legacy 206 path applies the Range header twice, corrupting every
  nonzero-offset seek (416 or wrong bytes). The Phase 0 report (risk R2) requires this bug to be
  fixed, not reproduced.
- **D10 (lists):** legacy list sorts lack tiebreaks, producing nondeterministic ordering on equal
  sort keys.

Authentication additionally needs a credential store: token+salt auth is
`md5(opensubsonicSecret + salt)` per user, so the server must be able to recompute the md5 from a
recoverable per-user secret. The legacy server stored the secret reversibly encrypted
(AES-GCM `SecretProtector`).

## Decision

1. **No JWT authentication branch exists on the Rust OpenSubsonic surface.** The `jwt`
   parameter is never consulted. A request carrying only `jwt` fails exactly like the captured
   jwt-only case (HTTP 200 + `subsonic-response` error 40). A request carrying `jwt` plus `u`/`p`
   or `u`/`t`/`s` is decided solely by the password or token branch — a forged unsigned JWT with a
   wrong password is rejected, which is the ADR-0113 regression case. This resolves the ADR-0113
   follow-up with client evidence: both inventoried native clients use token or password auth,
   and no legacy client could have depended on signed-JWT verification the legacy server never
   performed.
2. **The extension advertisement list is preserved byte-for-byte**, including the false
    `apiKeyAuthentication[1]` and `transcodeOffset[1]` claims and the malformed XML serialization
    quirk (attributes with no element name). The `apiKey` parameter remains unconsulted (captured:
    error 40) and transcode parameters remain silently ignored (captured: full original bytes at
    200). *(2026-08-16: the silent-ignore clause is superseded by ADR-0147 — `format`/`maxBitRate`
    now invoke configured transcoding; the frozen advertisement including `transcodeOffset[1]`,
    the unconsulted `apiKey`, and the `timeOffset` 404 + error 70 rejection are unchanged.)*
    Changing a frozen, client-visible advertisement is itself a compatibility event; no
   inventoried client can be using apiKey against Melodee because the legacy server never honored
   it, so preserving the captured behavior carries no real client risk. This is the explicit
   product decision resolving discrepancy D8: the advertisement is inherited for parity, and the
   false claim is documented rather than silently "fixed".
3. **Per-surface unknown-credential behavior is preserved exactly** (resolving D3 for this
   surface): OpenSubsonic authentication failures are the captured identical HTTP 200 +
   error 40 envelopes (HTTP 403 for the podcast controller), with no user enumeration.
4. **OpenSubsonic streaming implements correct single-pass range handling** (resolving D2):
   206 responses carry the true total size in `Content-Range`, nonzero-offset seeks return the
   requested bytes, unsatisfiable/invalid ranges and `timeOffset` return the captured
    404 + error 70 JSON envelope. The legacy double-range-processing bug is explicitly not
    preserved. ~~There is no transcoding: `maxBitRate`/`format` are silently ignored, matching the
    captured baseline.~~ *(2026-08-16: superseded by ADR-0147 — configured transcoding replaces
    silent-ignoring; the range-handling and `timeOffset` clauses of this decision stand.)*
5. **All adapter list responses use deterministic tiebreaks** (resolving D10 for this surface):
   every sorted list appends a stable secondary key (public id) so equal primary keys order
   deterministically. This is a recorded do-not-preserve normalization.
6. **Per-user OpenSubsonic secrets are stored in a dedicated adapter-owned table**
   (`opensubsonic_user_secrets`, migration `0020_phase8_opensubsonic.sql`) keyed by the native
   user id. The secret is an API token: it must be recoverable to recompute
   `md5(secret + salt)`, so it is stored as cleartext in the database — equivalent in exposure to
   the legacy reversible encryption, isolated from password hashes, never logged, and redacted
   from captures. Token comparison is case-insensitive hex, matching the captured contract.
   Password-form auth (`u+p`, plain or `enc:` hex) verifies against the existing bcrypt
   `native_users.password_hash`.
7. **The captured localhost/baseUrl/`/images/` auth bypass is preserved**: requests that are
   local, whose host equals the configured base URL, or whose path contains `/images/` require no
   credentials and synthesize no user (the captured BlankUserInfo behavior). Stream and cover-art
   routes depend on this bypass for client artwork loading.

## Alternatives considered

### Implement JWT authentication with proper signature verification

Rejected for this phase: the legacy server had no trusted OpenSubsonic JWT key configuration —
the branch existed only as the unsigned-acceptance flaw. There is no captured signed-JWT client
behavior to be compatible with, no configured key to verify against, and adding a new
key-management surface expands the security perimeter for zero evidenced client need. If a future
client requires signed JWTs, a superseding ADR with client evidence can add the branch.

### Remove the false extension advertisements or make apiKey functional

Rejected: both change a frozen client-visible contract. Dropping entries risks breaking clients
that gate features on advertised support; implementing apiKey changes the captured error-40
behavior and introduces a second credential type with no evidenced client need. Preservation plus
documentation is the minimal-risk resolution; a future product decision with client evidence can
supersede this ADR.

### Store OpenSubsonic secrets hashed

Impossible for token+salt auth: the server must recompute `md5(secret + salt)` for an arbitrary
client-chosen salt, which requires the cleartext secret. Password-form auth already uses bcrypt;
the token secret is a separate, revocable API credential.

## Rationale

Compatibility parity is the default for every captured behavior; deviations are limited to the
three classes the evidence program already flags as do-not-preserve (the auth bypass, the seek
corruption, nondeterministic sorts), each of which is a correctness or security defect rather than
observable contract. Keeping the advertisement list frozen avoids an unforced contract change.
Isolating the secret store in an adapter-owned table honors ADR-0102 surface separation while
reusing the Phase 7 user identity and password-verification primitives.

## Consequences

### Positive

- The ADR-0113 follow-up is closed: no JWT branch, and the forged-JWT + wrong-password attack is
  rejected by construction and by a mandatory regression test.
- The extension advertisement, error envelopes, and auth transports remain byte-compatible with
  the frozen golden captures; the differential suite needs no new masks.
- Streaming seeks are actually correct — the Rust server is measurably better than the legacy
  baseline on the D2 defect without changing any valid full-stream or suffix-range behavior.
- Deterministic list ordering removes a flaky-behavior class across all list endpoints.

### Negative / trade-offs

- Clients probing `apiKeyAuthentication` support and then attempting apiKey auth still receive
  error 40 — the inherited false claim persists by decision, documented here and in the coverage
  matrix.
- Cleartext per-user OpenSubsonic secrets in PostgreSQL require the same operational care as the
  legacy reversible encryption (database access implies token access); mitigated by redaction
  rules, separation from password hashes, and per-user revocation.

### Follow-up work

- The Phase 8 adapter MUST ship regression tests for: forged unsigned JWT + wrong password
  rejected (ADR-0113), jwt-only error 40, apiKey-only error 40, nonzero-offset seek correctness
  (D2), deterministic list ordering (D10).
- `compatibility/COVERAGE-MATRIX.md` discrepancies D1, D2, D3 (OpenSubsonic part), D8, and D10
  (OpenSubsonic part) are updated to reference this ADR as their resolution.

## Validation

- Golden differential replay of `compatibility/scenarios/opensubsonic-auth.json`,
  `opensubsonic-system.json`, `opensubsonic-media.json`, and `opensubsonic-lists.json` against
  the Rust adapter with only the scenario-declared volatile masks.
- Route-level Rust tests named by the Phase 8 OpenSubsonic route-evidence registry
  (`compatibility/opensubsonic/contracts/phase8-route-evidence.json`).

## References

- [PRD.md](../../PRD.md) (PRD-API-OS-001..007, PRD-STREAM-002)
- [SPEC.md](../../SPEC.md) §19.2, §20, §24
- [ADR-0113](ADR-0113-do-not-preserve-insecure-opensubsonic-unsigned-jwt-authentication.md)
- OpenSubsonic capture evidence (historical artifact removed) §3–§5
- `compatibility/COVERAGE-MATRIX.md` discrepancies D1, D2, D3, D8, D10
