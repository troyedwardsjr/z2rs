// z2rs web frontend glue — vanilla ES module, zero npm dependencies.
//
// Wiring: ./pkg/z2_web.js must exist (see site/README "Build") unless the
// page's #z2-config names another release (readConfig below). This module
// owns the DOM, canvas, IndexedDB, keyboard/Gamepad and AudioContext; the
// wasm side (crates/z2-web/src/lib.rs) owns emulation and only exchanges
// bytes, strings and numbers.
//
// Hosting: `make run-web` serves this file unbundled, straight to the
// browser. A hosting page may add an optional #z2-config element.
//
// Shared input contract (bits 0..7): A,B,Select,Start,Up,Down,Left,Right.
// Keyboard P1: Z=A, X=B, Shift=Select, Enter=Start, arrows=d-pad.
// Keyboard P2 (local co-op): G=A, F=B, R=Select, T=Start, W/A/S/D=d-pad.
// Gamepad (standard mapping): button 1=A, button 0=B, 8=Select, 9=Start,
//   d-pad 12..15 or left stick.
//
// NOTE: Z/X used to be swapped relative to the native app. They now MATCH it
// (Z=A, X=B) so the two frontends share one muscle memory and a native host can
// play with a web guest; the key table in index.html and the README say the same.

const NTSC_HZ = 60.0988;
const FRAME = 1 / NTSC_HZ;

const $ = (id) => document.getElementById(id);
const statusEl = $('status');
const screen = $('screen');
const ctx = screen.getContext('2d');

// The framebuffer is runtime-sized: widescreen widens it and an HD pack or a
// scale above 1 multiplies it, so W/H/img are re-derived from the emulator
// instead of being module constants. NEVER assume 256x240 anywhere.
let W = 256;
let H = 240;
let img = ctx.createImageData(W, H);
let zoom = 2;              // integer NES-pixel zoom used for the CSS width
const ZOOM_MIN = 1, ZOOM_MAX = 4;

// Re-read the emulator's frame size and resize the canvas to match. Safe to
// call every frame; it only touches the DOM when something actually changed.
//
// Two sizes are in play and must not be confused:
//   * backing store  (screen.width/height)  = emu.frame_width()/frame_height(),
//     i.e. NES pixels TIMES the HD output scale — what putImageData needs;
//   * CSS size (--css-w) = the LOGICAL width times `zoom`, so a 4x pack shows a
//     sharper picture at the same physical size instead of a huge one.
// `max-width: 100%` in the stylesheet still wins on a narrow screen, and
// `height: auto` keeps the aspect ratio there.
function syncCanvas() {
  if (!emu) return;
  const w = emu.frame_width(), h = emu.frame_height();
  if (w !== W || h !== H) {
    W = w; H = h;
    screen.width = W;
    screen.height = H;
    img = ctx.createImageData(W, H);
  }
  const cssW = emu.logical_width() * zoom;
  const want = `${cssW}px`;
  if (screen.style.getPropertyValue('--css-w') !== want) {
    screen.style.setProperty('--css-w', want);
    screen.style.aspectRatio = `${emu.logical_width()} / ${emu.logical_height()}`;
  }
}

function setZoom(z) {
  zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z | 0));
  syncCanvas();
}

// --- host configuration -----------------------------------------------------
// A page may carry <script id="z2-config" type="application/json">…</script>
// (a hosting page may render one; the vanilla index.html has none):
//   { wasm:  { js, wasm },          // URLs of the glue module and the binary
//     rom:   { url, keyRequired },  // the host serves the ROM from `url`
//     notes: [ … ] }                // lines to append to Status at boot
// Everything is optional. With no element the page behaves as it always has:
// ./pkg/z2_web.js next to this file, and the ROM only by drop.
const CONFIG = readConfig();
function readConfig() {
  const cfg = { wasm: null, rom: null, notes: [] };
  const el = document.getElementById('z2-config');
  if (!el) return cfg;
  try {
    const raw = JSON.parse(el.textContent || '{}');
    if (raw.wasm && typeof raw.wasm.js === 'string') {
      cfg.wasm = { js: raw.wasm.js, wasm: typeof raw.wasm.wasm === 'string' ? raw.wasm.wasm : null };
    }
    if (raw.rom && typeof raw.rom.url === 'string') {
      cfg.rom = { url: raw.rom.url, keyRequired: !!raw.rom.keyRequired };
    }
    if (Array.isArray(raw.notes)) cfg.notes = raw.notes.filter((n) => typeof n === 'string');
  } catch (e) {
    cfg.notes.push(`host config ignored: ${e}`);
  }
  return cfg;
}

let WebEmu = null;
let emu = null;
let running = false; // ROM loaded and loop armed
let paused = false;
let romLoadT0 = 0;
let firstFrameLogged = false;
let coopOn = false;      // local two-player co-op
let netActive = false;   // an online session exists
let netLastT = 0;        // performance.now() of the previous net tick
// QA only (`z2.ext.net.manual`): the rAF loop stops pumping the session so a
// test script can drive net_poll/net_step itself with frame-exact pads. A human
// never sets this; the panel has no control for it. See site/README.
let netManual = false;

// --- status ---------------------------------------------------------------
let statusNote = '';
function setStatus(extra = '') {
  if (!emu) { statusEl.textContent = `wasm ready — drop a ROM. ${extra}`; return; }
  const st = JSON.parse(emu.state());
  const replay = st.movieLen > 0 ? `REPLAY ${st.movieCursor}/${st.movieLen}` : 'live input';
  statusEl.textContent =
    `frame ${st.frame} · ROM ${st.crc32 || '(none)'} · ${replay} · ` +
    `${audioStatus()} · ${paused ? 'PAUSED' : 'running'}${statusNote}${extra}`;
}

// `audio off` until the button is pressed; then the context state, the
// worklet's queue depth (the delay behind the picture) and how often it ran
// dry.
function audioStatus() {
  if (!actx) return 'audio off';
  if (actx.state !== 'running') return `audio ${actx.state}`;
  const ms = Math.round((audioReport.queued * 1000) / actx.sampleRate);
  return `audio on (${ms} ms buffered, ${audioReport.underruns} underruns)`;
}

