// Helpers shared by the netplay QA scripts (netplay-e2e-rollback.mjs,
// netplay-play.mjs). Not part of the site: nothing in index.html / app.js /
// worklet.js imports it.

import { spawn } from 'node:child_process';
import { stat } from 'node:fs/promises';
import { connect, createServer } from 'node:net';
import { join, resolve } from 'node:path';

import { SITE } from './qa-server.mjs';

/** Repository root. */
export const REPO = resolve(SITE, '..', '..', '..');

/** A free loopback TCP port (closed again before it is returned). */
export async function freePort() {
  const s = createServer();
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  const { port } = s.address();
  await new Promise((r) => s.close(r));
  return port;
}

/**
 * Start `z2-signal` on a free loopback port and wait until it accepts.
 * Resolves to `{ url, port, proc, log(), exited(), kill() }`; rejects with a
 * message (and kills the process) when it never listens.
 */
export async function startSignal(bin = process.env.Z2_SIGNAL_BIN ||
  join(process.env.CARGO_TARGET_DIR || join(REPO, 'target'), 'release', 'z2-signal')) {
  await stat(bin);
  const port = await freePort();
  const proc = spawn(bin, ['--bind', `127.0.0.1:${port}`], { stdio: ['ignore', 'pipe', 'pipe'] });
  const lines = [];
  proc.stdout.on('data', (b) => lines.push(String(b)));
  proc.stderr.on('data', (b) => lines.push(String(b)));
  let exit = null;
  proc.on('exit', (c) => { exit = c; });
  const kill = () => { if (exit === null) proc.kill('SIGKILL'); };
  for (let i = 0; ; i++) {
    if (exit !== null) throw new Error(`z2-signal exited with ${exit} before it listened:\n${lines.join('')}`);
    try {
      await new Promise((ok, no) => {
        const s = connect(port, '127.0.0.1', () => { s.destroy(); ok(); });
        s.on('error', no);
      });
      break;
    } catch {
      if (i > 600) { kill(); throw new Error(`z2-signal never listened on ${port} within 30 s:\n${lines.join('')}`); }
      await new Promise((r) => setTimeout(r, 50));
    }
  }
  return {
    url: `ws://127.0.0.1:${port}`, port, proc, kill,
    log: () => lines.join(''),
    exited: () => exit,
  };
}

// `.fm2` pad columns are left-to-right RLDUTSBA; NES bits A,B,Select,Start,
// Up,Down,Left,Right = 0..7 (the contract crates/z2-web/src/movie.rs uses).
const FM2_COL_BITS = [7, 6, 5, 4, 3, 2, 1, 0];

/** Player-1 pad track of an `.fm2` movie. */
export function parseFm2Pads(text) {
  const pads = [];
  for (const line of text.split('\n')) {
    if (!line.startsWith('|')) continue;
    const field = line.split('|')[2];
    if (field === undefined || field.length !== 8) continue;
    let bits = 0;
    for (let i = 0; i < 8; i++) if (field[i] !== '.' && field[i] !== ' ') bits |= 1 << FM2_COL_BITS[i];
    pads.push(bits);
  }
  return Uint8Array.from(pads);
}

/** Load ROM bytes (base64) into a page through its own file input. */
export async function loadRomInto(page, romB64) {
  await page.evaluate(async (b64) => {
    const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bin], 'zelda2.nes'));
    const input = document.getElementById('romFile');
    input.files = dt.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, romB64);
  await page.waitForFunction(() => JSON.parse(window.z2.state()).romLoaded === true, null, { timeout: 30000 });
}

/** World X in pixels, so a page boundary is not read as a 256 px jump. */
export const worldX = (p) => (p === null || p === undefined ? null : p.page * 256 + p.x);
