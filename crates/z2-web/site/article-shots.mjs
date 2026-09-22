// Screenshots of the browser build for articles and docs: the real page in a
// real browser, widescreen 16:9, local co-op, and the online co-op panel.
// Not part of the site: nothing in index.html / app.js / worklet.js imports it.
//
//   Z2_ROM=/path/zelda2.nes Z2_SHOTS_OUT=/path/outside/the/repo \
//     node crates/z2-web/site/article-shots.mjs
//
// Needs site/pkg/ built with `--features hd,netplay`, target/release/z2-signal
// (for the online part) and the playwright npm module (make netplay-e2e-setup).
//
// Scenes are loaded the way README.md describes: take the
// page's own Z2WEB01 snapshot, patch the scene registers and game mode 0 into
// its RAM image, fix the CRC32 and restore it, so the game's own loader runs.
// Pads are then stepped deterministically with z2.ext.stepCoop.
//
// Knobs: Z2_MOVIE (an .fm2; default $Z2_CORPUS/movies/warpless.fm2, only used
// to get past the menus), Z2_PW_CHANNEL (e.g. "chrome" to drive an installed
// browser instead of Playwright's Chromium), Z2_SHOTS_PARTS ("page", "net" or
// "page,net"; default both).
//
// Output is ROM-derived: Z2_SHOTS_OUT inside a git work tree is refused.
//
// Exit codes: 0 done, 1 a capture failed, 2 environment-gated.

import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

import { SITE, serveSite } from './qa-server.mjs';
import { REPO, loadRomInto, startSignal } from './netplay-lib.mjs';

const gate = (msg) => { console.error(`ARTICLE-SHOTS-GATED: ${msg}`); process.exit(2); };
/** A gate found after servers or browsers exist: unwound through `cleanup`. */
class Gated extends Error {}

