const { _electron } = require('@playwright/test');
const path = require('path');
const ELECTRON_DIR = '/home/steven/src/github/meloamp/src/electron';
const API = 'http://127.0.0.1:8080/api/v1';

(async () => {
  const electron = await _electron.launch({ args: ['.', '--no-sandbox'], cwd: ELECTRON_DIR,
    executablePath: require(path.join(ELECTRON_DIR, 'node_modules/electron')).toString(), timeout: 60_000 });
  const win = await electron.firstWindow({ timeout: 60_000 });
  const events = [];
  let phase = 'boot';
  win.on('console', (m) => { if (m.type() === 'error') events.push(`${phase}: console ${m.text().slice(0, 140)}`); });
  win.on('response', async (r) => { if (r.status() >= 400) events.push(`${phase}: ${r.status()} ${r.request().method()} ${r.url().replace(/^http:\/\/[^/]+/, '')}`); });

  await win.evaluate(({ api }) => { localStorage.clear(); localStorage.setItem('serverUrl', api); }, { api: API });
  await win.reload();
  await win.waitForTimeout(2500);
  await win.getByLabel('Email').fill('steven@example.com');
  await win.getByLabel('Password').fill('bingo99');
  await win.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await win.waitForTimeout(4500);

  const audioState = async () => win.locator('audio').evaluateAll((els) => els.map((e) => ({ ready: e.readyState, paused: e.paused, dur: Math.round(e.duration || 0), t: Math.round(e.currentTime) })));

  // 1. Album → play
  phase = 'album';
  await win.goto('http://localhost:3001/albums', { waitUntil: 'load' });
  await win.waitForTimeout(3000);
  await win.locator('.MuiCard-root').first().click();
  await win.waitForTimeout(3500);
  const playBtn = win.getByRole('button', { name: /^play/i }).first();
  if (await playBtn.count()) { await playBtn.click(); phase = 'album-play'; await win.waitForTimeout(6000); }
  console.log('after album play, audio:', JSON.stringify(await audioState()));

  // 2. Song list → star/like first song
  phase = 'favorites';
  await win.goto('http://localhost:3001/songs', { waitUntil: 'load' });
  await win.waitForTimeout(3000);
  const starBtn = win.locator('button[aria-label*="star" i], button[aria-label*="favorite" i], button[aria-label*="like" i]').first();
  console.log('star buttons found:', await win.locator('button[aria-label*="star" i], button[aria-label*="favorite" i], button[aria-label*="like" i]').count());
  if (await starBtn.count()) { await starBtn.click(); await win.waitForTimeout(2500); }
  await win.goto('http://localhost:3001/favorites', { waitUntil: 'load' });
  await win.waitForTimeout(3000);
  console.log('favorites view:', (await win.locator('body').innerText()).replace(/\s+/g, ' ').slice(180, 420));

  // 3. Search: type in the search field on songs page
  phase = 'search';
  await win.goto('http://localhost:3001/songs', { waitUntil: 'load' });
  await win.waitForTimeout(2500);
  const searchInput = win.getByRole('textbox').first();
  if (await searchInput.count()) {
    await searchInput.fill('love');
    await win.waitForTimeout(3000);
    console.log('search results chars:', ((await win.locator('body').innerText()).replace(/\s+/g, ' ')).length);
  }

  // 4. Playlist: create via API-shaped UI flow
  phase = 'playlist';
  await win.goto('http://localhost:3001/playlists', { waitUntil: 'load' });
  await win.waitForTimeout(3000);
  const buttons = await win.locator('button').allInnerTexts();
  console.log('playlists page buttons:', buttons.slice(0, 12).join(' | '));
  const createBtn = win.getByRole('button', { name: /create|new/i }).first();
  if (await createBtn.count()) { await createBtn.click(); await win.waitForTimeout(2000); }
  const nameField = win.getByLabel(/name/i).first();
  if (await nameField.count()) {
    await nameField.fill('Parity Probe');
    const save = win.getByRole('button', { name: /save|create|ok/i }).first();
    if (await save.count()) { await save.click(); await win.waitForTimeout(3000); }
  }
  console.log('after playlist create:', (await win.locator('body').innerText()).replace(/\s+/g, ' ').slice(150, 400));

  console.log('\n=== failures ===');
  console.log([...new Set(events)].join('\n') || '(none)');
  await electron.close();
})().catch((e) => { console.error('FAILED:', e.message); process.exit(1); });
