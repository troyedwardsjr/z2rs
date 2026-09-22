# Playing on the desktop

```sh
make run ROM=/path/to/zelda2.nes [MOVIE=/path/to/movie.bk2] [ARGS="--widescreen 16:9"]
# or: cargo run --release -p z2-native -- --rom "$Z2_ROM" [--movie M.fm2|.bk2]
```

Play the release build. `make run` uses it already. `make run-debug` exists for debugging the interpreter, and it is slow: the debug interpreter manages about seven game frames per second, so reaching the title screen takes roughly 90 seconds. If the release window stays grey, look in the terminal for a `present render:` line (a GPU surface problem) or an `emulator wedged` line (the port diverged).

Without a ROM the app starts in a synthetic mode with no cartridge. You can also drag a ROM or a movie onto the running window. Movies play back without verification: their input bytes feed the emulator and nothing is compared.

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

Other keys: `Tab` fast-forward, `F5` save state, `F7` load state, `P` pause,
`.` single-step while paused, `Esc` quit.

There are ten save-state slots. `F6` cycles through them and the digits `1` to
`9` and `0` pick one directly. `F5` and `F7` use the selected slot, and the
window title shows ` [SLOT n]` while it is anything other than 0. Each slot is
a separate `savestate<n>.z2snap` file in the data directory.

## Command-line flags

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

## Config file

The same settings live in `<data-dir>/z2-native.json`: `widescreen`, `widescreen_fill_left_clip`, `widescreen_fill_right_clip`, `hd_pack`, `hd_scale`, `hd_record`, `coop_local`, `keys_p2`, `gamepad`, `gamepad_p2`, `gamepad_p2_index`, and a `netplay` object (`mode`, `signal_url`, `input_delay`, `stall_timeout_ms`, `ice_url`, `ice_username`, `ice_credential`). Command-line flags win over the file. Older config files still load, because every newer key has a default.

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
- There is no sound. When no audio device is available, the desktop app prints `audio disabled: ...` and keeps running silently. The underrun meter in the title bar only means something while audio is up.
