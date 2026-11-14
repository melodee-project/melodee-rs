// Phase 8 real-client smoke probe: drives the shipping Feishin web build
// (pinned release) through the documented smoke workflows against the live
// fixture server, once as a Jellyfin server and once as an OpenSubsonic
// server. Environment:
//   FEISHIN_URL          static origin serving the Feishin build
//   MELODEE_URL          fixture server base URL
//   MELODEE_USER / MELODEE_PASSWORD   fixture credentials
//   MELODEE_SERVER_NAME  label used when adding the server
//   SMOKE_EVIDENCE_DIR   output directory for screenshots + evidence JSON
//   PLAYWRIGHT_NODE_PATH optional extra module path (playwright resolution)
//   CHROMIUM_EXECUTABLE  optional chromium/headless-shell binary override
//
// The browser runs with --disable-web-security: client dev-server origin and
// fixture server origin differ on localhost, and the frozen baseline's strict
// (default-empty) CORS allowlist blocks that combination identically. This
// relaxes browser origin enforcement only; client code and server are stock.

if (process.env.PLAYWRIGHT_NODE_PATH) {
  module.paths.unshift(process.env.PLAYWRIGHT_NODE_PATH);
}
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const FEISHIN_URL = process.env.FEISHIN_URL;
const MELODEE_URL = process.env.MELODEE_URL;
// OpenSubsonic surface goes through the XFF-injecting relay: the captured
// loopback auth bypass otherwise skips credential checks entirely, which is
// not how a real remote client session behaves.
const MELODEE_OS_URL = process.env.MELODEE_OS_URL || MELODEE_URL;
const USER = process.env.MELODEE_USER;
const PASSWORD = process.env.MELODEE_PASSWORD;
// The captured OpenSubsonic token/salt branch validates MD5(secret + salt)
// where the secret is the per-user OpenSubsonic secret (generated at user
// creation), not the login password — so clients authenticate with that
// secret as their "password" (UserAuthenticationService.ValidateTokenAsync).
const OS_PASSWORD = process.env.MELODEE_OS_PASSWORD || PASSWORD;
const SERVER_NAME = process.env.MELODEE_SERVER_NAME || 'melodee-rs-phase8';
const OUT = process.env.SMOKE_EVIDENCE_DIR || '/tmp/phase8-smoke-evidence';

const CHROMIUM =
  process.env.CHROMIUM_EXECUTABLE ||
  chromium.executablePath();

const apiLog = [];
const assertions = [];

function check(name, condition, detail) {
  assertions.push({ name, pass: Boolean(condition), detail: detail || '' });
  console.log(`${condition ? 'PASS' : 'FAIL'}  ${name}${detail ? ` — ${detail}` : ''}`);
}

async function shot(page, name) {
  await page.screenshot({ path: path.join(OUT, `${name}.png`) });
}

async function addServer(page, typeLabel, serverUrl, password) {
  await page.goto(`${FEISHIN_URL}/`, { waitUntil: 'networkidle', timeout: 90000 });
  // The "new version installed" changelog modal races first paint; wait for
  // it explicitly and dismiss before touching the server form.
  const dismiss = page.getByRole('button', { name: 'Dismiss' });
  try {
    await dismiss.waitFor({ timeout: 10000 });
    await dismiss.click();
    await dismiss.waitFor({ state: 'detached', timeout: 10000 });
  } catch { /* modal absent */ }
  await page.waitForTimeout(500);
  await page.getByText(typeLabel, { exact: true }).click();
  await page.locator('input[data-path="name"]').fill(SERVER_NAME);
  const urlInput = page.locator('input[data-path="url"]');
  await urlInput.click({ clickCount: 3 });
  await urlInput.fill(serverUrl);
  await page.locator('input[data-path="username"]').fill(USER);
  await page.locator('input[data-path="password"]').fill(password);
  await page.getByRole('button', { name: 'ADD' }).click();
}

async function waitHomeLoaded(page) {
  // Home route renders the album grid ("Recently played"/"Most played" or
  // album cards); the sidebar appears only after a server is active.
  await page.waitForSelector('text=Home', { timeout: 60000 });
  await page.waitForTimeout(4000);
}

async function openFirstAlbumAndPlay(page, prefix) {
  // Sidebar -> Albums library page, open the first album card.
  await page.getByRole('link', { name: 'Albums' }).first().click();
  await page.waitForTimeout(4000);
  await shot(page, `${prefix}-albums`);
  const firstCard = page.locator('a[href*="/albums/"]').first();
  await firstCard.waitFor({ timeout: 30000 });
  await firstCard.click();
  await page.waitForTimeout(4000);
  await shot(page, `${prefix}-album-detail`);
  // Start album playback via the header Play button.
  await page.getByRole('button', { name: 'Play', exact: true }).click();
  await page.waitForTimeout(5000);
}

async function assertPlaying(page, prefix) {
  const first = await page.evaluate(() => {
    const audio = document.querySelector('audio');
    return audio
      ? { currentTime: audio.currentTime, paused: audio.paused, src: audio.src.slice(0, 160) }
      : null;
  });
  check(`${prefix}: audio element attached`, first !== null && first.src.length > 0,
    first ? first.src : 'no audio element');
  // Fixture tracks are seconds long; poll until playback progresses or ends.
  let progressed = false;
  let last = first;
  for (let i = 0; i < 30 && !progressed; i += 1) {
    await page.waitForTimeout(1000);
    last = await page.evaluate(() => {
      const audio = document.querySelector('audio');
      return audio
        ? { currentTime: audio.currentTime, ended: audio.ended, readyState: audio.readyState }
        : null;
    });
    progressed = last !== null && (last.currentTime > 0.2 || last.ended);
  }
  check(`${prefix}: playback progressed`, progressed,
    last ? `currentTime=${last.currentTime.toFixed(2)} ended=${last.ended} readyState=${last.readyState}` : 'n/a');
  await shot(page, `${prefix}-playing`);
}

