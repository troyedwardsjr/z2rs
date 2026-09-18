// End-to-end proof that ROLLBACK netplay works between two browser windows.
//
//   make netplay-e2e-rollback
//   node crates/z2-web/site/netplay-e2e-rollback.mjs
//
// Two windows on one machine have almost no latency, so a rollback session
// between them would never mispredict. Both pages therefore turn on the QA
// hook `z2.ext.net.simulate({ latencyMs, jitterMs, lossPct })`, which delays
// and drops packets inside the page's own transport wrapper. The claims:
//
//   0. WASM COST. One emulated frame, one save, one load, and a worst-case
//      rollback tick at prediction depths 0, 4 and 8, timed in the page with
//      performance.now() (z2.ext.perf.rollback). Reported, and the tick at the
//      page's default window must fit in one 16.7 ms frame.
//   1. THE LIVE PATH CONNECTS IN ROLLBACK MODE. The panel's mode selector
//      defaults to rollback; Host and Join are clicked with the rAF loop
//      pumping the session under simulated latency and loss, and both windows
//      reach a running rollback session that keeps advancing. Then Leave.
//   2. ROLLBACKS HAPPEN. A second session is driven frame by frame (the
//      movie's player-1 pads have to land on exact frames to walk into a
//      side-view area) and both windows must report rollbacks > 0.
//   3. LOCAL INPUT FEELS INSTANT. A key pressed in a window reaches that
//      window's own simulation exactly `input_delay` frames later, whatever
//      the network does, and moves its own player right after.
//   4. INPUT CROSSES THE WIRE both ways (sign reversal seen in the other
//      window's state, as in the lockstep test).
//   5. CONFIRMED STATE AGREES. Both windows log a state hash every 15 frames;
//      every frame both logged as final hashes the same, the protocol's own
//      checksums matched (hashesOk > 0) and there were zero desyncs.
//
// Exit codes: 0 pass, 1 assertion failure, 2 environment-gated.

