# Legal and asset policy

z2rs is a clean-room-style reconstruction of a 1980s NES game. The original ROM, its bytes, and any data derived from those bytes are not ours to distribute. This policy binds every contributor. The pre-commit hook and `.gitignore` enforce it.

## 1. Never commit

- The original game ROM in any form (`*.nes`, with or without the iNES header).
- `assets.bin`, or any other bundle extracted from the ROM.
- CHR graphics, maps, text dumps, music data, or any fixture derived from ROM bytes. This holds even when the data has been transformed (re-encoded, compressed, base64 and so on).
- TAS movies (`.fm2`, `.bk2`). They live in a `corpus/` directory outside this repository.
- Leaked or decompiled original source code, in whole or in part.
- HD pack template sheets rendered from the ROM's CHR (by `cargo xtask hdpack template` or by the recorder), and HD pack directories in general. Packs are keyed by numeric tile ids only. You may share only art you painted yourself, and it still lives outside this repository.

## 2. Bring your own ROM

- Get Zelda II: The Adventure of Link (USA) from your own cartridge or dump.
- Point the `Z2_ROM` environment variable at it: `Z2_ROM=/path/to/zelda2.nes`.
- `z2-assets::rom::open()` checks the file against the known-good No-Intro USA dump (body CRC32 `BA322865`, body SHA1 `11333adb723a5975e0ecca3aee8f4747aa8d2d26`) and rejects anything else. It strips a 16-byte iNES header automatically.
- Never copy a ROM into your checkout, not even temporarily. Read it in place through `Z2_ROM`.

## 3. Clean-room rules

- Do not paste disassembly listings, decompiled C, or leaked source into issues, pull requests, comments or docs. Describe behavior ("when X, the game does Y"), never the original expression.
- Reconstruction code must be original. Port algorithms from observed behavior and from the routine ledger (`ports.toml`). Do not transcribe assembly mnemonics into Rust line by line.
- If in doubt, ask before you commit. If ROM-derived material gets committed by mistake, report it immediately so it can be purged from history.

## 4. Enforcement

- `.githooks/pre-commit` refuses `*.nes`, `assets.bin`, any `*.fm2` or `*.bk2` inside the tree, and any staged blob whose hash matches a known ROM image.
- The HD pack tooling refuses to write template output inside a git work tree (`--force-in-repo` overrides this). `.gitignore` covers the usual output names, and the hook refuses template PNGs (tEXt `z2rs-template`), pack manifests and the template marker files.
- The test suite runs without a ROM by default (`Z2_ROM` unset). Tests that need ROM bytes skip themselves unless `Z2_ROM` points at your own dump. Expected values are sliced from your ROM while the test runs, and generated files such as `assets.bin` are gitignored.
