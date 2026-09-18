// End-to-end proof that ONLINE CO-OP works between two browser windows.
//
//   make netplay-e2e                 # builds what it needs, then runs this
//   node crates/z2-web/site/netplay-e2e.mjs
//
// The weak claim ("two peers connected") is not what this asserts. The claims
// are:
//
//   0. THE PAGE'S OWN LIVE PATH CONNECTS, FAST. Host and Join are clicked with
//      the page exactly as a person gets it: no QA mode, the default ICE
//      setting, the requestAnimationFrame loop pumping the session. Both
//      windows must reach a running session within Z2_E2E_CONNECT_MS (default
//      10 s) of the Join click, the connect stages must show in the status
//      line, and the local game must keep stepping while it connects (a stuck
//      connection used to freeze the page). Leave must cancel at any stage and
//      free the room on the signal server. If the ROM-free native peer
//      (target/release/examples/net_peer) is built, a browser also connects to
//      it, proving browser <-> desktop.
//   1. INPUT CROSSES THE WIRE. Window B holds Right for N frames and player 2
//      moves right *in window A's own game state*; then B holds Left and player
//      2 moves left there. Window A holds Right/Left and player 1 moves the
//      matching way *in window B's game state*. Sign reversal, not a single
//      delta, so ordinary gameplay drift cannot pass.
//   2. LOCKSTEP. At the end both windows are on the same session frame, report
//      the same `net_state_hash` (the value the protocol's own desync detector
//      compares), have compared hashes at least once (`hashesOk > 0`), and
//      neither ever reached `desynced`.
//
// Claims 1 and 2 need the host's pad for every frame from session frame 0 to
// be the movie's, so they run in a SECOND session whose frames are driven by
// the in-page QA driver (`z2.ext.net.manual`), opened after claim 0 has
// proved and closed the live one. The live rAF loop also drives that session
// again near the end (step 9).
//
// Everything is read through the `window.z2` QA hooks, never a screenshot.
// Screenshots are still written at the end, outside the repository, so a human
// can eyeball what the two peers were looking at.
//
// How the two peers reach a place where player 2 exists at all: player 2 is a
// side-view-only second Link, and a lockstep session always
// restarts from power-on, so the title screen is where both peers begin. The
// script replays the player-1 column of a TAS movie THROUGH THE SESSION,
// frame-exactly, until co-op reports player 2 live. That is not a shortcut
// around netplay: every one of those frames is a real lockstep frame that had
// to cross the WebRTC data channel and be confirmed by both peers.
//
// Environment (all required at run time, none at commit time):
//   - site/pkg/ built WITH the netplay feature
//     (wasm-pack build crates/z2-web --target web --out-dir site/pkg \
//        -- --features hd,netplay)
//   - target/release/z2-signal   (cargo build --release -p z2-signal)
//   - $Z2_ROM                    (your own Zelda II (USA) dump, never in repo)
//   - $Z2_MOVIE or $Z2_CORPUS/movies/warpless.fm2   (out-of-tree, see README.md)
//   - playwright npm module + a chromium download (make netplay-e2e-setup)
//
// Exit codes, matching site/smoke.mjs: 0 pass · 1 assertion failure ·
// 2 environment-gated (missing bundle / ROM / movie / signal server /
// playwright / browser) with a clear message on stderr.

