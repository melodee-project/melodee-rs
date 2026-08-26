# ADR-0139: Admin SPA session strategy and admin API gating

- **Date:** 2026-08-14
- **Status:** Accepted
- **Decision owners:** Phase 10
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-ADMIN-001, PRD-API-ADMIN-002, PRD-API-ADMIN-003, PRD-AUTH-003

## Context

The admin SPA needs a session mechanism. Phase 7 shipped the frozen native-v1 auth contract,
including cookie sign-in/sign-out variants (`native/auth.rs`) alongside Bearer tokens; that
contract must not change (PRD-API-ADMIN-003, AGENTS.md compatibility rules). Separately, the
Phase 2 diagnostics endpoints (`/api/admin/v1/ingest/*`, `/api/admin/v1/system/info`) were shipped
**without** authentication during early phases (documented as a pre-release dev-server caveat in
`docs/api/admin-api.md`); every other `/api/admin/v1` group already requires `AdminPrincipal`.
Leaving operator queue data, failure details, and system information anonymous-readable is not an
acceptable production posture.

The admin API is a Rust-native, non-frozen surface (Phase 9 report: "New routes added by Phase 9
are Rust-native admin/operator surfaces only, not frozen contracts"), so gating these endpoints is
a hardening change, not a contract break.

## Decision

- **SPA session:** the admin SPA authenticates through the existing native-v1 cookie sign-in
  endpoints (`POST /api/v1/auth/sign-in` cookie variant, refresh, sign-out). The SPA stores no
  access token in web storage; it relies on the server-issued cookie and same-origin deployment
  (ADR-0138). No change to any native-v1 route, DTO, or status code.
- **Authorization:** admin screens and all `/api/admin/v1` routes require an enabled, unlocked
  account with administrator rights (`isAdmin` or `Administrator` role) — the existing
  `AdminPrincipal` semantics. The SPA enforces route guards for UX only; the server enforces
  authorization for every data-changing action (SPEC §20), closing the legacy Blazor
  service-layer-auth mismatch flagged in historical artifact removed.
- **Diagnostics gating:** `/api/admin/v1/ingest/*` and `/api/admin/v1/system/info` adopt
  `AdminPrincipal` (401 anonymous / 403 non-admin with the stable error envelope). `/healthz` and
  `/readyz` remain anonymous infrastructure probes.
- **Audit:** mutations continue to record the acting administrator's public user id; destructive
  and identity decisions additionally carry an operator-supplied reason (SPEC §20).

## Alternatives considered

### Bearer tokens in SPA storage

Works cross-origin but exposes tokens to XSS exfiltration and duplicates refresh logic the cookie
flow already has; same-origin embedding makes it unnecessary.

### Leaving diagnostics anonymous

Rejected: operator queue contents, error details, and lease evidence are internal operational data;
the pre-release caveat was always intended to be resolved before the UI ships.

### New admin-specific auth endpoints

Unnecessary duplication; the native-v1 cookie flow already implements issue/refresh/revoke with
credential-revision semantics, and reusing it avoids a second session store.

## Rationale

Reusing the shipped, differentially tested Phase 7 cookie flow is the lowest-risk session strategy,
and uniform `AdminPrincipal` gating makes the admin API's authorization posture consistent and
auditable before Phase 11 production cutover.

## Consequences

### Positive

- Uniform 401/403 posture across the whole admin API.
- No frozen contract touched; native clients unaffected.
- Closes the documented pre-release caveat in `api/admin-api.md`.

### Negative / trade-offs

- Scripts/tests that called the diagnostics endpoints anonymously must authenticate (the dev-loop
  CLI already has admin credentials; integration tests mint admin JWTs per the existing harness
  pattern).

### Follow-up work

- Update `docs/api/admin-api.md` to remove the pre-release caveat and document the 401/403
  behavior (same change as the gating implementation).

## Validation

- Server API tests: anonymous → 401 with envelope; non-admin → 403; admin → 200 for each newly
  gated route; `/healthz`/`/readyz` remain 200 anonymous.
- SPA e2e: login → authenticated navigation; expired session → redirect to login; non-admin →
  access-denied state.

## References

- [PRD.md](../../PRD.md) §25
- [SPEC.md](../../SPEC.md) §19.4, §20
- [ADR-0124](ADR-0124-native-authentication-session-and-refresh-token-compatibility.md)
- Phase 9 report (historical artifact removed)
