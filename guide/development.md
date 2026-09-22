# Working on z2rs

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

## Building

The desktop `netplay` feature is on by default and pulls in `ring`, which builds C and assembly, so you need a C compiler. `cargo build -p z2-native --no-default-features` drops only the online transport. Widescreen, local co-op, movies and headless runs all still work. On the web the equivalent features (`hd`, `netplay`) are opt-in.

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

## Input movies

The long tests replay tool-assisted speedruns from TASVideos. The movies are not included here. Download them yourself into a directory outside the repository and point `Z2_CORPUS` at it (the Makefile defaults to a sibling `../z2-corpus`). Movies go in `$Z2_CORPUS/movies/`, and snapshots minted from them land in `$Z2_CORPUS/snapshots/`.

| Run | Authors | Length | Page |
|---|---|---|---|
| any% | Arc & Inzult | 34:58.74 | <https://tasvideos.org/4367M> |
| 100% | Arc & Inzult | 1:02:40.84 | <https://tasvideos.org/4425M> |
| warpless | Arc, FatRatKnight, Inzult & Rising_Tempest | 45:26.04 | <https://tasvideos.org/3254M> |
| warp glitch | TASeditor, Arc, Inzult & EZGames69 | 5:31.887 | <https://tasvideos.org/4234M> |

Both FCEUX `.fm2` and BizHawk `.bk2` files parse. `make corpus-mint` replays the movies in the oracle and saves labelled snapshots (boot, town entrances, spell pickups, palaces, bosses, the ending). `make fuzz-smoke SNAPSHOT=...` then starts from a snapshot, feeds both sides random input and looks for a divergence.

Every test that needs the ROM, the movies or a snapshot skips itself when that input is missing, so `cargo test --workspace` passes on a machine that has none of them.

## Troubleshooting

- A test or target says the corpus is missing. Movies and snapshots live outside the repository in `$Z2_CORPUS` and are never committed. `make corpus-mint` and `make fuzz-smoke` say exactly what they could not find.
- A headless `--movie` run fails. An unknown file extension exits with 2 and an unreadable file with 4.
- The picture or timing differs from the oracle (flicker, the HUD split on the wrong row, a wrong palette or wrong tiles after a transition). The normal comparison stops at the first differing byte, so use the tolerant driver. `cargo xtask verify --movie M --frames N --dut game --continue --rom "$Z2_ROM"` reports frame by frame which regions disagree (RAM, WRAM, OAM, palette, frame). Add `--dump-at F --out DIR` to get PNGs from both sides along with PPU, mapper, OAM and nametable state. `--watch a,b` traces RAM bytes, and `--trap-set` and `--untrap` let you bisect the set of ported routines. For questions inside a single frame, such as where the NMI lands or when the sprite-0 flag rises, `cargo xtask probe --frame N --movie M --rom "$Z2_ROM"` walks that frame of the oracle instruction by instruction next to the port's PPU event trace.
