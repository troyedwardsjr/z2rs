# z2rs

### Read the release announcment: [Moddable Zelda 2 PC / Web port with Online Multiplayer Co-op and Widescreen Released](https://x.com/troygentic/status/2101572848506573135)
### Join our Discord: [![Discord](https://img.shields.io/discord/1426351656144212102?color=7289DA&logo=discord&logoColor=white)](https://discord.gg/buZrPenm6K)


z2rs is a clean-room-style reconstruction of Zelda II: The Adventure of Link (NES) in Rust. It runs as a desktop app and in the browser (WebAssembly), and it is checked frame by frame against the original ROM running in a reference emulator.

You bring your own ROM. Nothing from the original game is distributed here. [PROVENANCE.md](PROVENANCE.md) records what reference material the project worked from and where every file in this repository came from, and [LEGAL.md](LEGAL.md) has the rules contributors follow.

On top of the original game, z2rs adds a few optional extras:

- widescreen, with real scenery in the margins instead of a stretched picture
- HD graphics packs, so you can repaint tiles and sprites at 2x to 8x
- two-player co-op with a second Link, on one machine or online
- save states, fast-forward, movie playback and gamepad support

## Requirements

- Rust stable (`cargo`, `rustc`).
- For the browser build: the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and `wasm-pack`.
- Your own dump of Zelda II (USA). Point the `Z2_ROM` environment variable at it. The file is only ever read in place. It is never copied into the repository and never committed. z2rs accepts one specific dump, identified by the hash of the ROM body with the 16-byte iNES header stripped: CRC32 `BA322865`, SHA1 `11333adb723a5975e0ecca3aee8f4747aa8d2d26` (No-Intro USA).

## Quick start

```sh
make build          # cargo build --workspace (no ROM needed)
make test           # cargo test --workspace (tests that need a ROM skip without Z2_ROM)
export Z2_ROM=/path/to/zelda2.nes
make run            # play the optimized desktop build
```

`make help` lists every target. A target that needs `Z2_ROM` prints what to set and exits with status 2 when it is missing.

## Play on the desktop

```sh
make run ROM=/path/to/zelda2.nes [MOVIE=/path/to/movie.bk2] [ARGS="--widescreen 16:9"]
# or: cargo run --release -p z2-native -- --rom "$Z2_ROM" [--movie M.fm2|.bk2]
```

Play the release build. `make run` uses it already. `make run-debug` exists for debugging the interpreter, and it is slow: the debug interpreter manages about seven game frames per second, so reaching the title screen takes roughly 90 seconds. If the release window stays grey, look in the terminal for a `present render:` line (a GPU surface problem) or an `emulator wedged` line (the port diverged).

Without a ROM the app starts in a synthetic mode with no cartridge. You can also drag a ROM or a movie onto the running window. Movies play back without verification: their input bytes feed the emulator and nothing is compared.

### Controls

| Action | Player 1 | Player 2 (co-op) |
|---|---|---|
| A | `Z` | `G` |
| B | `X` | `F` |
| Start | `Enter` | `T` |
| Select | `Shift` (either one) | `R` |
| D-pad | arrow keys | `W` `A` `S` `D` |

Both Shift keys are Select, because Select is how you cast a spell and the
left one is easier to reach while the right hand is on the arrow keys.

Other keys: `Tab` fast-forward, `F5` save state, `F7` load state, `P` pause,
`.` single-step while paused, `Esc` quit.

There are ten save-state slots. `F6` cycles through them and the digits `1` to
`9` and `0` pick one directly. `F5` and `F7` use the selected slot, and the
window title shows ` [SLOT n]` while it is anything other than 0. Each slot is
a separate `savestate<n>.z2snap` file in the data directory.

### Command-line flags

Besides `--rom`, `--movie` and `--config`:

| Flag | Effect |
|---|---|
| `--widescreen off\|16:10\|16:9\|N` | extra scenery left and right of the NES picture |
| `--coop-local` | two players at this machine |
| `--coop-host ROOM` / `--coop-join ROOM` | online co-op over WebRTC |
| `--signal URL` | signalling server (default `ws://127.0.0.1:3536`) |
| `--net-mode rollback\|lockstep` | online sync mode (default `rollback`; config key `netplay.mode`) |
| `--net-delay N` | netplay input delay in frames, default 2 (rollback 0 to 3, lockstep 0 to 8) |
| `--p2-pad INDEX` | pin player 2 to a gamepad by connection order |
| `--fill-left-clip on\|off` | paint the 8 columns the overworld blanks at x 0 to 7 (default on) |
| `--fill-right-clip on\|off` | paint the 8 columns the overworld masks at x 248 to 255 (default on) |
| `--hd-pack DIR` | HD graphics pack (the directory with `pack.json`); `''` turns it off |
| `--hd-scale N` | output multiplier 1 to 8 (default 1) |
| `--hd-record DIR` | on exit, write a template pack of the tiles this session drew |

