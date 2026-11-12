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
  win.on('console', (m) => { if (m.type() === 'error') events.push({ phase, kind: 'console', text: m.text().slice(0, 200) }); });
  win.on('response', async (r) => {
    if (r.status() >= 400) {
      let body = ''; try { body = (await r.text()).slice(0, 160); } catch {}
      events.push({ phase, kind: 'http', status: r.status(), method: r.request().method(), url: r.url(), body });
    }
  });

  await win.evaluate(({ api }) => { localStorage.clear(); localStorage.setItem('serverUrl', api); }, { api: API });
  await win.reload();
  await win.waitForTimeout(2500);
  await win.getByLabel('Email').fill('meloamp@example.com');
  await win.getByLabel('Password').fill('meloamp123');
  await win.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await win.waitForTimeout(4000);
  await win.waitForFunction(() => [...document.images].every((i) => i.complete), null, { timeout: 20_000 }).catch(() => {});

  const report = async (label) => {
    const imgs = await win.locator('img').evaluateAll((els) => els.map((e) => ({ src: e.currentSrc || e.getAttribute('src') || '', w: e.naturalWidth })));
    const broken = imgs.filter((i) => i.src.includes(':8080') && i.w === 0);
    console.log(`\n--- ${label} ---`);
    console.log('  text:', (await win.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 240));
    console.log(`  images: ${imgs.length} total, ${broken.length} broken-from-API`);
    if (broken.length) console.log('  broken:', broken.slice(0, 4).map((b) => b.src.replace(/^http:\/\/[^/]+/, '')).join(' '));
    const audio = await win.locator('audio').evaluateAll((els) => els.map((e) => ({ src: (e.currentSrc || e.src || '').replace(/jwt=[^&]+/, 'jwt=<redacted>'), ready: e.readyState, paused: e.paused, dur: Math.round(e.duration || 0) })));
    if (audio.length) console.log('  audio:', JSON.stringify(audio));
  };

  // Album detail via card click
  phase = 'albums';
  await win.goto('http://localhost:3001/albums', { waitUntil: 'load' });
  await win.waitForTimeout(3500);
  await report('albums list');
  await win.locator('.MuiCard-root').first().click();
  await win.waitForTimeout(4500);
  phase = 'album-detail';
  console.log('\n  url now:', win.url());
  await report('album detail');
  // Play the album
  phase = 'album-play';
  const playBtn = win.getByRole('button', { name: /^play/i }).first();
  if (await playBtn.count()) { await playBtn.click(); await win.waitForTimeout(7000); await report('after play'); }
  else console.log('  no play button found; buttons:', (await win.locator('button').allInnerTexts()).slice(0, 15).join(' | '));

  // Artist detail
  phase = 'artists';
  await win.goto('http://localhost:3001/artists', { waitUntil: 'load' });
  await win.waitForTimeout(3500);
  await win.locator('.MuiCard-root').first().click();
  await win.waitForTimeout(4500);
  phase = 'artist-detail';
  await report('artist detail');

  // Song play from list
  phase = 'songs';
  await win.goto('http://localhost:3001/songs', { waitUntil: 'load' });
  await win.waitForTimeout(3500);
  await report('songs list');
  const songCard = win.locator('.MuiCard-root').first();
  if (await songCard.count()) {
    const playIcon = songCard.locator('button').first();
    if (await playIcon.count()) { await playIcon.click(); await win.waitForTimeout(7000); phase = 'song-play'; await report('after song play'); }
  }

  console.log('\n=== failures ===');
  const seen = new Set();
  for (const e of events) {
    const key = `${e.phase}|${e.kind}|${e.status || ''}|${e.url || e.text}`;
    if (seen.has(key)) continue; seen.add(key);
    if (e.kind === 'http') console.log(`${e.phase}: ${e.status} ${e.method} ${e.url.replace(/^http:\/\/[^/]+/, '')} :: ${String(e.body || '').replace(/\s+/g, ' ').slice(0, 110)}`);
    else console.log(`${e.phase}: console :: ${e.text}`);
  }
  await electron.close();
})().catch((e) => { console.error('FAILED:', e.message); process.exit(1); });
