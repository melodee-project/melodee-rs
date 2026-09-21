# MEL-3 browser password-reset completion evidence

Status: complete locally; production notification delivery and release qualification remain
operator-owned gates.

## Scope and oracle

- C# behavior source: `4028b58dbd3302ed431e2c5b32d9dde5634db218`.
- Rust audit candidate: `7ab01c515295d9f883585157287436f05d37f422`.
- Rust replacement behavior is implemented on the current branch under ADR-0124, ADR-0134,
  ADR-0137, ADR-0126, and ADR-0161.

The C# anonymous flow accepts a reset request, returns a generic result for known and unknown
email addresses, sends a one-hour token link, validates without consuming, and consumes the token
only after the password and confirmation pass validation. Missing, malformed, expired, and replayed
tokens receive stable localized error presentation. Successful reset clears the token and routes
the user toward browser sign-in.

Rust preserves the Native API paths, JSON shapes, statuses, messages, one-hour expiry, digest-only
storage, one-use consumption, password minimum of eight characters, and non-enumerating request
response. Rust additionally performs the ADR-0124 atomic refresh-family revocation on successful
consumption. Under ADR-0161 the bounded page tells the user to sign in from MeloAmp or another
native client instead of redirecting to an unserved listener login route.

## Implemented replacement

- `apps/melodee-server/src/password_reset.rs` serves `GET /reset-password`, the exact path Rust
  already emits in notification and development URLs.
- `apps/melodee-server/src/main.rs` merges only this state-free security page; no listener SPA,
  account shell, or duplicated MeloAmp workflow is added.
- `apps/melodee-server/src/native/auth.rs` adds no-store response headers to the existing request,
  validation, and confirmation endpoints without changing their public response bodies or status
  codes.
- `apps/melodee-server/tests/password_reset_page.rs` covers URL round-trip, missing/malformed
  page delivery, localized RTL rendering, accessibility markers, cache policy, and token
  non-reflection.
- `apps/melodee-server/tests/native_auth_api.rs` covers API no-store headers, the emitted URL,
  generic responses, one-use behavior, refresh revocation, and request rate limiting.
- `apps/melodee-server/tests/notification_wiring.rs` continues to prove the notification dispatch
  and fail-degraded delivery audit; rendered private message content is not persisted.

## Browser and security evidence

The local Playwright run used a synthetic account and the notification URL emitted by the running
Rust server. It completed a valid reset, verified that the old refresh token returned `401`, and
verified that the new password logged in successfully. The browser evidence was:

```text
browser_completion: true
history_scrubbed: true
referrer: ""
third_party_requests: 0
password_cleared: true
no_cookie_created: true
old_refresh_revoked: true
subsequent_login: true
```

The same browser run verified stable states for replayed, expired, missing, and malformed links:

```text
replay: invalid
expired: invalid
missing: missing
malformed: invalid
```

The page has no external assets, uses a nonce-bound CSP, `X-Content-Type-Options: nosniff`,
`X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Permissions-Policy`, same-origin
resource policy, `Cache-Control: no-store, private`, `Pragma: no-cache`, and an expired `Expires`
value. Its JavaScript calls only the same-origin Native API with `credentials: omit` and
`cache: no-store`; it does not use local or session storage, third-party requests, redirects, or
client-side telemetry. The token is removed with `history.replaceState` before validation, is not
reflected into HTML, and is cleared from the page state after successful confirmation.

The server trace redaction test and a live log scan both found no raw reset token in request
telemetry or log output. The live scan did not print or persist the token. API and page error
states never render upstream response bodies or token values.

## Verification commands

- `cargo test -p melodee-server --test password_reset_page`
- `cargo test -p melodee-server --test native_auth_api password_reset_is_non_enumerating_one_use_and_revokes_refresh`
- `cargo test -p melodee-server --test native_auth_api password_reset_request_is_rate_limited_without_enumeration`
- `cargo fmt --all`
- Playwright headless browser run against the local Rust server, as recorded above.

## Compatibility and data impact

The Native API reset contract is unchanged except for additive no-store/referrer response headers.
The notification event and template key remain `user.password_reset`; the emitted URL is now
served. OpenSubsonic and Jellyfin routers, DTOs, authentication contracts, and route ownership
were not modified. No database migration or public-ID change is required. The existing digest-only
token table, atomic consume/password update, and refresh-family revocation remain authoritative.

The full `make check-fast`/`make check` results and any deployment or SMTP acceptance belong in
the work-program closeout because they depend on the final repository state and operator
environment.
