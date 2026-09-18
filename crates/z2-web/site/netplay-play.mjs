// Two visible browser windows, already in one rollback session, for a person
// to play with the keyboard.
//
//   make netplay-play            # builds the bundle + z2-signal, then runs this
//   node crates/z2-web/site/netplay-play.mjs
//
// It starts z2-signal on a free loopback port, serves the site twice (two
// origins, so the windows share no storage), opens two headed Chromium
// windows side by side, loads $Z2_ROM into both through the page's file
// input, and hosts in the left window / joins in the right one (rollback
// mode, ICE "none" because both peers are on this machine). Then it waits.
// Closing both windows, or Ctrl-C, stops the servers.
//
// Knobs: Z2_NET_DELAY (input delay, default 2), Z2_PLAY_ZOOM (default 2).
// For automated checks only: Z2_PLAY_HEADLESS=1 runs without windows and
// Z2_PLAY_EXIT_AFTER_MS=N closes everything after N ms once both run.
//
// Exit codes: 0 closed normally, 1 the session could not be set up,
// 2 environment-gated (no ROM, bundle, signal server or Playwright).

import { readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';

import { SITE, serveSite } from './qa-server.mjs';
import { loadRomInto, startSignal } from './netplay-lib.mjs';

const gate = (msg) => { console.error(`NETPLAY-PLAY-GATED: ${msg}`); process.exit(2); };

const romPath = process.env.Z2_ROM;
if (!romPath) gate('$Z2_ROM is not set — export Z2_ROM=/path/to/your/zelda2.nes (see LEGAL.md)');
try { await stat(romPath); } catch { gate(`$Z2_ROM='${romPath}' is not a file`); }
try { await stat(join(SITE, 'pkg', 'z2_web.js')); }
catch { gate('site/pkg/ missing — run `make netplay-play`, which builds it with --features hd,netplay'); }
let playwright;
try { playwright = await import('playwright'); }
catch { gate('the playwright npm module is not installed — run: make netplay-e2e-setup'); }

const headless = process.env.Z2_PLAY_HEADLESS === '1';
const exitAfter = Number(process.env.Z2_PLAY_EXIT_AFTER_MS || 0);
const DELAY = Number(process.env.Z2_NET_DELAY || 2);
const ZOOM = Number(process.env.Z2_PLAY_ZOOM || 2);
const ROOM = `play${Date.now().toString(36)}`;
const WIN_W = 860;
const WIN_H = 900;

const cleanup = [];
let closing = false;
async function shutdown(code) {
  if (closing) return;
  closing = true;
  for (const c of cleanup.reverse()) { try { await c(); } catch { /* best effort */ } }
  process.exit(code);
}
process.on('SIGINT', () => { console.log('\nCtrl-C: closing the windows and servers'); shutdown(0); });
process.on('SIGTERM', () => shutdown(0));

try {
  let signal;
  try { signal = await startSignal(); }
  catch (e) { gate(`${e.message} — run: cargo build --release -p z2-signal`); }
  cleanup.push(() => signal.kill());
  const srvs = [await serveSite(), await serveSite()];
  for (const s of srvs) cleanup.push(() => s.close());

  const romB64 = (await readFile(romPath)).toString('base64');
  const windows = [];
  for (const [i, role] of [[0, 'host'], [1, 'guest']]) {
    let browser;
    try {
      browser = await playwright.chromium.launch({
        headless,
        // This script owns Ctrl-C: Playwright's own handler would exit before
        // the signal server and the static servers are stopped.
        handleSIGINT: false,
        handleSIGTERM: false,
        handleSIGHUP: false,
        args: [`--window-position=${i * (WIN_W + 10)},0`, `--window-size=${WIN_W},${WIN_H}`],
      });
    } catch (e) {
      console.error(`NETPLAY-PLAY-GATED: Chromium did not start (${e.message.split('\n')[0]}) — ` +
        'run: make netplay-e2e-setup');
      await shutdown(2);
    }
    cleanup.push(() => browser.close());
    const context = await browser.newContext({ viewport: headless ? { width: WIN_W, height: WIN_H } : null });
    const page = await context.newPage();
    const q = new URLSearchParams({ net: 'rollback', signal: signal.url, room: ROOM, ice: 'none', zoom: String(ZOOM) });
    await page.goto(`${srvs[i].origin}/?${q}`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.z2 !== undefined, null, { timeout: 30000 });
    await loadRomInto(page, romB64);
    await page.selectOption('#netDelay', String(DELAY));
    windows.push({ browser, page, role });
  }

  const status = (w) => w.page.evaluate(() => JSON.parse(window.z2.ext.net.status()));
  await windows[0].page.click('#netHost');
  const t0 = Date.now();
  while ((await status(windows[0])).link !== 'waiting') {
    if (Date.now() - t0 > 10000) throw new Error(`the host never reached the room: ${JSON.stringify(await status(windows[0]))}`);
    await new Promise((r) => setTimeout(r, 50));
  }
  await windows[1].page.click('#netJoin');
  for (;;) {
    const sts = await Promise.all(windows.map(status));
    if (sts.every((s) => s.started && s.state === 'running')) break;
    const dead = sts.find((s) => !s.active || s.state === 'closed');
    if (dead || Date.now() - t0 > 20000) {
      throw new Error(`the session did not start: ${sts.map((s) => `${s.role}=${s.state}/${s.link} ${s.closeReason || ''}`).join(', ')}`);
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  // Put keyboard focus on the host window's page.
  await windows[0].page.bringToFront();

  console.log(`
netplay-play: rollback session '${ROOM}' is running (input delay ${DELAY}, ICE none)
  signal server : ${signal.url} (pid ${signal.proc.pid})
  LEFT window   : player 1 (host)   ${srvs[0].origin}
  RIGHT window  : player 2 (guest)  ${srvs[1].origin}

  Keys (in whichever window has focus — click it first):
    arrows = D-pad   Z = A (jump)   X = B (attack)   Enter = Start   Shift = Select
  Player 2 appears in side-view areas once player 1 walks into one.
  Click "Audio on" in a window to hear it. Close both windows or press Ctrl-C to stop.
`);

  if (exitAfter > 0) {
    await new Promise((r) => setTimeout(r, exitAfter));
    const sts = await Promise.all(windows.map(status));
    console.log(`netplay-play: exiting after ${exitAfter} ms: ` +
      sts.map((s) => `${s.role} ${s.state} f=${s.frame} rtt=${s.rttMs}ms`).join(', '));
    const ok = sts.every((s) => s.state === 'running' && s.frame > 0);
    await shutdown(ok ? 0 : 1);
  }
  let open = windows.length;
  for (const w of windows) {
    w.browser.on('disconnected', () => { open -= 1; if (open === 0) { console.log('both windows closed'); shutdown(0); } });
    // Closing the only page of a window also counts as closing it.
    w.page.on('close', () => { w.browser.close().catch(() => {}); });
  }
} catch (e) {
  console.error(`NETPLAY-PLAY-FAIL: ${e.message}`);
  await shutdown(1);
}
