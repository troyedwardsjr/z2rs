//! Widescreen indexed compositor: the NES frame verbatim in the centre,
//! margins painted from background tile identities.
//!
//! The output is a [`WideFrame`] of `wide_width(tiles) x 240` NES palette
//! indices. Its centre 256 columns are copied byte-for-byte from the indexed
//! frame (the parity law: the game frame itself is never altered). The
//! margins are drawn per line from a [`Margins`] structure — tile identities
//! supplied by a provider (e.g. level-RAM / world-map decoders in `z2-core`)
//! — using the CHR image and that line's recorded palette/mask from the
//! [`FrameRecord`].
//!
//! Limitations (by design): sprites are never drawn in the margins (OAM X is
//! 8-bit, and game logic despawns objects at the NES window edge), and lines
//! whose background is disabled, unrecorded, or marked
//! [`MarginFill::Backdrop`] get the line's backdrop colour.
//!
//! Two opt-in settings repaint the window's own edge strips in the wide
//! buffer only, so the game's edge blanking does not read as a black seam
//! between the picture and a painted margin: [`Margins::fill_left_clip`] for
//! the 8 columns `PPUMASK` clips, [`Margins::fill_right_clip`] for the 8 the
//! game hides behind an opaque edge-mask sprite. Both default off, and
//! neither can touch anything outside its own 8 columns.
//!
//! Slot geometry matches [`LineRecord::tiles`]: fetch slot `k` covers window
//! pixels `[8k - fine_x, 8k - fine_x + 8)`, where window x = 0 is NES column
//! 0 and margin pixels have negative x (left) or x >= 256 (right).

use crate::record::{BgTileId, FrameRecord, LineRecord, NO_PAGE, RECORD_TILES_PER_LINE};
use crate::state::{
    CHR_BANK_LEN, PPUMASK_GRAYSCALE, PPUMASK_SHOW_LEFT_BG, PPUMASK_SHOW_LEFT_SPRITES,
    PPUMASK_SHOW_SPRITES,
};
use crate::{IndexedFrame, HEIGHT, WIDTH};

/// Max margin per side in 8-px tiles (128 px; widest output 512 px).
pub const MAX_MARGIN_TILES: usize = 16;
/// Tile slots kept per side per line: `MAX_MARGIN_TILES + 1`, because the
/// right edge needs `M + 1` slots when `fine_x != 0`.
pub const MARGIN_SLOTS: usize = MAX_MARGIN_TILES + 1;

/// How one line's margins are filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarginFill {
    /// Solid backdrop colour (HUD rows, title screens, unknown modes).
    #[default]
    Backdrop,
    /// Draw [`MarginLine::left`] / [`MarginLine::right`] tile identities.
    Tiles,
}

/// Margin tile identities for one line.
///
/// `left[j]` is fetch slot `k = -1 - j` (j = 0 is the tile just left of
/// record slot 0); `right[j]` is slot `k = 32 + j`. A provider for `M` tiles
/// per side fills `left[0..M]` and `right[0..=M]`. Left-margin pixels that
/// fall in slot 0 (when `fine_x != 0`) use the record's own `tiles[0]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarginLine {
    /// Fill policy for this line.
    pub fill: MarginFill,
    /// Slots `-1, -2, ...` leftwards.
    pub left: [BgTileId; MARGIN_SLOTS],
    /// Slots `32, 33, ...` rightwards.
    pub right: [BgTileId; MARGIN_SLOTS],
}

impl MarginLine {
    /// A backdrop line with no tiles.
    pub const BACKDROP: MarginLine = MarginLine {
        fill: MarginFill::Backdrop,
        left: [BgTileId::NONE; MARGIN_SLOTS],
        right: [BgTileId::NONE; MARGIN_SLOTS],
    };
}

impl Default for MarginLine {
    fn default() -> Self {
        Self::BACKDROP
    }
}

