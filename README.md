# z2rs

z2rs is a clean-room-style reconstruction of Zelda II: The Adventure of Link (NES) in Rust. It runs as a desktop app, in the browser (WebAssembly) and on Android, and it is checked frame by frame against the original ROM running in a reference emulator.

You bring your own ROM. Nothing from the original game is distributed here. [PROVENANCE.md](PROVENANCE.md) records what reference material the project worked from and where every file in this repository came from, and [LEGAL.md](LEGAL.md) has the rules contributors follow.

On top of the original game, z2rs adds a few optional extras:

- widescreen, with real scenery in the margins instead of a stretched picture
- HD graphics packs, so you can repaint tiles and sprites at 2x to 8x
- two-player co-op with a second Link, on one machine or online
- save states, fast-forward, movie playback and gamepad support
- a randomizer that builds a new game from your ROM, with shuffled or generated overworlds, palaces, items, enemies and spells
- optional gameplay and display changes, many inspired by HoverBat's ZALiA remake, with an in-game options menu

Release announcement: [Moddable Zelda 2 PC / Web port with Online Multiplayer Co-op and Widescreen Released](https://x.com/troygentic/status/2101572848506573135). Chat is on [Discord](https://discord.gg/buZrPenm6K).

## Play it

1. Download the archive for your system from the [releases page](https://github.com/troyedwardsjr/z2rs/releases). There are builds for Linux, macOS (Intel and Apple Silicon) and Windows. Unpack it.
2. Start the launcher: `z2rs-launcher.exe` on Windows, `z2rs.app` on macOS, `z2rs-launcher` on Linux.
3. Pick your Zelda II (USA) ROM and press Play.

The launcher also sets the window size, fullscreen, widescreen (16:10, 16:9 or 21:9 ultrawide), an HD pack folder, and local or online co-op. Its Enhancements tab turns on optional gameplay and display changes, and its ROM randomizer tab sets up a randomized game. Online co-op uses a public signalling server by default, so two players only need to agree on a room name.

The game remembers the last ROM it loaded, so after the first run you can start `z2rs` directly and it boots straight into the game. Dragging a ROM onto the game window still works. `HOW-TO-PLAY.txt` in the archive lists the controls and where settings and saves are kept.

For Android 8.0 and newer, the releases page also has `z2rs-<version>-android.apk`. Install it, pick your ROM in the app and press Play. It has an on-screen gamepad, works with Bluetooth and USB controllers, and can import an HD pack. Online co-op, the randomizer and the enhancements are not available on Android yet. [guide/android.md](guide/android.md) covers installing, the controls and the settings.

Only one dump passes the check, identified by the hash of the ROM body with the 16-byte iNES header stripped: CRC32 `BA322865`, SHA1 `11333adb723a5975e0ecca3aee8f4747aa8d2d26` (No-Intro USA). The file is only ever read in place.

Nothing in the archive is code signed. On macOS, right-click `z2rs.app` and choose Open the first time, or use Open Anyway in System Settings > Privacy & Security on newer versions. Windows SmartScreen wants More info and Run anyway, and on Linux the files may need `chmod +x`.

From a terminal you can skip the launcher:

```sh
./z2rs --rom /path/to/zelda2.nes --scale 3
./z2rs --fullscreen
```

`z2-signal`, the third binary in the archive, is the signalling server for online co-op. You only need it to host your own instead of using the public one.

## Build from source

You need Rust stable (`cargo`, `rustc`). The browser build also needs the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`) and `wasm-pack`.

```sh
make build          # cargo build --workspace (no ROM needed)
make test           # cargo test --workspace (tests that need a ROM skip without Z2_ROM)
export Z2_ROM=/path/to/zelda2.nes
make run            # play the optimized desktop build
make run-web        # build the wasm bundle and serve it on :8080
```

The Android app is built with `make android`, which needs the Android SDK, NDK and `cargo-ndk`. [guide/android.md](guide/android.md) lists the prerequisites and explains how to sign a release APK.

`make help` lists every target. A target that needs `Z2_ROM` prints what to set and exits with status 2 when it is missing.

## Controls

| Action | Player 1 | Player 2 (co-op) |
|---|---|---|
| A | `Z` | `G` |
| B | `X` | `F` |
| Start | `Enter` | `T` |
| Select | `Shift` (either one) | `R` |
| D-pad | arrow keys | `W` `A` `S` `D` |

`O` opens the in-game options menu. `Tab` fast-forward, `F5` save state, `F7` load state, `F6` next save slot, `P` pause, `.` single-step while paused, `F11` or `Alt+Enter` fullscreen, `Esc` leave fullscreen or quit. Gamepads work too.

## Guides

| Guide | What is in it |
|---|---|
| [guide/desktop.md](guide/desktop.md) | the launcher, every key, save-state slots, command-line flags, the config file, gamepads |
| [guide/android.md](guide/android.md) | installing the APK, the on-screen gamepad, controllers, accessibility, building and signing the app |
| [guide/browser.md](guide/browser.md) | the web build, its URL parameters, optional features and bundle size |
| [guide/co-op.md](guide/co-op.md) | two Links locally or online, widescreen, how both work and what they cannot do |
| [guide/randomizer.md](guide/randomizer.md) | playing a randomized seed, presets, flag strings, what can be randomized and what is not done yet |
| [guide/enhancements.md](guide/enhancements.md) | the optional gameplay and display changes, the in-game options menu, and how they work with online play |
| [guide/hd-packs.md](guide/hd-packs.md) | making a pack by painting over spritesheets of the game (`make hd-sheets`, `make hd-pack`) and playing with it |
| [guide/development.md](guide/development.md) | repository layout, headless runs, and how the port is checked against the original |

## Contributing and legal

[CONTRIBUTING.md](CONTRIBUTING.md) covers setup, the pre-commit hook and the rules for tests that need a ROM. [LEGAL.md](LEGAL.md) explains what may never be committed. Read it before you open a pull request.

## License

The z2rs source code is released under the MIT license ([LICENSE](LICENSE)). The license does not cover the original game, which you must supply yourself.
