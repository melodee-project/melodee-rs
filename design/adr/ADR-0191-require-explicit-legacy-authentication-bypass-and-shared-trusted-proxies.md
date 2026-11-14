# ADR-0191: Require explicit legacy authentication bypass and shared trusted proxies

- **Date:** 2026-11-13
- **Status:** Accepted
- **Decision owners:** User-directed review remediation program
- **Supersedes:** ADR-0130 decision 7 for default localhost/baseUrl authentication bypass
- **Superseded by:** None
- **Related requirements:** PRD-SEC-001, PRD-AUTH-001, PRD-API-OS-001, PRD-TEST-001, PRD-TEST-004

## Context

ADR-0130 preserved the frozen C# localhost/baseUrl auth bypass. A reverse proxy connecting over
loopback can therefore expose the catalog without credentials when proxy trust is not configured.
A caller-controlled matching Host is also not an authentication credential. The user explicitly
assigned the consolidated security review for correction, including safe defaults and a client plan.
The existing `bypass_needs_no_credentials_and_synthesizes_no_user` test captures the old behavior.

## Decision

- `MELODEE__OPENSUBSONIC__LEGACY_AUTH_BYPASS` defaults to false. Protected OpenSubsonic routes
  require their existing password/token credentials regardless of peer address or Host. Public ping,
  extension discovery and separately routed native artwork retain their existing contracts.
- An explicit true flag restores captured bypass behavior for isolated legacy-client transition
  environments. Startup emits a warning. Invalid trusted forwarding headers never activate bypass.
- `MELODEE__SERVER__TRUSTED_PROXIES` configures the resolver shared by every API. The existing
  `MELODEE__NATIVE__TRUSTED_PROXIES` remains a fallback alias; an explicitly set new key, including
  an empty value, takes precedence. Neither key implicitly trusts loopback or a private subnet.
- Bcrypt work executes outside Tokio request workers with bounded active and admitted work. Existing
  wire credential/error semantics and stored password parameters remain unchanged. Saturation/worker
  errors map to each adapter's existing account-store failure response (native unavailable;
  OpenSubsonic error 40; Jellyfin 401). Invalid hashes remain credential failures.

## Compatibility and client migration

Before upgrading, configure password or token credentials in clients that relied on local or matching-
host access. Set the shared proxy allowlist to controlled proxy addresses and have that proxy replace
forwarding headers. Existing credentialed clients need no protocol change. For an isolated transition,
operators may opt into the legacy bypass, accepting that every matching-host/loopback request can
read the catalog without credentials; it must not be enabled for public proxy deployments. Restart
applies configuration. Binary downgrade restores the old defaults and is not a security rollback.
Frozen bypass captures run with explicit opt-in; secure-default regression tests exercise unauthenticated
loopback, matching Host, spoofed headers, and malformed trusted forwarding chains.

## Alternatives considered

Retaining the bypass by default preserves a deployment-sensitive credential bypass. Automatically
trusting private addresses permits header forgery on shared networks. Both are rejected; trust and
legacy behavior must be explicit.

## Consequences

Previously anonymous local clients must authenticate. No catalog IDs or user credentials change.
Real-client release qualification remains an operator gate; test fixtures do not establish it.

## Validation

OpenSubsonic historical opt-in and secure-default route tests, resolver precedence/forgery tests,
and bounded password-work concurrency/cancellation tests cover the change.

## References

- [ADR-0130](ADR-0130-phase-8-opensubsonic-authentication-extension-advertisement-and-streaming-corrections.md)
- [ADR-0124](ADR-0124-native-authentication-session-and-refresh-token-compatibility.md)
- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
