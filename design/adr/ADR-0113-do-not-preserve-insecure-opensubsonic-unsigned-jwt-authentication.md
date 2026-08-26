# ADR-0113: Do not preserve insecure OpenSubsonic unsigned-JWT authentication

- **Date:** 2026-08-09
- **Status:** Accepted
- **Decision owners:** Phase 0 closure work package (explicitly authorized by the assigning
  session prompt)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-OS-003, PRD-SEC-001 (security requirements), PRD-TEST-001/002
  (compatibility testing), ADR-0102 (separate API adapters), ADR-0103 (FOSS)

## Context

Phase 0 behavioral capture against the frozen legacy oracle (commit
`4028b58dbd3302ed431e2c5b32d9dde5634db218`) proved that the legacy C# server authenticates
OpenSubsonic requests carrying a **forged, unsigned JWT**:

- The `jwt` query/form parameter branch in `OpenSubsonicApiService` base64url-decodes the JWT
  payload and maps `sid`/`name`/`sub` claims to a user with **no signature verification**
  (golden evidence: `compatibility/opensubsonic/golden/auth/os-auth-jwt-forged-unsigned*`).
- The branch requires a username plus any password/token value to pass the request guard, then
  takes **precedence over password validation** — a wrong password is ignored when a JWT is
  present.
- Consequence: anyone who knows a username can authenticate as that user with a self-minted
  unsigned JWT.

Blind byte-for-byte behavioral parity would reproduce an authentication bypass. This is a
security vulnerability in the legacy server, not compatibility behavior worth preserving.

## Decision

The Rust implementation MUST reject:

- unsigned JWTs;
- JWTs with invalid signatures;
- malformed JWTs;
- JWTs signed with an untrusted key;
- expired JWTs where expiration is required;
- any JWT that otherwise fails configured validation.

This is an intentional, recorded compatibility deviation from the legacy C# behavioral oracle.
The deviation is scoped to the JWT authentication branch only; all other OpenSubsonic
authentication behavior (token+salt, password forms, failure envelopes, bypass contract)
remains parity-covered by golden fixtures.

## Alternatives considered

### Preserve the behavior exactly

Rejected: reproducing a known authentication bypass would make the rewrite objectively less
secure than acceptable and could expose user libraries to trivial takeover. Compatibility rules
never require preserving a vulnerability.

### Remove JWT authentication entirely from the OpenSubsonic surface

Not chosen at this time: the JWT path exists for Navidrome-style client compatibility. The
Phase 8 adapter may implement it with proper signature verification, or drop it if no current
client requires it — that decision belongs to the OpenSubsonic adapter phase with client
evidence. Either outcome satisfies this ADR (no unsigned acceptance).

## Rationale

Security requirements override preservation of a known authentication bypass. Golden fixtures
document historical behavior; they do not mandate reproducing vulnerabilities. Recording the
deviation explicitly keeps the compatibility program honest: the difference between the legacy
oracle and the Rust implementation is deliberate, reviewed, and testable.

## Consequences

### Positive

- The rewrite cannot inherit the authentication bypass.
- The deviation is documented, testable, and visible in compatibility tooling.

### Negative / trade-offs

- A hypothetical client depending on unsigned-JWT acceptance would break. No such client is
  known: both inventoried native clients use token or password auth, and Navidrome-compatible
  clients send signed JWTs.

### Follow-up work

- Compatibility tooling marks the captured legacy behavior as **Approved Compatibility
  Deviation** (see `compatibility/COVERAGE-MATRIX.md` discrepancy D1 and
  `scripts/build_coverage_matrix.py` dispositions) instead of expecting exact reproduction.
- The Phase 8 OpenSubsonic adapter MUST include a regression test proving the legacy attack
  case (forged unsigned JWT + wrong password) is rejected.
- Phase 8 decides whether JWT auth exists at all on the Rust OpenSubsonic surface; if it does,
  it must verify signatures against configured keys.

## Validation

- Phase 0 golden evidence of the legacy vulnerability:
  `compatibility/opensubsonic/golden/auth/os-auth-jwt-forged-unsigned.json` (+ metadata).
- Phase 8: adapter test suite must contain the forged-JWT rejection case before the surface
  ships.

## References

- [PRD.md](../../PRD.md) (PRD-API-OS-003, security requirements)
- [SPEC.md](../../SPEC.md)
- Phase 0 report (withdrawn evidence report `design/evidence/phase-0/PHASE-0-REPORT.md`) (risk R1)
- OpenSubsonic capture evidence (withdrawn evidence report
  `design/evidence/phase-0/opensubsonic-capture.md`) section 3
- `compatibility/opensubsonic/golden/auth/os-auth-jwt-forged-unsigned.json`
