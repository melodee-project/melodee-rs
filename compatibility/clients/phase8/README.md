# Phase 8 real-client smoke harness

Drives the shipping **Feishin** web build (pinned release **v1.15.1**, one of
the documented Phase 8 target clients; it supports both server types this
phase implements) through the smoke workflows against the live fixture
server, once as a Jellyfin server and once as an OpenSubsonic server.

## Components

- `feishin_smoke.cjs` — Playwright/Chromium probe. Workflows per surface:
  add server + authenticate → home renders → album browse → album detail →
  play a track (asserts the `<audio>` element streams and progresses) →
  search → favorite the now-playing track (asserts it lands on the client's
  Favorites page, i.e., round-tripped to the server). Screenshots, the API
  request log, and the assertion summary are written to
  `compatibility/captures/phase8-client-smoke/` (gitignored).
- `opensubsonic_remote_relay.py` — loopback relay injecting
  `X-Forwarded-For: 10.99.99.99`. The captured OpenSubsonic auth bypass
  trusts loopback clients (legacy behavior: no credentials checked, no
  principal created), so a same-host browser could never exercise
  authenticated flows. The relay makes the browser look remote, exactly how
  the Phase 0 capture harness defeated the bypass.
- `scripts/run_phase8_client_smoke.py` — boots the seeded fixture server
  (same seeder as the differential replay), serves the Feishin build, starts
  the relay, runs the probe, tears down.

## Harness relaxations (disclosed)

1. Chromium runs with `--disable-web-security`: the client dev-server origin
   and fixture-server origin differ on localhost, and the frozen baseline's
   strict (default-empty) CORS allowlist blocks that combination identically.
   Browser origin enforcement is relaxed; the client code and server are
   stock.
2. The OpenSubsonic surface is reached through the relay above.

## Prerequisites

- Fixture Postgres (`make postgres-up`) and a built `target/debug/melodee-server`.
- Playwright's Node package resolvable (e.g. `PLAYWRIGHT_NODE_PATH` pointing
  at an npx cache `node_modules`) and a Chromium/headless-shell binary
  (`CHROMIUM_EXECUTABLE` overrides the default `~/.cache/ms-playwright` path).
- A Feishin web build:

  ```bash
  git clone --depth 1 --branch v1.15.1 https://github.com/jeffvli/feishin
  cd feishin && pnpm install --frozen-lockfile && pnpm run build:web
  # output: out/web/
  ```

## Run

```bash
python3 scripts/run_phase8_client_smoke.py --feishin-dist /path/to/feishin/out/web
```

Exit code 0 = all assertions passed.

## Findings this harness produced

See historical artifact removed. In short: two
real-client-only compatibility gaps were found and fixed (case-insensitive
Jellyfin route matching; `ApiKey` query token alias), and the captured
OpenSubsonic secret-based token auth was confirmed working as designed.