/// Margin description for a whole frame.
#[derive(Debug, Clone)]
pub struct Margins {
    /// Margin width per side in tiles (`0..=MAX_MARGIN_TILES`).
    pub tiles: u8,
    /// Also paint the NES window's left 8 columns from record slots 0/1 when
    /// the line clipped both background and sprites there (`PPUMASK` bits
    /// 1-2 clear) and the line's fill is [`MarginFill::Tiles`]. Cosmetic;
    /// default off so the centre stays byte-identical to the frame.
    pub fill_left_clip: bool,
    /// Also paint the NES window's right 8 columns (x 248-255) from the
    /// line's own recorded tiles when the game hid them behind an opaque
    /// edge-mask sprite (see [`right_edge_masked`]) and the line's fill is
    /// [`MarginFill::Tiles`]. The overworld does exactly that — a column of
    /// 8x16 sprites at x 248 drawn in an all-black sprite palette — so
    /// without this there is a black seam between the play field and the
    /// right margin. Cosmetic; default off so the centre stays
    /// byte-identical to the frame.
    pub fill_right_clip: bool,
    /// One entry per visible line.
    pub lines: Box<[MarginLine; HEIGHT]>,
}

impl Margins {
    /// All-backdrop margins of `tiles` per side (clamped to
    /// [`MAX_MARGIN_TILES`]).
    #[must_use]
    pub fn new(tiles: u8) -> Self {
        let lines: Box<[MarginLine]> = vec![MarginLine::BACKDROP; HEIGHT].into_boxed_slice();
        let lines: Box<[MarginLine; HEIGHT]> = match lines.try_into() {
            Ok(b) => b,
            Err(_) => unreachable!("vec has HEIGHT lines"),
        };
        Self {
            tiles: clamp_tiles(tiles),
            fill_left_clip: false,
            fill_right_clip: false,
            lines,
        }
    }

    /// Change the margin width (clamped); line contents are kept.
    pub fn set_tiles(&mut self, tiles: u8) {
        self.tiles = clamp_tiles(tiles);
    }

    /// Set every line to [`MarginFill::Backdrop`].
    pub fn clear_backdrop(&mut self) {
        for l in self.lines.iter_mut() {
            l.fill = MarginFill::Backdrop;
        }
    }
}

impl Default for Margins {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Wide indexed frame (`width x height`, NES palette indices, row-major).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WideFrame {
    /// Pixels per row (`wide_width(tiles)`).
    pub width: usize,
    /// Rows (always [`HEIGHT`]).
    pub height: usize,
    /// `width * height` indices.
    pub pixels: Vec<u8>,
}

impl WideFrame {
    /// Zeroed frame for `margin_tiles` per side.
    #[must_use]
    pub fn new(margin_tiles: u8) -> Self {
        let width = wide_width(margin_tiles);
        Self {
            width,
            height: HEIGHT,
            pixels: vec![0; width * HEIGHT],
        }
    }

    /// Resize for `margin_tiles` per side (no-op, no allocation when the
    /// width is unchanged).
    pub fn resize(&mut self, margin_tiles: u8) {
        let width = wide_width(margin_tiles);
        if width != self.width || self.pixels.len() != width * HEIGHT {
            self.width = width;
            self.height = HEIGHT;
            self.pixels.resize(width * HEIGHT, 0);
        }
    }

    /// Margin width per side in pixels.
    #[must_use]
    pub fn margin_px(&self) -> usize {
        (self.width - WIDTH) / 2
    }