`--help` lists all of these. An unknown flag is a usage error (exit 2).

### Config file

The same settings live in `<data-dir>/z2-native.json`: `widescreen`, `widescreen_fill_left_clip`, `widescreen_fill_right_clip`, `hd_pack`, `hd_scale`, `hd_record`, `coop_local`, `keys_p2`, `gamepad`, `gamepad_p2`, `gamepad_p2_index`, and a `netplay` object (`mode`, `signal_url`, `input_delay`, `stall_timeout_ms`, `ice_url`, `ice_username`, `ice_credential`). Command-line flags win over the file. Older config files still load, because every newer key has a default.

The data directory is `$XDG_DATA_HOME/z2rs` when `XDG_DATA_HOME` is set. Otherwise it is `~/Library/Application Support/z2rs` on macOS, `%APPDATA%/z2rs` on Windows and `~/.local/share/z2rs` elsewhere. Save states, battery saves and the config file live there. The ROM is never stored there.

### Gamepads

Gamepads work through `gilrs`. The pad that most recently pressed a button drives player 1.

| NES button | Pad input |
|---|---|
| A | East (right face button) |
| B | South (bottom face button) |
| Select / Start | Select / Start |
| D-pad | D-pad, mirrored on the left stick (0.5 deadzone) |

The mapping is positional, like the NES pad, where B sits left of and below A. On an Xbox pad the B button is NES A and the A button is NES B. On PlayStation, Circle is A and Cross is B. On a Switch Pro controller, A and B match.

On macOS, some pads are claimed by Apple's own drivers (the Xbox One S controller over USB or Bluetooth, for example) and report nothing through `gilrs`. z2rs reads those through the GameController framework instead, and they drive player 1. The Retrolink SNES controller and its clones get a mapping fix so the d-pad reads correctly. Pads can be connected and disconnected while the game runs. Inputs with no NES equivalent (second stick, triggers, extra face buttons) are ignored.

Testing status: the mapping, the bit layout (`A,B,Select,Start,Up,Down,Left,Right` = bits 0..7), the deadzone and the hot-plug path are covered by unit tests and code review only. Nobody has tested them on physical gamepads yet, and enumerating several pads at once in particular has not been tried on real hardware. Custom gamepad bindings in the config (`gamepad`, `gamepad_p2`) are honoured, and the defaults are the positional layout above. Keyboard rebinding covers the keys named in the two default tables.

## Play in the browser

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

### Optional features and bundle size

HD packs and online co-op are cargo features (`hd` and `netplay`). The `net` feature alone gives you the netplay protocol without the WebRTC transport.

```sh
make run-web-net   # wasm-pack ... -- --features hd,netplay
```

The default bundle is about 295 KB of wasm and already includes widescreen and local co-op, which need no extra dependencies. `hd` and `netplay` each add about 279 KB, and everything together is about 794 KB. Every exported method exists in every build. `hd_supported()` and `net_supported()` return `false` when a feature is off, so the page needs no build-time branching.

### Testing online co-op in real browsers

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

## HD graphics packs

A pack is a directory of PNG sheets plus a `pack.json`. It replaces the game's 8x8 tiles and sprites with art drawn at 2x to 8x. You start a pack from your own ROM's graphics and paint over the cells.

```sh
cargo xtask hdpack template --out ~/z2-art/mypack --scale 2   # start from your ROM's CHR
# ...paint the sheets...
cargo xtask hdpack check --pack ~/z2-art/mypack               # validate
cargo run --release -p z2-native -- --rom "$Z2_ROM" --hd-pack ~/z2-art/mypack --hd-scale 2
```

`--hd-record DIR` works the other way round. Play normally, and when you quit with `Esc` or by closing the window, z2rs writes a template pack that contains exactly the `(page, tile, palette)` combinations that session drew. That is a much smaller starting point than all 32 CHR pages. Only those two ways of quitting save the recording. A `kill` loses it.

In the browser, the folder picker loads the same packs in a build that has the `hd` feature. Choose the directory that holds `pack.json`.

Template sheets are rendered from your ROM, so they must stay outside this repository. The tooling refuses to write them inside a git work tree (see [LEGAL.md](LEGAL.md)). `cargo xtask hdpack --help` covers the rest of the workflow, and the module docs in `crates/z2-render/src/pack.rs` describe the `pack.json` format.