// --- wasm boot ------------------------------------------------------------
// The glue module comes from CONFIG.wasm (a host pointing at an uploaded
// release) or, by default, from ./pkg/ next to this file (the vanilla site,
// `make run-web`). The binary's URL is passed explicitly when the host names
// one; otherwise the glue resolves z2_web_bg.wasm beside itself.
async function boot() {
  const jsUrl = CONFIG.wasm ? CONFIG.wasm.js : './pkg/z2_web.js';
  if (hostRomWanted()) {
    // The host serves the ROM, so the page says nothing about ROMs at all: the
    // "drop your own dump" intro goes, and the drop box only shows progress. It
    // comes back, with its prompt, if the host's ROM cannot be had.
    $('romIntro').hidden = true;
    showDropPrompt(false);
    romSay('loading the game…');
  }
  try {
    const pkg = await import(jsUrl);
    if (typeof pkg.default !== 'function') {
      throw new Error('the module loaded but is empty or is not a wasm-pack bundle (no init export)');
    }
    await pkg.default(CONFIG.wasm && CONFIG.wasm.wasm ? { module_or_path: CONFIG.wasm.wasm } : undefined);
    WebEmu = pkg.WebEmu;
  } catch (e) {
    statusEl.textContent = CONFIG.wasm
      ? `wasm release failed to load (${jsUrl}).\n${e}`
      : `wasm bundle missing (./pkg/z2_web.js). Build it first — see site/README "Build".\n${e}`;
    if (CONFIG.notes.length) statusEl.textContent += `\n${CONFIG.notes.join('\n')}`;
    if (hostRomWanted()) romSay('the game engine failed to load — see Status below.', 'err');
    return;
  }
  emu = new WebEmu();
  publishZ2();
  applyUrlParams();
  setStatus();
  $('pauseBtn').disabled = true;
  $('netStatus').textContent = netStatusLine(JSON.parse(emu.net_state()));
  if (!$('netIce').value) $('netIce').value = emu.net_default_ice();
  if (!emu.net_supported()) $('netBox').classList.add('unsupported');
  syncNetButtons();
  syncHdPanel();
  statusEl.textContent += `\nz2-web ${emu.version()} ready — ` +
    (hostRomWanted() ? 'starting the game.' : 'drop a Zelda II (USA) .nes file.');
  if (CONFIG.notes.length) statusEl.textContent += `\n${CONFIG.notes.join('\n')}`;
  if (hostRomWanted()) await loadRomFromHost();
}

// `?widescreen=16:9` (or `?wide=`) and `?coop=1` so a QA run or a bookmark can
// arrive with the features already on. A bad value is reported and ignored —
// never fatal, and appended after the existing status text because smoke.mjs
// reads #status for readiness.
function applyUrlParams() {
  const q = new URLSearchParams(location.search);
  const wide = q.get('widescreen') ?? q.get('wide');
  if (wide) {
    try {
      emu.set_widescreen_preset(wide);
      $('wideSel').value = emu.widescreen_tiles() === 0 ? 'off' : wide;
      syncCanvas();
    } catch (e) {
      statusNote += `\nwidescreen '${wide}' rejected: ${e}`;
    }
  }
  // `?clip=0` turns off painting the window's blanked left 8 columns,
  // `?rclip=0` the masked right 8. Both default on.
  const clip = q.get('clip');
  if (clip !== null) {
    const on = !(clip === '0' || clip === 'false');
    $('clipChk').checked = on;
    try { emu.set_fill_left_clip(on); syncCanvas(); }
    catch (e) { statusNote += `\nfill left edge: ${e}`; }
  }
  const rclip = q.get('rclip');
  if (rclip !== null) {
    const on = !(rclip === '0' || rclip === 'false');
    $('rclipChk').checked = on;
    try { emu.set_fill_right_clip(on); syncCanvas(); }
    catch (e) { statusNote += `\nfill right edge: ${e}`; }
  }
  // `?scale=2` picks the HD output multiplier (above 1 needs an `hd` bundle).
  const scale = q.get('scale');
  if (scale) {
    try {
      emu.set_output_scale(Number(scale) >>> 0);
      $('hdScale').value = String(emu.requested_scale());
      syncCanvas();
    } catch (e) {
      statusNote += `\nscale '${scale}' rejected: ${e}`;
    }
  }
  // `?zoom=3` changes the on-screen size only (CSS pixels per NES pixel).
  const z = q.get('zoom');
  if (z) setZoom(Number(z));
  // Netplay panel prefill: `?net=lockstep` (default rollback), `?signal=`,
  // `?room=`, `?ice=` (e.g. `none`), `?delay=`. Nothing connects by itself.
  const net = q.get('net');
  if (net !== null) {
    if (net === 'rollback' || net === 'lockstep') $('netMode').value = net;
    else statusNote += `\nnet mode '${net}' rejected (rollback or lockstep)`;
  }
  for (const [param, id] of [['signal', 'netSignal'], ['room', 'netRoom'], ['ice', 'netIce']]) {
    const v = q.get(param);
    if (v !== null) $(id).value = v;
  }
  const delay = q.get('delay');
  if (delay !== null && [...$('netDelay').options].some((o) => o.value === delay)) $('netDelay').value = delay;
  syncNetMode();
  const coop = q.get('coop');
  if (coop === '1' || coop === 'true') setCoop(true);
}

