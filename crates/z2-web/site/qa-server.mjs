// Tiny static file server shared by the site's QA scripts (smoke.mjs,
// netplay-e2e.mjs). Not part of the site: nothing in index.html / app.js /
// worklet.js imports it, and `make run-web` serves with python3 instead.
//
// Two things it has to get right (see site/README "Serve"):
//   * `.wasm` must be `application/wasm`, or instantiateStreaming fails and
//     app.js reports the bundle as missing;
//   * `/` must map to index.html. `join(SITE, '/')` DROPS the trailing slash,
//     so comparing a joined path against `SITE + '/'` never matches and `/`
//     ends up reading the directory itself — Chromium then treats the reply as
//     a download and `page.goto` fails. Decide on the URL pathname instead.

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { join, dirname, extname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Absolute path of the static site directory (this file's own directory). */
export const SITE = dirname(fileURLToPath(import.meta.url));

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
  '.png': 'image/png',
  '.css': 'text/css; charset=utf-8',
};

/**
 * Serve `root` (default: the site directory) on a free loopback port.
 *
 * Resolves to `{ port, origin, close() }`. Call it twice to get two ports:
 * a different port is a different origin, which is how netplay-e2e.mjs gives
 * its two peers separate storage and separate IndexedDB rather than two views
 * of one page. Bound to 127.0.0.1 only — never reachable off this machine.
 */
export async function serveSite(root = SITE) {
  const base = resolve(root);
  const server = createServer(async (req, res) => {
    try {
      const rel = decodeURIComponent(new URL(req.url, 'http://x').pathname);
      const path = rel === '/' ? join(base, 'index.html') : join(base, rel);
      // Path traversal: require the resolved path to stay under `base`. The
      // `sep` guard stops `/siteevil` passing a bare startsWith(base) check.
      if (path !== base && !path.startsWith(base + sep)) {
        res.writeHead(403);
        res.end();
        return;
      }
      const body = await readFile(path);
      res.writeHead(200, {
        'Content-Type': MIME[extname(path)] || 'application/octet-stream',
        'Cache-Control': 'no-store',
      });
      res.end(body);
    } catch {
      res.writeHead(404);
      res.end('nope');
    }
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const { port } = server.address();
  return {
    port,
    origin: `http://127.0.0.1:${port}`,
    close: () => new Promise((r) => server.close(r)),
  };
}
