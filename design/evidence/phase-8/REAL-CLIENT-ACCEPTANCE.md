# Phase 8 Real-client Acceptance

**Captured:** 2026-08-13

**Rust base commit:** `8426897` plus the Phase 8 real-client change set (case-insensitive
Jellyfin routing, `ApiKey` query token alias under ADR-0132, redirect query-key casing fix).

**Client:** Feishin **v1.15.1** (git tag `v1.15.1`, `pnpm install --frozen-lockfile &&
pnpm run build:web`), one of the documented Phase 8 target clients. Feishin implements both
server types this phase ships — Jellyfin and OpenSubsonic — so one shipping client exercises
both surfaces. Finamp/Streamyfin remain future work: they are Flutter/React-Native mobile apps
and need the emulator harness; the wire contract they consume is the one this acceptance
exercises.

**Harness:** `scripts/run_phase8_client_smoke.py` boots the real `melodee-server` against a
disposable PostgreSQL database seeded with the frozen fixture catalog (the same seeder as the
differential replay), serves the pinned Feishin web build, and drives it with
Playwright/Chromium (`compatibility/clients/phase8/feishin_smoke.cjs`). See
`compatibility/clients/phase8/README.md` for reproduction.

Disclosed harness relaxations (neither touches client or server code):

1. Chromium runs with `--disable-web-security` — the client origin and fixture-server origin
   differ on localhost, and the frozen baseline's strict (default-empty) CORS allowlist blocks
   that combination identically.
2. The OpenSubsonic surface is reached through a loopback relay injecting
   `X-Forwarded-For: 10.99.99.99` (`opensubsonic_remote_relay.py`): the captured legacy auth
   bypass trusts loopback clients (no principal is ever created for them), so a same-host
   browser cannot exercise authenticated flows. The relay emulates a remote client exactly as
   the Phase 0 capture harness did.

## Workflows and results

Both surfaces completed the full smoke workflow; evidence (screenshots per step, API request
log, assertion summary) is in `compatibility/captures/phase8-client-smoke/` (gitignored
captures tree), produced by the clean runner run:

```text
12/12 assertions passed
```

Per surface (`assertions.json`):

- **Jellyfin** (server type "Jellyfin"):
  1. server added + authenticated (`POST /users/authenticatebyname` — lowercase path, see
     finding 1), home rendered;
  2. album library browse and album detail (tracks, artist, "More from this Artist");
  3. playback: `<audio>` attached to `/Items/{id}/Download?apiKey=...` (see finding 2) and
     progressed (readyState 4);
  4. search palette finds fixture track "Solar";
  5. favorite toggled on the now-playing track and the track listed on the client's Favorites
     page (round-trip through `FavoriteItems`).
- **OpenSubsonic** (server type "OpenSubsonic"):
  1. server added + authenticated with token/salt (see finding 3), home rendered;
  2. album library browse and album detail (incl. playlist sidebar from `getPlaylists`);
  3. playback: `<audio>` attached to `/rest/stream.view?id=song_...` and progressed;
  4. search finds "Solar";
  5. favorite round-trip through `star`/`getStarred2` — the track listed on Favorites.

Non-blocking observations: the client's unauthenticated `<img>` artwork requests 401/404
(captured 401-without-credentials behavior plus the documented no-durable-artwork gap;
placeholder art renders), and Feishin logs a client-side "Missing ':genreId' param" console
error when rendering its Favorites view (client-internal routing quirk; workflow completes).

## Findings produced by this gate

1. **Jellyfin route matching must be case-insensitive (fixed).** Feishin calls
   `/users/authenticatebyname`, `/system/info`, etc. Legacy ASP.NET matched route templates
   case-insensitively with preserved parameter values; the axum router 404'd. The adapter now
   canonicalizes the static segments of Jellyfin-owned roots in the outer fallback and replays
   (`jellyfin::case_rewriting_fallback`), leaving native/OpenSubsonic surfaces case-sensitive
   as captured. Test: `lowercase_jellyfin_routes_match_case_insensitively`.
2. **Jellyfin query token alias `ApiKey` (fixed; ADR-0132).** Feishin builds stream URLs as
   `/Items/{id}/Download?apiKey={token}`; legacy read only `api_key`, so playback was broken
   against the legacy server too — a legacy real-client defect the frozen captures never
   exercised (no golden sends `ApiKey`). Upstream Jellyfin reads `ApiKey` as the primary query
   token. Strictly additive widening; test:
   `apikey_query_param_authenticates_alongside_api_key`.
3. **OpenSubsonic token/salt validates against the per-user OpenSubsonic secret (confirmed
   working as captured).** Legacy `UserAuthenticationService.ValidateTokenAsync` prefers
   `OpenSubsonicSecretProtected` over the login password, so real clients authenticate with the
   OpenSubsonic secret as their client password. The fixture user's secret is the documented
   fixture value; Feishin authenticated and used every authenticated flow with it.
4. **Redirect query reconstruction is case-insensitive on keys (fixed).** The captured
   `/Users/{id}/Items` 307 reconstruction matched whitelist keys case-sensitively; legacy
   `[FromQuery]` binding matched case-insensitively, so Feishin's `IncludeItemTypes` was
   dropped and the Albums library page rendered empty. Test:
   `users_items_redirect_matches_query_keys_case_insensitively`.

## Regression posture

Full frozen-golden differential after these fixes
(`scripts/run_phase8_differential.py`, both surfaces): **0 unexpected differences, 468
enumerated known differences** — identical to the pre-change baseline (the fallback initially
regressed the bare-root marker probe by bypassing the routing middleware; fixed by replaying
`/` through the app, verified JF-side before the combined rerun).

## Dependency decision

`tower` (`=0.5.3`, `util` feature) moved from dev-dependency to workspace production
dependency for the fallback's request replay (`ServiceExt::oneshot`). It is already compiled
into every binary transitively via axum/tower-http; no new crate enters the dependency graph.
Documented here per the dependency-decision rule.
