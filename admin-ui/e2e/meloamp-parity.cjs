#!/usr/bin/env node
/**
 * MeloAmp × melodee-rs parity driver.
 *
 * Launches the MeloAmp Electron client with Playwright, signs in once, walks every
 * navigation target, and reports console errors, failed requests and broken
 * artwork per view. It asserts nothing about the *client's* correctness: its job is
 * to surface server-side parity defects from a real client, which is the only way
 * envelope/field mismatches become visible (they answer `200` with a well-formed
 * body either way).
 *
 * This is a Node script because the Playwright Electron API is only available for
 * Node; it is otherwise parameterised like any other tool in this repository — no
 * credentials, no absolute paths, no assumptions about the client's location.
 *
 * Usage
 * -----
 *   MELOAMP_DIR=/path/to/meloamp/src/electron \
 *   MELODEE_LIVE_USERNAME=admin MELODEE_LIVE_PASSWORD=secret \
 *     node admin-ui/e2e/meloamp-parity.cjs
 *
 * Environment
 * -----------
 *   MELOAMP_DIR            Required. The MeloAmp `src/electron` directory (the
 *                          package that runs `electron .`).
 *   MELODEE_LIVE_USERNAME  Required. Native account used to sign in.
 *   MELODEE_LIVE_PASSWORD  Required.
 *   MELODEE_LIVE_API_URL   API origin (default http://127.0.0.1:8080).
 *   MELODEE_LIVE_STATE     Playwright storage-state path (default
 *                          /tmp/melodee-live-auth-state.json); reused when its
 *                          cookie domain matches the API host.
 *   MELOAMP_BOUND          Optional integer; treat a view as failing when fewer
 *                          than this many characters render (default 200).
 *
 * Exit code is non-zero when any view reports a failed request, a console error or
 * broken API artwork, so it can gate a parity pass.
 */

'use strict';

const { spawnSync } = require('node:child_process');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');

const NAV = [
  '/', '/albums', '/artists', '/songs', '/genres', '/playlists', '/smart-playlists',
  '/favorites', '/history', '/recommendations', '/charts', '/search-advanced',
  '/artist-lookup', '/analytics', '/playback-tools', '/requests', '/equalizer',
  '/themes', '/shares', '/dashboard', '/settings',
];

/**
 * Views whose failures are *frozen behaviour*, not parity defects:
 *
 * - `/playback-tools`: the C# service seeds `jukebox.enabled = false`, so
 *   `/playback-backend/status` and `/capabilities` answer `400` there too and the
 *   page reports it as unavailable.
 * - `/charts`: the frozen chart DTO advertises `imageUrl = …/charts/{id}/image`, a
 *   route the C# service does not implement; the client falls back to its own
 *   placeholder (recorded in design/evidence/meloamp-parity/MELOAMP-VERIFY.md).
 */
const EXPECTED_FAILURES = new Map([
  ['/playback-tools', { statuses: [400], console: [/400 \(Bad Request\)/], artwork: [] }],
  ['/charts', { statuses: [404], console: [/404 \(Not Found\)/], artwork: [/\/charts\/[^/]+\/image$/] }],
]);

function require_env(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`missing required environment variable ${name} (see --help in the header)`);
    process.exit(2);
  }
  return value;
}

function loadPlaywright() {
  // Resolve from the admin-ui package so the script works from any directory.
  const probe = spawnSync(
    process.execPath,
    ['-e', "require.resolve('@playwright/test')"],
    { cwd: join(__dirname, '..'), encoding: 'utf8' },
  );
  if (probe.status !== 0) {
    console.error('@playwright/test is not installed in admin-ui; run pnpm install there first');
    process.exit(2);
  }
  return require(require.resolve('@playwright/test', { paths: [join(__dirname, '..')] }));
}

async function sign_in(page) {
  await page.getByLabel('Email').fill(require_env('MELODEE_LIVE_USERNAME'));
  await page.getByLabel('Password').fill(require_env('MELODEE_LIVE_PASSWORD'));
  await page.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await page.waitForTimeout(4500);
}