// --- window.z2 QA contract ----------------------------------------
// The 9 stable members are unchanged byte-for-byte: { facts, state, input,
// step, snapshot, restore, loadMovie, screenshot, version }. Backed by
// Game::facts + the Z2WEB01/Z2SNAP01 codec + the .fm2/Input-Log parsers.
//
// Everything added for widescreen / co-op / netplay lives under `z2.ext`,
// which is explicitly NOT part of the stable QA contract (see site/README).
function publishZ2() {
  window.z2 = {
    facts: () => emu.facts(),
    state: () => emu.state(),
    input: (mask = 0, frames = 1) => emu.hold_input(mask >>> 0, frames >>> 0),
    step: (n = 1) => emu.step_movie_or_idle(n >>> 0),
    snapshot: () => emu.snapshot(),
    restore: (bytes) => emu.restore(bytes),
    loadMovie: (text) => emu.load_movie(text),
    screenshot: () => { blit(); return screen.toDataURL('image/png'); },
    version: emu.version(),
    ext: {
      // widescreen
      // Keeps the on-screen control in step with the emulator, so a QA script
      // and a human never disagree about what is selected.
      setWide: (preset) => {
        emu.set_widescreen_preset(String(preset));
        const sel = $('wideSel');
        const want = emu.widescreen_tiles() === 0 ? 'off' : String(preset);
        if ([...sel.options].some((o) => o.value === want)) sel.value = want;
        syncCanvas();
        syncHdPanel();
        return emu.widescreen_tiles();
      },
      wideTiles: () => emu.widescreen_tiles(),
      frameSize: () => ({
        width: emu.frame_width(),
        height: emu.frame_height(),
        logicalWidth: emu.logical_width(),
        logicalHeight: emu.logical_height(),
        scale: emu.output_scale(),
      }),
      setFillLeftClip: (on) => {
        emu.set_fill_left_clip(!!on);
        $('clipChk').checked = emu.fill_left_clip();
        syncCanvas();
        return emu.fill_left_clip();
      },
      setFillRightClip: (on) => {
        emu.set_fill_right_clip(!!on);
        $('rclipChk').checked = emu.fill_right_clip();
        syncCanvas();
        return emu.fill_right_clip();
      },
      setZoom: (z) => { setZoom(Number(z)); return zoom; },
      // HD graphics packs
      hd: {
        supported: () => emu.hd_supported(),
        // files: [{ name, bytes: Uint8Array }] — the same wasm path the folder
        // picker uses, so a QA script and a human load packs identically.
        loadPack: (files) => loadHdFiles(files),
        clearPack: () => { emu.hd_pack_clear(); syncCanvas(); syncHdPanel(); return true; },
        packInfo: () => emu.hd_pack_info(),
        setScale: (n) => { emu.set_output_scale(Number(n) >>> 0); syncCanvas(); syncHdPanel(); return emu.output_scale(); },
        scale: () => emu.output_scale(),
      },
      // local co-op
      // audio: what the AudioWorklet last reported, for QA (`peak` > 0 means
      // non-silent samples actually reached the output; `queued` over `rate`
      // is how far the sound trails the picture).
      audio: () => ({
        state: actx ? actx.state : 'off',
        rate: actx ? actx.sampleRate : emu.audio_rate(),
        ...audioReport,
      }),
      coopEnable: (on) => { setCoop(!!on); return emu.coop_enabled(); },
      coopEnabled: () => emu.coop_enabled(),
      coopStatus: () => emu.coop_status(),
      coopHash: () => emu.coop_hash_hex(),
      stepCoop: (p1, p2, n = 1) => emu.step_frames2(p1 >>> 0, p2 >>> 0, n >>> 0),
      // online co-op
      net: {
        supported: () => emu.net_supported(),
        connect: (signal, room, host, delay = 2, ice = emu.net_default_ice()) =>
          emu.net_connect(String(signal), String(room), !!host, delay >>> 0, String(ice)),
        // Protocol for the next session: 'rollback' (default) or 'lockstep',
        // plus the rollback prediction window. Keeps the panel selector in step.
        setMode: (mode, maxPrediction = JSON.parse(emu.net_mode()).defaultMaxPrediction) => {
          emu.net_set_mode(String(mode), maxPrediction >>> 0);
          $('netMode').value = String(mode);
          return JSON.parse(emu.net_mode());
        },
        mode: () => JSON.parse(emu.net_mode()),
        // TEST HOOK, off by default and never used by normal play: delay every
        // packet this page sends by latencyMs + rand(0..jitterMs) of session
        // time and drop lossPct % of unreliable packets, inside the page's
        // transport wrapper. `simulate({})` turns it off again.
        simulate: ({ latencyMs = 0, jitterMs = 0, lossPct = 0 } = {}) =>
          JSON.parse(emu.net_simulate(latencyMs >>> 0, jitterMs >>> 0, Math.min(100, lossPct >>> 0))),
        // Rollback: [[frame, hash], ...] for every 15th saved state that is final.
        confirmedHashes: () => emu.net_confirmed_hashes(),
        status: () => emu.net_state(),
        // Also repaints the status line, so a QA script driving the session by
        // hand leaves the same text on screen the rAF loop would have.
        poll: (dtMs = 0) => {
          const json = emu.net_poll(dtMs >>> 0);
          $('netStatus').textContent = netStatusLine(JSON.parse(json));
          return json;
        },
        disconnect: () => endNetSession('netplay: left the session'),
        // The desync hash the two peers compare (16 hex chars). Two peers that
        // report the same value at the same session frame are in lockstep by
        // the protocol's own definition — this is what site/netplay-e2e.mjs
        // asserts instead of eyeballing screenshots.
        stateHash: () => emu.net_state_hash_hex(),
        // --- QA driving hooks -------------------------------------------------
        // `manual(true)` stops the rAF loop pumping the session so a test can
        // supply frame-exact pads through `step()` below; `manual(false)` hands
        // the session back to live keyboard/gamepad input. No UI control sets
        // this and nothing in the page turns it on by itself.
        manual: (on) => { netManual = !!on; return netManual; },
        isManual: () => netManual,
        // One session tick under `manual(true)`: latch a local pad and step up
        // to `max` confirmed frames. Returns frames stepped (0 = still waiting
        // for the peer's input). `pad` null/undefined samples the LIVE
        // keyboard + gamepad through the same `pollInput()` the rAF loop uses,
        // so a test can hold a real key and still advance frame by frame.
        step: (pad = null, max = 1) =>
          emu.net_step(pad === null || pad === undefined ? pollInput() : pad >>> 0, max >>> 0),
      },
      // On-screen controller: show/hide it, and read the pad byte it contributes.
      touch: {
        show: (on) => { showTouchPad(!!on); return !touchPad.hidden; },
        shown: () => !touchPad.hidden,
        mask: () => touchMask(),
      },
      trapsetId: () => emu.trapset_id_hex(),
      // Rollback cost probes, timed here with performance.now(). Leaves the
      // game where it was. Refused during a netplay session.
      perf: {
        rollback: (opts = {}) => rollbackPerf(opts),
      },
    },
  };
}

// `z2.ext.perf.rollback({ iterations, depths })`: one emulated frame, one
// save, one load, and a worst-case rollback tick per depth (load, re-simulate
// `depth` frames silently, save + simulate the new frame, checksum, render).
function rollbackPerf({ iterations = 120, depths = [0, 4, 8] } = {}) {
  const n = Math.max(1, iterations >>> 0);
  const stats = (xs) => {
    const s = [...xs].sort((a, b) => a - b);
    return {
      mean: s.reduce((a, b) => a + b, 0) / s.length,
      p95: s[Math.min(s.length - 1, Math.floor(s.length * 0.95))],
      max: s[s.length - 1],
    };
  };
  const time = (fn) => {
    const xs = [];
    for (let i = 0; i < n; i++) {
      const t0 = performance.now();
      fn(i);
      xs.push(performance.now() - t0);
    }
    return stats(xs);
  };
  emu.perf_begin();
  try {
    const out = { iterations: n, tick: {} };
    // Warm up so the first measured call does not pay for lazy work.
    for (let i = 0; i < 10; i++) { emu.perf_load(0); emu.perf_step(0, 0, false); emu.perf_save(1); }
    out.frame = time(() => emu.perf_step(0, 0, true));
    emu.perf_load(0);
    // A save or load is far below the timer's resolution (browsers clamp
    // performance.now() to 0.1 ms or coarser), so each sample times a batch.
    const BATCH = 500;
    const per = (s) => ({ mean: s.mean / BATCH, p95: s.p95 / BATCH, max: s.max / BATCH });
    out.save = per(time(() => { for (let k = 0; k < BATCH; k++) emu.perf_save(1); }));
    out.load = per(time(() => { for (let k = 0; k < BATCH; k++) emu.perf_load(0); }));
    for (const d of depths) out.tick[d >>> 0] = time(() => emu.perf_tick(d >>> 0));
    return out;
  } finally {
    emu.perf_end();
  }
}

// --- input ----------------------------------------------------------------
const keys = new Set();
addEventListener('keydown', (e) => {
  if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', ' '].includes(e.key)) e.preventDefault();
  keys.add(e.code);
});
addEventListener('keyup', (e) => keys.delete(e.code));
// A key held while the window loses focus never delivers its keyup here
// (clicking across to the other netplay-play window, say), so it would stay
// pressed until it was pressed and released again in this window. Release
// everything on blur instead.
addEventListener('blur', () => { keys.clear(); touchRelease(); });

// Player 1: Z=A, X=B (matches the native app).
function keyboardMask() {
  let m = 0;
  if (keys.has('KeyZ')) m |= 1 << 0;
  if (keys.has('KeyX')) m |= 1 << 1;
  if (keys.has('ShiftLeft') || keys.has('ShiftRight')) m |= 1 << 2;
  if (keys.has('Enter')) m |= 1 << 3;
  if (keys.has('ArrowUp')) m |= 1 << 4;
  if (keys.has('ArrowDown')) m |= 1 << 5;
  if (keys.has('ArrowLeft')) m |= 1 << 6;
  if (keys.has('ArrowRight')) m |= 1 << 7;
  return m;
}

