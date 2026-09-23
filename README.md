# z2rs

z2rs is a clean-room-style reconstruction of Zelda II: The Adventure of Link (NES) in Rust. It runs as a desktop app and in the browser (WebAssembly), and it is checked frame by frame against the original ROM running in a reference emulator.

You bring your own ROM. Nothing from the original game is distributed here. [PROVENANCE.md](PROVENANCE.md) records what reference material the project worked from and where every file in this repository came from, and [LEGAL.md](LEGAL.md) has the rules contributors follow.

On top of the original game, z2rs adds a few optional extras:

- widescreen, with real scenery in the margins instead of a stretched picture
- HD graphics packs, so you can repaint tiles and sprites at 2x to 8x
- two-player co-op with a second Link, on one machine or online
- save states, fast-forward, movie playback and gamepad support

Release announcement: [Moddable Zelda 2 PC / Web port with Online Multiplayer Co-op and Widescreen Released](https://x.com/troygentic/status/2101572848506573135). Chat is on [Discord](https://discord.gg/buZrPenm6K).

## Download

Builds for Linux, macOS (Intel and Apple Silicon) and Windows are on the [releases page](https://github.com/troyedwardsjr/z2rs/releases). Unpack the archive for your system and run the game with your own ROM:

```sh
./z2rs --rom /path/to/zelda2.nes
```

Only one dump passes the check, identified by the hash of the ROM body with the 16-byte iNES header stripped: CRC32 `BA322865`, SHA1 `11333adb723a5975e0ecca3aee8f4747aa8d2d26` (No-Intro USA). The file is only ever read in place. You can also start the app with no ROM and drag one onto the window.

Nothing in the archive is code signed. macOS wants right click and Open the first time, Windows SmartScreen wants More info and Run anyway, and on Linux the file may need `chmod +x`. The second binary in the archive, `z2-signal`, is the signalling server for online co-op, and you only need it if you host a session yourself.

## Build from source

You need Rust stable (`cargo`, `rustc`). The browser build also needs the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and `wasm-pack`.

```sh
make build          # cargo build --workspace (no ROM needed)
make test           # cargo test --workspace (tests that need a ROM skip without Z2_ROM)
export Z2_ROM=/path/to/zelda2.nes
make run            # play the optimized desktop build
make run-web        # build the wasm bundle and serve it on :8080
```

`make help` lists every target. A target that needs `Z2_ROM` prints what to set and exits with status 2 when it is missing.

## Controls

| Action | Player 1 | Player 2 (co-op) |
|---|---|---|
| A | `Z` | `G` |
| B | `X` | `F` |
| Start | `Enter` | `T` |
| Select | `Shift` (either one) | `R` |
| D-pad | arrow keys | `W` `A` `S` `D` |

`Tab` fast-forward, `F5` save state, `F7` load state, `F6` next save slot, `P` pause, `.` single-step while paused, `Esc` quit. Gamepads work too.

## Guides

| Guide | What is in it |
|---|---|
| [guide/desktop.md](guide/desktop.md) | every key, save-state slots, command-line flags, the config file, gamepads |
| [guide/browser.md](guide/browser.md) | the web build, its URL parameters, optional features and bundle size |
| [guide/co-op.md](guide/co-op.md) | two Links locally or online, widescreen, how both work and what they cannot do |
| [guide/hd-packs.md](guide/hd-packs.md) | making a pack by painting over spritesheets of the game (`make hd-sheets`, `make hd-pack`) and playing with it |
| [guide/development.md](guide/development.md) | repository layout, headless runs, and how the port is checked against the original |

## Contributing and legal

[CONTRIBUTING.md](CONTRIBUTING.md) covers setup, the pre-commit hook and the rules for tests that need a ROM. [LEGAL.md](LEGAL.md) explains what may never be committed. Read it before you open a pull request.

## License

The z2rs source code is released under the MIT license ([LICENSE](LICENSE)). The license does not cover the original game, which you must supply yourself.