import { spawn } from 'node:child_process';
import { mkdtemp, readFile, stat, writeFile } from 'node:fs/promises';
import { connect, createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { SITE, serveSite } from './qa-server.mjs';

const REPO = resolve(SITE, '..', '..', '..');

const say = (msg) => console.log(`  ${msg}`);

// Deferred cleanup, run on every exit path including a thrown assertion.
const cleanup = [];
let failed = null;
const fail = (msg) => { if (!failed) failed = msg; throw new Error(msg); };

// Before anything is spawned a gate can just exit; afterwards it must unwind
// through `cleanup` instead, or the signalling server is orphaned.
let armed = false;
const gate = (msg) => {
  if (!armed) { console.error(`NETPLAY-E2E-GATED: ${msg}`); process.exit(2); }
  throw Object.assign(new Error(msg), { gated: true });
};

// --- environment gates -----------------------------------------------------

try { await stat(join(SITE, 'pkg', 'z2_web.js')); }
catch {
  gate('site/pkg/ missing — run: wasm-pack build crates/z2-web --target web ' +
    '--out-dir site/pkg -- --features hd,netplay  (or just: make netplay-e2e)');
}

const romPath = process.env.Z2_ROM;
if (!romPath) gate('$Z2_ROM is not set (point it at your own Zelda II (USA) dump; see LEGAL.md)');
try { await stat(romPath); } catch { gate(`$Z2_ROM='${romPath}' is not a file`); }

const corpus = process.env.Z2_CORPUS || resolve(REPO, '..', 'z2-corpus');
const moviePath = process.env.Z2_MOVIE || join(corpus, 'movies', 'warpless.fm2');
try { await stat(moviePath); }
catch {
  gate(`no movie at '${moviePath}' — the run needs a player-1 input track to walk ` +
    'into a side-view area, where player 2 exists. Fetch the ' +
    'out-of-tree corpus (README.md) or set Z2_MOVIE=/path/to/track.fm2. ' +
    'A .fm2 (text) is required: .bk2 is a ZIP and is rejected by design.');
}

const signalBin = process.env.Z2_SIGNAL_BIN || join(REPO, 'target', 'release', 'z2-signal');
try { await stat(signalBin); }
catch { gate(`no signalling server at '${signalBin}' — run: cargo build --release -p z2-signal`); }

// Optional: the ROM-free native peer for the browser <-> desktop check.
const netPeerBin = process.env.Z2_NET_PEER_BIN ||
  join(REPO, 'target', 'release', 'examples', 'net_peer');
let haveNetPeer = true;
try { await stat(netPeerBin); } catch { haveNetPeer = false; }

let playwright;
try { playwright = await import('playwright'); }
catch { gate('playwright module not installed — run: make netplay-e2e-setup'); }

// --- knobs -----------------------------------------------------------------

const DELAY = Number(process.env.Z2_NET_DELAY || 2);   // input-delay frames
const ROOM = `e2e${Date.now().toString(36)}`;          // fresh room every run
const WARMUP_CAP = Number(process.env.Z2_E2E_WARMUP || 4000);   // frames to look for P2
const HOLD_FRAMES = Number(process.env.Z2_E2E_HOLD || 90);      // frames per direction hold
const LOCKSTEP_FRAMES = Number(process.env.Z2_E2E_FRAMES || 3000); // total frames before the hash check
const MIN_MOVE = 3;                                    // pixels a hold must produce
// Join click -> both windows running, through the live page. Two windows on
// one machine take about 1-2 s; the bug this guards against took ~40 s.
const CONNECT_BUDGET_MS = Number(process.env.Z2_E2E_CONNECT_MS || 10000);

// --- the movie's player-1 pad track ---------------------------------------
// `.fm2` pad columns are left-to-right RLDUTSBA and map onto NES shift-register
// bits A,B,Select,Start,Up,Down,Left,Right = bits 0..7 — the same contract
// crates/z2-web/src/movie.rs and z2-verify's movie_fm2 use. Parsed here rather
// than through `z2.loadMovie` because the session needs the pad for a specific
// frame, not an armed replay track (movies are locked during a session anyway).
const FM2_COL_BITS = [7, 6, 5, 4, 3, 2, 1, 0];
function parseFm2Pads(text) {
  const pads = [];
  for (const line of text.split('\n')) {
    if (!line.startsWith('|')) continue;
    const cells = line.split('|');
    // |command|pad0|pad1|...|  -> cells[0] is '' and cells[1] is the command.
    const field = cells[2];
    if (field === undefined || field.length !== 8) continue;
    let bits = 0;
    for (let i = 0; i < 8; i++) if (field[i] !== '.' && field[i] !== ' ') bits |= 1 << FM2_COL_BITS[i];
    pads.push(bits);
  }
  return Uint8Array.from(pads);
}

// --- free port -------------------------------------------------------------
async function freePort() {
  const s = createServer();
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  const { port } = s.address();
  await new Promise((r) => s.close(r));
  return port;
}

// --- in-page session driver ------------------------------------------------
// Installed once per page. It runs INSIDE the tab, because a lockstep frame
// needs the JS event loop to turn over (matchbox delivers data-channel messages
// from its own spawn_local task) — one CDP round trip per frame would be two
// orders of magnitude slower and would not make the run any more real.
//
// While `z2.ext.net.manual(true)` is set the page's own rAF loop does not pump
// the session, so this driver is the only thing latching pads: frame f gets
// exactly the pad we chose for it.
const DRIVER = `
window.__z2e2e = {
  // Pump the transport until the session reports one of \`states\`.
  async until(states, timeoutMs) {
    const t0 = Date.now();
    for (;;) {
      const ns = JSON.parse(window.z2.ext.net.poll(16));
      if (states.includes(ns.state) && (!states.includes('running') || ns.started)) return ns;
      if (Date.now() - t0 > timeoutMs) return { ...ns, __timeout: true };
      await new Promise((r) => setTimeout(r, 4));
    }
  },
  // Advance the session to \`target\` frames.
  //  * pads   : base64 pad track; the pad for frame f is pads[f + delay],
  //             which is where \`latch_local\` puts what we hand it. Null means
  //             "sample the live keyboard/gamepad" (z2.ext.net.step()).
  //  * stopOn : 'p2' stops as soon as co-op reports player 2 live.
  async run({ target, padsB64, delay, stopOn, timeoutMs }) {
    const pads = padsB64 ? Uint8Array.from(atob(padsB64), (c) => c.charCodeAt(0)) : null;
    const t0 = Date.now();
    let last = Date.now();
    for (;;) {
      const ns = JSON.parse(window.z2.ext.net.poll(16));
      if (ns.state === 'desynced' || ns.state === 'closed') return { ns, reason: ns.state };
      if (ns.frame >= target) return { ns, reason: 'target' };
      if (stopOn === 'p2') {
        const c = JSON.parse(window.z2.ext.coopStatus() || 'null');
        if (c && c.active) return { ns, reason: 'p2', coop: c };
      }
      const pad = pads ? (pads[ns.frame + delay] || 0) : null;
      const stepped = window.z2.ext.net.step(pad, 1);
      if (stepped > 0) last = Date.now();
      if (Date.now() - last > timeoutMs) return { ns, reason: 'stalled' };
      if (Date.now() - t0 > timeoutMs * 20) return { ns, reason: 'timeout' };
      await new Promise((r) => setTimeout(r, 0));
    }
  },
  // Everything an assertion needs, sampled at one instant.
  probe() {
    const ns = JSON.parse(window.z2.ext.net.status());
    const facts = JSON.parse(window.z2.facts());
    const coop = JSON.parse(window.z2.ext.coopStatus() || 'null');
    let hash = null;
    try { hash = window.z2.ext.net.stateHash(); } catch (e) { hash = 'ERR:' + e; }
    return {
      frame: ns.frame, state: ns.state, role: ns.role, delay: ns.delay,
      hashesOk: ns.hashesOk, error: ns.error, closeReason: ns.closeReason,
      hash,
      p1: { page: facts.link.page, x: facts.link.x, y: facts.link.y, hp: facts.link.hp },
      p2: coop && coop.active
        ? { page: coop.p2Page, x: coop.p2X, y: coop.p2Y, hp: coop.p2Hp, alive: coop.p2Alive }
        : null,
      mode: facts.mode,
    };
  },
};
`;

// World X in pixels, so a page boundary is not read as a 256 px jump backwards.
const worldX = (p) => (p === null ? null : p.page * 256 + p.x);

// --- run -------------------------------------------------------------------

let exitCode = 0;
try {
  armed = true;
  const movieText = await readFile(moviePath, 'utf8');
  const pads = parseFm2Pads(movieText);
  if (pads.length < 600) fail(`movie '${moviePath}' parsed to only ${pads.length} frames`);
  const padsB64 = Buffer.from(pads).toString('base64');
  say(`movie: ${moviePath} (${pads.length} frames of player-1 input)`);

  // 1. signalling server on a free port, killed on every exit path.
  const signalPort = await freePort();
  const signalUrl = `ws://127.0.0.1:${signalPort}`;
  const signal = spawn(signalBin, ['--bind', `127.0.0.1:${signalPort}`], { stdio: ['ignore', 'pipe', 'pipe'] });
  const signalLog = [];
  signal.stdout.on('data', (b) => signalLog.push(String(b)));
  signal.stderr.on('data', (b) => signalLog.push(String(b)));
  let signalExit = null;
  signal.on('exit', (c) => { signalExit = c; });
  cleanup.push(() => { if (signalExit === null) signal.kill('SIGKILL'); });
  // Wait for the port to accept, rather than sleeping a guessed amount. The
  // window is generous (30 s): `make netplay-e2e` has just run wasm-pack and
  // cargo, so the machine can still be loaded enough that a fresh process takes
  // seconds to reach its first bind. Five seconds was not enough and made this
  // gate fire spuriously.
  const LISTEN_TRIES = 600;
  for (let i = 0; ; i++) {
    if (signalExit !== null) {
      gate(`z2-signal exited with ${signalExit} before it listened:\n${signalLog.join('')}`);
    }
    try {
      await new Promise((ok, no) => {
        const s = connect(signalPort, '127.0.0.1', () => { s.destroy(); ok(); });
        s.on('error', no);
      });
      break;
    } catch {
      if (i > LISTEN_TRIES) {
        gate(`z2-signal ('${signalBin}') never listened on ${signalPort} within ` +
          `${(LISTEN_TRIES * 50) / 1000} s. Its output was:\n${signalLog.join('') || '(nothing)'}`);
      }
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  say(`signal: ${signalUrl}/z2-${ROOM} (pid ${signal.pid})`);

  // 2. Two static servers = two ports = two ORIGINS, so the peers get separate
  //    storage and IndexedDB. Two browser contexts on top of that keep their
  //    sessions apart as well.
  const srvA = await serveSite();
  const srvB = await serveSite();
  cleanup.push(() => srvA.close());
  cleanup.push(() => srvB.close());

  let browser;
  try { browser = await playwright.chromium.launch(); }
  catch (e) { gate(`browser launch failed (${e.message}) — run: make netplay-e2e-setup`); }
  cleanup.push(() => browser.close());

  const romBytes = (await readFile(romPath)).toString('base64');
  const peers = [];
  for (const [name, srv] of [['A', srvA], ['B', srvB]]) {
    const context = await browser.newContext();
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(`console: ${m.text()}`); });
    // 'load', not 'networkidle': the page keeps a requestAnimationFrame loop
    // running from the first paint, so "no network for 500 ms" never settles.
    await page.goto(`${srv.origin}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.z2 !== undefined, null, { timeout: 30000 });
    await page.addScriptTag({ content: DRIVER });
    peers.push({ name, page, context, errors, origin: srv.origin });
  }
  const [A, B] = peers;
  say(`windows: A=${A.origin} B=${B.origin} (separate origins, separate contexts)`);

  // 3. ROM into both tabs through the real file input (hash-gated in-tab).
  for (const p of peers) {
    await p.page.evaluate(async (b64) => {
      const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const dt = new DataTransfer();
      dt.items.add(new File([bin], 'zelda2.nes'));
      const input = document.getElementById('romFile');
      input.files = dt.files;
      input.dispatchEvent(new Event('change', { bubbles: true }));
    }, romBytes);
    await p.page.waitForFunction(() => JSON.parse(window.z2.state()).romLoaded === true, null, { timeout: 30000 });
  }
  const supported = await Promise.all(peers.map((p) => p.page.evaluate(() => window.z2.ext.net.supported())));
  if (!supported.every(Boolean)) {
    gate('this site/pkg/ bundle has no WebRTC transport — rebuild it with ' +
      '`-- --features hd,netplay` (net_supported() is false)');
  }
  say('ROM accepted in both windows; both report net.supported() === true');

  // 4. CLAIM 0 — the live path, exactly as a person uses the page.
  const netStatus = (p) => p.page.evaluate(() => ({
    ns: JSON.parse(window.z2.ext.net.status()),
    text: document.getElementById('netStatus').textContent,
    emuFrame: JSON.parse(window.z2.state()).frame,
    manual: window.z2.ext.net.isManual(),
    hostDisabled: document.getElementById('netHost').disabled,
  }));
  const signalText = () => signalLog.join('');
  const disconnectsIn = (room) =>
    signalText().split(`disconnected room=z2-${room} `).length - 1;
  async function waitFor(what, pred, timeoutMs) {
    const t0 = Date.now();
    for (;;) {
      const v = await pred();
      if (v) return v;
      if (Date.now() - t0 > timeoutMs) fail(`timed out after ${timeoutMs} ms waiting for ${what}`);
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  async function fillPanel(p, room) {
    await p.page.fill('#netSignal', signalUrl);
    await p.page.fill('#netRoom', room);
    // This script proves LOCKSTEP (the native peer speaks it too); rollback
    // is the page default and has its own test, netplay-e2e-rollback.mjs.
    // Mode first: rollback disables delays above 3.
    await p.page.selectOption('#netMode', 'lockstep');
    await p.page.selectOption('#netDelay', String(DELAY)); // it is a <select>
  }
  const timelineText = (tl) => tl.map((e) => `+${e.ms}ms ${e.key} "${e.text}"`).join('\n      ');

  const liveRoom = `${ROOM}live`;
  for (const p of peers) {
    await fillPanel(p, liveRoom);
    const st = await netStatus(p);
    if (st.manual) fail(`window ${p.name} is in QA manual mode before the live connect`);
  }
  // The field may not exist in an older page; the live path is the same.
  const iceDefault = await A.page.evaluate(() => {
    const el = document.getElementById('netIce');
    return el ? el.value : '(no ICE field on this page)';
  });
  say(`live connect: room '${liveRoom}', ICE left at the page default: ${iceDefault}`);

  // 4a. Host, then Join, then time the whole connection through the live
  //     loops. The host's own wait for the other player is part of the record.
  const timeline = { A: [], B: [] };
  const framesWhileConnecting = { A: [], B: [] };
  const clickT = Date.now();
  let joinT = null;
  await A.page.click('#netHost');
  let both = null;
  for (;;) {
    const sts = await Promise.all(peers.map((p) => netStatus(p)));
    const ms = Date.now() - clickT;
    sts.forEach((st, i) => {
      const name = peers[i].name;
      if (!st.ns.active && joinT === null && name === 'B') return; // not clicked yet
      const key = `${st.ns.state}/${st.ns.link}`;
      const tl = timeline[name];
      if (!tl.length || tl[tl.length - 1].key !== key) tl.push({ ms, key, text: st.text });
      if (st.ns.active && !st.ns.started) framesWhileConnecting[name].push(st.emuFrame);
    });
    // Join once the host is in the room (or after 1.5 s, whatever it shows),
    // like a second person would.
    if (joinT === null && (sts[0].ns.link === 'waiting' || ms > 1500)) {
      await B.page.click('#netJoin');
      joinT = Date.now();
      continue;
    }
    if (joinT !== null && sts.every((st) => st.ns.started && st.ns.state === 'running')) {
      both = { sts, ms: Date.now() - joinT };
      break;
    }
    const sinceJoin = joinT === null ? 0 : Date.now() - joinT;
    const dead = sts.find((st, i) => (i === 0 || joinT !== null) &&
      (!st.ns.active || st.ns.state === 'closed' || st.ns.state === 'desynced'));
    if (dead || sinceJoin > CONNECT_BUDGET_MS) {
      fail(`live connect did not reach a running session within ${CONNECT_BUDGET_MS} ms of the Join ` +
        `click (after ${sinceJoin} ms: A=${sts[0].ns.state}/${sts[0].ns.link} ` +
        `B=${sts[1].ns.state}/${sts[1].ns.link}; A reason=${sts[0].ns.closeReason} ` +
        `B reason=${sts[1].ns.closeReason}).\n    timeline A (ms since Host click):\n      ` +
        `${timelineText(timeline.A)}\n    timeline B:\n      ${timelineText(timeline.B)}` +
        `\n    signal log:\n${signalText()}`);
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  for (const n of ['A', 'B']) {
    console.log(`    ${n} (ms since Host click): ` + timeline[n].map((e) => `+${e.ms} ${e.key}`).join(' -> '));
  }
  say(`PROVED: live connect through the page's own loop in ${both.ms} ms from the Join click ` +
    `(budget ${CONNECT_BUDGET_MS} ms)`);
  for (const n of ['A', 'B']) {
    const f = framesWhileConnecting[n];
    // A session start rebuilds the game at frame 0, so look for any rise.
    const rose = f.some((v, i) => i > 0 && v > f[i - 1]);
    if (f.length > 2 && !rose) {
      fail(`window ${n}'s game froze while connecting: frames ${f.join(',')}`);
    }
  }
  const shown = [...timeline.A, ...timeline.B].map((e) => e.text).join(' | ');
  if (!/[1-4]\/4/.test(shown)) {
    fail(`no numbered connect stage was ever shown in the status line: ${shown}`);
  }
  say(`the games kept running while connecting (A frames ${framesWhileConnecting.A[0]}..` +
    `${framesWhileConnecting.A.at(-1)}, B ${framesWhileConnecting.B[0]}..${framesWhileConnecting.B.at(-1)})`);

  // 4b. The running session advances under the live loops alone.
  const live0 = both.sts.map((st) => st.ns.frame);
  await A.page.waitForTimeout(2000);
  const live1 = await Promise.all(peers.map(async (p) => (await netStatus(p)).ns));
  live1.forEach((ns, i) => {
    if (ns.state !== 'running' && ns.state !== 'stalled') {
      fail(`window ${peers[i].name} left the live session: ${ns.state} (${ns.closeReason})`);
    }
    if (!(ns.frame >= live0[i] + 60)) {
      fail(`window ${peers[i].name}'s live session barely advanced: ${live0[i]} -> ${ns.frame} in 2 s`);
    }
  });
  say(`live session runs: A ${live0[0]}->${live1[0].frame}, B ${live0[1]}->${live1[1].frame} in 2 s`);

  // 4c. Leave (running) on both, through the button; the room empties.
  for (const p of peers) await p.page.click('#netLeave');
  await waitFor('both leaves to reach the signal server', async () => disconnectsIn(liveRoom) >= 2, 5000);
  for (const p of peers) {
    const st = await netStatus(p);
    if (st.ns.active || st.hostDisabled) {
      fail(`window ${p.name} did not return to idle after Leave: active=${st.ns.active} text='${st.text}'`);
    }
  }
  say('Leave while running: both windows idle, the signal server saw both disconnect');

  // 4d. Leave while still waiting for the other player cancels cleanly too.
  const cancelRoom = `${ROOM}cancel`;
  await fillPanel(A, cancelRoom);
  await A.page.click('#netHost');
  const waiting = await waitFor('the host to reach the waiting stage again', async () => {
    const st = await netStatus(A);
    return st.ns.link === 'waiting' ? st : null;
  }, CONNECT_BUDGET_MS);
  if (!/2\/4/.test(waiting.text)) fail(`the waiting stage is not shown: '${waiting.text}'`);
  await A.page.click('#netLeave');
  await waitFor('the cancelled host to leave the room', async () => disconnectsIn(cancelRoom) >= 1, 5000);
  say(`Leave while waiting ("${waiting.text}"): cancelled, the room is free again`);

  // 4e. Browser <-> desktop: a native peer (the desktop app's transport) hosts,
  //     window B joins through the live page.
  if (haveNetPeer) {
    const deskRoom = `${ROOM}desk`;
    const trapset = await B.page.evaluate(() => window.z2.ext.trapsetId());
    const deskLog = [];
    const desk = spawn(netPeerBin, ['--signal', signalUrl, '--room', deskRoom, '--trapset', trapset,
      '--frames', '240', '--timeout-ms', String(CONNECT_BUDGET_MS + 20000)],
    { stdio: ['ignore', 'pipe', 'pipe'] });
    let deskExit = null;
    desk.stdout.on('data', (b) => deskLog.push(String(b)));
    desk.stderr.on('data', (b) => deskLog.push(String(b)));
    desk.on('exit', (c) => { deskExit = c; });
    cleanup.push(() => { if (deskExit === null) desk.kill('SIGKILL'); });
    await waitFor('the native peer to wait in its room',
      async () => deskLog.join('').includes('stage waiting') || deskExit !== null, 10000);
    await fillPanel(B, deskRoom);
    await B.page.click('#netJoin');
    const deskT = Date.now();
    const deskUp = await waitFor(`window B to run a session with the native peer`, async () => {
      const st = await netStatus(B);
      if (!st.ns.active || st.ns.state === 'closed') {
        fail(`browser <-> native connect failed: ${st.text}\n    native peer:\n${deskLog.join('')}`);
      }
      if (Date.now() - deskT > CONNECT_BUDGET_MS) {
        fail(`browser <-> native connect took over ${CONNECT_BUDGET_MS} ms (${st.text})\n` +
          `    native peer:\n${deskLog.join('')}`);
      }
      return st.ns.started && st.ns.state === 'running' ? Date.now() - deskT : null;
    }, CONNECT_BUDGET_MS + 1000);
    await waitFor('the native peer to step 240 session frames', async () => deskExit !== null, 30000);
    if (deskExit !== 0) fail(`native peer exited ${deskExit}:\n${deskLog.join('')}`);
    const ok = deskLog.join('').match(/NET-PEER-OK.*/);
    await B.page.click('#netLeave');
    say(`PROVED: browser <-> native peer connected in ${deskUp} ms and ran (${ok ? ok[0] : '?'})`);
  } else {
    say(`browser <-> native check skipped: no '${netPeerBin}' (cargo build --release -p z2-net ` +
      '--features matchbox --example net_peer)');
  }

  // 5. CLAIMS 1 and 2 need the movie's pad on every session frame from 0, so
  //    the in-page driver steps this second session (see the header). The
  //    live connection path was proved above.
  for (const p of peers) await p.page.evaluate(() => window.z2.ext.net.manual(true));
  for (const [p, btn] of [[A, '#netHost'], [B, '#netJoin']]) {
    await fillPanel(p, ROOM);
    await p.page.click(btn);
  }
  const handshake = await Promise.all(peers.map((p) =>
    p.page.evaluate((ms) => window.__z2e2e.until(['running', 'stalled'], ms), CONNECT_BUDGET_MS)));
  handshake.forEach((ns, i) => {
    if (ns.__timeout) {
      fail(`window ${peers[i].name} never reached a running session within ${CONNECT_BUDGET_MS} ms: ` +
        `state=${ns.state} link=${ns.link} started=${ns.started} error=${ns.error} ` +
        `closeReason=${ns.closeReason} — signal log:\n${signalLog.join('')}`);
    }
  });
  say(`session up: A=${handshake[0].role}/${handshake[0].state} ` +
    `B=${handshake[1].role}/${handshake[1].state} delay=${handshake[0].delay}`);
  if (handshake[0].role !== 'host' || handshake[1].role !== 'guest') {
    fail(`roles are wrong: A=${handshake[0].role} B=${handshake[1].role} (host must be player 1)`);
  }

  // 6. Walk into a side-view area, over the wire, frame-exactly. The host
  //    replays the movie's player-1 column; the guest holds nothing.
  const warm = await Promise.all([
    A.page.evaluate((a) => window.__z2e2e.run(a),
      { target: WARMUP_CAP, padsB64, delay: DELAY, stopOn: 'p2', timeoutMs: 20000 }),
    B.page.evaluate((a) => window.__z2e2e.run(a),
      { target: WARMUP_CAP, padsB64: null, delay: DELAY, stopOn: 'p2', timeoutMs: 20000 }),
  ]);
  warm.forEach((r, i) => {
    if (r.reason !== 'p2' && r.reason !== 'target') {
      fail(`window ${peers[i].name} stopped walking in (${r.reason}) at frame ${r.ns.frame}: ` +
        `state=${r.ns.state} error=${r.ns.error} closeReason=${r.ns.closeReason}`);
    }
  });
  let probes = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2e2e.probe())));
  say(`walked in: A f=${probes[0].frame} B f=${probes[1].frame}; ` +
    `P2 live A=${!!probes[0].p2} B=${!!probes[1].p2}`);
  if (!probes[0].p2 || !probes[1].p2) {
    fail('player 2 never became live in a side-view area within ' +
      `${WARMUP_CAP} frames (A=${JSON.stringify(probes[0].p2)} B=${JSON.stringify(probes[1].p2)}) — ` +
      'no observable state for the guest\'s pad to move, so the input-crossing ' +
      'assertion cannot be made. Try a longer Z2_E2E_WARMUP or another Z2_MOVIE.');
  }

  // 7. THE ASSERTION. Hold a real key in one window; read the OTHER window's
  //    game state. `net.step()` with no pad samples the live keyboard through
  //    `pollInput()`, the same function the rAF loop uses, so these are real
  //    key presses travelling the real input path and the real data channel.
  //
  //    Both peers must keep stepping while the key is held, so both drivers run
  //    concurrently and only the pressing side has a key down.
  const crossings = [];
  async function hold({ presser, observer, key, who }) {
    await presser.page.keyboard.down(key);
    const before = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2e2e.probe())));
    const target = before[0].frame + HOLD_FRAMES;
    const r = await Promise.all(peers.map((p) => p.page.evaluate((a) => window.__z2e2e.run(a),
      { target, padsB64: null, delay: DELAY, stopOn: null, timeoutMs: 20000 })));
    await presser.page.keyboard.up(key);
    r.forEach((x, i) => {
      if (x.reason !== 'target') {
        fail(`window ${peers[i].name} stopped while ${presser.name} held ${key} ` +
          `(${x.reason}) at frame ${x.ns.frame}: state=${x.ns.state} error=${x.ns.error}`);
      }
    });
    const after = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2e2e.probe())));
    const oi = peers.indexOf(observer);
    const pick = (probe) => (who === 'p2' ? probe.p2 : probe.p1);
    const d = worldX(pick(after[oi])) - worldX(pick(before[oi]));
    crossings.push({
      presser: presser.name, observer: observer.name, key, who,
      from: worldX(pick(before[oi])), to: worldX(pick(after[oi])), delta: d,
      frames: after[oi].frame - before[oi].frame,
    });
    return d;
  }

  // B's pad is player 2 (the guest). Observe it in A's own state.
  const p2Right = await hold({ presser: B, observer: A, key: 'ArrowRight', who: 'p2' });
  const p2Left = await hold({ presser: B, observer: A, key: 'ArrowLeft', who: 'p2' });
  // A's pad is player 1 (the host). Observe it in B's own state.
  const p1Right = await hold({ presser: A, observer: B, key: 'ArrowRight', who: 'p1' });
  const p1Left = await hold({ presser: A, observer: B, key: 'ArrowLeft', who: 'p1' });

  console.log('\n  input crossing the wire (world X pixels, observed in the OTHER window):');
  for (const c of crossings) {
    console.log(`    ${c.presser} holds ${c.key.padEnd(10)} -> ${c.observer} sees ` +
      `${c.who} ${String(c.from).padStart(5)} -> ${String(c.to).padStart(5)} ` +
      `(delta ${c.delta > 0 ? '+' : ''}${c.delta} over ${c.frames} frames)`);
  }
  console.log('');

  const check = (right, left, who, presser, observer) => {
    if (!(right >= MIN_MOVE)) {
      fail(`${presser} held Right but ${observer} saw ${who} move ${right} px ` +
        `(need >= +${MIN_MOVE}): the guest's pad is not reaching the other peer's game state`);
    }
    if (!(left <= -MIN_MOVE)) {
      fail(`${presser} held Left but ${observer} saw ${who} move ${left} px ` +
        `(need <= -${MIN_MOVE})`);
    }
  };
  check(p2Right, p2Left, 'player 2', 'B', 'A');
  check(p1Right, p1Left, 'player 1', 'A', 'B');
  say('PROVED: a key held in one window moves the matching player in the other ' +
    'window\'s state, and reverses when the opposite key is held');

  // 8. LOCKSTEP. Run on to LOCKSTEP_FRAMES with no input, then compare.
  const run = await Promise.all(peers.map((p) => p.page.evaluate((a) => window.__z2e2e.run(a),
    { target: LOCKSTEP_FRAMES, padsB64: null, delay: DELAY, stopOn: null, timeoutMs: 30000 })));
  run.forEach((r, i) => {
    if (r.reason !== 'target') {
      fail(`window ${peers[i].name} could not reach frame ${LOCKSTEP_FRAMES} (${r.reason}) ` +
        `at frame ${r.ns.frame}: state=${r.ns.state} error=${r.ns.error}`);
    }
  });

  // Both peers are at the target frame but may have stepped a different number
  // of frames past it in their last tick, so settle them onto one frame before
  // hashing: a hash taken at different frames proves nothing either way.
  const settle = Math.max(...(await Promise.all(peers.map(async (p) =>
    (await p.page.evaluate(() => window.__z2e2e.probe())).frame))));
  await Promise.all(peers.map((p) => p.page.evaluate((a) => window.__z2e2e.run(a),
    { target: settle, padsB64: null, delay: DELAY, stopOn: null, timeoutMs: 20000 })));
  probes = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2e2e.probe())));

  // 9. Hand the session back to the page's own requestAnimationFrame loop for a
  //    moment, so the live play path is proved to pump it too — not only the
  //    QA driver.
  for (const p of peers) await p.page.evaluate(() => window.z2.ext.net.manual(false));
  const liveBefore = probes.map((x) => x.frame);
  await A.page.waitForTimeout(2500);
  const liveAfter = await Promise.all(peers.map(async (p) =>
    (await p.page.evaluate(() => window.__z2e2e.probe())).frame));
  for (const p of peers) await p.page.evaluate(() => window.z2.ext.net.manual(true));
  if (!liveAfter.every((f, i) => f > liveBefore[i])) {
    fail(`the page's own rAF loop did not advance the session: ` +
      `${liveBefore.join('/')} -> ${liveAfter.join('/')}`);
  }
  say(`live rAF loop also pumps the session: A ${liveBefore[0]}->${liveAfter[0]}, ` +
    `B ${liveBefore[1]}->${liveAfter[1]} in 2.5 s`);

  // Re-settle after the free-running segment and take the verdict sample.
  const finalFrame = Math.max(...liveAfter) + 30;
  await Promise.all(peers.map((p) => p.page.evaluate((a) => window.__z2e2e.run(a),
    { target: finalFrame, padsB64: null, delay: DELAY, stopOn: null, timeoutMs: 20000 })));
  const settle2 = Math.max(...(await Promise.all(peers.map(async (p) =>
    (await p.page.evaluate(() => window.__z2e2e.probe())).frame))));
  await Promise.all(peers.map((p) => p.page.evaluate((a) => window.__z2e2e.run(a),
    { target: settle2, padsB64: null, delay: DELAY, stopOn: null, timeoutMs: 20000 })));
  probes = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2e2e.probe())));

  console.log('  lockstep verdict:');
  for (const [i, x] of probes.entries()) {
    console.log(`    window ${peers[i].name} (${x.role}): frame=${x.frame} ` +
      `hash=${x.hash} hashesOk=${x.hashesOk} state=${x.state} ` +
      `p1=(${x.p1.page}:${x.p1.x},${x.p1.y}) ` +
      `p2=${x.p2 ? `(${x.p2.page}:${x.p2.x},${x.p2.y})` : 'hidden'}`);
  }
  console.log('');

  if (probes[0].frame !== probes[1].frame) {
    fail(`the two windows are on different frames: A=${probes[0].frame} B=${probes[1].frame}`);
  }
  if (probes[0].hash !== probes[1].hash) {
    fail(`STATE HASHES DIFFER at frame ${probes[0].frame}: A=${probes[0].hash} B=${probes[1].hash} ` +
      '— the two peers are not running the same game');
  }
  if (!/^[0-9a-f]{16}$/.test(probes[0].hash)) fail(`hash is not 16 hex chars: ${probes[0].hash}`);
  for (const [i, x] of probes.entries()) {
    if (x.state !== 'running') {
      fail(`window ${peers[i].name} is '${x.state}', not running ` +
        `(error=${x.error} closeReason=${x.closeReason})`);
    }
    if (!(x.hashesOk > 0)) {
      fail(`window ${peers[i].name} compared 0 state hashes with its peer (hashesOk=${x.hashesOk}) ` +
        '— desync detection never actually ran, so "no desync" would mean nothing');
    }
    if (x.error !== null) fail(`window ${peers[i].name} reported an error: ${x.error}`);
  }

  // 10. Screenshots, outside the repository (frames are ROM-derived art).
  const shotDir = await mkdtemp(join(tmpdir(), 'z2rs-netplay-e2e-'));
  const shots = [];
  for (const [i, p] of peers.entries()) {
    const url = await p.page.evaluate(() => window.z2.screenshot());
    const out = join(shotDir, `window-${p.name}-${probes[i].role}-f${probes[i].frame}.png`);
    await writeFile(out, Buffer.from(url.split(',')[1], 'base64'));
    shots.push(out);
  }
  await writeFile(join(shotDir, 'summary.json'), JSON.stringify({
    room: ROOM, signalUrl, delay: DELAY, movie: moviePath, crossings, probes,
  }, null, 2));
  shots.push(join(shotDir, 'summary.json'));

  const pageErrors = peers.flatMap((p) => p.errors.map((e) => `${p.name}: ${e}`));
  if (pageErrors.length) fail(`page errors: ${pageErrors.join(' | ').slice(0, 600)}`);

  console.log('NETPLAY-E2E-OK');
  console.log(`  input crossed the wire both ways: P2 ${p2Right > 0 ? '+' : ''}${p2Right}/` +
    `${p2Left} px seen by A, P1 ${p1Right > 0 ? '+' : ''}${p1Right}/${p1Left} px seen by B`);
  console.log(`  lockstep at frame ${probes[0].frame}: both windows hash ${probes[0].hash}, ` +
    `${probes[0].hashesOk} hash comparisons matched, 0 desyncs`);
  for (const s of shots) console.log(`  evidence: ${s}`);
} catch (e) {
  if (e.gated) {
    console.error(`NETPLAY-E2E-GATED: ${e.message}`);
    exitCode = 2;
  } else {
    console.error(`NETPLAY-E2E-FAIL: ${failed || e.message}`);
    if (!failed) console.error(e.stack);
    exitCode = 1;
  }
} finally {
  for (const c of cleanup.reverse()) { try { await c(); } catch { /* best effort */ } }
}
process.exit(exitCode);