// Player 2: the same layout the native app's `keys_p2` default uses, so the
// two frontends document one set of co-op keys.
function keyboardMaskP2() {
  let m = 0;
  if (keys.has('KeyG')) m |= 1 << 0;
  if (keys.has('KeyF')) m |= 1 << 1;
  if (keys.has('KeyR')) m |= 1 << 2;
  if (keys.has('KeyT')) m |= 1 << 3;
  if (keys.has('KeyW')) m |= 1 << 4;
  if (keys.has('KeyS')) m |= 1 << 5;
  if (keys.has('KeyA')) m |= 1 << 6;
  if (keys.has('KeyD')) m |= 1 << 7;
  return m;
}

function padBits(gp) {
  let m = 0;
  const b = (i) => gp.buttons[i] && gp.buttons[i].pressed;
  if (b(1)) m |= 1 << 0; // right button -> A
  if (b(0)) m |= 1 << 1; // bottom button -> B
  if (b(8)) m |= 1 << 2;
  if (b(9)) m |= 1 << 3;
  if (b(12)) m |= 1 << 4;
  if (b(13)) m |= 1 << 5;
  if (b(14)) m |= 1 << 6;
  if (b(15)) m |= 1 << 7;
  const ax = gp.axes[0] || 0, ay = gp.axes[1] || 0;
  if (ay < -0.4) m |= 1 << 4;
  if (ay > 0.4) m |= 1 << 5;
  if (ax < -0.4) m |= 1 << 6;
  if (ax > 0.4) m |= 1 << 7;
  return m;
}

// `navigator.getGamepads()` returns a sparse array: disconnected slots are null
// and the indices are NOT compacted, so "gamepad 0 and 1" is wrong. Take the
// connected pads in index order instead.
function connectedPads() {
  const out = [];
  for (const gp of navigator.getGamepads ? navigator.getGamepads() : []) {
    if (gp && gp.connected) out.push(gp);
  }
  return out;
}

// With co-op off, every pad is OR-ed (unchanged behaviour). With co-op on the
// first connected pad is player 1 and the second is player 2.
function gamepadMask(player = 0) {
  const pads = connectedPads();
  if (!coopOn) return pads.reduce((m, gp) => m | padBits(gp), 0);
  const gp = pads[player];
  return gp ? padBits(gp) : 0;
}

const pollInput = () => keyboardMask() | gamepadMask(0) | touchMask();
const pollInputP2 = () => keyboardMaskP2() | gamepadMask(1);

// --- touch gamepad (on-screen controller, player 1) ---------------------------
// #touchpad is a d-pad, Select/Start and B/A floating over the bottom of the
// viewport. Every finger is tracked by pointer id and re-hit-tested as it moves,
// so a thumb can roll from B onto A or slide round the d-pad without lifting.
// A finger that started on the d-pad keeps steering even after it drifts off
// the disc. The pad bits are OR-ed into pollInput(), so the overlay drives
// exactly what the keyboard drives: local play, co-op player 1 and netplay.
const TOUCH_UP = 1 << 4, TOUCH_DOWN = 1 << 5, TOUCH_LEFT = 1 << 6, TOUCH_RIGHT = 1 << 7;
const TOUCH_DEAD = 0.2; // d-pad dead zone, as a fraction of its radius
// Eight ways, clockwise from east (atan2 has +y pointing down the screen).
const TOUCH_OCTANTS = [TOUCH_RIGHT, TOUCH_RIGHT | TOUCH_DOWN, TOUCH_DOWN, TOUCH_DOWN | TOUCH_LEFT,
  TOUCH_LEFT, TOUCH_LEFT | TOUCH_UP, TOUCH_UP, TOUCH_UP | TOUCH_RIGHT];
const touchPad = $('touchpad');
const touchDpad = $('tpDpad');
const touchPointers = new Map(); // pointerId -> { bits, dpad }

function touchMask() {
  let m = 0;
  for (const p of touchPointers.values()) m |= p.bits;
  return m;
}

function touchDpadBits(x, y) {
  const r = touchDpad.getBoundingClientRect();
  const dx = (x - (r.left + r.width / 2)) / (r.width / 2);
  const dy = (y - (r.top + r.height / 2)) / (r.height / 2);
  if (Math.hypot(dx, dy) < TOUCH_DEAD) return 0;
  const oct = Math.round(Math.atan2(dy, dx) / (Math.PI / 4));
  return TOUCH_OCTANTS[(oct + 8) % 8];
}

function touchBitsAt(p, x, y) {
  if (p.dpad) return touchDpadBits(x, y);
  const el = document.elementFromPoint(x, y);
  const btn = el && el.closest ? el.closest('#touchpad [data-pad]') : null;
  return btn ? Number(btn.dataset.pad) : 0;
}

function touchPaint() {
  const m = touchMask();
  for (const b of touchPad.querySelectorAll('[data-pad]')) b.classList.toggle('on', (m & Number(b.dataset.pad)) !== 0);
  touchDpad.classList.toggle('up', (m & TOUCH_UP) !== 0);
  touchDpad.classList.toggle('down', (m & TOUCH_DOWN) !== 0);
  touchDpad.classList.toggle('left', (m & TOUCH_LEFT) !== 0);
  touchDpad.classList.toggle('right', (m & TOUCH_RIGHT) !== 0);
}

function touchRelease() {
  touchPointers.clear();
  touchPaint();
}

touchPad.addEventListener('pointerdown', (e) => {
  e.preventDefault(); // no focus, no text selection, no long-press menu
  const p = { bits: 0, dpad: !!(e.target.closest && e.target.closest('#tpDpad')) };
  p.bits = touchBitsAt(p, e.clientX, e.clientY);
  touchPointers.set(e.pointerId, p);
  // Keep getting this finger's moves after it slides off the control.
  try { e.target.setPointerCapture(e.pointerId); } catch { /* synthetic pointer: nothing to capture */ }
  touchPaint();
});
touchPad.addEventListener('pointermove', (e) => {
  const p = touchPointers.get(e.pointerId);
  if (!p) return;
  const bits = touchBitsAt(p, e.clientX, e.clientY);
  if (bits !== p.bits) { p.bits = bits; touchPaint(); }
});
for (const type of ['pointerup', 'pointercancel']) {
  touchPad.addEventListener(type, (e) => {
    if (!touchPointers.delete(e.pointerId)) return;
    touchPaint();
    // Lifting a finger is a user gesture (pressing one down is not, for touch),
    // so this is where a phone gets its sound without hunting for the button.
    if (type === 'pointerup' && !actx && !$('audioBtn').disabled) $('audioBtn').click();
  });
}
touchPad.addEventListener('contextmenu', (e) => e.preventDefault());

// Shown by default where touch is the main way in; `?touch=1` / `?touch=0` and
// the Touch pad button override that either way.
function touchWanted() {
  const q = new URLSearchParams(location.search).get('touch');
  if (q === '1' || q === 'true') return true;
  if (q === '0' || q === 'false') return false;
  return matchMedia('(pointer: coarse)').matches;
}

function showTouchPad(on) {
  touchPad.hidden = !on;
  $('touchSpacer').hidden = !on;
  $('touchBtn').textContent = on ? 'Hide touch pad' : 'Touch pad';
  if (!on) touchRelease();
}
$('touchBtn').addEventListener('click', () => showTouchPad(touchPad.hidden));
showTouchPad(touchWanted());