async function searchFlow(page, prefix, term) {
  // The sidebar search field is readonly; clicking it opens the search
  // palette with its own input.
  await page.locator('input[placeholder*="Search" i]').first().click();
  await page.waitForTimeout(1000);
  const palette = page.locator('.mantine-Spotlight-root input, [role="dialog"] input, [class*="spotlight" i] input').first();
  if (await palette.count()) {
    await palette.fill(term);
  } else {
    await page.keyboard.type(term);
  }
  await page.waitForTimeout(5000);
  await shot(page, `${prefix}-search`);
  const body = await page.textContent('body');
  check(`${prefix}: search finds "${term}"`, body.includes(term));
  await page.keyboard.press('Escape');
}

async function favoriteNowPlaying(page, prefix) {
  // The now-playing bar exposes a favorite (heart) toggle. After toggling,
  // the Favorites library page must list the track — end-to-end proof the
  // favorite landed server-side.
  // react-icons hearts carry no stable class and tooltips set no aria-label;
  // find the player-bar favorite button by hovering bottom-bar buttons and
  // reading the tooltip ("Favorite"/"Unfavorite").
  let heart = null;
  for (const candidate of await page.locator('button').all()) {
    const box = await candidate.boundingBox();
    if (!box || box.y < 780) continue;
    await candidate.hover();
    await page.waitForTimeout(400);
    const tooltip = page.locator('[role="tooltip"]');
    if ((await tooltip.count()) && /favorite/i.test((await tooltip.first().textContent()) || '')) {
      heart = candidate;
      break;
    }
  }
  check(`${prefix}: favorite toggle present`, heart !== null);
  if (!heart) return;
  const title = await page.evaluate(() => navigator.mediaSession?.metadata?.title || '');
  await heart.click();
  await page.waitForTimeout(2000);
  await page.getByRole('link', { name: 'Favorites' }).first().click();
  await page.waitForTimeout(4000);
  await shot(page, `${prefix}-favorites`);
  const body = await page.textContent('body');
  check(`${prefix}: favorited track listed on Favorites page`,
    title.length > 0 && body.includes(title), `title="${title}"`);
}

async function runSurface(browser, typeLabel, prefix, searchTerm, serverUrl) {
  const context = await browser.newContext({ viewport: { width: 1400, height: 900 } });
  const page = await context.newPage();
  page.on('pageerror', (error) => console.log(`PAGE-ERROR [${prefix}]:`, String(error).slice(0, 300)));
  page.on('console', (message) => {
    if (message.type() === 'error') console.log(`CONSOLE [${prefix}]:`, message.text().slice(0, 200));
  });
  const stripBase = (url) =>
    url.startsWith(MELODEE_OS_URL) ? url.slice(MELODEE_OS_URL.length)
      : url.startsWith(MELODEE_URL) ? url.slice(MELODEE_URL.length)
        : null;
  page.on('request', (request) => {
    const path = stripBase(request.url());
    if (path !== null) {
      apiLog.push({ surface: prefix, method: request.method(), url: path });
    }
  });
  page.on('response', (response) => {
    const path = stripBase(response.url());
    if (path !== null && response.status() >= 400) {
      apiLog.push({ surface: prefix, error: response.status(), url: path });
    }
  });
  try {
    await addServer(page, typeLabel, serverUrl, prefix === 'opensubsonic' ? OS_PASSWORD : PASSWORD);
    await waitHomeLoaded(page);
    await shot(page, `${prefix}-home`);
    const body = await page.textContent('body');
    check(`${prefix}: server added and home rendered`, !body.includes('SERVER REQUIRED'));
    await openFirstAlbumAndPlay(page, prefix);
    await assertPlaying(page, prefix);
    await searchFlow(page, prefix, searchTerm);
    await favoriteNowPlaying(page, prefix);
  } catch (error) {
    check(`${prefix}: workflow completed without driver error`, false, String(error).slice(0, 300));
    await shot(page, `${prefix}-error`);
  }
  await context.close();
}

(async () => {
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch({
    executablePath: CHROMIUM,
    args: ['--disable-web-security', '--autoplay-policy=no-user-gesture-required'],
  });
  await runSurface(browser, 'Jellyfin', 'jellyfin', 'Solar', MELODEE_URL);
  await runSurface(browser, 'OpenSubsonic', 'opensubsonic', 'Solar', MELODEE_OS_URL);
  await browser.close();

  fs.writeFileSync(path.join(OUT, 'api-log.json'), JSON.stringify(apiLog, null, 1));
  fs.writeFileSync(path.join(OUT, 'assertions.json'), JSON.stringify(assertions, null, 1));
  const failed = assertions.filter((a) => !a.pass);
  console.log(`\n${assertions.length - failed.length}/${assertions.length} assertions passed`);
  process.exit(failed.length ? 1 : 0);
})().catch((error) => {
  console.error(error);
  process.exit(2);
});
