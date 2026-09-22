# Playing in the browser

```sh
make run-web   # wasm-pack build, then serve crates/z2-web/site on :8080 (PORT= to change)
```

Open `http://localhost:8080/` and drop your `.nes` file on the page. The page checks the ROM hash inside the tab and never uploads the file.

The web build runs the same engine as the desktop app, in a tab, with no npm and no bundler. The keys are the same too: `Z` is A and `X` is B, and player 2 uses `G`/`F`/`R`/`T` and `W`/`A`/`S`/`D`. Every desktop feature has a control in the page, and most can also be set from the URL:

| In the page | As a URL parameter |
|---|---|
| Screen: Standard, Wide 16:10 (384x240), Wide 16:9 (432x240) | `?widescreen=` or `?wide=`: `off`, `16:10`, `16:9`, or 1 to 16 tiles per side |
| Fill left edge checkbox (paints the 8 columns the ROM blanks) | `?clip=0` turns it off |
| Local co-op checkbox | `?coop=1` |
| HD pack folder picker: choose the directory that holds `pack.json` | none |
| Scale 1x to 4x (the page warns about the cost above 2x), plus a separate CSS zoom | `?scale=`, `?zoom=` |
| Online co-op: mode (rollback by default, or lockstep), signal URL, room, delay, ICE, Host (P1) / Join (P2) / Leave | `?net=lockstep`, `?signal=`, `?room=`, `?ice=`, `?delay=` |

The canvas sizes itself from the emulator and works down to phone width. HD pack PNGs are decoded inside wasm. If a pack fails to load, the page says so in red and the game keeps playing with the original art. During online play, a status line shows your role, the frame, the delay, stalls, and the reason the session closed.

More detail about the page, its `window.z2` scripting API and its tests is in [crates/z2-web/site/README](crates/z2-web/site/README).

## Optional features and bundle size

HD packs and online co-op are cargo features (`hd` and `netplay`). The `net` feature alone gives you the netplay protocol without the WebRTC transport.

```sh
make run-web-net   # wasm-pack ... -- --features hd,netplay
```

The default bundle is about 295 KB of wasm and already includes widescreen and local co-op, which need no extra dependencies. `hd` and `netplay` each add about 279 KB, and everything together is about 794 KB. Every exported method exists in every build. `hd_supported()` and `net_supported()` return `false` when a feature is off, so the page needs no build-time branching.

## Testing online co-op in real browsers

```sh
make netplay-e2e-setup    # once: npm install playwright and download chromium (dev only)
make netplay-e2e          # needs Z2_ROM and a .fm2 input track
```

This test hosts a session in one browser window and joins it from another. It then checks that a key held in one window moves the matching player in the other window's game state, and moves it back when the opposite key is held. Both windows must finish on the same frame with the same state hash and no desyncs. Playwright is a test dependency only. The site itself still has no npm dependency. `cargo test -p z2-net --features matchbox` proves the same thing without a browser.

The page uses rollback by default. `make netplay-e2e-rollback` runs the same kind of test over a deliberately bad simulated link (50 ms of latency with 20 ms of jitter and 5% packet loss each way, injected inside the page's transport). Both windows have to roll back, a key has to reach the pressing window's own simulation after exactly the input delay, input has to cross the wire in both directions, and every confirmed-frame state hash has to agree. The test also prints what rollback costs in wasm. A worst-case 8-frame rollback tick takes about 4 ms in headless Chromium on an Apple Silicon Mac.

To play online co-op by yourself on one machine:

```sh
make netplay-play         # two browser windows side by side, already joined
```

The left window is player 1 (the host) and the right one is player 2. Arrows move, `Z` jumps, `X` attacks and `Enter` is Start. Close both windows or press Ctrl-C to stop.