// --- frame loop (rAF accumulator @ NTSC_HZ) --------------------------------
let lastT = 0;
let acc = 0;
let fpsFrames = 0;
let fpsT0 = 0;
let fps = 0;

function blit() {
  emu.render_frame();
  // The frame can change width (widescreen toggled), so make sure the
  // ImageData matches before copying into it.
  syncCanvas();
  img.data.set(emu.frame_rgba());
  ctx.putImageData(img, 0, 0);
}

// Ordinary single-page stepping: movie replay, local co-op or one player.
function stepLocal(n) {
  const st = JSON.parse(emu.state());
  if (st.movieLen > 0 && st.movieCursor < st.movieLen) {
    emu.step_movie_or_idle(n); // deterministic replay while a movie is armed
  } else if (coopOn) {
    emu.step_frames2(pollInput(), pollInputP2(), n);
  } else {
    emu.step_frames(pollInput(), n);
  }
}

function loop(t) {
  requestAnimationFrame(loop);
  if (!running || paused || !emu.rom_loaded()) return;
  if (!lastT) lastT = t;
  let dt = (t - lastT) / 1000;
  lastT = t;
  if (dt > 0.5) dt = 0.5; // tab was backgrounded: no spiral of death
  acc += dt / FRAME;
  let n = Math.floor(acc);
  acc -= n;
  if (n > 5) { n = 5; acc = 0; } // catch-up cap: slow down instead of spiralling
  if (n <= 0 && !netActive) return;
  if (netActive && netManual) {
    // A QA script owns net_poll/net_step this tick (z2.ext.net.manual(true)),
    // so the loop must not latch a pad of its own — it would race the script's
    // frame-exact sequence. Keep painting what the script has stepped.
    blit();
    return;
  }
  if (netActive) {
    // One session tick: pump the transport, then step only the frames the
    // session has confirmed. `pollInput()` is sampled once per tick and reused
    // for each stepped frame — the same semantics the live path already had.
    // Until the session has started (signal server, waiting for the other
    // player, peer link, handshake) the local game keeps running as usual:
    // `Started` restarts both peers from power-on anyway, so nothing played
    // here can leak into the session, and a slow connection never looks like
    // a hung page.
    const now = performance.now();
    const dtMs = netLastT ? Math.min(1000, now - netLastT) : 0;
    netLastT = now;
    let ns;
    try {
      ns = JSON.parse(emu.net_poll(dtMs));
    } catch (e) {
      $('netStatus').textContent = `netplay error: ${e}`;
      return;
    }
    if (!ns.started && (ns.state === 'closed' || ns.state === 'desynced')) {
      // Failed before it ever started (timeout, unreachable server, room
      // full, mismatch): end the attempt so Host/Join work again, and leave
      // the reason on screen.
      endNetSession(netStatusLine(ns));
      if (n > 0) { stepLocal(n); blit(); }
      return;
    }
    $('netStatus').textContent = netStatusLine(ns);
    if (ns.started && ns.mode === 'rollback' && (ns.state === 'running' || ns.state === 'stalled')) {
      // Rollback: exactly one session tick per due NES frame (never more
      // than the display rate asks for, or a 120 Hz screen would run the
      // game at double speed). The tick runs every save/load/step request
      // in Rust; the frame is presented once below, after all of them.
      if (n <= 0) return;
      if (emu.net_step(pollInput(), n) === 0) return; // stalled: nothing new to show
    } else if (ns.started && (ns.state === 'running' || ns.state === 'stalled')) {
      const stepped = emu.net_step(pollInput(), Math.max(n, 1));
      if (stepped === 0) return; // waiting for the peer: nothing new to show
    } else if (!ns.started) {
      if (n <= 0) return;
      stepLocal(n); // still connecting: the local game keeps running
    } else {
      return; // the session ended after it started: keep the last frame up
    }
  } else {
    stepLocal(n);
  }
  blit();
  if (!firstFrameLogged) {
    firstFrameLogged = true;
    statusNote = `\nfirst frame ${(performance.now() - romLoadT0).toFixed(0)} ms after ROM load`;
  }
  pushAudio();
  fpsFrames += n;
  if (t - fpsT0 > 1000) {
    fps = (fpsFrames * 1000) / (t - fpsT0);
    fpsFrames = 0; fpsT0 = t;
    setStatus(` · ${fps.toFixed(1)} fps`);
  }
}

// --- audio (AudioWorklet ring buffer) --------------------------------------
let actx = null;
let worklet = null;
// Last report from the worklet: `{underruns, queued, peak, dropped}` (see
// worklet.js).
let audioReport = { underruns: 0, queued: 0, peak: 0, dropped: 0 };

$('audioBtn').addEventListener('click', async () => {
  try {
    const Ctx = window.AudioContext || window.webkitAudioContext;
    // Take the device's own rate and synthesise at it, rather than demanding
    // 44100. Forcing a rate either buys a pointless resample (the browser's,
    // on a 48 kHz device) or is quietly ignored — and a context running at a
    // different rate from the synth drifts by 3900 samples/s, which is heard
    // as a delay that grows by about a second every 11 seconds, or as a
    // permanent underrun, depending on which way the mismatch goes.
    // 'interactive' asks for the smallest output buffer the device offers.
    actx = new Ctx({ latencyHint: 'interactive' });
    let rate = emu.set_audio_rate(actx.sampleRate);
    if (rate !== actx.sampleRate) {
      // An exotic context rate the synth cannot render (it ships 44100 and
      // 48000 only): rebuild the context at the rate it fell back to.
      await actx.close();
      actx = new Ctx({ sampleRate: rate, latencyHint: 'interactive' });
      rate = actx.sampleRate;
    }
    // Resolve against this module, not the document: a host may serve the
    // page at / and this file at /z2/app.js, so a document-relative
    // './worklet.js' would miss.
    await actx.audioWorklet.addModule(new URL('./worklet.js', import.meta.url));
    worklet = new AudioWorkletNode(actx, 'z2-ring', { processorOptions: { rate } });
    worklet.port.onmessage = (e) => { audioReport = e.data; };
    worklet.connect(actx.destination);
    await actx.resume();
    $('audioBtn').disabled = true;
    $('audioBtn').textContent = 'Audio on';
  } catch (e) {
    statusNote = `\naudio failed: ${e}`;
  }
  setStatus();
});

function pushAudio() {
  if (emu.audio_queued() === 0) return;
  const pcm = emu.take_audio_f32();
  // Audio off or suspended: the samples are dropped, so switching it on at
  // any point starts from the live frame rather than seconds of backlog.
  if (!worklet || !actx || actx.state !== 'running') return;
  worklet.port.postMessage(pcm);
}

// --- pause -----------------------------------------------------------------
// `autoPaused` marks a pause the page took by itself (tab hidden). Only that
// kind is undone when the tab comes back: a page that loaded its ROM in a
// background tab, or a phone that was locked for a moment, must not greet the
// player with a frozen picture and a Resume button. A pause the player asked
// for stays until the player ends it.
let autoPaused = false;
function setPaused(p, auto = false) {
  paused = p;
  autoPaused = p && auto;
  $('pauseBtn').textContent = paused ? 'Resume' : 'Pause';
  if (actx) { paused ? actx.suspend() : actx.resume(); }
  setStatus();
}
$('pauseBtn').addEventListener('click', () => setPaused(!paused));
document.addEventListener('visibilitychange', () => {
  // Never auto-pause during a session: a paused peer stalls the other one.
  // (requestAnimationFrame stops in a hidden tab anyway, so the peer sees a
  // stall regardless.)
  if (document.hidden) {
    touchRelease();
    if (running && !paused && !netActive) setPaused(true, true);
  } else if (autoPaused) {
    lastT = 0; acc = 0; // do not try to catch up on the time spent hidden
    setPaused(false);
  }
});

