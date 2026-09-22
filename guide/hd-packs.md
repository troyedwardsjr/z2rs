# HD graphics packs

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

## How packs work

Three things identify every tile the PPU draws: the CHR page it came from, its tile index, and the three opaque colours of the palette it was drawn in. A pack maps those keys to cells in a PNG sheet. Where a tile has no cell, z2rs scales up the original art instead, so a half-finished pack plays fine and you can repaint the game a few tiles at a time.

Replacement art never changes how the game behaves. The pack's alpha channel decides which HD pixels get painted, but the opacity of the original pixels still decides sprite priority, background priority and sprite-zero timing. The game logic keeps running on the ROM's own graphics while the screen shows yours.

`hdpack template` writes sheets for all 32 CHR pages of your ROM. `--hd-record` does the reverse: play, quit, and get a pack with exactly the tile and palette combinations that session drew.

## Limits

These apply to both the desktop app and the browser build.

- HD art cannot extend a sprite or a tile beyond its original opaque pixels. Substitution happens in place, per 8x8 cell and per sprite, and a transparent NES pixel stays transparent. That rules out bigger swords and outlines that spill past the original shape.
- Split and greyscale scanlines keep the original art. The compositor cannot tell which tile one half of a split line belongs to, so it upscales those lines instead.
- Widescreen and a pack together leave a seam of 1 to 7 pixels of original art at the left edge of the play field. This is a known gap in the `z2-render` compositor.