## Co-op and widescreen

z2rs can run two Links, locally or over the internet, and it can paint extra scenery beside the NES picture. Both are off unless you ask for them, and neither touches the verification path. The game only sees a second controller when you turn co-op on.

```sh
# two players, one machine, 16:9
cargo run --release -p z2-native -- --rom "$Z2_ROM" --coop-local --widescreen 16:9

# online: one signalling server, one host, one guest
cargo run --release -p z2-signal                                     # machine S
cargo run --release -p z2-native -- --rom "$Z2_ROM" --coop-host myroom --signal ws://S:3536
cargo run --release -p z2-native -- --rom "$Z2_ROM" --coop-join myroom --signal ws://S:3536
```

## How the features work

### Widescreen

The NES draws 256 pixels across because that is all the hardware has: two screens of background map, scrolled. z2rs does not stretch that picture. While the software PPU renders a frame, it records what it drew on every scanline: which tile, from which CHR page, in which palette, at which scroll position. After the frame ends, z2rs decodes the extra columns straight from the game's own memory. Side-view areas come from the level RAM the game builds at `$6000`, and the overworld comes from the run-length map it keeps in WRAM at `$7C00`. Those tiles go into the margins with the palette that scanline was using, so the scenery continues instead of repeating or blurring.

The centre 256 columns are copied byte for byte from the normal frame. That copy is what keeps widescreen out of the verification path, and the game never learns the screen got wider.

Widescreen also recovers two black strips. The game blanks its leftmost 8 pixels with a mask bit, and on the overworld it hides its rightmost 8 pixels behind a column of opaque black sprites. z2rs repaints both from the same tile data, so the picture runs edge to edge without the seams the original hardware needed.

### HD packs

Three things identify every tile the PPU draws: the CHR page it came from, its tile index, and the three opaque colours of the palette it was drawn in. A pack maps those keys to cells in a PNG sheet. Where a tile has no cell, z2rs scales up the original art instead, so a half-finished pack plays fine and you can repaint the game a few tiles at a time.

Replacement art never changes how the game behaves. The pack's alpha channel decides which HD pixels get painted, but the opacity of the original pixels still decides sprite priority, background priority and sprite-zero timing. The game logic keeps running on the ROM's own graphics while the screen shows yours.

`hdpack template` writes sheets for all 32 CHR pages of your ROM. `--hd-record` does the reverse: play, quit, and get a pack with exactly the tile and palette combinations that session drew.

### Co-op (two Links)

Nobody wrote a second character. Every frame, the game's own player update and draw routines run a second time with player 2's state swapped into the memory addresses the original code reads, and the state is swapped back out afterwards. Player 2 gets the real physics, sword collision and damage handling, because it is the ROM's own code running twice.

The camera follows player 1, and player 2 is clamped into the visible screen so you cannot strand each other. The clamp does no collision check, so a fast-scrolling player 1 can drag player 2 through solid tiles.

Player 2 only exists in side-view areas. On the overworld there is no player 2: player 1 walks the map, and player 2 reappears beside them in the next area. The overworld runs a different driver built around a single map position, and entering and leaving areas is global state, so giving each player a map position would be a much larger change. The same goes for the title screen, file select, area loads and the death screen. While player 2 is hidden, the window title says `[coop P2 hidden]`.

The swap restores every byte it touched, so with co-op off the original game is byte for byte identical. That was checked against the reference emulator across thousands of frames from three different TAS runs.

### Online play

Online play uses rollback by default, on the desktop and in the browser. Input-delay lockstep is still available with `--net-mode lockstep`, the Mode select in the page, or `?net=lockstep`.

With rollback, your own input applies after a short delay (2 frames by default) and the other player's input is predicted until it arrives. When a prediction turns out wrong, z2rs re-simulates the frames since then from a saved state, within the same tick.

Both machines run the same build and the same frame numbers. Only controller bytes cross the wire, never positions or sprites. The host is player 1. Each tick, a peer sends its input for a frame a few frames ahead (`--net-delay`, default 2) so the packet has time to arrive before that frame is stepped. Inputs are acknowledged and resent, with a reliable channel behind the unreliable one, so a dropped packet causes a stall and not a desync. Every 60 frames each side hashes RAM, WRAM and OAM and compares the result with the other side, so a divergence gets reported instead of going unnoticed.