(async () => {
  const electronDir = require_env('MELOAMP_DIR');
  const api = process.env.MELODEE_LIVE_API_URL ?? 'http://127.0.0.1:8080';
  const minChars = Number(process.env.MELOAMP_BOUND ?? '200');
  const { _electron } = loadPlaywright();

  const app = await _electron.launch({
    args: ['.', '--no-sandbox'],
    cwd: electronDir,
    executablePath: require(join(electronDir, 'node_modules/electron')).toString(),
    timeout: 60_000,
  });
  const win = await app.firstWindow({ timeout: 60_000 });

  const events = [];
  let view = 'boot';
  win.on('console', (message) => {
    if (message.type() === 'error') {
      events.push({ view, kind: 'console', text: message.text().slice(0, 200) });
    }
  });
  win.on('response', async (response) => {
    if (response.status() >= 400) {
      events.push({
        view,
        kind: 'http',
        status: response.status(),
        url: response.url().replace(/^https?:\/\/[^/]+/, ''),
      });
    }
  });

  // Start from a clean client state so the pass exercises the sign-in path, then
  // keep the session for every subsequent navigation.
  await win.goto('http://localhost:3001/', { waitUntil: 'load' });
  await win.waitForTimeout(2000);
  await win.evaluate((origin) => {
    localStorage.clear();
    localStorage.setItem('serverUrl', `${origin}/api/v1`);
  }, api);
  await win.goto('http://localhost:3001/', { waitUntil: 'load' });
  await win.waitForTimeout(2500);
  const loginVisible = await win
    .getByLabel('Email')
    .waitFor({ state: 'visible', timeout: 8_000 })
    .then(() => true)
    .catch(() => false);
  if (loginVisible) {
    await sign_in(win);
  } else {
    console.log('note: client already authenticated; skipping sign-in');
  }

  let failures = 0;
  for (const route of NAV) {
    view = route;
    const from = events.length;
    const expected = EXPECTED_FAILURES.get(route) ?? { statuses: [], console: [], artwork: [] };
    await win.goto(`http://localhost:3001${route}`, { waitUntil: 'load' });
    await win.waitForTimeout(3000);
    await win.evaluate(async () => {
      for (let y = 0; y < document.body.scrollHeight; y += 400) {
        window.scrollTo(0, y);
        await new Promise((resolve) => setTimeout(resolve, 60));
      }
      window.scrollTo(0, 0);
    });
    await win
      .waitForFunction(() => [...document.images].every((image) => image.complete), null, {
        timeout: 15_000,
      })
      .catch(() => {});

    const images = await win
      .locator('img')
      .evaluateAll((elements) =>
        elements.map((element) => ({
          src: element.currentSrc || element.getAttribute('src') || '',
          width: element.naturalWidth,
        })),
      );
    const brokenArtwork = images
      .filter((image) => image.src.includes(':8080') || image.src.startsWith('/api/'))
      .filter((image) => image.width === 0)
      .map((image) => image.src.replace(/^https?:\/\/[^/]+/, ''))
      .filter((src) => !expected.artwork.some((pattern) => pattern.test(src)));

    const body = await win.locator('body').innerText();
    const viewEvents = events.slice(from);
    const http = [
      ...new Set(
        viewEvents
          .filter((event) => event.kind === 'http' && !expected.statuses.includes(event.status))
          .map((event) => `${event.status} ${event.url}`),
      ),
    ];
    const console_errors = [
      ...new Set(
        viewEvents
          .filter((event) => event.kind === 'console')
          .map((event) => event.text)
          .filter((text) => !expected.console.some((pattern) => pattern.test(text))),
      ),
    ];
    const thin = body.replace(/\s+/g, ' ').length < minChars;
    const failed = http.length > 0 || console_errors.length > 0 || brokenArtwork.length > 0 || thin;
    if (failed) failures += 1;

    console.log(`${failed ? 'FAIL' : 'OK  '} ${route}`);
    for (const line of http) console.log(`       http: ${line}`);
    for (const line of console_errors) console.log(`       console: ${line}`);
    if (brokenArtwork.length) console.log(`       broken artwork: ${brokenArtwork.slice(0, 3).join(', ')}`);
    if (thin) console.log(`       rendered ${body.replace(/\s+/g, ' ').length} chars (< ${minChars})`);
  }

  console.log(`\n${NAV.length - failures}/${NAV.length} views clean`);
  await app.close();
  process.exit(failures === 0 ? 0 : 1);
})().catch((error) => {
  console.error(`parity driver failed: ${error.message}`);
  process.exit(1);
});
