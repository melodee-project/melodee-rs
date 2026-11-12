const { _electron } = require('@playwright/test');
const path = require('path');
const ELECTRON_DIR = '/home/steven/src/github/meloamp/src/electron';
const API = 'http://127.0.0.1:8080/api/v1';
const EMAIL = process.env.MELOAMP_EMAIL;
const PASSWORD = process.env.MELOAMP_PASSWORD;
const NAV = ['/', '/albums', '/artists', '/songs', '/genres', '/playlists', '/smart-playlists', '/favorites',
  '/history', '/recommendations', '/charts', '/search-advanced', '/artist-lookup', '/analytics',
  '/playback-tools', '/requests', '/equalizer', '/themes', '/shares', '/dashboard', '/settings'];

(async () => {
  const electron = await _electron.launch({ args: ['.', '--no-sandbox'], cwd: ELECTRON_DIR,
    executablePath: require(path.join(ELECTRON_DIR, 'node_modules/electron')).toString(), timeout: 60_000 });
  const win = await electron.firstWindow({ timeout: 60_000 });
  const events = [];
  let phase = 'boot';
  win.on('console', (m) => { if (m.type() === 'error') events.push({ phase, kind: 'console', text: m.text().slice(0, 160) }); });
  win.on('response', async (r) => {
    if (r.status() >= 400) events.push({ phase, kind: 'http', status: r.status(), method: r.request().method(), url: r.url() });
  });

  await win.evaluate(({ api }) => { localStorage.clear(); localStorage.setItem('serverUrl', api); }, { api: API });
  await win.reload();
  await win.waitForTimeout(2500);
  await win.getByLabel('Email').fill(EMAIL);
  await win.getByLabel('Password').fill(PASSWORD);
  await win.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await win.waitForTimeout(5000);

  const rows = [];
  for (const route of NAV) {
    phase = route;
    const from = events.length;
    await win.goto('http://localhost:3001' + route, { waitUntil: 'load' });
    await win.waitForTimeout(3000);
    // Scroll to the bottom so lazy artwork actually requests.
    await win.evaluate(async () => {
      for (let y = 0; y < document.body.scrollHeight; y += 400) { window.scrollTo(0, y); await new Promise((r) => setTimeout(r, 60)); }
      window.scrollTo(0, 0);
    });
    await win.waitForFunction(() => [...document.images].every((i) => i.complete), null, { timeout: 15_000 }).catch(() => {});
    const imgs = await win.locator('img').evaluateAll((els) => els.map((e) => ({ src: e.currentSrc || e.getAttribute('src') || '', w: e.naturalWidth })));
    const brokenImgs = imgs.filter((i) => i.src.includes(':8080') && i.w === 0).map((i) => i.src.replace(/^http:\/\/[^/]+/, ''));
    const text = (await win.locator('body').innerText()).replace(/\s+/g, ' ');
    const fails = events.slice(from).filter((e) => e.kind !== 'mark');
    rows.push({ route, contentChars: text.length, brokenImgs, fails });
  }

  console.log(`\n=== ${EMAIL} ===`);
  let problems = 0;
  for (const r of rows) {
    const http = [...new Set(r.fails.filter((f) => f.kind === 'http').map((f) => `${f.status} ${f.url.replace(/^http:\/\/[^/]+/, '')}`))];
    const cons = [...new Set(r.fails.filter((f) => f.kind === 'console').map((f) => f.text))];
    const ok = !http.length && !cons.length && !r.brokenImgs.length;
    if (!ok) problems++;
    console.log(`${ok ? 'OK  ' : 'FAIL'} ${r.route.padEnd(18)} chars=${String(r.contentChars).padStart(5)} imgs_broken=${r.brokenImgs.length}`);
    for (const h of http) console.log(`       http: ${h}`);
    for (const c of cons) console.log(`       console: ${c}`);
    if (r.brokenImgs.length) console.log(`       broken imgs: ${r.brokenImgs.slice(0, 3).join(', ')}`);
  }
  console.log(`\nviews with problems: ${problems}/${rows.length}`);
  await electron.close();
})().catch((e) => { console.error('FAILED:', e.message); process.exit(1); });
