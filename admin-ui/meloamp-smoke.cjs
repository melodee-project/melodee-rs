const { _electron } = require('@playwright/test');
const path = require('path');
const os = require('os');

const ELECTRON_DIR = '/home/steven/src/github/meloamp/src/electron';

(async () => {
  const electron = await _electron.launch({
    args: ['.', '--no-sandbox'],
    cwd: ELECTRON_DIR,
    env: { ...process.env, DISPLAY: process.env.DISPLAY || ':0' },
    executablePath: require(path.join(ELECTRON_DIR, 'node_modules/electron')).toString(),
    timeout: 60_000,
  });
  const win = await electron.firstWindow({ timeout: 60_000 });
  console.log('window title:', await win.title());
  await win.waitForTimeout(4000);
  console.log('url:', win.url());
  console.log('body:', (await win.locator('body').innerText()).slice(0, 500).replace(/\n/g, ' | '));
  await win.screenshot({ path: '/tmp/meloamp-login.png' });
  await electron.close();
})().catch((error) => { console.error('LAUNCH FAILED:', error.message); process.exit(1); });