import { mkdtemp, readFile, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { SITE, serveSite } from './qa-server.mjs';
import { REPO, loadRomInto, parseFm2Pads, startSignal, worldX } from './netplay-lib.mjs';

const say = (msg) => console.log(`  ${msg}`);
const cleanup = [];
let failed = null;
const fail = (msg) => { if (!failed) failed = msg; throw new Error(msg); };
let armed = false;
const gate = (msg) => {
  if (!armed) { console.error(`NETPLAY-E2E-ROLLBACK-GATED: ${msg}`); process.exit(2); }
  throw Object.assign(new Error(msg), { gated: true });
};

try { await stat(join(SITE, 'pkg', 'z2_web.js')); }
catch { gate('site/pkg/ missing — run: make netplay-e2e-rollback (builds it with --features hd,netplay)'); }
const romPath = process.env.Z2_ROM;
if (!romPath) gate('$Z2_ROM is not set (point it at your own Zelda II (USA) dump; see LEGAL.md)');
try { await stat(romPath); } catch { gate(`$Z2_ROM='${romPath}' is not a file`); }
const corpus = process.env.Z2_CORPUS || join(REPO, '..', 'z2-corpus');
const moviePath = process.env.Z2_MOVIE || join(corpus, 'movies', 'warpless.fm2');
try { await stat(moviePath); }
catch { gate(`no movie at '${moviePath}' — set Z2_CORPUS or Z2_MOVIE=/path/to/track.fm2 (see README.md)`); }
let playwright;
try { playwright = await import('playwright'); }
catch { gate('playwright module not installed — run: make netplay-e2e-setup'); }

// --- knobs -----------------------------------------------------------------
const DELAY = Number(process.env.Z2_NET_DELAY || 2);
const SIM = {
  latencyMs: Number(process.env.Z2_SIM_LATENCY_MS || 50),
  jitterMs: Number(process.env.Z2_SIM_JITTER_MS || 20),
  lossPct: Number(process.env.Z2_SIM_LOSS_PCT || 5),
};
const ROOM = `rb${Date.now().toString(36)}`;
const WARMUP_CAP = Number(process.env.Z2_E2E_WARMUP || 4000);
const HOLD_FRAMES = Number(process.env.Z2_E2E_HOLD || 90);
const FINAL_FRAMES = Number(process.env.Z2_E2E_FRAMES || 3000);
const CONNECT_BUDGET_MS = Number(process.env.Z2_E2E_CONNECT_MS || 10000);
const MIN_MOVE = 3;
// Frames Zelda II itself takes between a pad reaching the simulation and
// Link's pixel X changing (he accelerates in sub-pixels first; measured 4
// from a standstill). Network conditions must not add to it.
const GAME_REACTION_MAX = 4;
const REST_FRAMES = 90;
const BUDGET_MS = 1000 / 60.0988;

// In-page driver: one rollback tick per iteration, 16 ms of session clock per
// tick, so simulated latency is measured in session time whatever the host
// machine's speed.
const DRIVER = `
window.__z2rb = {
  who(which) {
    if (which === 'p1') { const l = JSON.parse(window.z2.facts()).link; return { page: l.page, x: l.x }; }
    const c = JSON.parse(window.z2.ext.coopStatus() || 'null');
    return c && c.active ? { page: c.p2Page, x: c.p2X } : null;
  },
  async until(states, timeoutMs) {
    const t0 = Date.now();
    for (;;) {
      const ns = JSON.parse(window.z2.ext.net.poll(16));
      if (states.includes(ns.state) && ns.started) {
        // Keep pumping a little: packets held by the simulated link (the
        // host's Start, say) only leave while the page polls.
        const t1 = Date.now();
        while (Date.now() - t1 < 1500) {
          window.z2.ext.net.poll(16);
          await new Promise((r) => setTimeout(r, 16));
        }
        return JSON.parse(window.z2.ext.net.status());
      }
      if (Date.now() - t0 > timeoutMs) return { ...ns, __timeout: true };
      await new Promise((r) => setTimeout(r, 4));
    }
  },
  async run({ target, padsB64, delay, stopOn, timeoutMs, watch }) {
    const pads = padsB64 ? Uint8Array.from(atob(padsB64), (c) => c.charCodeAt(0)) : null;
    const wx = (p) => (p ? p.page * 256 + p.x : null);
    const t0 = Date.now();
    let last = Date.now();
    let press = null, applied = null, moved = null, x0 = null;
    for (;;) {
      const ns = JSON.parse(window.z2.ext.net.poll(16));
      if (ns.state === 'desynced' || ns.state === 'closed') return { ns, reason: ns.state };
      if (ns.frame >= target && (!watch || moved !== null)) {
        // Release what the simulated link still holds before handing over,
        // or the peer waits for pads that never leave this page.
        for (let i = 0; i < 12; i++) {
          window.z2.ext.net.poll(16);
          await new Promise((r) => setTimeout(r, 2));
        }
        return { ns, reason: 'target', press, applied, moved };
      }
      if (stopOn === 'p2') {
        const c = JSON.parse(window.z2.ext.coopStatus() || 'null');
        if (c && c.active) return { ns, reason: 'p2' };
      }
      if (watch && press === null) { press = ns.frame; x0 = wx(this.who(watch.who)); }
      const before = ns.frame;
      const pad = pads ? (pads[ns.frame + delay] || 0) : null;
      window.z2.ext.net.step(pad, 1);
      const after = JSON.parse(window.z2.ext.net.status());
      if (watch) {
        if (applied === null && after.lastLocalFrame >= press && (after.lastLocalPad & watch.bit)) {
          applied = after.lastLocalFrame;
        }
        if (moved === null && after.frame > before) {
          // First simulated frame on which the player moved the pressed way.
          const x = wx(this.who(watch.who));
          if (x !== null && x0 !== null && (x - x0) * watch.dir > 0) moved = after.frame - 1;
          if (x !== null) x0 = x;
        }
      }
      if (after.frame > before) last = Date.now();
      if (Date.now() - last > timeoutMs) return { ns: after, reason: 'stalled' };
      if (Date.now() - t0 > timeoutMs * 20) return { ns: after, reason: 'timeout' };
      await new Promise((r) => setTimeout(r, after.frame > before ? 0 : 2));
    }
  },
  probe() {
    const ns = JSON.parse(window.z2.ext.net.status());
    const facts = JSON.parse(window.z2.facts());
    const coop = JSON.parse(window.z2.ext.coopStatus() || 'null');
    return {
      ns,
      p1: { page: facts.link.page, x: facts.link.x, y: facts.link.y },
      p2: coop && coop.active ? { page: coop.p2Page, x: coop.p2X, y: coop.p2Y } : null,
    };
  },
};
`;

let exitCode = 0;
try {
  armed = true;
  const pads = parseFm2Pads(await readFile(moviePath, 'utf8'));
  if (pads.length < 600) fail(`movie '${moviePath}' parsed to only ${pads.length} frames`);
  const padsB64 = Buffer.from(pads).toString('base64');

  let signal;
  try { signal = await startSignal(); }
  catch (e) { gate(`${e.message} — run: cargo build --release -p z2-signal`); }
  cleanup.push(() => signal.kill());
  say(`signal: ${signal.url} (pid ${signal.proc.pid})`);

  const srvA = await serveSite();
  const srvB = await serveSite();
  cleanup.push(() => srvA.close());
  cleanup.push(() => srvB.close());
  let browser;
  try { browser = await playwright.chromium.launch(); }
  catch (e) { gate(`browser launch failed (${e.message}) — run: make netplay-e2e-setup`); }
  cleanup.push(() => browser.close());

  const romB64 = (await readFile(romPath)).toString('base64');
  const peers = [];
  for (const [name, srv] of [['A', srvA], ['B', srvB]]) {
    const context = await browser.newContext();
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e)));
    page.on('console', (m) => { if (m.type() === 'error') errors.push(`console: ${m.text()}`); });
    await page.goto(`${srv.origin}/`, { waitUntil: 'load' });
    await page.waitForFunction(() => window.z2 !== undefined, null, { timeout: 30000 });
    await page.addScriptTag({ content: DRIVER });
    await loadRomInto(page, romB64);
    peers.push({ name, page, errors });
  }
  const [A, B] = peers;
  if (!(await A.page.evaluate(() => window.z2.ext.net.supported()))) {
    gate('this site/pkg/ bundle has no WebRTC transport — rebuild with -- --features hd,netplay');
  }

  // 0. WASM cost, measured in the page.
  await A.page.waitForTimeout(500);
  const perf = await A.page.evaluate(() => window.z2.ext.perf.rollback({ iterations: 120, depths: [0, 4, 8] }));
  const defaultWindow = await A.page.evaluate(() => window.z2.ext.net.mode().maxPrediction);
  const fmt = (s) => `mean ${s.mean.toFixed(3)} ms, p95 ${s.p95.toFixed(3)} ms, max ${s.max.toFixed(3)} ms`;
  console.log('\n  wasm rollback cost (headless Chromium, performance.now(), ' +
    `${perf.iterations} iterations each):`);
  console.log(`    one emulated frame : ${fmt(perf.frame)}`);
  console.log(`    one save           : ${fmt(perf.save)}`);
  console.log(`    one load           : ${fmt(perf.load)}`);
  for (const d of Object.keys(perf.tick)) console.log(`    worst tick depth ${String(d).padStart(2)}: ${fmt(perf.tick[d])}`);
  console.log(`    page default prediction window: ${defaultWindow}\n`);
  const nearest = Object.keys(perf.tick).map(Number).filter((d) => d >= defaultWindow).sort((a, b) => a - b)[0];
  if (nearest !== undefined && perf.tick[nearest].p95 > BUDGET_MS) {
    fail(`a worst-case tick at depth ${nearest} (p95 ${perf.tick[nearest].p95.toFixed(2)} ms) does not fit ` +
      `the ${BUDGET_MS.toFixed(1)} ms frame budget`);
  }

  // 1. Live connect, rollback mode as the panel defaults it, simulated network.
  for (const p of peers) {
    const mode = await p.page.evaluate(() => document.getElementById('netMode').value);
    if (mode !== 'rollback') fail(`window ${p.name}'s mode selector defaults to '${mode}', not rollback`);
    await p.page.evaluate((s) => window.z2.ext.net.simulate(s), SIM);
  }
  say(`simulated network in both pages: ${JSON.stringify(SIM)} (each direction)`);
  const status = (p) => p.page.evaluate(() => JSON.parse(window.z2.ext.net.status()));
  async function fillPanel(p, room) {
    await p.page.fill('#netSignal', signal.url);
    await p.page.fill('#netRoom', room);
    await p.page.selectOption('#netDelay', String(DELAY));
  }
  const liveRoom = `${ROOM}live`;
  for (const p of peers) await fillPanel(p, liveRoom);
  await A.page.click('#netHost');
  await A.page.waitForTimeout(300);
  await B.page.click('#netJoin');
  const joinT = Date.now();
  for (;;) {
    const sts = await Promise.all(peers.map(status));
    if (sts.every((s) => s.started && (s.state === 'running' || s.state === 'stalled'))) {
      sts.forEach((s, i) => {
        if (s.mode !== 'rollback') fail(`window ${peers[i].name} runs '${s.mode}', not rollback`);
      });
      say(`PROVED: live rollback session up in ${Date.now() - joinT} ms from the Join click`);
      break;
    }
    const dead = sts.find((s) => !s.active || s.state === 'closed' || s.state === 'desynced');
    if (dead || Date.now() - joinT > CONNECT_BUDGET_MS) {
      fail(`live rollback connect failed: A=${JSON.stringify(sts[0])} B=${JSON.stringify(sts[1])}`);
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  const l0 = await Promise.all(peers.map(status));
  await A.page.waitForTimeout(2500);
  const l1 = await Promise.all(peers.map(status));
  l1.forEach((s, i) => {
    if (!(s.frame >= l0[i].frame + 60)) fail(`window ${peers[i].name}'s live rollback session barely advanced: ${l0[i].frame} -> ${s.frame}`);
    if (s.rttMs === null || s.rttMs < SIM.latencyMs) fail(`window ${peers[i].name} rtt ${s.rttMs} ms does not reflect the simulated ${SIM.latencyMs} ms`);
  });
  const liveText = await A.page.evaluate(() => document.getElementById('netStatus').textContent);
  if (!/rollback/.test(liveText) || !/rtt/.test(liveText)) fail(`status line lacks mode/rtt: '${liveText}'`);
  say(`live loop runs it: A ${l0[0].frame}->${l1[0].frame}, B ${l0[1].frame}->${l1[1].frame} in 2.5 s; ` +
    `status: "${liveText}"`);
  for (const p of peers) await p.page.click('#netLeave');

  // 2. Frame-driven session: walk in with the movie over a lossy, late link.
  for (const p of peers) await p.page.evaluate(() => window.z2.ext.net.manual(true));
  for (const [p, btn] of [[A, '#netHost'], [B, '#netJoin']]) {
    await fillPanel(p, ROOM);
    await p.page.click(btn);
  }
  const hs = await Promise.all(peers.map((p) =>
    p.page.evaluate((ms) => window.__z2rb.until(['running', 'stalled'], ms), CONNECT_BUDGET_MS)));
  hs.forEach((ns, i) => { if (ns.__timeout) fail(`window ${peers[i].name} never ran: ${JSON.stringify(ns)}`); });
  if (hs[0].role !== 'host' || hs[1].role !== 'guest') fail(`roles wrong: ${hs[0].role}/${hs[1].role}`);
  const delay = hs[0].delay;
  say(`session up: mode=${hs[0].mode} delay=${delay} window=${hs[0].maxPrediction}`);

  const runBoth = (argsA, argsB) => Promise.all([
    A.page.evaluate((a) => window.__z2rb.run(a), argsA),
    B.page.evaluate((a) => window.__z2rb.run(a), argsB),
  ]);
  const base = { delay, timeoutMs: 20000 };
  const warm = await runBoth(
    { ...base, target: WARMUP_CAP, padsB64, stopOn: 'p2' },
    { ...base, target: WARMUP_CAP, padsB64: null, stopOn: 'p2' },
  );
  warm.forEach((r, i) => {
    if (r.reason !== 'p2' && r.reason !== 'target') fail(`window ${peers[i].name} stopped walking in: ${r.reason} ${JSON.stringify(r.ns)}`);
  });
  // The guest may see player 2 a few frames before the host; let both settle.
  const settle = Math.max(...warm.map((r) => r.ns.frame)) + 30;
  await runBoth({ ...base, target: settle, padsB64: null }, { ...base, target: settle, padsB64: null });
  let probes = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2rb.probe())));
  if (!probes[0].p2 || !probes[1].p2) fail(`player 2 never became live (A=${JSON.stringify(probes[0].p2)} B=${JSON.stringify(probes[1].p2)})`);
  say(`walked in over the simulated link: A f=${probes[0].ns.frame} rollbacks=${probes[0].ns.rollbacks}, ` +
    `B f=${probes[1].ns.frame} rollbacks=${probes[1].ns.rollbacks}`);

  // 3 + 4. Hold keys; measure local latency in the pressing window and the
  //        movement in the other window.
  const RIGHT = 1 << 7;
  const LEFT = 1 << 6;
  const crossings = [];
  const latencies = [];
  async function hold({ presser, observer, key, bit, who }) {
    const oi = peers.indexOf(observer);
    // Let both players come to rest first, so the reaction measured below is
    // Link starting to walk, not the end of a previous slide.
    const rest = Math.max(...(await Promise.all(peers.map(status))).map((s) => s.frame)) + REST_FRAMES;
    await runBoth({ ...base, target: rest, padsB64: null }, { ...base, target: rest, padsB64: null });
    const before = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2rb.probe())));
    const target = Math.max(before[0].ns.frame, before[1].ns.frame) + HOLD_FRAMES;
    const dir = bit === RIGHT ? 1 : -1;
    await presser.page.keyboard.down(key);
    const r = await Promise.all(peers.map((p) => p.page.evaluate((a) => window.__z2rb.run(a),
      { ...base, target, padsB64: null, watch: p === presser ? { who, bit, dir } : null })));
    await presser.page.keyboard.up(key);
    r.forEach((x, i) => { if (x.reason !== 'target') fail(`window ${peers[i].name} stopped while ${presser.name} held ${key}: ${x.reason}`); });
    const pr = r[peers.indexOf(presser)];
    latencies.push({ presser: presser.name, key, press: pr.press, applied: pr.applied, moved: pr.moved,
      appliedAfter: pr.applied - pr.press, movedAfter: pr.moved - pr.press });
    // Run past the hold with no keys so the observer's view of it is confirmed.
    const t2 = target + 20;
    await runBoth({ ...base, target: t2, padsB64: null }, { ...base, target: t2, padsB64: null });
    const after = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2rb.probe())));
    const pick = (x) => (who === 'p2' ? x.p2 : x.p1);
    const d = worldX(pick(after[oi])) - worldX(pick(before[oi]));
    crossings.push({ presser: presser.name, observer: observer.name, key, who, delta: d });
    return d;
  }
  const p2R = await hold({ presser: B, observer: A, key: 'ArrowRight', bit: RIGHT, who: 'p2' });
  const p2L = await hold({ presser: B, observer: A, key: 'ArrowLeft', bit: LEFT, who: 'p2' });
  const p1R = await hold({ presser: A, observer: B, key: 'ArrowRight', bit: RIGHT, who: 'p1' });
  const p1L = await hold({ presser: A, observer: B, key: 'ArrowLeft', bit: LEFT, who: 'p1' });

  console.log('\n  local input latency (frames after the key was down, in the pressing window):');
  for (const l of latencies) {
    console.log(`    ${l.presser} ${l.key.padEnd(10)} press@${l.press} pad simulated@${l.applied} (+${l.appliedAfter}) ` +
      `own player moved@${l.moved} (+${l.movedAfter})`);
    if (l.applied === null || l.appliedAfter !== delay) {
      fail(`${l.presser}'s ${l.key} reached its own simulation ${l.appliedAfter} frames after the press, want exactly input_delay=${delay}`);
    }
    if (l.moved === null || l.movedAfter > delay + GAME_REACTION_MAX) {
      fail(`${l.presser}'s own player moved ${l.movedAfter} frames after ${l.key}, want <= ${delay} + ${GAME_REACTION_MAX}`);
    }
  }
  console.log('  input crossing the wire (world X delta seen in the OTHER window):');
  for (const c of crossings) console.log(`    ${c.presser} holds ${c.key.padEnd(10)} -> ${c.observer} sees ${c.who} ${c.delta > 0 ? '+' : ''}${c.delta}`);
  const check = (right, left, who) => {
    if (!(right >= MIN_MOVE)) fail(`${who} did not move right in the other window (${right} px)`);
    if (!(left <= -MIN_MOVE)) fail(`${who} did not move left in the other window (${left} px)`);
  };
  check(p2R, p2L, 'player 2');
  check(p1R, p1L, 'player 1');

  // 5. Run on, then compare confirmed hashes.
  const fin = Math.max(FINAL_FRAMES, ...(await Promise.all(peers.map(status))).map((s) => s.frame + 120));
  const done = await runBoth({ ...base, target: fin, padsB64: null }, { ...base, target: fin, padsB64: null });
  done.forEach((r, i) => { if (r.reason !== 'target') fail(`window ${peers[i].name} could not reach ${fin}: ${r.reason}`); });
  // A few more ticks on both sides so the last inputs are acknowledged.
  await runBoth({ ...base, target: fin + 30, padsB64: null }, { ...base, target: fin + 30, padsB64: null });
  const logs = await Promise.all(peers.map((p) => p.page.evaluate(() => JSON.parse(window.z2.ext.net.confirmedHashes()))));
  const mapB = new Map(logs[1].map(([f, h]) => [f, h]));
  const common = logs[0].filter(([f]) => mapB.has(f));
  const differ = common.filter(([f, h]) => mapB.get(f) !== h);
  probes = await Promise.all(peers.map((p) => p.page.evaluate(() => window.__z2rb.probe())));

  console.log('\n  rollback verdict:');
  for (const [i, x] of probes.entries()) {
    const s = x.ns;
    console.log(`    window ${peers[i].name} (${s.role}): frame=${s.frame} final=${s.finalFrame} rollbacks=${s.rollbacks} ` +
      `replayed=${s.rollbackFrames} maxRollback=${s.maxRollbackDepth} maxPred=${s.maxPredictionDepth}/${s.maxPrediction} ` +
      `stalls=${s.stalledTicks} waits=${s.waitRecommendations} rtt=${s.rttMs}ms hashesOk=${s.hashesOk} ` +
      `simDropped=${s.simDropped} state=${s.state}`);
  }
  console.log(`    confirmed hashes compared: ${common.length} frames (${common.length ? `${common[0][0]}..${common.at(-1)[0]}` : '-'}), ${differ.length} differ\n`);

  if (differ.length) fail(`CONFIRMED STATE DIFFERS at frames ${differ.map(([f]) => f).slice(0, 10).join(',')}`);
  if (common.length < 20) fail(`only ${common.length} confirmed hashes in common; need >= 20 for a meaningful comparison`);
  for (const [i, x] of probes.entries()) {
    const s = x.ns;
    if (s.mode !== 'rollback') fail(`window ${peers[i].name} is not in rollback mode`);
    if (s.state !== 'running' && s.state !== 'stalled') fail(`window ${peers[i].name} is '${s.state}' (${s.closeReason} / ${s.error})`);
    if (!(s.rollbacks > 0)) fail(`window ${peers[i].name} never rolled back (rollbacks=${s.rollbacks}) — the test proved nothing`);
    if (!(s.hashesOk > 0)) fail(`window ${peers[i].name} matched 0 protocol checksums`);
    if (s.error !== null) fail(`window ${peers[i].name} reported: ${s.error}`);
    if (!(s.simDropped > 0)) fail(`window ${peers[i].name} dropped no packets: loss simulation inactive`);
  }

  const shotDir = await mkdtemp(join(tmpdir(), 'z2rs-netplay-e2e-rollback-'));
  await writeFile(join(shotDir, 'summary.json'), JSON.stringify({ sim: SIM, delay, perf, latencies, crossings,
    status: probes.map((x) => x.ns), confirmedCompared: common.length }, null, 2));
  const pageErrors = peers.flatMap((p) => p.errors.map((e) => `${p.name}: ${e}`));
  if (pageErrors.length) fail(`page errors: ${pageErrors.join(' | ').slice(0, 600)}`);

  console.log('NETPLAY-E2E-ROLLBACK-OK');
  console.log(`  simulated ${SIM.latencyMs}±${SIM.jitterMs} ms, ${SIM.lossPct}% loss each way; ` +
    `rollbacks A=${probes[0].ns.rollbacks} B=${probes[1].ns.rollbacks}; ` +
    `local pad reaches the simulation after exactly ${delay} frames; ` +
    `${common.length} confirmed hashes agree, ${probes[0].ns.hashesOk} protocol checksums matched, 0 desyncs`);
  console.log(`  evidence: ${join(shotDir, 'summary.json')}`);
} catch (e) {
  if (e.gated) { console.error(`NETPLAY-E2E-ROLLBACK-GATED: ${e.message}`); exitCode = 2; }
  else {
    console.error(`NETPLAY-E2E-ROLLBACK-FAIL: ${failed || e.message}`);
    if (!failed) console.error(e.stack);
    exitCode = 1;
  }
} finally {
  for (const c of cleanup.reverse()) { try { await c(); } catch { /* best effort */ } }
}
process.exit(exitCode);
