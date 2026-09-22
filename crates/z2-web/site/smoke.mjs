// Playwright smoke spec sketch for z2-web.
//
//   node crates/z2-web/site/smoke.mjs
//
// Loads the static page, drops the Z2_ROM fixture, asserts frames advance
// and window.z2.facts() parses as valid JSON with the GameFacts shape, then
// exercises window.z2.ext (widescreen resize, co-op toggle, netplay status).
//
// Environment (all required at run time, none at commit time):
//   - site/pkg/ built      (wasm-pack build crates/z2-web --target web ...)
//   - $Z2_ROM               (user-supplied Zelda II USA dump, never in repo)
//   - playwright npm module (npm i -D playwright && npx playwright install)
//
// Exit codes: 0 pass · 1 assertion failure · 2 environment-gated (missing
// pkg / ROM / playwright / browser) with a clear message on stderr.

import { readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';

import { SITE, serveSite } from './qa-server.mjs';

const gate = (msg) => { console.error(`SMOKE-GATED: ${msg}`); process.exit(2); };
const fail = (msg) => { console.error(`SMOKE-FAIL: ${msg}`); process.exit(1); };

try { await stat(join(SITE, 'pkg', 'z2_web.js')); }
catch { gate('site/pkg/ missing — run: wasm-pack build crates/z2-web --target web --out-dir crates/z2-web/site/pkg'); }

const romPath = process.env.Z2_ROM;
if (!romPath) gate('$Z2_ROM is not set (points at your Zelda II USA dump)');

let playwright;
try { playwright = await import('playwright'); }
catch { gate('playwright module not installed (npm i -D playwright)'); }

// Tiny static server with the correct .wasm MIME and `/` -> index.html
// (both subtleties, and why they matter, are documented in qa-server.mjs).
const server = await serveSite();
const base = `${server.origin}/`;

let browser;
try {
  browser = await playwright.chromium.launch();
} catch (e) { gate(`browser launch failed (${e.message})`); }

try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  // 'load', not 'networkidle': the page keeps a requestAnimationFrame loop
  // running from the first paint, so "no network for 500 ms" is not a signal
  // worth waiting 30 s for. Readiness is `window.z2`, asserted next.
  await page.goto(base, { waitUntil: 'load' });
  await page.waitForFunction(() => window.z2 !== undefined, null, { timeout: 15000 });

  // Drop the ROM: DataTransfer with the real file bytes from $Z2_ROM.
  const romBytes = (await readFile(romPath)).toString('base64');
  await page.evaluate(async (b64) => {
    const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    // Drive the same path as a real drop: feed the file input.
    const dt = new DataTransfer();
    dt.items.add(new File([bin], 'zelda2.nes'));
    const input = document.getElementById('romFile');
    input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, romBytes);

  await page.waitForFunction(() => JSON.parse(window.z2.state()).romLoaded === true, null, { timeout: 15000 });
  const f0 = JSON.parse(await page.evaluate(() => window.z2.state())).frame;
  await page.waitForTimeout(1500);
  const st = JSON.parse(await page.evaluate(() => window.z2.state()));
  if (!(st.frame > f0)) fail(`frames did not advance (before=${f0} after=${st.frame}) state=${JSON.stringify(st)}`);

  // The QA contract is exactly these nine members plus the unstable `ext`
  // namespace. The COUNT is asserted so a silently added top-level member
  // breaks the test instead of the contract: new controls belong under `ext`,
  // and both lists here must match the table in site/README.
  const STABLE = ['facts', 'state', 'input', 'step', 'snapshot', 'restore',
    'loadMovie', 'screenshot', 'version'];
  const members = await page.evaluate(() => Object.keys(window.z2));
  const stable = members.filter((m) => m !== 'ext');
  if (stable.length !== STABLE.length) {
    fail(`window.z2 has ${stable.length} stable members, the documented contract has ` +
      `${STABLE.length}: got [${stable.join(', ')}] — add new members under z2.ext and ` +
      `update site/README`);
  }
  for (const m of STABLE) {
    if (!members.includes(m)) fail(`window.z2.${m} is missing (QA contract)`);
  }
  if (!members.includes('ext')) fail('window.z2.ext is missing');

  // facts() must be valid JSON with the GameFacts shape.
  const facts = JSON.parse(await page.evaluate(() => window.z2.facts()));
  for (const k of ['link', 'spells', 'items', 'world', 'mode', 'enemies', 'timers']) {
    if (!(k in facts)) fail(`facts() missing key ${k}: ${JSON.stringify(facts).slice(0, 200)}`);
  }
  if (!Array.isArray(facts.enemies) || facts.enemies.length !== 6) fail('facts().enemies is not 6 slots');

  // Snapshot roundtrip in-tab (counter advances; memory restores).
  const roundtrip = await page.evaluate(() => {
    const a = window.z2.snapshot();
    window.z2.step(5);
    window.z2.restore(a);
    const b = window.z2.snapshot();
    window.z2.restore(b);
    const c = window.z2.snapshot();
    return b.length === c.length && b.every((v, i) => v === c[i]);
  });
  if (!roundtrip) fail('snapshot/restore roundtrip mismatch');

  // Screenshot returns a PNG data URL.
  const shot = await page.evaluate(() => window.z2.screenshot());
  if (typeof shot !== 'string' || !shot.startsWith('data:image/png')) fail('screenshot() is not a PNG data URL');

  // `window.z2.ext` — the UNSTABLE surface for widescreen / co-op / netplay.
  // Checked separately from the 9 stable members above, so a change here can
  // never be mistaken for a break of the QA contract. See site/README.
  const ext = await page.evaluate(() => {
    const e = window.z2.ext;
    const before = e.frameSize();
    e.setWide('16:9');
    const wide = e.frameSize();
    const canvasWide = document.getElementById('screen').width;
    e.setWide('off');
    const back = e.frameSize();
    e.coopEnable(true);
    const coopOn = e.coopEnabled();
    const status = JSON.parse(e.coopStatus());
    e.coopEnable(false);
    return {
      before, wide, back, canvasWide, coopOn, status,
      hash: e.coopHash(), trapset: e.trapsetId(),
      netSupported: e.net.supported(), netStatus: JSON.parse(e.net.status()),
      // The netplay QA surface site/netplay-e2e.mjs drives. `manual` must
      // round-trip and end up OFF, so a smoke run can never leave the page
      // with the rAF loop refusing to pump a session.
      netManualOn: e.net.manual(true), netManualOff: e.net.manual(false),
      netStepShape: typeof e.net.step, netHashShape: typeof e.net.stateHash,
      // Rollback surface: mode selector round-trip, the simulated-network test
      // hook (must end OFF), the confirmed-hash log and the cost probe.
      modeDefault: e.net.mode(), panelMode: document.getElementById('netMode').value,
      modeLockstep: e.net.setMode('lockstep').mode, modeBack: e.net.setMode('rollback').mode,
      simOn: e.net.simulate({ latencyMs: 40, jitterMs: 10, lossPct: 3 }), simOff: e.net.simulate({}),
      confirmed: e.net.confirmedHashes(),
      perfFrame0: JSON.parse(window.z2.state()).frame,
      perf: e.perf.rollback({ iterations: 3, depths: [0, 2] }),
      perfFrame1: JSON.parse(window.z2.state()).frame,
    };
  });
  if (ext.modeDefault.mode !== 'rollback' || ext.panelMode !== 'rollback') {
    fail(`netplay must default to rollback: ${JSON.stringify(ext.modeDefault)} panel=${ext.panelMode}`);
  }
  if (ext.modeLockstep !== 'lockstep' || ext.modeBack !== 'rollback') fail('net.setMode() did not round-trip');
  // A bundle without the `net` feature has no transport to wrap and answers
  // all zeros; a netplay bundle must apply the setting.
  if (ext.netSupported && (ext.simOn.latencyMs !== 40 || ext.simOn.lossPct !== 3)) {
    fail(`net.simulate() not applied: ${JSON.stringify(ext.simOn)}`);
  }
  if (ext.simOff.latencyMs !== 0 || ext.simOff.jitterMs !== 0 || ext.simOff.lossPct !== 0) {
    fail(`net.simulate({}) must turn the test hook off: ${JSON.stringify(ext.simOff)}`);
  }
  if (ext.confirmed !== '[]') fail(`net.confirmedHashes() should be [] with no session, got ${ext.confirmed}`);
  for (const k of ['frame', 'save', 'load']) {
    if (!(ext.perf[k] && ext.perf[k].mean >= 0)) fail(`perf.rollback().${k} missing: ${JSON.stringify(ext.perf)}`);
  }
  if (!(ext.perf.tick[2] && ext.perf.tick[2].max >= 0)) fail(`perf.rollback().tick[2] missing: ${JSON.stringify(ext.perf)}`);
  if (ext.perfFrame1 !== ext.perfFrame0) {
    fail(`perf.rollback() must leave the game where it was (frame ${ext.perfFrame0} -> ${ext.perfFrame1})`);
  }
  if (ext.before.width !== 256) fail(`ext.frameSize() should start at 256, got ${ext.before.width}`);
  if (ext.wide.width !== 432) fail(`16:9 should report 432 px, got ${ext.wide.width}`);
  if (ext.canvasWide !== 432) fail(`the canvas should resize to 432 px, got ${ext.canvasWide}`);
  if (ext.back.width !== 256) fail(`widescreen off should restore 256, got ${ext.back.width}`);
  if (!ext.coopOn) fail('ext.coopEnable(true) did not take effect');
  if (typeof ext.status.active !== 'boolean') fail(`coopStatus().active missing: ${JSON.stringify(ext.status)}`);
  if (!/^[0-9a-f]{16}$/.test(ext.hash)) fail(`coopHash() should be 16 hex chars, got ${ext.hash}`);
  if (!/^[0-9a-f]{16}$/.test(ext.trapset)) fail(`trapsetId() should be 16 hex chars, got ${ext.trapset}`);
  if (typeof ext.netSupported !== 'boolean') fail('net.supported() must be a boolean');
  if (ext.netStatus.active !== false) fail('net.status().active should be false with no session');
  if (typeof ext.netStatus.hashesOk !== 'number') {
    fail(`net.status().hashesOk must always be present: ${JSON.stringify(ext.netStatus)}`);
  }
  if (ext.netManualOn !== true || ext.netManualOff !== false) {
    fail(`net.manual() must round-trip and turn off again (on=${ext.netManualOn} off=${ext.netManualOff})`);
  }
  if (ext.netStepShape !== 'function') fail('net.step must be a function');
  if (ext.netHashShape !== 'function') fail('net.stateHash must be a function');
  // `stateHash()` is the value the two peers compare. It exists only in a
  // `net`/`netplay` bundle; a default bundle must refuse it loudly, not lie.
  const netHash = await page.evaluate(() => {
    try { return { ok: window.z2.ext.net.stateHash() }; } catch (e) { return { err: String(e) }; }
  });
  if (ext.netSupported) {
    if (!/^[0-9a-f]{16}$/.test(netHash.ok || '')) {
      fail(`net.stateHash() should be 16 hex chars in a netplay bundle, got ${JSON.stringify(netHash)}`);
    }
  } else if (!netHash.err && !/^[0-9a-f]{16}$/.test(netHash.ok || '')) {
    fail(`net.stateHash() must return a hash or throw, got ${JSON.stringify(netHash)}`);
  }

  // The widescreen canvas must not overflow a phone-width viewport: the CSS
  // caps the element at 100% of its container while keeping the aspect ratio.
  const narrow = await page.evaluate(async () => {
    window.z2.ext.setWide('16:9');
    const c = document.getElementById('screen');
    const r = c.getBoundingClientRect();
    return { backing: c.width, cssW: r.width, cssH: r.height };
  });
  await page.setViewportSize({ width: 380, height: 800 });
  await page.waitForTimeout(150);
  const phone = await page.evaluate(() => {
    const c = document.getElementById('screen').getBoundingClientRect();
    return {
      cssW: c.width,
      cssH: c.height,
      scrollW: document.documentElement.scrollWidth,
      clientW: document.documentElement.clientWidth,
    };
  });
  if (phone.cssW > 380) fail(`the canvas is ${phone.cssW} px wide on a 380 px screen`);
  if (phone.scrollW > phone.clientW + 1) {
    fail(`the page scrolls sideways on a phone (${phone.scrollW} > ${phone.clientW})`);
  }
  const ratio = phone.cssW / phone.cssH;
  if (Math.abs(ratio - 432 / 240) > 0.05) fail(`widescreen aspect ratio is off: ${ratio}`);
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => window.z2.ext.setWide('off'));

  // HD packs: the surface must answer in every bundle, and a bad pack must be
  // reported without stopping the game.
  const hd = await page.evaluate(() => {
    const h = window.z2.ext.hd;
    const supported = h.supported();
    let rejected = null;
    try {
      h.loadPack([{ name: 'pack.json', bytes: new TextEncoder().encode('{ not json') }]);
    } catch (e) {
      rejected = String(e);
    }
    return { supported, rejected, info: h.packInfo(), scale: h.scale() };
  });
  if (typeof hd.supported !== 'boolean') fail('ext.hd.supported() must be a boolean');
  if (!hd.rejected) fail('a malformed pack.json must throw, not load silently');
  if (hd.info !== 'null') fail(`a rejected pack must leave no pack loaded, got ${hd.info}`);
  const alive = await page.evaluate(() => {
    const a = JSON.parse(window.z2.state()).frame;
    window.z2.step(3);
    return JSON.parse(window.z2.state()).frame > a;
  });
  if (!alive) fail('the emulator stopped stepping after a rejected pack');
  if (narrow.backing !== 432) fail(`expected a 432 px backing store, got ${narrow.backing}`);

  // Audio: the button is pressed seconds after the game started, the way a
  // player does it. Non-silent samples must reach the worklet's output (the
  // title theme is playing), and the worklet must hold live frames only, not
  // a backlog from before the button was pressed.
  await page.click('#audioBtn');
  await page.waitForFunction(() => window.z2.ext.audio().state === 'running', null, { timeout: 10000 })
    .catch(() => fail('the AudioContext never reached "running" after pressing Enable audio'));
  let audio = await page.evaluate(() => window.z2.ext.audio());
  let loudest = 0;
  for (let i = 0; i < 60 && loudest < 0.01; i++) {
    await page.waitForTimeout(250);
    audio = await page.evaluate(() => window.z2.ext.audio());
    loudest = Math.max(loudest, audio.peak);
  }
  if (!(loudest >= 0.01)) fail(`audio is on but silent after 15 s: ${JSON.stringify(audio)}`);
  // The worklet resyncs to a 50 ms target and never lets the queue past
  // 90 ms (worklet.js), so anything near a tenth of a second means the cap
  // stopped working and the old creeping delay is back. The bound is in
  // milliseconds because the context rate now follows the device.
  const rate = await page.evaluate(() => window.z2.ext.audio().rate);
  const latencyMs = (audio.queued * 1000) / rate;
  if (latencyMs > 150) {
    fail(`the worklet is holding ${audio.queued} samples at ${rate} Hz (${latencyMs.toFixed(0)} ms of latency)`);
  }

  if (errors.length) fail(`page errors: ${errors.join(' | ').slice(0, 500)}`);
  console.log(`SMOKE-OK: frame ${f0} -> ${st.frame}, facts valid, snapshot+shot ok, audio peak ${loudest.toFixed(3)} (${latencyMs.toFixed(0)} ms queued, ${audio.underruns} underruns, ${audio.dropped || 0} dropped)`);
} finally {
  await browser.close();
  await server.close();
}