    /// Row `y` (panics when out of range).
    #[must_use]
    pub fn row(&self, y: usize) -> &[u8] {
        &self.pixels[y * self.width..(y + 1) * self.width]
    }
}

const fn clamp_tiles(tiles: u8) -> u8 {
    if tiles as usize > MAX_MARGIN_TILES {
        MAX_MARGIN_TILES as u8
    } else {
        tiles
    }
}

/// Output width for `margin_tiles` per side: `256 + 16 * tiles` (tiles
/// clamped to [`MAX_MARGIN_TILES`]).
#[must_use]
pub const fn wide_width(margin_tiles: u8) -> usize {
    WIDTH + 16 * clamp_tiles(margin_tiles) as usize
}

/// Parse a widescreen preset into margin tiles per side, assuming square
/// pixels: `"off"`/`"0"` -> 0, `"16:10"` -> 8 (384x240, exactly 1.6),
/// `"16:9"` -> 11 (432x240, 1.8; true 16:9 is not reachable at tile
/// granularity), `"N"` with `1 <= N <= 16` -> N. Case/whitespace tolerant;
/// anything else is `None`.
#[must_use]
pub fn preset_tiles(name: &str) -> Option<u8> {
    let n = name.trim();
    if n.eq_ignore_ascii_case("off") || n.eq_ignore_ascii_case("none") {
        return Some(0);
    }
    match n {
        "16:10" => Some(8),
        "16:9" => Some(11),
        _ => match n.parse::<u8>() {
            Ok(t) if usize::from(t) <= MAX_MARGIN_TILES => Some(t),
            _ => None,
        },
    }
}

/// Raw 2-bit pattern value of tile `id` at column `sub_x` (0-7) and row
/// `id.fine_y` from the full CHR image (`page * 4096 + tile * 16 + fine_y`,
/// plane 1 at `+8`). `None` when `!id.fetched`, the page is [`NO_PAGE`], or
/// the byte lies outside `chr_rom`.
#[must_use]
pub fn chr_sub(chr_rom: &[u8], id: BgTileId, sub_x: u8) -> Option<u8> {
    if !id.fetched || id.page == NO_PAGE {
        return None;
    }
    let (lo, hi) = chr_row(chr_rom, id.page, id.tile, id.fine_y)?;
    let bit = 7 - (sub_x & 7);
    Some(((lo >> bit) & 1) | (((hi >> bit) & 1) << 1))
}

/// The two pattern-plane bytes of `tile` row `fine_row` on CHR asset `page`
/// (`page * 4096 + tile * 16 + fine_row`, plane 1 at `+8`). `None` for
/// [`NO_PAGE`] or a byte outside `chr_rom`.
#[must_use]
pub fn chr_row(chr_rom: &[u8], page: u8, tile: u8, fine_row: u8) -> Option<(u8, u8)> {
    if page == NO_PAGE {
        return None;
    }
    let base =
        usize::from(page) * CHR_BANK_LEN + usize::from(tile) * 16 + usize::from(fine_row & 7);
    Some((*chr_rom.get(base)?, *chr_rom.get(base + 8)?))
}

/// True when line `y` of `record` is covered by an *edge-mask sprite*: a
/// chosen, in-front sprite parked at x 248 whose pattern row is opaque in all
/// eight pixels, so it hides the background across the window's whole right
/// strip.
///
/// This is how Zelda II blanks the right edge — the NES has a `PPUMASK` bit
/// for the left 8 columns but none for the right, so the overworld parks a
/// column of 8x16 sprites (tile `$FE`, a sprite palette whose four entries
/// are all `$0F`) at x 248. Side-view areas have no such column, so the
/// right-edge fill leaves real sprites at the screen edge alone.
#[must_use]
pub fn right_edge_masked(record: &FrameRecord, y: usize, chr_rom: &[u8]) -> bool {
    let rec = record.line(y);
    if rec.mask & PPUMASK_SHOW_SPRITES == 0 {
        return false;
    }
    record.sprites_on(y).iter().any(|s| {
        usize::from(s.x) == WIDTH - 8
            && !s.behind
            && chr_row(chr_rom, s.page, s.tile, s.fine_row).is_some_and(|(lo, hi)| lo | hi == 0xFF)
    })
}

/// Indexed colour of tile `id` at `sub_x` on `rec`'s line; backdrop for
/// transparent / unknown pixels.
fn tile_pixel(chr_rom: &[u8], rec: &LineRecord, id: BgTileId, sub_x: u8) -> u8 {
    match chr_sub(chr_rom, id, sub_x) {
        None | Some(0) => rec.backdrop,
        Some(s) => {
            // Background indices are 0-15: a plain lookup (no mirror fold).
            let px = rec.palette[usize::from(id.pal & 3) * 4 + usize::from(s)];
            if rec.mask & PPUMASK_GRAYSCALE != 0 {
                px & 0x30
            } else {
                px
            }
        }
    }
}

/// Compose `out`: centre = `frame` verbatim, margins from `margins` and the
/// per-line palette/mask of `record`. `out` is resized to
/// `wide_width(margins.tiles)`; no allocation when the size is unchanged.
pub fn render_wide_indexed(
    frame: &IndexedFrame,
    record: &FrameRecord,
    margins: &Margins,
    chr_rom: &[u8],
    out: &mut WideFrame,
) {
    let tiles = clamp_tiles(margins.tiles);
    out.resize(tiles);
    let mp = 8 * usize::from(tiles);
    let width = out.width;
    for y in 0..HEIGHT {
        let row = &mut out.pixels[y * width..(y + 1) * width];
        row[mp..mp + WIDTH].copy_from_slice(&frame[y * WIDTH..(y + 1) * WIDTH]);
        let rec = record.line(y);
        let ml = &margins.lines[y];
        let paint = rec.valid && ml.fill == MarginFill::Tiles && FrameRecord::show_bg(rec);
        if !paint {
            row[..mp].fill(rec.backdrop);
            row[mp + WIDTH..].fill(rec.backdrop);
            continue;
        }
        let fine_x = i32::from(rec.fine_x & 7);
        let m = i32::from(tiles);
        // Left margin: window x in [-8M, 0).
        for wx in -8 * m..0 {
            let k = (wx + fine_x).div_euclid(8);
            let sub_x = (wx + fine_x).rem_euclid(8) as u8;
            let id = if k < 0 {
                ml.left
                    .get((-1 - k) as usize)
                    .copied()
                    .unwrap_or(BgTileId::NONE)
            } else {
                rec.tiles[k as usize]
            };
            row[(wx + 8 * m) as usize] = tile_pixel(chr_rom, rec, id, sub_x);
        }
        // Right margin: window x in [256, 256 + 8M).
        for wx in WIDTH as i32..WIDTH as i32 + 8 * m {
            let k = (wx + fine_x).div_euclid(8);
            let sub_x = (wx + fine_x).rem_euclid(8) as u8;
            let id = ml
                .right
                .get((k - 32) as usize)
                .copied()
                .unwrap_or(BgTileId::NONE);
            row[(wx + 8 * m) as usize] = tile_pixel(chr_rom, rec, id, sub_x);
        }
        if margins.fill_left_clip
            && rec.mask & (PPUMASK_SHOW_LEFT_BG | PPUMASK_SHOW_LEFT_SPRITES) == 0
        {
            for wx in 0..8i32 {
                let k = ((wx + fine_x) >> 3) as usize;
                let sub_x = ((wx + fine_x) & 7) as u8;
                let id = rec.tiles[k.min(RECORD_TILES_PER_LINE - 1)];
                row[mp + wx as usize] = tile_pixel(chr_rom, rec, id, sub_x);
            }
        }
        if margins.fill_right_clip && right_edge_masked(record, y, chr_rom) {
            for wx in (WIDTH as i32 - 8)..WIDTH as i32 {
                let k = ((wx + fine_x) >> 3) as usize;
                let sub_x = ((wx + fine_x) & 7) as u8;
                let id = rec.tiles[k.min(RECORD_TILES_PER_LINE - 1)];
                row[mp + wx as usize] = tile_pixel(chr_rom, rec, id, sub_x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::SpriteRef;
    use crate::state::PPUMASK_SHOW_BG;

    #[test]
    fn preset_table() {
        assert_eq!(preset_tiles("off"), Some(0));
        assert_eq!(preset_tiles(" OFF "), Some(0));
        assert_eq!(preset_tiles("0"), Some(0));
        assert_eq!(preset_tiles("16:10"), Some(8));
        assert_eq!(preset_tiles("16:9"), Some(11));
        assert_eq!(preset_tiles("1"), Some(1));
        assert_eq!(preset_tiles("16"), Some(16));
        assert_eq!(preset_tiles("17"), None);
        assert_eq!(preset_tiles("4:3"), None);
        assert_eq!(preset_tiles(""), None);
        assert_eq!(preset_tiles("wide"), None);
    }

    #[test]
    fn widths() {
        assert_eq!(wide_width(0), 256);
        assert_eq!(wide_width(8), 384);
        assert_eq!(wide_width(11), 432);
        assert_eq!(wide_width(16), 512);
        assert_eq!(wide_width(200), 512, "clamped");
        let mut w = WideFrame::new(11);
        assert_eq!((w.width, w.height, w.pixels.len()), (432, 240, 432 * 240));
        assert_eq!(w.margin_px(), 88);
        w.resize(0);
        assert_eq!(
            (w.width, w.pixels.len(), w.margin_px()),
            (256, 256 * 240, 0)
        );
        assert_eq!(w.row(3).len(), 256);
        assert_eq!(Margins::new(99).tiles, 16);
    }

    /// Two-page CHR image: page 2 tile 5 row 3 has plane 0 = `1000_0001`;
    /// page 2 tile 6 is solid sub 3 on every row.
    fn chr_image() -> Vec<u8> {
        let mut chr = vec![0u8; CHR_BANK_LEN * 3];
        chr[2 * CHR_BANK_LEN + 5 * 16 + 3] = 0b1000_0001;
        for r in 0..8 {
            chr[2 * CHR_BANK_LEN + 6 * 16 + r] = 0xFF;
            chr[2 * CHR_BANK_LEN + 6 * 16 + r + 8] = 0xFF;
        }
        chr
    }

    fn id(tile: u8, pal: u8) -> BgTileId {
        BgTileId {
            page: 2,
            tile,
            pal,
            fine_y: 3,
            fetched: true,
            ..BgTileId::NONE
        }
    }

    fn record_line(fine_x: u8) -> LineRecord {
        let mut l = LineRecord::EMPTY;
        l.valid = true;
        l.mask = PPUMASK_SHOW_BG;
        l.fine_x = fine_x;
        l.backdrop = 0x0F;
        for (i, p) in l.palette.iter_mut().enumerate() {
            *p = 0x20 + i as u8;
        }
        l
    }

    fn pattern_frame() -> Box<IndexedFrame> {
        let mut f = Box::new([0u8; WIDTH * HEIGHT]);
        for (i, px) in f.iter_mut().enumerate() {
            *px = ((i * 7 + i / 256) % 64) as u8;
        }
        f
    }

    #[test]
    fn centre_is_byte_identical() {
        let frame = pattern_frame();
        let mut rec = FrameRecord::new();
        let chr = chr_image();
        for y in 0..HEIGHT {
            rec.lines[y] = record_line((y % 8) as u8);
        }
        for tiles in [0u8, 1, 8, 11, 16] {
            let mut m = Margins::new(tiles);
            for (y, l) in m.lines.iter_mut().enumerate() {
                l.fill = if y % 3 == 0 {
                    MarginFill::Backdrop
                } else {
                    MarginFill::Tiles
                };
                l.left = [id(6, 1); MARGIN_SLOTS];
                l.right = [id(5, 2); MARGIN_SLOTS];
            }
            let mut out = WideFrame::new(0);
            render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
            let mp = out.margin_px();
            assert_eq!(out.width, wide_width(tiles));
            for y in 0..HEIGHT {
                assert_eq!(
                    &out.row(y)[mp..mp + WIDTH],
                    &frame[y * WIDTH..(y + 1) * WIDTH],
                    "tiles {tiles} row {y}"
                );
            }
        }
    }

    #[test]
    fn margins_backdrop_when_bg_off_policy_backdrop_or_unrecorded() {
        let frame = pattern_frame();
        let chr = chr_image();
        let mut rec = FrameRecord::new();
        rec.lines[0] = record_line(0);
        rec.lines[0].backdrop = 0x11; // policy Backdrop
        rec.lines[1] = record_line(0);
        rec.lines[1].mask = 0; // bg off
        rec.lines[1].backdrop = 0x12;
        // line 2 stays unrecorded (EMPTY backdrop $0F)
        let mut m = Margins::new(2);
        for l in m.lines.iter_mut() {
            l.left = [id(6, 1); MARGIN_SLOTS];
            l.right = [id(6, 1); MARGIN_SLOTS];
        }
        m.lines[1].fill = MarginFill::Tiles;
        m.lines[2].fill = MarginFill::Tiles;
        let mut out = WideFrame::new(2);
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        for (y, want) in [(0usize, 0x11u8), (1, 0x12), (2, 0x0F)] {
            let r = out.row(y);
            assert!(r[..16].iter().all(|&p| p == want), "line {y} left");
            assert!(r[16 + 256..].iter().all(|&p| p == want), "line {y} right");
        }
    }

    #[test]
    fn margin_pixel_formula_with_fine_x() {
        let frame = pattern_frame();
        let chr = chr_image();
        let tiles = 3u8;
        let m_i = i32::from(tiles);
        let mut rec = FrameRecord::new();
        rec.lines[10] = record_line(5);
        rec.lines[11] = record_line(7);
        rec.lines[12] = record_line(5);
        rec.lines[12].mask |= PPUMASK_GRAYSCALE;
        let mut m = Margins::new(tiles);
        for y in 10..13 {
            let l = &mut m.lines[y];
            l.fill = MarginFill::Tiles;
            l.left[0] = id(5, 2);
            l.right[0] = id(5, 1);
            l.right[usize::from(tiles)] = id(6, 3);
        }
        let mut out = WideFrame::new(0);
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        let at = |y: usize, wx: i32| out.row(y)[(wx + 8 * m_i) as usize];
        // fine_x 5: x = -6 -> k = -1, sub_x 7 -> plane bit set -> pal 2 idx 1.
        assert_eq!(at(10, -6), 0x20 + 2 * 4 + 1);
        // x = -13 -> k = -1, sub_x 0 -> set.
        assert_eq!(at(10, -13), 0x20 + 9);
        // x = -12 -> k = -1, sub_x 1 -> transparent -> backdrop.
        assert_eq!(at(10, -12), 0x0F);
        // x = -1 -> k = 0 -> record tiles[0] (unfetched) -> backdrop.
        assert_eq!(at(10, -1), 0x0F);
        // x = 256 -> k = 32 (right[0]), sub_x 5 -> clear -> backdrop.
        assert_eq!(at(10, 256), 0x0F);
        // x = 259 -> k = 33 -> right[1] unset -> backdrop; x = 258 -> k 32 sub 7 set.
        assert_eq!(at(10, 258), 0x20 + 4 + 1);
        // fine_x 7, last pixel x = 255 + 8M -> k = 32 + M (right[M]) solid sub 3 pal 3.
        assert_eq!(at(11, 255 + 8 * m_i), 0x20 + 3 * 4 + 3);
        // Greyscale masks tile pixels.
        assert_eq!(at(12, -6), (0x20 + 9) & 0x30);
        // Unset page -> backdrop.
        let mut m2 = m.clone();
        m2.lines[10].left[0].page = NO_PAGE;
        let mut out2 = WideFrame::new(0);
        render_wide_indexed(&frame, &rec, &m2, &chr, &mut out2);
        assert_eq!(out2.row(10)[(-6 + 8 * m_i) as usize], 0x0F);
    }

    #[test]
    fn left_clip_fill_is_opt_in() {
        let frame = Box::new([0x0Fu8; WIDTH * HEIGHT]);
        let chr = chr_image();
        let mut rec = FrameRecord::new();
        rec.lines[0] = record_line(0);
        rec.lines[0].tiles[0] = id(6, 1);
        let mut m = Margins::new(1);
        m.lines[0].fill = MarginFill::Tiles;
        let mut out = WideFrame::new(1);
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        assert!(
            out.row(0)[8..16].iter().all(|&p| p == 0x0F),
            "off: verbatim"
        );
        m.fill_left_clip = true;
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        assert!(
            out.row(0)[8..16].iter().all(|&p| p == 0x20 + 7),
            "on: painted"
        );
        assert_eq!(out.row(0)[16], 0x0F, "column 8 untouched");
    }

    /// The overworld's right-edge mask: one opaque 8x16 sprite parked at
    /// x 248, drawn in front. `tile` 6 of [`chr_image`] is opaque on every
    /// row; tile 5 is not.
    fn mask_sprite(tile: u8, behind: bool) -> SpriteRef {
        SpriteRef {
            oam_index: 4,
            x: (WIDTH - 8) as u8,
            page: 2,
            tile,
            fine_row: 3,
            row_in_sprite: 3,
            pal: 1,
            behind,
            tall: true,
            ..SpriteRef::default()
        }
    }

    /// One frame record whose line 0 draws `tiles[31]` and carries `sprites`.
    fn edge_record(sprites: &[SpriteRef]) -> FrameRecord {
        let mut rec = FrameRecord::new();
        rec.lines[0] = record_line(0);
        rec.lines[0].mask |= PPUMASK_SHOW_SPRITES;
        rec.lines[0].tiles[31] = id(6, 1);
        rec.lines[0].sprite_start = 0;
        rec.lines[0].sprite_len = sprites.len() as u8;
        rec.sprites.extend_from_slice(sprites);
        rec
    }

    #[test]
    fn right_clip_fill_is_opt_in_and_only_under_an_edge_mask_sprite() {
        let frame = Box::new([0x0Fu8; WIDTH * HEIGHT]);
        let chr = chr_image();
        let rec = edge_record(&[mask_sprite(6, false)]);
        let mut m = Margins::new(1);
        m.fill_left_clip = true; // the left path must not reach the right edge
        m.lines[0].fill = MarginFill::Tiles;
        let mut out = WideFrame::new(1);

        // Off: the centre is the frame verbatim, seam and all.
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        assert_eq!(
            &out.row(0)[8..8 + WIDTH],
            &frame[..WIDTH],
            "off: centre byte-identical"
        );
        assert!(
            out.row(0)[8 + WIDTH - 8..8 + WIDTH]
                .iter()
                .all(|&p| p == 0x0F),
            "off: right strip is the masked backdrop"
        );

        // On: the right strip carries tiles[31] in its own palette.
        m.fill_right_clip = true;
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        let want = 0x20 + 4 + 3; // palette group 1, pattern 3
        assert!(
            out.row(0)[8 + WIDTH - 8..8 + WIDTH]
                .iter()
                .all(|&p| p == want),
            "on: right strip painted from the recorded tile, got {:02X?}",
            &out.row(0)[8 + WIDTH - 8..8 + WIDTH]
        );
        assert_eq!(
            &out.row(0)[8..8 + WIDTH - 8],
            &frame[..WIDTH - 8],
            "on: the only centre difference is inside the masked strip"
        );
        for y in 1..HEIGHT {
            assert_eq!(
                &out.row(y)[8..8 + WIDTH],
                &frame[y * WIDTH..(y + 1) * WIDTH],
                "on: unmasked line {y} untouched"
            );
        }

        // Nothing to undo without an opaque, in-front sprite at x 248.
        for sprites in [
            vec![],
            vec![mask_sprite(5, false)], // not opaque across the row
            vec![mask_sprite(6, true)],  // behind the background
            vec![SpriteRef {
                x: (WIDTH - 16) as u8,
                ..mask_sprite(6, false)
            }],
        ] {
            let rec = edge_record(&sprites);
            render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
            assert_eq!(
                &out.row(0)[8..8 + WIDTH],
                &frame[..WIDTH],
                "no edge mask ({sprites:?}): centre byte-identical"
            );
        }

        // Sprites disabled on the line: the mask was never drawn either.
        let mut rec = edge_record(&[mask_sprite(6, false)]);
        rec.lines[0].mask &= !PPUMASK_SHOW_SPRITES;
        assert!(!right_edge_masked(&rec, 0, &chr));
    }

    #[test]
    fn chr_sub_bounds() {
        let chr = chr_image();
        assert_eq!(chr_sub(&chr, id(5, 0), 0), Some(1));
        assert_eq!(chr_sub(&chr, id(5, 0), 1), Some(0));
        assert_eq!(chr_sub(&chr, id(6, 0), 4), Some(3));
        assert_eq!(chr_sub(&chr, BgTileId::NONE, 0), None);
        let mut far = id(5, 0);
        far.page = 30;
        assert_eq!(chr_sub(&chr, far, 0), None, "outside image");
    }
}