The transport is WebRTC data channels through matchbox. The desktop app and the browser tab use the same path and can play each other. `z2-signal` is the introduction service. It allows two peers per room and refuses a third, and it only carries the handshake. After that, traffic is peer to peer.

There is no late join. The guest receives the host's save memory when the session starts and both sides reset together. If the ROM hash, protocol version or feature flags differ, the handshake refuses to start.

## Limits

These apply to both the desktop app and the browser build.

Widescreen:

- The margins hold no sprites, so enemies and projectiles still appear at the original screen edge.
- Dialogue boxes and the pause and spell pane do not extend into the margins, and title and menu screens leave them blank.
- Where the level data runs out (the west end of a town, for instance) the margin falls back to the backdrop colour.
- The overworld's own edge blanking (`PPUMASK $18` on the left, a column of opaque black sprites on the right) is repainted by `--fill-left-clip` and `--fill-right-clip`, both on by default. With an HD pack loaded, the right-hand seam remains.

Co-op:

- Player 2 exists in side-view areas only, not on the overworld.
- Player 2 cannot use doors, elevators, NPCs, shops or spells.
- Enemies target player 1.
- There is one save and one set of stats.

Online play:

- There is no late join. Both peers restart from power-on with the host's save.
- Pause, fast-forward, save states and movies are disabled during a session, and a hash mismatch ends it.
- The guest autosaves to `sram-coop.sav` and never over its own `sram.sav`.
- The signalling server has no TLS and no authentication. Put it behind a reverse proxy for `wss://` (browsers on an https page require it) and use a room name nobody can guess, because anyone who knows the name can take the free slot.
- WebRTC reveals each peer's public IP address to the other peer and to the STUN server. Symmetric NATs need a TURN server (`netplay.ice_*` in the config).

HD packs:

- HD art cannot extend a sprite or a tile beyond its original opaque pixels. Substitution happens in place, per 8x8 cell and per sprite, and a transparent NES pixel stays transparent. That rules out bigger swords and outlines that spill past the original shape.
- Split and greyscale scanlines keep the original art. The compositor cannot tell which tile one half of a split line belongs to, so it upscales those lines instead.
- Widescreen and a pack together leave a seam of 1 to 7 pixels of original art at the left edge of the play field. This is a known gap in the `z2-render` compositor.

Building: the desktop `netplay` feature is on by default and pulls in `ring`, which builds C and assembly, so you need a C compiler. `cargo build -p z2-native --no-default-features` drops only the online transport. Widescreen, local co-op, movies and headless runs all still work. On the web the equivalent features (`hd`, `netplay`) are opt-in.

## Headless runs

These need no display, ROM or reference emulator. They run a synthetic `Game` with no cartridge:

```sh
cargo run -p z2-native -- --headless --frames 30 --dump facts.json --dump-frame out.png
cargo run -p z2-native -- --headless --movie track.bk2 --dump facts.json
```

`--frames 0` without `--movie` is the smoke path: it exports the initial facts and steps nothing. With `--movie` it runs the whole track. `--snapshot` seeds RAM from a raw 2048-byte image. Exit codes are 0 for a normal run or `--help`, 2 for a usage error and 4 for an I/O error. Exit code 3 is reserved.

## Checking the port against the original

z2rs replaces the game's 6502 routines with Rust one at a time. A 6502 interpreter runs the ROM, and a trap table hands control to the Rust version of a routine whenever the game calls one that has been ported. `ports.toml` lists every routine in the ROM and whether it has been ported yet. `cargo xtask coverage` prints the totals per bank.

To check that a port behaves like the original, `z2-verify` runs the ROM in a reference emulator (`tetanes-core` 0.15, called the oracle in this repository) next to z2rs. Both get the same controller input, and after every frame the verifier compares RAM, battery WRAM, sprite OAM, the palette and the framebuffer. It stops at the first byte that differs.

```sh
export Z2_ROM=/path/to/zelda2.nes
make verify-smoke                                                    # 30 frames with no input
cargo xtask verify --movie M --frames N --dut oracle --rom "$Z2_ROM" # the oracle against itself
cargo xtask verify --movie M --frames N --dut game --rom "$Z2_ROM"   # z2rs against the oracle (N=0: whole movie)
```

The comparison makes two allowances by default. `tetanes-core` 0.15 draws no sprite pixels in the rightmost column (x 255), and it draws sprites on scanline 0, where real hardware shows none, so the comparator skips framebuffer mismatches on those two edges. It also compares the stack page only above the live stack pointer, because the bytes below it are stale return addresses.

### Input movies

