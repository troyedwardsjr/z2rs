# HD graphics packs

A pack is a directory of PNG sheets plus a `pack.json`. It replaces the game's 8x8 tiles and sprites with art drawn at 2x to 8x. The simplest way to make one is to paint over spritesheets of the game (next section). You can also start from your own ROM's graphics and paint the cells directly.

```sh
cargo xtask hdpack template --out ~/z2-art/mypack --scale 2   # start from your ROM's CHR
# ...paint the sheets...
cargo xtask hdpack check --pack ~/z2-art/mypack               # validate
cargo run --release -p z2-native -- --rom "$Z2_ROM" --hd-pack ~/z2-art/mypack --hd-scale 2
```

`--hd-record DIR` works the other way round. Play normally, and when you quit with `Esc` or by closing the window, z2rs writes a template pack that contains exactly the `(page, tile, palette)` combinations that session drew. That is a much smaller starting point than all 32 CHR pages. Only those two ways of quitting save the recording. A `kill` loses it.

In the browser, the folder picker loads the same packs in a build that has the `hd` feature. Choose the directory that holds `pack.json`.

Template sheets are rendered from your ROM, so they must stay outside this repository. The tooling refuses to write them inside a git work tree (see [LEGAL.md](LEGAL.md)). `cargo xtask hdpack --help` covers the rest of the workflow, and the module docs in `crates/z2-render/src/pack.rs` describe the `pack.json` format.

## Painting over spritesheets

You can make a pack by painting over spritesheets of the game. Each character is laid out frame by frame and each scene's tileset metatile by metatile. When you are done, one command turns the painted sheets into a pack, so you never edit `pack.json` or look up CHR page numbers.

```sh
export Z2_ROM=/path/to/zelda2.nes   # your own dump, see LEGAL.md
make hd-sheets                      # capture the game and write the sheets (about 6 minutes once built, 1.7 GB of captures)
# paint over ~/z2-art/sheets/sheets/*.png in any image editor
make hd-pack                        # turn them into ~/z2-art/my-pack and validate it
make hd-play                        # play with it
```

`make hd-sheets` needs the input movies described in [development.md](development.md#input-movies). It replays them into every town, palace, boss room and field, walks and attacks through each one, records which tile drew every pixel, and writes two kinds of sheet into `~/z2-art/sheets/sheets/`:

| sheet | one slot | one sheet |
|---|---|---|
| `fig-cXX-YY-ZZ.png` | a whole figure in one animation frame, such as Link mid-stab or a townsperson | one sprite palette, named by its three NES colours, which is usually one character |
| `tile-wW-rR-sSS.png` | one 16x16 metatile, four CHR tiles | one scene, with the most-used metatiles first |

The sheets are drawn from your ROM at 4x in the game's own colours, so each one looks like the game when you open it. Most editors show the transparent areas as a checkerboard.

When you paint:

- Paint in place. Keep every slot where it is and keep the image the same size. The dark gutters between slots are ignored.
- A slot you leave untouched keeps the original art. `make hd-pack` compares each slot with what `make hd-sheets` wrote and skips the ones that match, so you can paint one character, build, play, and come back for the rest.
- Figures can grow. Each figure slot has 8 NES pixels of empty margin around the sprite, and paint that goes into it is drawn in the game, so a longer sword or a cape works. Keep the figure standing where it is, because the hitboxes and the animation timing don't change.
- Transparency is on or off. Alpha below 128 is transparent and nothing is blended, so paint hard edges.
- Metatiles sit next to each other in the level, so match the edges of tiles you expect to touch.
- Tiles with no pixels at all are left off the sheets. That tile is the backdrop colour in every sky in the game. Paint a sky or distant hills as a layer instead (see below).
- If two poses share a CHR tile and you paint it two different ways, the version with more painted pixels wins.

`HD_ART` (default `~/z2-art`), `HD_SHEETS`, `HD_PACK` and `HD_SCALE` change where things go and the scale. `HD_PACK_NAME` names the pack. Keep all of them outside this repository: everything here is drawn from your ROM, and only art you painted yourself may be shared.

The targets run `tools/hd-sheets/specs.py` and the `article_shots` example with `--tilemap` to capture the game, then `tools/hd-sheets/paint_sheets.py build`. `make hd-pack` runs `paint_sheets.py cut` and `cargo xtask hdpack check`. Run `python3 tools/hd-sheets/paint_sheets.py build --help` to rename sheets (`--name c18-36-2A=link`) or to build from your own captures.

## Sprite shapes and layers

A pack can set `"sprite_alpha": "art"` in `pack.json`. A replaced sprite then takes its outline from the art's own alpha instead of the original sprite's pixels, and a `tiles[]` entry can carry `bleed: [left, top, right, bottom]`, up to 8 NES pixels of art drawn around the 8x8 box. Sprites still stack in the order the game draws them, and a sprite behind the background still hides behind opaque background pixels. Packs made with `make hd-pack` use both.

`layers[]` places whole images in one scene: a sky or far hills behind the background (`"depth": "back"`), or art over the background tiles (`"front"`). A layer can follow the camera at a fraction of its speed (`scroll`, a percentage), repeat horizontally, cover only chosen tiles (`over_tiles`), and apply to one scene through `when` (`world`, `region`, `scene`). `paint_sheets.py cut --layer` adds them; its `--help` lists the options. Older builds ignore these keys and draw every sprite inside its original outline.

## How packs work

Three things identify every tile the PPU draws: the CHR page it came from, its tile index, and the three opaque colours of the palette it was drawn in. A pack maps those keys to cells in a PNG sheet. Where a tile has no cell, z2rs scales up the original art instead, so a half-finished pack plays fine and you can repaint the game a few tiles at a time.

Replacement art never changes how the game behaves. The pack's alpha channel decides which HD pixels get painted, but the opacity of the original pixels still decides sprite priority, background priority and sprite-zero timing. The game logic keeps running on the ROM's own graphics while the screen shows yours.

`hdpack template` writes sheets for all 32 CHR pages of your ROM. `--hd-record` does the reverse: play, quit, and get a pack with exactly the tile and palette combinations that session drew.

## Limits

These apply to both the desktop app and the browser build.

- A background cell stays inside its 8x8 tile, and a sprite reaches at most 8 NES pixels past its box (`bleed`, with `"sprite_alpha": "art"`). Without that key, replaced sprites are also cut to their original opaque pixels. Art that belongs to no tile goes in a layer.
- Each tile identity has one cell. Every place the game draws that tile in those colours shows the same art. A layer with `over_tiles` can vary a repeated tile by position.
- Greyscale lines, lines with a mid-line `$2001` or palette write, and lines where fine X changed mid-line keep the original art, because the slot-to-pixel mapping doesn't hold for the whole line. In practice that is the row under the status bar and effects such as the palace-entry fade.
- Layers are the most expensive part of a pack. The worst case (every tile replaced plus three layers) takes about 13.5 ms a frame at 4x widescreen on an Apple Silicon Mac, which still fits a 60 Hz frame. A pack without layers takes about 2 ms.
