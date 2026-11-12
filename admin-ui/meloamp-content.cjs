const { _electron } = require('@playwright/test');
const path = require('path');
const ELECTRON_DIR = '/home/steven/src/github/meloamp/src/electron';
const API = 'http://127.0.0.1:8080/api/v1';

const checks = [
  ['/favorites', [/LIKED|Songs/i]],
  ['/history', [/\d{1,2}:\d{2}/]],
  ['/playlists', [/Parity Playlist/]],
  ['/smart-playlists', [/Parity Smart Playlist/]],
  ['/charts', [/Parity Chart/]],
  ['/requests', [/Parity request/i]],
  ['/shares', [/Parity share/i]],
  ['/analytics', [/Top|Listening/i]],
  ['/equalizer', [/Parity Flat|60/]],
  ['/genres', [/Black Metal|Classical/]],
  ['/dashboard', [/Albums|Artists|Songs/]],
];

(async () => {
  const electron = await _electron.launch({ args: ['.', '--no-sandbox'], cwd: ELECTRON_DIR,
    executablePath: require(path.join(ELECTRON_DIR, 'node_modules/electron')).toString(), timeout: 60_000 });
  const win = await electron.firstWindow({ timeout: 60_000 });
  await win.evaluate(({ api }) => { localStorage.clear(); localStorage.setItem('serverUrl', api); }, { api: API });
  await win.reload();
  await win.waitForTimeout(2500);
  await win.getByLabel('Email').fill('steven@example.com');
  await win.getByLabel('Password').fill('bingo99');
  await win.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await win.waitForTimeout(4500);

  let failures = 0;
  for (const [route, patterns] of checks) {
    await win.goto('http://localhost:3001' + route, { waitUntil: 'load' });
    await win.waitForTimeout(3500);
    const body = await win.locator('body').innerText();
    const missing = patterns.filter((p) => !p.test(body));
    if (missing.length) failures++;
    console.log(`${missing.length ? 'MISS' : 'OK  '} ${route.padEnd(18)} ${missing.length ? 'missing ' + missing.join(',') : 'content present'}`);
  }
  console.log(`\ncontent checks: ${checks.length - failures}/${checks.length} screens show seeded data`);
  await win.screenshot({ path: '/tmp/meloamp-final.png' });
  await electron.close();
})().catch((e) => { console.error('FAILED:', e.message); process.exit(1); });
