const { _electron } = require('@playwright/test');
const path = require('path');
const fs = require('fs');

const ELECTRON_DIR = '/home/steven/src/github/meloamp/src/electron';
const API = 'http://127.0.0.1:8080/api/v1';
const EMAIL = process.env.MELOAMP_EMAIL || 'meloamp@example.com';
const PASSWORD = process.env.MELOAMP_PASSWORD || 'meloamp123';
const NAV = ['/', '/albums', '/artists', '/songs', '/genres', '/playlists', '/smart-playlists',
  '/favorites', '/history', '/recommendations', '/charts', '/search-advanced', '/artist-lookup',
  '/analytics', '/playback-tools', '/requests', '/equalizer', '/themes', '/shares', '/dashboard',
  '/settings'];

(async () => {
  const electron = await _electron.launch({
    args: ['.', '--no-sandbox'],
    cwd: ELECTRON_DIR,
    env: { ...process.env },
    executablePath: require(path.join(ELECTRON_DIR, 'node_modules/electron')).toString(),
    timeout: 60_000,
  });
  const win = await electron.firstWindow({ timeout: 60_000 });
  const events = [];
  let current = 'boot';
  win.on('console', (m) => { if (m.type() === 'error') events.push({ view: current, kind: 'console', text: m.text().slice(0, 300) }); });
  win.on('response', async (r) => {
    if (r.status() >= 400) {
      let body = '';
      try { body = (await r.text()).slice(0, 200); } catch { body = '<unreadable>'; }
      events.push({ view: current, kind: 'http', status: r.status(), method: r.request().method(), url: r.url(), body });
    }
  });

  await win.evaluate(({ api }) => {
    localStorage.clear();
    localStorage.setItem('serverUrl', api);
  }, { api: API });
  await win.reload();
  await win.waitForTimeout(3000);

  // Log in.
  current = 'login';
  await win.getByLabel('Email').fill(EMAIL);
  await win.getByLabel('Password').fill(PASSWORD);
  await win.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await win.waitForTimeout(6000);
  const loggedIn = (await win.locator('body').innerText()).includes('BROWSE');
  console.log('login ok:', loggedIn);

  const report = [];
  for (const route of NAV) {
    current = route;
    events.push({ view: route, kind: 'mark', text: 'start' });
    const before = events.length;
    await win.goto('http://localhost:3001' + route, { waitUntil: 'load' });
    await win.waitForTimeout(5000);
    const text = (await win.locator('main, #root').first().innerText().catch(() => win.locator('body').innerText())).replace(/\s+/g, ' ').slice(0, 700);
    const failures = events.slice(before).filter((e) => e.kind !== 'mark');
    report.push({ route, text, failures });
    await win.screenshot({ path: `/tmp/meloamp${route.replace(/\//g, '_') || '_home'}.png`, fullPage: false });
  }

  fs.writeFileSync('/tmp/meloamp-audit.json', JSON.stringify({ loggedIn, report }, null, 2));
  console.log('\n=== per-view summary ===');
  for (const r of report) {
    const codes = r.failures.filter((f) => f.kind === 'http').map((f) => `${f.status} ${f.method} ${f.url.replace(/^http:\/\/[^/]+/, '')}`);
    const consoleErrs = r.failures.filter((f) => f.kind === 'console').map((f) => f.text.slice(0, 120));
    console.log(`\n${r.route}\n  text: ${r.text.slice(0, 150)}`);
    if (codes.length) console.log('  http:', [...new Set(codes)].join('\n        '));
    if (consoleErrs.length) console.log('  console:', [...new Set(consoleErrs)].join('\n           '));
  }
  await electron.close();
})().catch((e) => { console.error('FAILED:', e.message); process.exit(1); });