// --- ROM loading ------------------------------------------------------------
// One entry point for every source (drop, file picker, host download): the
// wasm side hash-gates the bytes, and nothing here stores them anywhere.
function loadRomBytes(buf) {
  romLoadT0 = performance.now();
  firstFrameLogged = false;
  try {
    emu.load_rom(buf);
  } catch (e) {
    statusEl.textContent = `ROM rejected: ${e}`;
    return;
  }
  // Re-apply the UI's feature state to the freshly built game. `load_rom`
  // constructs a new `Game` (and re-arms the render record from the settings
  // already held in wasm), so without this a second ROM load would run with
  // widescreen and co-op off while the controls still showed them as on.
  try {
    emu.set_widescreen_preset($('wideSel').value);
    emu.set_fill_left_clip($('clipChk').checked);
    emu.set_fill_right_clip($('rclipChk').checked);
  } catch (e) {
    statusNote += `\nwidescreen: ${e}`;
  }
  try {
    emu.coop_enable(coopOn);
  } catch (e) {
    statusNote += `\nco-op: ${e}`;
  }
  syncCanvas();
  syncHdPanel();
  running = true;
  lastT = 0; acc = 0;
  for (const id of ['pauseBtn', 'audioBtn', 'snapSave', 'snapLoad', 'sramSave', 'sramLoad', 'snapExport', 'movieRewind']) {
    $(id).disabled = false;
  }
  syncNetButtons();
  setPaused(false);
  setStatus();
  // On a phone the picture is the page: bring it under the thumbs' controller.
  if (!touchPad.hidden) screen.scrollIntoView({ block: 'start', behavior: 'smooth' });
}

async function loadRomFile(file) {
  loadRomBytes(new Uint8Array(await file.arrayBuffer()));
  if (CONFIG.rom && emu.rom_loaded()) romSay('playing the dropped ROM.');
}

const drop = $('drop');
drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
drop.addEventListener('dragleave', () => drop.classList.remove('over'));
drop.addEventListener('drop', (e) => {
  e.preventDefault(); drop.classList.remove('over');
  if (e.dataTransfer.files.length) loadRomFile(e.dataTransfer.files[0]);
});
$('romFile').addEventListener('change', (e) => { if (e.target.files.length) loadRomFile(e.target.files[0]); });

// --- ROM from the host ------------------------------------------------------
// With CONFIG.rom the host serves the ROM itself (from private storage, or a
// local Z2_ROM file in development), so the page starts without a
// drop and without saying anything about it: once the game is running the
// drop box is hidden. `?rom=drop` skips the download and plays a dropped dump
// instead. A 401 means the host wants an access key (Z2_ROM_ACCESS_KEY): the
// key box asks once and keeps the key in localStorage for this origin. The
// ROM bytes never touch storage; only the key does.
const ROM_KEY_STORAGE = 'z2rs-rom-key';
let romKeyMem = ''; // the key for this page load when localStorage is unavailable

function hostRomWanted() {
  if (!CONFIG.rom) return false;
  const p = new URLSearchParams(location.search).get('rom');
  return !(p === '0' || p === 'drop' || p === 'none');
}

function romSay(text, cls = '') {
  const el = $('romAuto');
  if (!el) return;
  el.hidden = !text;
  el.className = `sub ${cls}`.trim();
  el.textContent = text;
}

// The "Drop .nes ROM here, or [Choose File]" prompt inside the drop box, and
// the box itself.
function showDropPrompt(on) {
  const el = $('dropPrompt');
  if (el) el.hidden = !on;
}
function showDropBox(on) {
  $('drop').hidden = !on;
}

function romKey() {
  if (romKeyMem) return romKeyMem;
  try { return localStorage.getItem(ROM_KEY_STORAGE) || ''; } catch { return ''; }
}

function showKeyBox(why) {
  romSay(why, 'warn');
  showDropBox(true);
  showDropPrompt(true);
  $('romKeyBox').hidden = false;
  $('romKey').focus();
}

async function loadRomFromHost() {
  $('romKeyBox').hidden = true;
  showDropBox(true);
  romSay('loading the game…');
  let res;
  try {
    const headers = {};
    const key = romKey();
    if (key) headers['x-z2-rom-key'] = key;
    res = await fetch(CONFIG.rom.url, { headers });
  } catch (e) {
    showDropPrompt(true);
    romSay(`ROM download failed (${e}) — drop a .nes instead.`, 'err');
    return;
  }
  if (res.status === 401 || res.status === 403) {
    showKeyBox(res.status === 403 && romKey()
      ? 'that access key was refused — enter the current one (or drop your own .nes).'
      : 'this deployment needs an access key to load its ROM (or drop your own .nes).');
    return;
  }
  if (!res.ok) {
    let why = `HTTP ${res.status}`;
    try { why = (await res.text()).trim() || why; } catch { /* keep the status code */ }
    showDropPrompt(true);
    romSay(`no ROM from this deployment (${why}) — drop a .nes instead.`, 'err');
    return;
  }
  loadRomBytes(new Uint8Array(await res.arrayBuffer()));
  if (emu.rom_loaded()) {
    // Running: nothing to announce, the player can see it.
    romSay('');
    showDropBox(false);
  } else {
    showDropPrompt(true);
    romSay("the deployment's ROM was rejected (see Status) — drop a .nes instead.", 'err');
  }
}

$('romKeyBtn').addEventListener('click', () => {
  const key = $('romKey').value.trim();
  romKeyMem = key;
  try {
    if (key) localStorage.setItem(ROM_KEY_STORAGE, key);
    else localStorage.removeItem(ROM_KEY_STORAGE);
  } catch { /* private browsing: romKeyMem carries the key for this load */ }
  $('romKey').value = '';
  loadRomFromHost();
});
$('romKey').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('romKeyBtn').click(); });

// --- movie ------------------------------------------------------------------
$('movieFile').addEventListener('change', async (e) => {
  if (!e.target.files.length) return;
  const text = await e.target.files[0].text();
  try {
    const rep = JSON.parse(emu.load_movie(text));
    $('movieInfo').textContent = `${rep.kind}: ${rep.frames} frames ${rep.warnings.join('; ')}`;
  } catch (err) {
    $('movieInfo').textContent = `rejected: ${err}`;
  }
  setStatus();
});
$('movieRewind').addEventListener('click', () => { emu.movie_rewind(); setStatus(); });