The long tests replay tool-assisted speedruns from TASVideos. The movies are not included here. Download them yourself into a directory outside the repository and point `Z2_CORPUS` at it (the Makefile defaults to a sibling `../z2-corpus`). Movies go in `$Z2_CORPUS/movies/`, and snapshots minted from them land in `$Z2_CORPUS/snapshots/`.

| Run | Authors | Length | Page |
|---|---|---|---|
| any% | Arc & Inzult | 34:58.74 | <https://tasvideos.org/4367M> |
| 100% | Arc & Inzult | 1:02:40.84 | <https://tasvideos.org/4425M> |
| warpless | Arc, FatRatKnight, Inzult & Rising_Tempest | 45:26.04 | <https://tasvideos.org/3254M> |
| warp glitch | TASeditor, Arc, Inzult & EZGames69 | 5:31.887 | <https://tasvideos.org/4234M> |

Both FCEUX `.fm2` and BizHawk `.bk2` files parse. `make corpus-mint` replays the movies in the oracle and saves labelled snapshots (boot, town entrances, spell pickups, palaces, bosses, the ending). `make fuzz-smoke SNAPSHOT=...` then starts from a snapshot, feeds both sides random input and looks for a divergence.

Every test that needs the ROM, the movies or a snapshot skips itself when that input is missing, so `cargo test --workspace` passes on a machine that has none of them.

## Repository layout

| Path | What it is |
|---|---|
| `crates/z2-core` | the game engine: 6502 interpreter, ported routines, RAM map accessors, game facts |
| `crates/z2-ppu` | software PPU (picture) model |
| `crates/z2-apu` | APU synth and the port of the game's sound engine |
| `crates/z2-assets` | ROM hash check and asset extractor |
| `crates/z2-render` | HD graphics packs, template sheets, the tile recorder |
| `crates/z2-net` | co-op netplay sessions (lockstep and rollback), optional WebRTC transport |
| `crates/z2-native` | desktop app |
| `crates/z2-web` | browser build and its static site |
| `crates/z2-debug` | debug overlay panels (egui) |
| `crates/z2-verify` | reference-emulator oracle, lockstep comparison, movie and snapshot tooling |
| `tools/xtask` | developer tasks: `ledger`, `coverage`, `verify`, `fuzz`, `probe`, `extract`, `hdpack`, `corpus` |
| `tools/z2-signal` | netplay signalling server |
| `third_party/z2disassembly` | community disassembly, used as a porting reference (git submodule) |
| `vendor/matchbox_socket` | matchbox 0.14 with three small patches, described in the root `Cargo.toml` |
| `ports.toml`, `ram-map.toml` | the routine ledger and the RAM map the accessors are generated from |

## Troubleshooting

- The ROM is rejected. Only the No-Intro USA dump passes the check (body CRC32 `BA322865`, SHA1 `11333adb...`). Dump your cartridge again. The desktop app reads the ROM directly. `assets.bin` is an optional product of the extractor and the app does not need it to start.
- A test or target says the corpus is missing. Movies and snapshots live outside the repository in `$Z2_CORPUS` and are never committed. `make corpus-mint` and `make fuzz-smoke` say exactly what they could not find.
- There is no sound. When no audio device is available, the desktop app prints `audio disabled: ...` and keeps running silently. The underrun meter in the title bar only means something while audio is up.
- A headless `--movie` run fails. An unknown file extension exits with 2 and an unreadable file with 4.
- The picture or timing differs from the oracle (flicker, the HUD split on the wrong row, a wrong palette or wrong tiles after a transition). The normal comparison stops at the first differing byte, so use the tolerant driver. `cargo xtask verify --movie M --frames N --dut game --continue --rom "$Z2_ROM"` reports frame by frame which regions disagree (RAM, WRAM, OAM, palette, frame). Add `--dump-at F --out DIR` to get PNGs from both sides along with PPU, mapper, OAM and nametable state. `--watch a,b` traces RAM bytes, and `--trap-set` and `--untrap` let you bisect the set of ported routines. For questions inside a single frame, such as where the NMI lands or when the sprite-0 flag rises, `cargo xtask probe --frame N --movie M --rom "$Z2_ROM"` walks that frame of the oracle instruction by instruction next to the port's PPU event trace.

## Contributing and legal

[CONTRIBUTING.md](CONTRIBUTING.md) covers setup, the pre-commit hook and the rules for tests that need a ROM. [LEGAL.md](LEGAL.md) explains what may never be committed. Read it before you open a pull request.

## License

The z2rs source code is released under the MIT license ([LICENSE](LICENSE)). The license does not cover the original game, which you must supply yourself.
