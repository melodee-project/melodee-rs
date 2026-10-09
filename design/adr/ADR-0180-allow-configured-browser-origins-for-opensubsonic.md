# ADR-0180: Allow configured browser origins for OpenSubsonic

- **Date:** 2026-10-08
- **Status:** Accepted
- **Decision owners:** User-requested browser-client compatibility correction
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-API-OS-001, PRD-API-OS-002, PRD-API-OS-003, PRD-API-OS-004

## Context

The hosted Feishin browser client received HTTP 200 from `/rest/getUser.view` but could not
read the response because the OpenSubsonic router emitted no `Access-Control-Allow-Origin`.
The existing native API allowlist does not apply to this separate compatibility adapter.
Legacy stream responses also carry `Access-Control-Allow-Origin: *`, which must not bypass
an explicit browser policy. This is an additive browser transport decision with one narrow
correction to browser stream headers; protocol bodies and authentication remain unchanged.

## Decision

- Add `MELODEE__OPENSUBSONIC__CORS__ALLOWED_ORIGINS`, a comma-separated list of exact HTTP(S)
  origins. The default is empty. Reject wildcards, credentials, paths, trailing slashes,
  queries, fragments, overlong configuration, and more than 100 origins.
- Apply the policy to the OpenSubsonic router before parameter parsing and authentication so
  credential-free OPTIONS preflights can succeed for configured origins. Support GET, POST,
  HEAD, and OPTIONS; admit authentication, content-type, conditional, and range headers.
- Return the matching origin on actual responses, including protocol error envelopes, and
  expose the bounded set of paging and media response headers needed by browser clients.
  Do not enable cookie credentials: OpenSubsonic clients use their existing token/form fields.
- Keep native, Admin UI/API, and Jellyfin policies separate. No global origin reflection or
  wildcard policy is introduced. Deployments explicitly list the browser applications they trust.
- For requests carrying `Origin`, replace legacy stream wildcard headers with this policy.
  Unconfigured origins receive no allow-origin header. Requests without `Origin` retain
  their captured stream headers. Existing routes, authentication/bypasses, payloads, status
  codes, identifiers, and media range semantics remain unchanged.

## Alternatives considered

### Enable wildcard CORS on the entire server

Rejected: this exposes unrelated API surfaces and does not express operator trust boundaries.

### Add Feishin to the native origin list

Rejected: native CORS does not wrap `/rest` routes and would not fix this failure.

## Rationale

A protocol-owned allowlist enables the requested browser client while preserving existing
OpenSubsonic authentication and keeping its configuration independent of other adapters.

## Consequences

### Positive

- Hosted browser clients can read authenticated API responses and complete preflights.
- Operators control which browser applications can read OpenSubsonic data.

### Negative / trade-offs

- Browser streaming that previously depended on wildcard headers now requires an explicitly
  configured origin. Non-browser clients remain unaffected.
- This controls browser response access; it is not a replacement for authentication or a
  general CSRF policy, and the captured localhost authentication bypass remains unchanged.

### Client rollout

Source deployments add the new setting to `.env`; the Compose example forwards
`MELODEE_OPENSUBSONIC_CORS_ALLOWED_ORIGINS`. For the reported hosted client, configure
`https://feishin.vercel.app` and restart the server. No database migration is required.
The user-requested development Compose default lists Feishin under
`x-melodee-server-environment`; `make up` supplies this to the host-run server. Root `.env`
and exported settings override that development default, including an empty value to disable
browser access. Packaged deployments retain an empty default.

## Validation

The integration regression records the former HTTP-200/missing-header behavior, then checks
identical authenticated `getUser` bytes with the allowed origin, unauthenticated error bodies,
plain and `.view` preflights, untrusted origins, and exclusion of unrelated Admin API routes.
Unit tests cover bounded origin parsing and browser/non-browser stream wildcard handling.
Existing OpenSubsonic golden tests continue to protect protocol and streaming behavior.
Validate the running local server with allowed/disallowed Origin and OPTIONS requests.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [ADR-0126: Native browser policy](ADR-0126-rate-limiting-and-client-address-semantics-for-native-api-v1.md)
- [OpenSubsonic documentation](../../docs/src/content/docs/api/opensubsonic.md)