// --- IndexedDB saves ----------------------------------------------------------
const DB = 'z2rs-web';
function idb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => req.result.createObjectStore('slots', { keyPath: 'name' });
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}
async function idbPut(name, bytes) {
  const db = await idb();
  return new Promise((resolve, reject) => {
    const tx = db.transaction('slots', 'readwrite');
    tx.objectStore('slots').put({ name, bytes, ts: Date.now() });
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}
async function idbGet(name) {
  const db = await idb();
  return new Promise((resolve, reject) => {
    const req = db.transaction('slots').objectStore('slots').get(name);
    req.onsuccess = () => resolve(req.result || null);
    req.onerror = () => reject(req.error);
  });
}
const slotKey = (kind) => `${$('slotSel').value}:${kind}`;

$('snapSave').addEventListener('click', async () => {
  try { await idbPut(slotKey('snapshot'), emu.snapshot()); setStatus(' · snapshot saved'); }
  catch (e) { setStatus(` · save failed: ${e}`); }
});
$('snapLoad').addEventListener('click', async () => {
  const rec = await idbGet(slotKey('snapshot'));
  if (!rec) { setStatus(' · slot empty'); return; }
  try { emu.restore(rec.bytes); setStatus(' · snapshot restored'); }
  catch (e) { setStatus(` · restore failed: ${e}`); }
});
$('sramSave').addEventListener('click', async () => {
  try { await idbPut(slotKey('sram'), emu.sram_bytes()); setStatus(' · SRAM saved'); }
  catch (e) { setStatus(` · save failed: ${e}`); }
});
$('sramLoad').addEventListener('click', async () => {
  const rec = await idbGet(slotKey('sram'));
  if (!rec) { setStatus(' · slot empty'); return; }
  try { emu.load_sram(rec.bytes); setStatus(' · SRAM loaded'); }
  catch (e) { setStatus(` · SRAM load failed: ${e}`); }
});
$('snapExport').addEventListener('click', () => {
  const blob = new Blob([emu.snapshot()], { type: 'application/octet-stream' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = `z2-snapshot-f${JSON.parse(emu.state()).frame}.z2web`;
  a.click();
  URL.revokeObjectURL(a.href);
});
$('snapImport').addEventListener('change', async (e) => {
  if (!e.target.files.length) return;
  try {
    const msg = emu.restore(new Uint8Array(await e.target.files[0].arrayBuffer()));
    setStatus(` · imported ${msg}`);
  } catch (err) { setStatus(` · import failed: ${err}`); }
});

// --- widescreen / local co-op ------------------------------------------------
function setCoop(on) {
  coopOn = !!on;
  $('coopChk').checked = coopOn;
  try {
    emu.coop_enable(coopOn);
  } catch (e) {
    setStatus(` · co-op: ${e}`);
  }
}

$('wideSel').addEventListener('change', () => {
  const preset = $('wideSel').value;
  try {
    emu.set_widescreen_preset(preset);
    syncCanvas();
    setStatus(` · widescreen ${preset}`);
  } catch (e) {
    setStatus(` · widescreen rejected: ${e}`);
  }
});

$('coopChk').addEventListener('change', () => setCoop($('coopChk').checked));

$('clipChk').addEventListener('change', () => {
  try {
    emu.set_fill_left_clip($('clipChk').checked);
    syncCanvas();
    setStatus(` · fill left edge ${$('clipChk').checked ? 'on' : 'off'}`);
  } catch (e) {
    setStatus(` · fill left edge: ${e}`);
  }
});

$('rclipChk').addEventListener('change', () => {
  try {
    emu.set_fill_right_clip($('rclipChk').checked);
    syncCanvas();
    setStatus(` · fill right edge ${$('rclipChk').checked ? 'on' : 'off'}`);
  } catch (e) {
    setStatus(` · fill right edge: ${e}`);
  }
});

$('zoomIn').addEventListener('click', () => setZoom(zoom + 1));
$('zoomOut').addEventListener('click', () => setZoom(zoom - 1));

// --- HD graphics packs --------------------------------------------------------
// One code path for a human and for QA: raw (name, bytes) pairs go into wasm,
// which finds pack.json, strips the picker's folder prefix and decodes the PNG
// sheets in Rust. Nothing is uploaded and JS never touches image data.

// Scales above this warn before they are applied: 4x widescreen composes
// 1728x960 RGBA per frame and a 4x pack holds ~1 MiB of decoded sheet per CHR
// page, so it is a deliberate choice rather than an accident.
const HD_SCALE_WARN = 3;

function hdSay(text, cls = '') {
  const el = $('hdStatus');
  el.className = `sub ${cls}`.trim();
  el.textContent = text;
}

// Re-read the wasm side and redraw the panel. Never trusts local bookkeeping:
// a pack's own scale can override the requested one.
function syncHdPanel() {
  if (!emu) return;
  const supported = emu.hd_supported();
  $('hdDir').disabled = !supported || netActive;
  $('hdScale').disabled = !supported || netActive;
  if (!supported) {
    $('hdBox').classList.add('unsupported');
    $('hdClear').disabled = true;
    hdSay('HD packs are not in this build — rebuild with `--features hd` (see site/README).');
    return;
  }
  const info = JSON.parse(emu.hd_pack_info());
  $('hdClear').disabled = !info || netActive;
  const eff = emu.output_scale(), want = emu.requested_scale();
  $('hdScale').value = String(want);
  const size = `${emu.frame_width()}×${emu.frame_height()}`;
  if (!info) {
    hdSay(`no pack: original art at ${eff}× (${size}).`);
    return;
  }
  const over = eff !== want ? ` — the pack's ${eff}× wins over the ${want}× you picked` : '';
  hdSay(
    `pack "${info.name}"${info.author ? ` by ${info.author}` : ''}: ${info.scale}×, ` +
    `${info.tiles} tiles, ${info.variants} variants, ${info.sheets} sheet(s) → ${size}${over}`,
  );
}

// Load a pack from [{ name, bytes }]. Returns the info object, or throws with
// the wasm message. A failure leaves the previous presentation alone: the tab
// keeps playing with whatever art it had.
function loadHdFiles(files) {
  emu.hd_pack_begin();
  for (const f of files) emu.hd_pack_add_file(f.name, f.bytes);
  const info = JSON.parse(emu.hd_pack_commit());
  syncCanvas();
  syncHdPanel();
  return info;
}

$('hdDir').addEventListener('change', async (e) => {
  const picked = [...e.target.files];
  if (!picked.length) return;
  hdSay(`reading ${picked.length} file(s)…`);
  try {
    // `webkitRelativePath` is the path inside the picked folder, which is what
    // HdPack::from_files wants; plain `name` is the fallback for a QA drop of
    // loose files.
    const files = [];
    for (const f of picked) {
      files.push({
        name: f.webkitRelativePath || f.name,
        bytes: new Uint8Array(await f.arrayBuffer()),
      });
    }
    const info = loadHdFiles(files);
    setStatus(` · HD pack "${info.name}" loaded`);
  } catch (err) {
    // Loud but harmless: say exactly what was wrong and keep playing. A failed
    // load never disturbs what is already on screen, so name that rather than
    // claiming a fallback to the original art that may not have happened.
    const kept = JSON.parse(emu.hd_pack_info());
    hdSay(
      `pack rejected: ${err} — still playing with ` +
      (kept ? `the "${kept.name}" pack.` : 'the original art.'),
      'err',
    );
    syncCanvas();
  }
  e.target.value = ''; // let the same folder be re-picked after an edit
});

$('hdScale').addEventListener('change', () => {
  const n = Number($('hdScale').value) >>> 0;
  if (n >= HD_SCALE_WARN) {
    hdSay(`${n}× is expensive: about ${(0.14 * n * n).toFixed(1)} ms per frame at 256 wide ` +
      `(more in widescreen) plus ~${(0.0625 * n * n).toFixed(2)} MiB of decoded sheet per CHR page.`,
    'warn');
  }
  try {
    emu.set_output_scale(n);
    syncCanvas();
    syncHdPanel();
  } catch (e) {
    hdSay(`scale ${n}× rejected: ${e}`, 'err');
    $('hdScale').value = String(emu.requested_scale());
  }
});

$('hdClear').addEventListener('click', () => {
  try {
    emu.hd_pack_clear();
    syncCanvas();
    syncHdPanel();
    setStatus(' · back to the original art');
  } catch (e) {
    hdSay(`could not clear the pack: ${e}`, 'err');
  }
});

// --- online co-op -------------------------------------------------------------
// Connecting is shown as numbered stages, each with how long it has lasted, so
// a slow step is visible as exactly that step. `ns.link` comes from the
// transport (z2_net::ConnectStage::as_str).
const NET_STAGES = {
  signalling: '1/4 connecting to the signal server',
  waiting: '2/4 in the room, waiting for the other player',
  linking: '3/4 other player found, establishing the peer link',
  connected: '4/4 connected, starting the session',
};

function netStatusLine(ns) {
  if (!ns.supported) {
    return 'netplay: not in this build — serve a bundle built with `make run-web-net`';
  }
  if (!ns.active) return 'netplay: idle — enter a room, then Host or Join';
  const who = ns.role === 'host' ? 'host (P1)' : 'guest (P2)';
  if (!ns.started) {
    if (ns.state === 'closed' || ns.state === 'desynced') {
      return `netplay: ${who} could not connect — ${ns.closeReason || ns.error || ns.state}`;
    }
    const stage = ns.state === 'handshake' ? NET_STAGES.connected
      : (NET_STAGES[ns.link] || NET_STAGES.signalling);
    return `netplay: ${who} ${stage}… ${(ns.linkMs / 1000).toFixed(1)} s ` +
      '(the game keeps running; Leave cancels)';
  }
  const rtt = ns.rttMs === null || ns.rttMs === undefined ? '?' : `${ns.rttMs}ms`;
  let s;
  if (ns.mode === 'rollback') {
    s = `netplay: ${who} rollback ${ns.state} f=${ns.frame} delay=${ns.delay} rtt=${rtt} ` +
      `rollbacks=${ns.rollbacks} pred=${ns.predictionDepth}/${ns.maxPrediction}`;
  } else {
    s = `netplay: ${who} lockstep ${ns.state} f=${ns.frame} ahead=${ns.remoteAhead} ` +
      `delay=${ns.delay} rtt=${rtt}`;
  }
  if (ns.started && ns.requestedDelay !== ns.delay) {
    s += ` (host chose ${ns.delay}, you asked for ${ns.requestedDelay})`;
  }
  if (ns.state === 'stalled' && ns.mode === 'rollback') s += ' — no packets from the peer';
  else if (ns.state === 'stalled') s += ` — peer paused ${(ns.stallMs / 1000).toFixed(1)} s`;
  if (ns.state === 'desynced') s += ' — DESYNC, the session is over';
  if (ns.closeReason) s += ` — closed (${ns.closeReason})`;
  if (ns.error && ns.error !== ns.closeReason) s += ` — ${ns.error}`;
  if (ns.state === 'closed' || ns.state === 'desynced') s += ' — press Leave to play on alone';
  return s;
}

// Anything that rewinds, replaces or re-times state desyncs a lockstep session,
// so those controls are locked for its whole lifetime.
// Anything that changes the pixel pipeline is fine mid-session (both peers may
// look different), but anything that rewinds or replaces game state is not —
// and a pack load stalls the tab for a moment, which a lockstep peer feels as a
// stall, so the pack picker is locked too.
const NET_LOCKED = ['movieFile', 'movieRewind', 'snapSave', 'snapLoad', 'sramLoad',
  'snapImport', 'pauseBtn', 'coopChk', 'hdDir', 'hdClear', 'netMode'];

// The panel's Mode selector is the source of truth for the next session.
// Rollback allows an input delay of 0-3 (lockstep 0-8), as on the desktop, so
// larger delays are disabled in rollback mode.
const ROLLBACK_MAX_DELAY = 3;
function syncNetMode() {
  const rollback = $('netMode').value === 'rollback';
  const sel = $('netDelay');
  for (const o of sel.options) o.disabled = rollback && Number(o.value) > ROLLBACK_MAX_DELAY;
  if (rollback && Number(sel.value) > ROLLBACK_MAX_DELAY) sel.value = String(ROLLBACK_MAX_DELAY);
  if (!emu) return;
  try {
    emu.net_set_mode($('netMode').value, JSON.parse(emu.net_mode()).maxPrediction);
  } catch (e) {
    $('netStatus').textContent = `netplay: ${e}`;
  }
}
$('netMode').addEventListener('change', syncNetMode);

function syncNetButtons() {
  const romOk = !!emu && emu.rom_loaded();
  const supported = !!emu && emu.net_supported();
  $('netHost').disabled = netActive || !romOk || !supported;
  $('netJoin').disabled = netActive || !romOk || !supported;
  $('netLeave').disabled = !netActive;
  for (const id of NET_LOCKED) {
    const el = $(id);
    if (el) el.disabled = netActive;
  }
  syncHdPanel(); // re-derives hdDir/hdScale/hdClear from support + netActive
}

function netConnect(isHost) {
  if (!emu || !emu.rom_loaded()) {
    $('netStatus').textContent = 'netplay: drop a ROM first — both peers must run the same one';
    return;
  }
  const signal = $('netSignal').value.trim();
  const room = $('netRoom').value.trim();
  const ice = $('netIce').value.trim();
  // The browser blocks ws:// from an https page with an opaque failure; say so
  // plainly instead of letting it surface as a generic transport error.
  if (location.protocol === 'https:' && signal.startsWith('ws://')) {
    $('netStatus').textContent =
      'netplay: this page is https, so the signal URL must be wss:// (put z2-signal behind a TLS proxy)';
    return;
  }
  try {
    syncNetMode();
    emu.net_connect(signal, room, isHost, +$('netDelay').value, ice);
  } catch (e) {
    $('netStatus').textContent = `netplay: ${e}`;
    return;
  }
  netActive = true;
  netLastT = 0;
  // A paused page never calls net_poll, so a session started while paused could
  // never connect.
  setPaused(false);
  syncNetButtons();
  $('netStatus').textContent = `netplay: ${NET_STAGES.signalling} (room '${room}')…`;
}

// Drop the session (any stage) and say why. Safe to call more than once.
function endNetSession(message) {
  emu.net_disconnect();
  netActive = false;
  netLastT = 0;
  syncNetButtons();
  $('netStatus').textContent = message;
}

$('netHost').addEventListener('click', () => netConnect(true));
$('netJoin').addEventListener('click', () => netConnect(false));
$('netLeave').addEventListener('click', () => endNetSession('netplay: left the session'));

// --- go -----------------------------------------------------------------------
boot();
requestAnimationFrame(loop);