const romPath = process.env.Z2_ROM;
if (!romPath) gate('$Z2_ROM is not set — export Z2_ROM=/path/to/your/zelda2.nes (see LEGAL.md)');
try { await stat(romPath); } catch { gate(`$Z2_ROM='${romPath}' is not a file`); }
const OUT = process.env.Z2_SHOTS_OUT;
if (!OUT) gate('$Z2_SHOTS_OUT is not set — name a directory outside the repository');
{
  // Refuse before creating anything: probe the nearest existing ancestor.
  let probe = resolve(OUT);
  while (!existsSync(probe)) probe = dirname(probe);
  let inside = 'false';
  try {
    inside = execFileSync('git', ['-C', probe, 'rev-parse', '--is-inside-work-tree'],
      { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
  } catch { /* not a work tree: fine */ }
  if (inside === 'true') gate(`${OUT} is inside a git work tree; screenshots are ROM-derived (LEGAL.md)`);
}
await mkdir(OUT, { recursive: true });
try { await stat(join(SITE, 'pkg', 'z2_web.js')); }
catch { gate('site/pkg/ missing — wasm-pack build crates/z2-web --target web --out-dir site/pkg -- --features hd,netplay'); }
const corpus = process.env.Z2_CORPUS || resolve(REPO, '..', 'z2-corpus');
const moviePath = process.env.Z2_MOVIE || join(corpus, 'movies', 'warpless.fm2');
let movie;
try { movie = await readFile(moviePath, 'utf8'); }
catch { gate(`no movie at '${moviePath}' — set Z2_CORPUS or Z2_MOVIE=/path/to/track.fm2 (README.md)`); }
let playwright;
try { playwright = await import('playwright'); }
catch { gate('the playwright npm module is not installed — run: make netplay-e2e-setup'); }

const parts = new Set((process.env.Z2_SHOTS_PARTS || 'page,net').split(','));
const channel = process.env.Z2_PW_CHANNEL || undefined;
const romB64 = (await readFile(romPath)).toString('base64');
const VIEW = { viewport: { width: 1180, height: 860 }, deviceScaleFactor: 2 };
const A = 1, B = 2, RIGHT = 128;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const cleanup = [];
async function launch() {
  let browser;
  try { browser = await playwright.chromium.launch({ headless: true, channel }); }
  catch (e) { throw new Gated(`the browser did not start (${e.message.split('\n')[0]}) — make netplay-e2e-setup, or Z2_PW_CHANNEL=chrome`); }
  cleanup.push(() => browser.close());
  return browser;
}

/** Page screenshot plus the bare canvas, after forcing a blit. */
async function shot(page, name) {
  const data = await page.evaluate(() => { window.scrollTo(0, 0); return window.z2.screenshot(); });
  await page.screenshot({ path: join(OUT, `${name}.png`) });
  await writeFile(join(OUT, `${name}.canvas.png`), Buffer.from(data.split(',')[1], 'base64'));
  console.log(`  ${name}`);
}

/** Patch RAM bytes into the page's own snapshot and restore it. */
async function loadScene(page, pokes) {
  await page.evaluate((list) => {
    const crc32 = (buf) => {
      let crc = 0xFFFFFFFF;
      for (const byte of buf) {
        let c = (crc ^ byte) & 0xFF;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1;
        crc = (crc >>> 8) ^ c;
      }
      return (crc ^ 0xFFFFFFFF) >>> 0;
    };
    const blob = new Uint8Array(window.z2.snapshot());
    const ram = 8 + 2 + 8 + 4; // magic, version, frame count, RAM length
    for (const [addr, val] of list) blob[ram + addr] = val;
    new DataView(blob.buffer, blob.byteOffset).setUint32(blob.length - 4, crc32(blob.subarray(0, blob.length - 4)), true);
    window.z2.restore(blob);
  }, pokes);
}

const endGame = [[0x777, 8], [0x778, 8], [0x779, 8], [0x783, 8], [0x784, 8], [0x773, 0xFF], [0x774, 0xFF], [0x785, 1]];
const scene = (region, world, idx, sc, page, town, palace, extra = []) => [
  [0x706, region], [0x707, world], [0x748, idx], [0x56B, town], [0x56C, palace],
  [0x561, sc], [0x75C, page], [0x701, 0], ...extra, [0x736, 0],
];
const step = (page, p1, p2, n) => page.evaluate(([a, b, c]) => window.z2.ext.stepCoop(a, b, c), [p1, p2, n]);

async function pagePart() {
  console.log('page: widescreen 16:9 + local co-op');
  const srv = await serveSite();
  cleanup.push(() => srv.close());
  const page = await (await (await launch()).newContext(VIEW)).newPage();
  await page.goto(`${srv.origin}/?widescreen=16:9&coop=1&zoom=2`, { waitUntil: 'load' });
  await page.waitForFunction(() => window.z2 !== undefined, null, { timeout: 30000 });
  await page.screenshot({ path: join(OUT, 'web_drop_rom.png') });
  await loadRomInto(page, romB64);
  await sleep(11000); // the live loop reaches the title logo by itself
  await shot(page, 'web_title');
  await page.click('#pauseBtn'); // from here on every frame is stepped by hand
  await page.evaluate((m) => window.z2.loadMovie(m), movie);
  await page.evaluate(() => window.z2.step(600)); // past the menus, onto the overworld

  await loadScene(page, scene(0, 1, 0x2C, 1, 0, 0, 0)); // Rauru
  await step(page, 0, 0, 150);
  await step(page, RIGHT, 0, 25);
  await step(page, RIGHT, RIGHT, 95);
  await step(page, 0, RIGHT, 40);
  await step(page, 0, 0, 20);
  await shot(page, 'web_rauru_coop');

  await loadScene(page, scene(2, 5, 0x36, 53, 1, 0, 2, endGame)); // Thunderbird
  await step(page, 0, 0, 150);
  await step(page, RIGHT, 0, 8);
  await step(page, RIGHT, RIGHT, 130);
  await step(page, RIGHT, A, 12);
  await step(page, B, RIGHT | A, 6);
  await shot(page, 'web_thunderbird_coop');
}

async function netPart() {
  console.log('net: online co-op panel, two browsers, one room');
  let signal;
  try { signal = await startSignal(); }
  catch (e) { throw new Gated(`${e.message} — run: cargo build --release -p z2-signal`); }
  cleanup.push(() => signal.kill());
  const peers = [];
  for (let i = 0; i < 2; i++) {
    const srv = await serveSite(); // two origins: the pages share no storage
    cleanup.push(() => srv.close());
    // One browser per peer: two pages of one browser throttle the hidden one.
    const page = await (await (await launch()).newContext(VIEW)).newPage();
    const q = new URLSearchParams({ net: 'rollback', signal: signal.url, room: 'hyrule-7f3a', ice: 'none', zoom: '2', widescreen: '16:9' });
    await page.goto(`${srv.origin}/?${q}`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.z2 !== undefined, null, { timeout: 30000 });
    await loadRomInto(page, romB64);
    peers.push(page);
  }
  const [host, guest] = peers;
  const status = (p) => p.evaluate(() => JSON.parse(window.z2.ext.net.status()));
  const until = async (p, pred, ms, what) => {
    const t0 = Date.now();
    for (;;) {
      const s = await status(p);
      if (pred(s)) return s;
      if (Date.now() - t0 > ms) throw new Error(`${what}: ${JSON.stringify(s)}`);
      await sleep(50);
    }
  };
  await sleep(9000); // title logo behind the panel
  await host.click('#netHost');
  await until(host, (s) => s.link === 'waiting', 10000, 'the host never reached the room');
  await sleep(1000);
  await host.screenshot({ path: join(OUT, 'net_host_waiting.png'), fullPage: true });
  console.log('  net_host_waiting');
  await guest.click('#netJoin');
  await until(host, (s) => s.link === 'linking' || s.started, 10000, 'the guest never arrived');
  const box = await host.locator('#netBox').boundingBox();
  if (box) await host.screenshot({ path: join(OUT, 'net_panel_linking.png'), clip: box });
  console.log('  net_panel_linking');

  // The session itself needs a working WebRTC path between the two browsers.
  // On a host whose only non-loopback interface does not route to itself (some
  // VPNs, sandboxes) the link never forms; that is reported, not faked.
  await Promise.all(peers.map((p) => until(p, (s) => s.started && s.state === 'running', 20000,
    'the peer link never formed (WebRTC needs a route between the two browsers; see README.md)')));
  const tap = async (key, hold = 120, gap = 450) => { await host.keyboard.down(key); await sleep(hold); await host.keyboard.up(key); await sleep(gap); };
  await host.bringToFront();
  await sleep(2500);
  await tap('Enter', 150, 1500); // title -> file select
  await tap('Enter', 150, 1500); // -> register
  for (let i = 0; i < 4; i++) await tap('KeyZ', 100, 250);
  for (let i = 0; i < 3; i++) await tap('ShiftLeft', 100, 300); // cursor to END
  await tap('Enter', 150, 1500); // back to file select
  await tap('Enter', 150, 500); // start: North Palace, where player 2 appears
  const t0 = Date.now();
  for (;;) {
    const c = await host.evaluate(() => JSON.parse(window.z2.ext.coopStatus() || 'null'));
    if (c && c.active) break;
    if (Date.now() - t0 > 25000) throw new Error('player 2 never became active on the host');
    await sleep(200);
  }
  await guest.bringToFront();
  await guest.keyboard.down('ArrowLeft'); await sleep(700); await guest.keyboard.up('ArrowLeft');
  await host.bringToFront();
  await host.keyboard.down('ArrowRight'); await sleep(500); await host.keyboard.up('ArrowRight');
  await sleep(600);
  await shot(host, 'net_session_host');
  await shot(guest, 'net_session_guest');
}

let code = 0;
try {
  if (parts.has('page')) await pagePart();
  if (parts.has('net')) await netPart();
} catch (e) {
  code = e instanceof Gated ? 2 : 1;
  console.error(`ARTICLE-SHOTS-${code === 2 ? 'GATED' : 'FAIL'}: ${e.message}`);
} finally {
  for (const c of cleanup.reverse()) { try { await c(); } catch { /* best effort */ } }
}
process.exit(code);
