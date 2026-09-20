# Provenance

"There is no ROM in this repository" is true, but on its own it says little about how the code came to exist. This file records the rest: what reference material the project worked from, what was written down along the way, who is responsible for the code, how the history is kept, and where every file in the repository came from.

It is a record of facts, not legal advice. It covers materials and process. It does not describe the editors or other tools used to write the code. [LEGAL.md](LEGAL.md) holds the rules contributors follow, and this file describes what was actually done under them.

## What "clean-room-style" means here

z2rs calls itself a clean-room-style reconstruction. The term needs pinning down.

It does not mean a two-team clean room, where one group studies the original program and writes a specification, and a second group that never sees the original writes new code from that specification alone. There is no such wall in this project. One maintainer, troyedwardsjr, both works from the reference material and is responsible for the Rust code, and the two live in the same repository.

What the term does mean:

- No ROM bytes and no data derived from ROM bytes are committed. That covers graphics, maps, text and music.
- Rust code reproduces what the game does. The check is behavioural: the real game runs in a reference emulator next to z2rs, and the two must agree frame by frame.
- The bulk of the game's data is read from your own ROM while the program runs. The source holds offsets and lengths, not the bytes.

z2rs is also not yet a standalone reimplementation. At run time it executes your ROM in a 6502 interpreter, and wherever a routine has been ported it runs the Rust version instead. `ports.toml` lists every routine in the ROM and records which ones have a port.

## Who examined the original, and how

The project did not produce its own static analysis of the game. Its knowledge of how the original code is laid out (routine names, addresses, boundaries and the call graph) comes from a community disassembly:

