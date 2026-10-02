# Playing on the desktop

## The launcher

The release archives include `z2rs-launcher` (`z2rs-launcher.exe` on Windows, `z2rs.app` on macOS). Double-click it, pick your ROM and press Play. The launcher starts the game binary that sits next to it with the options you chose:

- window size (the `--scale` multiplier) and fullscreen
- whole-number scaling (the "Whole-number scaling only" checkbox, `--scale-mode integer`)
- widescreen
- an HD pack folder
- local co-op, or online co-op as host or guest with a room name

The Enhancements tab holds optional gameplay and display changes ([enhancements.md](enhancements.md)), and the ROM randomizer tab sets up a randomized game ([randomizer.md](randomizer.md)).

For online co-op the launcher fills in the public signalling server, `wss://signal.z2rs.com`, so both players only need the same room name. You can point it at your own `z2-signal` instead. See [co-op.md](co-op.md).

The game remembers the last ROM it loaded, whether it came from the launcher, `--rom` or a file dropped onto the window. It stores the path as `rom_path` in the config file, so starting `z2rs` with no arguments (a double-click in a file manager, for example) boots straight into the game. The ROM stays where it is. Only its path is saved.

## Window size and fullscreen

The picture keeps its shape and grows to fill the window, so in fullscreen it fills the height of the screen. It is scaled by whatever factor fits, including fractions, and sampled so the pixels stay sharp. `--scale-mode integer` (or `"scale_mode": "integer"` in the config file, or the launcher's "Whole-number scaling only" checkbox) keeps the picture to whole multiples instead. That is a little sharper, but it can leave black borders.

For a screen with no borders at the sides, pick the widescreen preset that matches the screen's shape: 16:9 for most TVs and monitors, 16:10 for the Steam Deck, the Legion Go and many laptops. To fill the height, widescreen may trim up to one tile from each margin, which is how 16:9 covers a 16:9 screen. 16:9 on a 16:10 screen leaves thin bars above and below.

Without `--scale`, the window opens at the largest of 3x, 2x or 1x that fits the screen, measured against the screen's size after display scaling, with room left for the title bar and taskbar. On a very small screen it opens below 1x. An HD pack does not make the window bigger.

## Running from a terminal

```sh
./z2rs --rom /path/to/zelda2.nes [--scale 3] [--fullscreen]
```

From a source checkout:

```sh
make run ROM=/path/to/zelda2.nes [MOVIE=/path/to/movie.bk2] [ARGS="--widescreen 16:9"]
# or: cargo run --release -p z2-native -- --rom "$Z2_ROM" [--movie M.fm2|.bk2]
```

Play the release build. `make run` uses it already. `make run-debug` exists for debugging the interpreter, and it is slow: the debug interpreter manages about seven game frames per second, so reaching the title screen takes roughly 90 seconds. If the release window stays grey, look in the terminal for a `present render:` line (a GPU surface problem) or an `emulator wedged` line (the port diverged).

Without a ROM, and with none remembered, the app starts in a synthetic mode with no cartridge. You can also drag a ROM or a movie onto the running window. Movies play back without verification: their input bytes feed the emulator and nothing is compared.

## Controls

| Action | Player 1 | Player 2 (co-op) |
|---|---|---|
| A | `Z` | `G` |
| B | `X` | `F` |
| Start | `Enter` | `T` |
| Select | `Shift` (either one) | `R` |
| D-pad | arrow keys | `W` `A` `S` `D` |

Both Shift keys are Select, because Select is how you cast a spell and the
left one is easier to reach while the right hand is on the arrow keys.

Other keys: `O` opens the in-game options menu, `Tab` fast-forward, `F5` save state, `F7` load state, `P` pause,
`.` single-step while paused, `F11` or `Alt+Enter` to switch between a window
and fullscreen, `Esc` quit.

In fullscreen, `Esc` returns to a window first. Press it again in the window
to quit. That way a stray `Esc` does not close the game from fullscreen.

There are ten save-state slots. `F6` cycles through them and the digits `1` to
`9` and `0` pick one directly. `F5` and `F7` use the selected slot, and the
window title shows ` [SLOT n]` while it is anything other than 0. Each slot is
a separate `savestate<n>.z2snap` file in the data directory.

## Command-line flags

Besides `--rom`, `--movie` and `--config`:

| Flag | Effect |
|---|---|
| `--scale N` | window size as a multiple of the picture, 1 to 8 |
| `--fullscreen` | start in fullscreen (`F11` or `Alt+Enter` switches back) |
| `--scale-mode fit\|integer` | `fit` (default) fills the window or screen height at any size; `integer` uses whole multiples only |
| `--widescreen off\|16:10\|16:9\|21:9\|N` | extra scenery left and right of the NES picture: 16:10 is 384x240, 16:9 is 432x240, 21:9 ultrawide is 560x240, or N tiles per side (0 to 20) |
| `--coop-local` | two players at this machine |
| `--coop-host ROOM` / `--coop-join ROOM` | online co-op over WebRTC |
| `--signal URL` | signalling server (default `ws://127.0.0.1:3536`; the launcher passes `wss://signal.z2rs.com`) |
| `--net-mode rollback\|lockstep` | online sync mode (default `rollback`; config key `netplay.mode`) |
| `--net-delay N` | netplay input delay in frames, default 2 (rollback 0 to 3, lockstep 0 to 8) |
| `--p2-pad INDEX` | pin player 2 to a gamepad by connection order |
| `--fill-left-clip on\|off` | paint the 8 columns the overworld blanks at x 0 to 7 (default on) |
| `--fill-right-clip on\|off` | paint the 8 columns the overworld masks at x 248 to 255 (default on) |
| `--margin-sprites on\|off` | draw side-view enemies, townspeople and items that are outside the NES picture into the margins (default on) |
| `--wide-gameplay on\|off` | with widescreen, enemies spawn and live out in the margins (default on, off with `--movie`; changes the game, so both online players must match) |
| `--load-state PATH` | load a `.z2snap` save state at startup, the same way `F7` does, before the first frame and before `--movie` starts |
| `--p2-follow N` | in local co-op movie playback, player 2 repeats player 1's input N frames later (Start and Select are left out) |
| `--hd-pack DIR` | HD graphics pack (the directory with `pack.json`); `''` turns it off |
| `--hd-scale N` | output multiplier 1 to 8 (default 1) |
| `--hd-record DIR` | on exit, write a template pack of the tiles this session drew |
| `--seed TEXT` / `--rando-flags STRING` | play a randomized game built from your ROM (see [randomizer.md](randomizer.md)) |
| `--rando-spoiler PATH` | with a randomized game, write a spoiler log |
| `--sprite-ips PATH` | with a randomized game, apply your own Link sprite patch |
| `--enh-json JSON` / `--display-enh-json JSON` | gameplay and display enhancements as JSON, inline or `@PATH` (see [enhancements.md](enhancements.md)) |

`--help` lists all of these. An unknown flag is a usage error (exit 2).

## Config file

The same settings live in `<data-dir>/z2-native.json`: `rom_path` (the remembered ROM), `window_scale`, `fullscreen`, `scale_mode`, `widescreen`, `widescreen_fill_left_clip`, `widescreen_fill_right_clip`, `widescreen_margin_sprites`, `widescreen_gameplay`, `hd_pack`, `hd_scale`, `hd_record`, `coop_local`, `keys_p2`, `gamepad`, `gamepad_p2`, `gamepad_p2_index`, `gamepads_enabled`, `allow_opposing_directions`, `gpu_backend`, `enhancements`, `display_enh`, and a `netplay` object (`mode`, `signal_url`, `input_delay`, `stall_timeout_ms`, `ice_url`, `ice_username`, `ice_credential`). Command-line flags win over the file. Older config files still load, because every newer key has a default.

The data directory is `$XDG_DATA_HOME/z2rs` when `XDG_DATA_HOME` is set. Otherwise it is `~/Library/Application Support/z2rs` on macOS, `%APPDATA%/z2rs` on Windows and `~/.local/share/z2rs` elsewhere. Save states, battery saves and the config file live there. The ROM is never stored there.

## Gamepads

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

## Troubleshooting

- The ROM is rejected. Only the No-Intro USA dump passes the check (body CRC32 `BA322865`, SHA1 `11333adb...`). Dump your cartridge again. The desktop app reads the ROM directly. `assets.bin` is an optional product of the extractor and the app does not need it to start.
- The game closes right after starting on Windows. z2rs tries DirectX 12 first, then Vulkan, and if one of them crashed the game while starting it skips that one next time. You can pick one yourself with `gpu_backend` in the config (`dx12`, `vulkan`, `gl` or `auto`) or the `WGPU_BACKEND` environment variable. If a controller seems to be the cause, set `gamepads_enabled` to `false`. Each run leaves a short log, `z2-native.log`, in the data directory, which shows how far startup got.
- Link shoots backwards at high speed. That happens when Left and Right are held at the same time, which a real NES pad cannot do. z2rs lets the most recent of the two win. `allow_opposing_directions` in the config turns that filter off.
- There is no sound. When no audio device is available, the desktop app prints `audio disabled: ...` and keeps running silently. The underrun meter in the title bar only means something while audio is up.
- The game seems to run at the wrong speed. Game speed follows the NES clock, not the audio device, and the sound is resampled to whatever rate the output device uses, so a device set to 96 or 192 kHz plays at normal speed.
