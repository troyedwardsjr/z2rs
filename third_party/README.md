# third_party

`z2disassembly/` is a git submodule of <https://github.com/FiendsOfTheElements/z2disassembly.git>, pinned at `c4c3a4c7d4156b41f54f726dc40644d45812ad29` (upstream `HEAD` on 2026-09-04, "cleanup more reset code"). It is a community disassembly released under CC0, and z2rs uses it as a porting reference. The same pin is recorded in the `[meta]` section of `ports.toml`.

Fetch it with:

```sh
git submodule update --init third_party/z2disassembly
```

`cargo xtask ledger` expects this layout under `z2disassembly/`: `src/prg0..7.asm`, `src/variables.asm`, `inc/nes.asm`, `inc/mmc1.asm`, `nes.cfg`, `build.sh`, `rip-chr.sh`, `bin/header.bin` and `ram-map.txt`. `xtask ledger --rebuild` stops with an error when `nes.cfg`, `src/prg0.asm` or `inc/nes.asm` is missing, which is how it notices that the upstream layout has changed.

Never commit ROM-derived bytes here. That means no `.nes` files, no `bin/chr*.bin`, and no copies of your ROM. CHR bytes are extracted from your own dump at build time by the upstream `rip-chr.sh`, and they stay outside the tree.
