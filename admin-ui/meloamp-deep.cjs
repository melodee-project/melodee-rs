const { _electron } = require('@playwright/test');
const path = require('path');

const ELECTRON_DIR = '/home/steven/src/github/meloamp/src/electron';
const API = 'http://127.0.0.1:8080/api/v1';

(async () => {
  const electron = await _electron.launch({
    args: ['.', '--no-sandbox'],
    cwd: ELECTRON_DIR,
    executablePath: require(path.join(ELECTRON_DIR, 'node_modules/electron')).toString(),
    timeout: 60_000,
  });
  const win = await electron.firstWindow({ timeout: 60_000 });
  const events = [];
  let phase = 'boot';
  win.on('console', (m) => { if (m.type() === 'error') events.push({ phase, kind: 'console', text: m.text().slice(0, 240) }); });
  win.on('response', async (r) => {
    if (r.status() >= 400) {
      let body = ''; try { body = (await r.text()).slice(0, 200); } catch { body = '<unreadable>'; }
      events.push({ phase, kind: 'http', status: r.status(), method: r.request().method(), url: r.url(), body });
    }
  });

  await win.evaluate(({ api }) => { localStorage.clear(); localStorage.setItem('serverUrl', api); }, { api: API });
  await win.reload();
  await win.waitForTimeout(2500);
  await win.getByLabel('Email').fill('meloamp@example.com');
  await win.getByLabel('Password').fill('meloamp123');
  await win.getByRole('button', { name: /sign in|log in|login/i }).first().click();
  await win.waitForTimeout(5000);

  const snap = async (label) => {
    const marks = [];
    const imgs = await win.locator('img').evaluateAll((els) => els.slice(0, 12).map((e) => ({
      src: e.getAttribute('src'), w: e.naturalWidth, h: e.naturalHeight,
    })));
    console.log(`\n--- ${label} ---`);
    console.log('  body:', (await win.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 300));
    console.log('  imgs:', JSON.stringify(imgs.filter((i) => i.src).slice(0, 6)));
    const audio = await win.locator('audio').evaluateAll((els) => els.map((e) => ({ src: e.getAttribute('src'), ready: e.readyState, dur: e.duration, paused: e.paused })));
    if (audio.length) console.log('  audio:', JSON.stringify(audio));
    return marks;
  };

  // Albums → detail → play
  phase = 'albums';
  await win.goto('http://localhost:3001/albums', { waitUntil: 'load' });
  await win.waitForTimeout(4000);
  await snap('albums list');
  const albumCard = win.locator('a[href^="/albums/"]').first();
  console.log('album links:', await win.locator('a[href^="/albums/"]').count());
  if (await albumCard.count()) {
    await albumCard.click();
    await win.waitForTimeout(5000);
    phase = 'album-detail';
    await snap('album detail');
    // Play the album (first play button)
    const playBtn = win.getByRole('button', { name: /play/i }).first();
    if (await playBtn.count()) {
      await playBtn.click();
      await win.waitForTimeout(6000);
      phase = 'album-play';
      await snap('after play click');
    }
  }

  // Artist detail
  phase = 'artists';
  await win.goto('http://localhost:3001/artists', { waitUntil: 'load' });
  await win.waitForTimeout(4000);
  const artistCard = win.locator('a[href^="/artists/"]').first();
  console.log('\nartist links:', await win.locator('a[href^="/artists/"]').count());
  if (await artistCard.count()) {
    await artistCard.click();
    await win.waitForTimeout(5000);
    phase = 'artist-detail';
    await snap('artist detail');
  }

  // Song detail
  phase = 'songs';
  await win.goto('http://localhost:3001/songs', { waitUntil: 'load' });
  await win.waitForTimeout(4000);
  await snap('songs list');
  const songRow = win.locator('a[href^="/songs/"], tr[data-testid], [role="row"]').first();
  console.log('song rows:', await win.locator('a[href^="/songs/"]').count());

  // Search
  phase = 'search';
  await win.goto('http://localhost:3001/search-advanced', { waitUntil: 'load' });
  await win.waitForTimeout(4000);
  await snap('advanced search');

  console.log('\n=== failures ===');
  const seen = new Set();
  for (const e of events) {
    const key = `${e.phase}|${e.kind}|${e.status || ''}|${e.url || e.text}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (e.kind === 'http') console.log(`${e.phase}: ${e.status} ${e.method} ${e.url.replace(/^http:\/\/[^/]+/, '')} :: ${e.body.replace(/\s+/g, ' ').slice(0, 120)}`);
    else console.log(`${e.phase}: console :: ${e.text}`);
  }
  await electron.close();
})().catch((e) => { console.error('FAILED:', e.message); process.exit(1); });