- [FiendsOfTheElements/z2disassembly](https://github.com/FiendsOfTheElements/z2disassembly), which credits Trax for the original disassembly. Its authors released it under CC0 1.0.
- z2rs pins it at commit `c4c3a4c7d4156b41f54f726dc40644d45812ad29` as a git submodule. The repository stores only that pointer. None of the disassembly's files are copied in.
- The CC0 dedication is a statement by the disassembly's authors about their own work. It does not change who owns the original game.

Other published reference material, all cited in the source where it is used:

- The Data Crystal pages for Zelda II (ROM map, RAM map as of the 2025-01-31 revision, and the text table).
- Dwedit's notes on side-view area data, as quoted on the Data Crystal ROM map.
- The nesdev wiki, for how the NES audio hardware behaves.

The ROM itself is one specific dump, the No-Intro USA release, identified by hash (body CRC32 `BA322865`, SHA1 `11333adb723a5975e0ecca3aee8f4747aa8d2d26`). The maintainer uses their own copy, read in place through the `Z2_ROM` environment variable. It is used in these ways:

- z2rs's own 6502 interpreter executes it. That is how the game runs at all.
- `tetanes-core` 0.15 (MIT OR Apache-2.0) executes it as the reference emulator that ports are compared against.
- Mesen 2 runs it for a second opinion on power-on state (`tools/mesen_probe.lua`).
- `cargo xtask verify --continue` and `cargo xtask probe` print state differences and instruction-level timelines of the game running in the reference emulator. They are used to find the point where a port stops matching.
- `cargo xtask ledger --rebuild --verify` reassembles the disassembly with ca65 and ld65 and checks the result against the ROM hash. This ties the routine ledger to that exact ROM.

## What was written down

These files are the project's working specification. All of them are in the repository.

| File | What it records |
|---|---|
| `ports.toml` | All 2960 routines from the disassembly: name, bank, address, size, kind, callers, callees, and the disassembly source line each one comes from. `cargo xtask ledger` generates it. Only the `status` and `notes` fields are maintained by hand. |
| `ram-map.toml` | 59 blocks of CPU RAM: address, name, the listing term each one cites, its source, and a `verified` flag. 56 are marked verified, which means a test found that term at that address in the disassembly's listing. The other 3 come from Data Crystal or project notes and are marked unverified. |
| `crates/z2-assets/src/extract_tables.rs` | The layout of the ROM as 100 sections: offset, length and label. It holds no data bytes. 93 of the records carry a key that says whether they come from Data Crystal, a disassembly label or Dwedit's notes. |
| Module docs in `crates/z2-core/src` | For each ported routine: the disassembly label, bank and CPU address it corresponds to, what it does, and known gaps. Some quote a few instructions from the listing to pin down the behaviour being reproduced. Ports that have to match the hardware register for register are described as such. |
| Tests | Synthetic tests that need no ROM, plus tests that compare against the reference emulator when `Z2_ROM` is set. |

Longer working notes from development are kept with the private history and are not part of this repository.

## Who is responsible for the code

troyedwardsjr is the project's only maintainer and holds the copyright in the Rust code, which is released under the MIT license ([LICENSE](LICENSE)). No pull request from another person has been merged so far.

Code that did not originate here:

- `vendor/matchbox_socket` is matchbox_socket 0.14.0 (MIT OR Apache-2.0) with three small patches. Each patch is marked `z2rs patch` in the source, and the root `Cargo.toml` describes all three.
- Everything else comes from crates.io at the versions pinned in `Cargo.lock`. Playwright, pinned in `package-lock.json`, is used only by the browser tests.

LEGAL.md forbids leaked or decompiled source code from the original game, in whole or in part.

## Commit history

This repository starts with a single commit, `74c7d92`, made on 2026-09-18. It is a snapshot of a private development tree, with development-only files and internal references removed before publication. The snapshot therefore differs from the private tree it was taken from.

The work before that snapshot is 97 commits made between 2026-09-02 and 2026-09-18. That history is kept in a private repository. From the first public commit onward, changes land here as ordinary commits.

## Where every file came from

As of the first public commit the repository holds 277 text files and one submodule pointer. There are no binary files: no images, audio, fonts or compiled data.

| Path | What it is | Where it came from |
|---|---|---|
| `crates/*/src`, `crates/*/tests`, `crates/*/examples`, `tools/*/src` | Rust source and tests | Written for z2rs. |
| Gameplay constants in `crates/z2-core/src` | About 40 small byte tables such as enemy damage, spell costs, experience thresholds and spawn positions, plus lists of RAM and ROM addresses. No table is larger than 64 bytes, and together they come to roughly 500 bytes. | The values as they appear in the disassembly. The doc comment on each table, or on the group it belongs to, names the label and address. |
| Town and spell names in `crates/z2-core/src/town.rs` | Eight town names and eight spell names, used in reports | Common names for things in the game. They are not read from the ROM. |
| `crates/z2-apu/src/tables.rs` and the other APU modules | Length counter, duty, noise and DMC tables | NES hardware behaviour as documented on the nesdev wiki. The game's own note and envelope tables are not here. They are loaded from your ROM. |
| `crates/z2-ppu/src/palette.rs` | 64 RGB values for the NES master palette, used for display only | The widely circulated "common `.pal`" NTSC approximation, in the style FCEUX and Mesen use. Verification compares palette indices and never touches it. |
| `ports.toml` | Routine ledger | Generated from the disassembly by `cargo xtask ledger`. |
| `ram-map.toml` | RAM map | Data Crystal's RAM map plus the disassembly's `ram-map.txt` and `src/variables.asm`, with a source recorded per entry. |
| `crates/z2-assets/src/extract_tables.rs` | ROM section offsets and lengths | Data Crystal's ROM map, disassembly labels and Dwedit's notes. |
| `crates/z2-web/site` | HTML, JavaScript and test scripts for the browser build | Written for z2rs. |
| `tools/mesen_probe.lua` | Lua script for Mesen 2 | Written for z2rs. |
| `third_party/z2disassembly` | Submodule pointer | The community disassembly described above. |
| `vendor/matchbox_socket` | Vendored crate | Upstream matchbox_socket 0.14.0 plus three marked patches. |
| `Cargo.lock`, `package.json`, `package-lock.json` | Dependency pins | Generated by cargo and npm. |
| `Makefile`, `.githooks/pre-commit`, `.gitignore`, `.gitmodules`, `.cargo/config.toml` | Build and repository plumbing | Written for z2rs. |
| `README.md`, `CONTRIBUTING.md`, `LEGAL.md`, `PROVENANCE.md`, `LICENSE` and the other READMEs | Documentation | Written for z2rs. |

What is deliberately not here, and where it comes from when you run z2rs:

- The game ROM is your own dump. z2rs checks its hash, reads it in place and never stores it.
- Graphics, maps, text, enemy placement and the music data are read from that ROM at run time.
- `assets.bin` is an optional file the extractor builds on your machine from your ROM. It is gitignored.
- The input movies used by the long tests are tool-assisted speedruns published on TASVideos. You download them yourself from the pages listed in the README, into a directory outside the repository.
- Snapshots are minted on your machine from those movies and your ROM, into the same outside directory.
- HD pack template sheets are rendered from your ROM's graphics. The tooling refuses to write them inside a git work tree. Only art that a pack's author painted themselves may be shared, and packs live outside the repository too.
- Save files and the config file live in your data directory.

## How the rules are enforced

- `.githooks/pre-commit` refuses `*.nes` files, `assets.bin`, movie files, HD pack output and template PNGs. It also refuses any staged file whose SHA1 matches the ROM, with or without its iNES header.
- `.gitignore` covers the same names.
- The HD pack tools refuse to write inside a git work tree unless you pass `--force-in-repo`.
- Every test that needs the ROM, movies or snapshots skips itself when they are missing, so nobody has to commit a fixture to get a green run.

## Checking this yourself

```sh
git ls-files -z | xargs -0 file --mime-encoding | grep binary   # only the submodule pointer
git submodule status                                            # the pinned disassembly commit
cargo xtask ledger --rebuild --verify    # needs the cc65 toolchain and Z2_ROM
cargo xtask ledger --check               # fails if ports.toml no longer matches the disassembly
```

If you think anything in this repository is derived from the ROM, or that this record is wrong, open an issue. LEGAL.md asks for such reports to be made immediately, so that the material can be removed from history.
