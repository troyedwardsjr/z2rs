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
//! Sprites: OAM X is 8-bit, so nothing in OAM can reach the margins. The game
//! does keep objects alive well outside the window, though, and a provider can
//! describe them as [`MarginSprite`]s in window coordinates
//! ([`Margins::sprites`]); they are drawn over the margin background with the
//! hardware sprite rules ([`margin_sprite_rows`]). Lines whose background is
//! disabled, unrecorded, or marked [`MarginFill::Backdrop`] get the line's
//! backdrop colour and no margin sprites.
//!
//! Two opt-in settings repaint the window's own edge strips in the wide
//! buffer only, so the game's edge blanking does not read as a black seam
//! between the picture and a painted margin: [`Margins::fill_left_clip`] for
//! the 8 columns `PPUMASK` clips, [`Margins::fill_right_clip`] for the 8 the
//! game hides behind an opaque edge-mask sprite. Both default off, and
//! neither can touch anything outside its own 8 columns. Inside a strip the
//! provider's own identities ([`MarginLine::edge_left`] /
//! [`MarginLine::edge_right`]) win over the record's, because what a game
//! hides there can be stale; [`edge_fill`] and [`wide_bg_tile`] are the
//! single definition of which tile each wide pixel shows, shared with the
//! HD compositor in `z2-render`.
//! A third, [`Margins::fill_left_sprites`], draws the in-window part of
//! margin sprites that start just left of the window (the game hides such a
//! sprite whole, since its X would wrap), again only inside the left 8
//! columns.
//!
//! Slot geometry matches [`LineRecord::tiles`]: fetch slot `k` covers window
//! pixels `[8k - fine_x, 8k - fine_x + 8)`, where window x = 0 is NES column
//! 0 and margin pixels have negative x (left) or x >= 256 (right).

use crate::record::{BgTileId, FrameRecord, LineRecord, SpriteRef, NO_PAGE};
use crate::state::{
    CHR_BANK_LEN, PPUCTRL_SPRITE_TABLE, PPUCTRL_TALL_SPRITES, PPUMASK_GRAYSCALE,
    PPUMASK_SHOW_LEFT_BG, PPUMASK_SHOW_LEFT_SPRITES, PPUMASK_SHOW_SPRITES,
};
use crate::{IndexedFrame, MarginSprite, HEIGHT, WIDTH};

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
/// fall in slot 0 (when `fine_x != 0`) use `edge_left[0]` when the provider
/// set it, else the record's own `tiles[0]`.
///
/// `edge_left` / `edge_right` are the provider's own identities for the
/// window's edge slots `0, 1` and `31, 32`. They exist because a game can
/// hide stale tiles in its edge strips: the overworld scrolls through a
/// single 32-column nametable ring, so slot 0 and slot 32 are the same
/// nametable column and can only hold one of the two world columns, and the
/// column being streamed in sits in slot 1 or 31 half-written. A provider
/// that knows the real scenery sets these; an unset (`!fetched`) entry
/// falls back to the record. They are only read where the wide buffer
/// repaints what the game hid: the edge strips ([`EdgeFill`]) and slot 0's
/// leading columns in the left margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarginLine {
    /// Fill policy for this line.
    pub fill: MarginFill,
    /// Slots `-1, -2, ...` leftwards.
    pub left: [BgTileId; MARGIN_SLOTS],
    /// Slots `32, 33, ...` rightwards.
    pub right: [BgTileId; MARGIN_SLOTS],
    /// Slots `0` and `1`, overriding the record in the left edge strip.
    pub edge_left: [BgTileId; 2],
    /// Slots `31` and `32`, overriding the record in the right edge strip.
    pub edge_right: [BgTileId; 2],
}

impl MarginLine {
    /// A backdrop line with no tiles.
    pub const BACKDROP: MarginLine = MarginLine {
        fill: MarginFill::Backdrop,
        left: [BgTileId::NONE; MARGIN_SLOTS],
        right: [BgTileId::NONE; MARGIN_SLOTS],
        edge_left: [BgTileId::NONE; 2],
        edge_right: [BgTileId::NONE; 2],
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
    /// Also draw the in-window part (x 0-7) of margin sprites whose left
    /// column lies at window x -7..-1. The game hides such a sprite entirely
    /// (its OAM X would wrap to the right edge), so without this an object
    /// sliding out of the left edge vanishes 1-7 px early. Only where the line
    /// shows sprites in the left 8 columns, or [`Margins::fill_left_clip`]
    /// repainted them; never over a real sprite pixel. Cosmetic; default off
    /// so the centre stays byte-identical to the frame.
    pub fill_left_sprites: bool,
    /// Sprites outside the window, in priority order (earlier wins), drawn
    /// over the margin background of lines whose fill is
    /// [`MarginFill::Tiles`] and whose `PPUMASK` shows sprites. Providers
    /// refill this every frame; it is empty by default.
    pub sprites: Vec<MarginSprite>,
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
            fill_left_sprites: false,
            sprites: Vec::new(),
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

/// Which of the window's edge strips the wide image repaints on one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeFill {
    /// Window x 0-7 ([`Margins::fill_left_clip`] on a line that clipped both
    /// left-edge background and sprites).
    pub left: bool,
    /// Window x 248-255 ([`Margins::fill_right_clip`] on a line covered by
    /// the edge-mask sprite column, see [`right_edge_masked`]).
    pub right: bool,
}

/// The edge strips line `y` repaints. Both need a line whose margins are
/// [`MarginFill::Tiles`] with background rendering on; this is the one gate
/// every wide compositor (indexed and HD) uses, so they cannot disagree.
#[must_use]
pub fn edge_fill(record: &FrameRecord, margins: &Margins, y: usize, chr_rom: &[u8]) -> EdgeFill {
    let rec = record.line(y);
    let tiles_line = rec.valid
        && FrameRecord::show_bg(rec)
        && margins
            .lines
            .get(y)
            .is_some_and(|ml| ml.fill == MarginFill::Tiles);
    if !tiles_line {
        return EdgeFill::default();
    }
    EdgeFill {
        left: margins.fill_left_clip
            && rec.mask & (PPUMASK_SHOW_LEFT_BG | PPUMASK_SHOW_LEFT_SPRITES) == 0,
        right: margins.fill_right_clip && right_edge_masked(record, y, chr_rom),
    }
}

/// Background tile identity and pattern column behind wide pixel `wx` of
/// one line (window x; negative in the left margin, `>= 256` in the right).
///
/// * Margins (`ml` present and [`MarginFill::Tiles`]): `ml.left` /
///   `ml.right`, except slot 0's leading `fine_x` columns, which are slot 0:
///   `ml.edge_left[0]` when set, else `rec.tiles[0]`. Otherwise
///   [`BgTileId::NONE`].
/// * Window: `rec.tiles[k]`, except inside an edge strip `fill` repaints,
///   where `ml.edge_left` / `ml.edge_right` win when set. A pixel the
///   hardware clipped (left 8 with the left-background bit clear and no
///   left fill) or a line with background off is [`BgTileId::NONE`].
#[must_use]
pub fn wide_bg_tile(
    rec: &LineRecord,
    ml: Option<&MarginLine>,
    fill: EdgeFill,
    wx: i32,
) -> (BgTileId, u8) {
    let fine_x = i32::from(rec.fine_x & 7);
    let k = (wx + fine_x).div_euclid(8);
    let sub_x = (wx + fine_x).rem_euclid(8) as u8;
    let record_slot = |k: i32| {
        usize::try_from(k)
            .ok()
            .and_then(|k| rec.tiles.get(k).copied())
            .unwrap_or(BgTileId::NONE)
    };
    let or_record = |id: Option<BgTileId>, k: i32| match id {
        Some(id) if id.fetched => id,
        _ => record_slot(k),
    };
    if !FrameRecord::show_bg(rec) {
        return (BgTileId::NONE, sub_x);
    }
    let ml_tiles = ml.filter(|ml| ml.fill == MarginFill::Tiles);
    let id = if wx < 0 {
        match ml_tiles {
            None => BgTileId::NONE,
            Some(ml) if k >= 0 => or_record(ml.edge_left.first().copied(), k),
            Some(ml) => ml
                .left
                .get((-1 - k) as usize)
                .copied()
                .unwrap_or(BgTileId::NONE),
        }
    } else if wx >= WIDTH as i32 {
        match ml_tiles {
            None => BgTileId::NONE,
            Some(ml) => ml
                .right
                .get((k - 32) as usize)
                .copied()
                .unwrap_or(BgTileId::NONE),
        }
    } else if wx < 8 {
        if fill.left {
            or_record(
                ml_tiles.and_then(|ml| ml.edge_left.get(k as usize).copied()),
                k,
            )
        } else if rec.mask & PPUMASK_SHOW_LEFT_BG == 0 {
            BgTileId::NONE
        } else {
            record_slot(k)
        }
    } else if wx >= WIDTH as i32 - 8 && fill.right {
        or_record(
            ml_tiles.and_then(|ml| ml.edge_right.get((k - 31) as usize).copied()),
            k,
        )
    } else {
        record_slot(k)
    };
    (id, sub_x)
}

/// One line's row of a [`MarginSprite`], resolved like an OAM sprite row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarginSpriteRow {
    /// Index into [`Margins::sprites`] (lower wins).
    pub index: usize,
    /// Window x of the sprite's left column.
    pub x: i32,
    /// The row as the PPU would have chosen it: CHR page, resolved tile and
    /// pattern row (8x16 half and V-flip applied), palette, flips, priority.
    /// `x` and `oam_index` are 0 — the window x is [`MarginSpriteRow::x`].
    pub sprite: SpriteRef,
}

impl MarginSpriteRow {
    /// Pattern identity of the row (for [`chr_sub`]).
    #[must_use]
    pub fn pattern(&self) -> BgTileId {
        BgTileId {
            page: self.sprite.page,
            tile: self.sprite.tile,
            pal: self.sprite.pal,
            fine_y: self.sprite.fine_row & 7,
            fetched: true,
            ..BgTileId::NONE
        }
    }
}

/// Resolve sprite `s` on line `y` of `rec` the way the PPU's sprite pass
/// does: top row `s.y + 1`; 8x16 when `PPUCTRL` bit 5 is set (pattern table
/// from `tile & 1`, top half `tile & $FE`, bottom half `+ 1`), else 8x8 from
/// the `PPUCTRL` sprite table; V-flip swaps rows (and halves). `None` when
/// the sprite does not cover the line or the line has no CHR page for it.
#[must_use]
pub fn margin_sprite_on_line(rec: &LineRecord, y: usize, s: &MarginSprite) -> Option<SpriteRef> {
    let tall = rec.ctrl & PPUCTRL_TALL_SPRITES != 0;
    let height: u16 = if tall { 16 } else { 8 };
    let top = u16::from(s.y) + 1;
    let line = y as u16;
    if line < top || line >= top + height {
        return None;
    }
    let row_in_sprite = line - top;
    let flip_v = s.attr & 0x80 != 0;
    let (table, tile, fine_row) = if tall {
        let r = if flip_v {
            15 - row_in_sprite
        } else {
            row_in_sprite
        };
        let top_tile = s.tile & 0xFE;
        let t = if r < 8 {
            top_tile
        } else {
            top_tile.wrapping_add(1)
        };
        (usize::from(s.tile & 1), t, (r & 7) as u8)
    } else {
        let r = if flip_v {
            7 - row_in_sprite
        } else {
            row_in_sprite
        };
        (
            usize::from(rec.ctrl & PPUCTRL_SPRITE_TABLE != 0),
            s.tile,
            r as u8,
        )
    };
    let page = rec.chr_page[table];
    if page == NO_PAGE {
        return None;
    }
    Some(SpriteRef {
        oam_index: 0,
        x: 0,
        page,
        tile,
        fine_row,
        row_in_sprite: row_in_sprite as u8,
        pal: s.attr & 3,
        flip_h: s.attr & 0x40 != 0,
        flip_v,
        behind: s.attr & 0x20 != 0,
        tall,
    })
}

/// The rows of [`Margins::sprites`] on line `y`, in priority order. Empty
/// unless the line was recorded, shows sprites, and its margin fill is
/// [`MarginFill::Tiles`] (HUD rows, pause panes and title screens keep plain
/// backdrop margins). Both compositors draw margin sprites through this, then
/// test each pixel with [`margin_sprite_paints`].
pub fn margin_sprite_rows<'a>(
    margins: &'a Margins,
    record: &'a FrameRecord,
    y: usize,
) -> impl Iterator<Item = MarginSpriteRow> + 'a {
    let rec = record.line(y);
    let live = !margins.sprites.is_empty()
        && rec.valid
        && rec.mask & PPUMASK_SHOW_SPRITES != 0
        && FrameRecord::show_bg(rec)
        && margins.lines[y].fill == MarginFill::Tiles;
    let sprites: &[MarginSprite] = if live { &margins.sprites } else { &[] };
    sprites.iter().enumerate().filter_map(move |(index, s)| {
        margin_sprite_on_line(rec, y, s).map(|sprite| MarginSpriteRow {
            index,
            x: i32::from(s.x),
            sprite,
        })
    })
}

/// Whether [`Margins::fill_left_sprites`] applies on `rec`'s line: the flag
/// is on and the line shows sprites in its left 8 columns — or those columns
/// were clipped and [`Margins::fill_left_clip`] repainted them.
#[must_use]
pub fn left_sprite_fill(margins: &Margins, rec: &LineRecord) -> bool {
    margins.fill_left_sprites
        && (rec.mask & PPUMASK_SHOW_LEFT_SPRITES != 0
            || (margins.fill_left_clip
                && rec.mask & (PPUMASK_SHOW_LEFT_BG | PPUMASK_SHOW_LEFT_SPRITES) == 0))
}

/// Whether a margin sprite pixel at window x `wx` is drawn: inside a margin
/// `margin_px` wide, or — for a row starting left of the window
/// (`row_x < 0`) when `left_fill` ([`left_sprite_fill`]) — in window
/// columns 0-7.
#[must_use]
pub fn margin_sprite_paints(wx: i32, row_x: i32, margin_px: i32, left_fill: bool) -> bool {
    (-margin_px..0).contains(&wx)
        || (WIDTH as i32..WIDTH as i32 + margin_px).contains(&wx)
        || (left_fill && row_x < 0 && (0..8).contains(&wx))
}

/// Whether a real (recorded) sprite drew an opaque pixel at window x `x` of
/// line `y` — a margin sprite never covers one.
#[must_use]
pub fn window_sprite_opaque(record: &FrameRecord, y: usize, chr_rom: &[u8], x: usize) -> bool {
    let rec = record.line(y);
    if rec.mask & PPUMASK_SHOW_SPRITES == 0 || (x < 8 && rec.mask & PPUMASK_SHOW_LEFT_SPRITES == 0)
    {
        return false;
    }
    record.sprites_on(y).iter().any(|s| {
        let left = usize::from(s.x);
        if !(left..left + 8).contains(&x) {
            return false;
        }
        let dx = (x - left) as u8;
        let col = if s.flip_h { 7 - dx } else { dx };
        let id = BgTileId {
            page: s.page,
            tile: s.tile,
            fine_y: s.fine_row & 7,
            fetched: true,
            ..BgTileId::NONE
        };
        chr_sub(chr_rom, id, col).is_some_and(|v| v != 0)
    })
}

/// Draw [`Margins::sprites`] into one wide row (`mp` = margin pixels).
fn paint_margin_sprites(
    record: &FrameRecord,
    y: usize,
    margins: &Margins,
    chr_rom: &[u8],
    mp: usize,
    row: &mut [u8],
) {
    let rec = record.line(y);
    let ml = &margins.lines[y];
    let left_fill = left_sprite_fill(margins, rec);
    let fill = edge_fill(record, margins, y, chr_rom);
    let mut claimed = [false; WIDTH + 2 * 8 * MAX_MARGIN_TILES];
    if left_fill {
        for x in 0..8 {
            claimed[mp + x] = window_sprite_opaque(record, y, chr_rom, x);
        }
    }
    let grey = rec.mask & PPUMASK_GRAYSCALE != 0;
    for r in margin_sprite_rows(margins, record, y) {
        let s = r.sprite;
        let id = r.pattern();
        for dx in 0..8i32 {
            let wx = r.x + dx;
            if !margin_sprite_paints(wx, r.x, mp as i32, left_fill) {
                continue;
            }
            let col = if s.flip_h { 7 - dx } else { dx } as u8;
            let sub = chr_sub(chr_rom, id, col).unwrap_or(0);
            if sub == 0 {
                continue;
            }
            let sx = (wx + mp as i32) as usize;
            if claimed[sx] {
                continue;
            }
            claimed[sx] = true;
            if s.behind {
                let (bg, bg_sub) = wide_bg_tile(rec, Some(ml), fill, wx);
                if chr_sub(chr_rom, bg, bg_sub).is_some_and(|v| v != 0) {
                    continue;
                }
            }
            let px = rec.palette_entry(0x10 + usize::from(s.pal & 3) * 4 + usize::from(sub));
            row[sx] = if grey { px & 0x30 } else { px };
        }
    }
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
        let m = i32::from(tiles);
        let fill = edge_fill(record, margins, y, chr_rom);
        let mut paint_px = |wx: i32| {
            let (id, sub_x) = wide_bg_tile(rec, Some(ml), fill, wx);
            row[(wx + 8 * m) as usize] = tile_pixel(chr_rom, rec, id, sub_x);
        };
        // Margins: window x in [-8M, 0) and [256, 256 + 8M).
        (-8 * m..0).for_each(&mut paint_px);
        (WIDTH as i32..WIDTH as i32 + 8 * m).for_each(&mut paint_px);
        // The edge strips the game hid, only where the fill applies.
        if fill.left {
            (0..8).for_each(&mut paint_px);
        }
        if fill.right {
            (WIDTH as i32 - 8..WIDTH as i32).for_each(&mut paint_px);
        }
        if !margins.sprites.is_empty() {
            paint_margin_sprites(record, y, margins, chr_rom, mp, row);
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

    /// The one gate both compositors share: a strip is repainted only on a
    /// tile-margin line with background on, left under the left clip, right
    /// under the edge-mask sprite; an unset edge identity falls back to the
    /// record, a set one wins only inside a repainted strip.
    #[test]
    fn edge_fill_gate_and_wide_bg_tile_sources() {
        let chr = chr_image();
        let mut rec = edge_record(&[mask_sprite(6, false)]);
        rec.lines[0].tiles[0] = id(5, 0);
        rec.lines[0].tiles[1] = id(5, 1);
        let mut m = Margins::new(1);
        m.fill_left_clip = true;
        m.fill_right_clip = true;
        assert_eq!(
            edge_fill(&rec, &m, 0, &chr),
            EdgeFill::default(),
            "backdrop line"
        );
        m.lines[0].fill = MarginFill::Tiles;
        let fill = edge_fill(&rec, &m, 0, &chr);
        assert_eq!(
            fill,
            EdgeFill {
                left: true,
                right: true
            }
        );
        rec.lines[0].mask |= PPUMASK_SHOW_LEFT_SPRITES;
        assert!(!edge_fill(&rec, &m, 0, &chr).left, "left sprites shown");
        rec.lines[0].mask &= !PPUMASK_SHOW_LEFT_SPRITES;

        let line = *rec.line(0);
        let ml = m.lines[0];
        // Unset edge identities: the record.
        assert_eq!(wide_bg_tile(&line, Some(&ml), fill, 0).0, id(5, 0));
        assert_eq!(wide_bg_tile(&line, Some(&ml), fill, 250).0, id(6, 1));
        let mut ml = ml;
        ml.edge_left = [id(6, 2), id(6, 3)];
        ml.edge_right = [id(5, 2), id(5, 3)];
        assert_eq!(wide_bg_tile(&line, Some(&ml), fill, 0).0, id(6, 2));
        assert_eq!(wide_bg_tile(&line, Some(&ml), fill, 250).0, id(5, 2));
        // Outside the strips the record still rules, whatever is set.
        assert_eq!(wide_bg_tile(&line, Some(&ml), fill, 8).0, id(5, 1));
        assert_eq!(wide_bg_tile(&line, Some(&ml), fill, 247).0, line.tiles[30]);
        // Strips off: clipped left pixel shows nothing, right is the record.
        let off = EdgeFill::default();
        assert!(!wide_bg_tile(&line, Some(&ml), off, 0).0.fetched);
        assert_eq!(wide_bg_tile(&line, Some(&ml), off, 250).0, id(6, 1));
        // No margins at all: margin pixels have no tile.
        assert!(!wide_bg_tile(&line, None, off, -1).0.fetched);
        assert!(!wide_bg_tile(&line, None, off, 256).0.fetched);
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

    fn msprite(x: i16, y: u8, tile: u8, attr: u8) -> MarginSprite {
        MarginSprite { x, y, tile, attr }
    }

    #[test]
    fn margin_sprite_row_expansion_8x8_and_8x16() {
        let mut l = record_line(0);
        l.chr_page = [1, 2];
        // 8x8, sprite table 1 (page 2): top row is y + 1.
        l.ctrl = PPUCTRL_SPRITE_TABLE;
        let s = msprite(-20, 9, 0x44, 0x03);
        assert_eq!(
            margin_sprite_on_line(&l, 9, &s),
            None,
            "row y is above the sprite"
        );
        let r = margin_sprite_on_line(&l, 10, &s).unwrap();
        assert_eq!(
            (r.page, r.tile, r.fine_row, r.pal, r.tall),
            (2, 0x44, 0, 3, false)
        );
        assert_eq!(margin_sprite_on_line(&l, 17, &s).unwrap().fine_row, 7);
        assert_eq!(margin_sprite_on_line(&l, 18, &s), None, "8 rows only");
        // V-flip reverses rows; H-flip and priority are carried.
        let r = margin_sprite_on_line(&l, 10, &msprite(0, 9, 0x44, 0xE0)).unwrap();
        assert_eq!(
            (r.fine_row, r.flip_v, r.flip_h, r.behind),
            (7, true, true, true)
        );
        // 8x8 on sprite table 0 reads page 1.
        l.ctrl = 0;
        assert_eq!(margin_sprite_on_line(&l, 10, &s).unwrap().page, 1);

        // 8x16: table from tile bit 0, top half tile & $FE, bottom half + 1.
        l.ctrl = PPUCTRL_TALL_SPRITES;
        let s = msprite(300, 99, 0x45, 0x00);
        let top = margin_sprite_on_line(&l, 100, &s).unwrap();
        assert_eq!(
            (top.page, top.tile, top.fine_row, top.tall),
            (2, 0x44, 0, true)
        );
        let bottom = margin_sprite_on_line(&l, 108, &s).unwrap();
        assert_eq!(
            (bottom.tile, bottom.fine_row, bottom.row_in_sprite),
            (0x45, 0, 8)
        );
        assert!(margin_sprite_on_line(&l, 115, &s).is_some());
        assert_eq!(margin_sprite_on_line(&l, 116, &s), None, "16 rows only");
        // V-flipped 8x16: the first line shows the bottom half's last row.
        let r = margin_sprite_on_line(&l, 100, &msprite(300, 99, 0x44, 0x80)).unwrap();
        assert_eq!((r.page, r.tile, r.fine_row), (1, 0x45, 7));
        // Y $FF never reaches a line; no CHR page -> nothing.
        assert_eq!(margin_sprite_on_line(&l, 0, &msprite(0, 0xFF, 0, 0)), None);
        l.chr_page = [NO_PAGE; 2];
        assert_eq!(margin_sprite_on_line(&l, 100, &s), None);
    }

    /// Line 11 shows sprites (8x8, table 0 = page 2) over margins of tile 5
    /// (row 3: pattern 1 only in its first and last column).
    fn sprite_scene() -> (FrameRecord, Margins) {
        let mut rec = FrameRecord::new();
        let mut l = record_line(0);
        l.mask |= PPUMASK_SHOW_SPRITES | PPUMASK_SHOW_LEFT_SPRITES | PPUMASK_SHOW_LEFT_BG;
        l.chr_page = [2, 2];
        rec.lines[11] = l;
        let mut m = Margins::new(2);
        m.lines[11].fill = MarginFill::Tiles;
        m.lines[11].left = [id(5, 0); MARGIN_SLOTS];
        m.lines[11].right = [id(5, 0); MARGIN_SLOTS];
        (rec, m)
    }

    #[test]
    fn margin_sprites_paint_only_the_margins_with_sprite_rules() {
        let frame = pattern_frame();
        let chr = chr_image();
        let (rec, mut m) = sprite_scene();
        // Tile 6 row 3 is pattern 3 in every column. Sprite at y 7 covers
        // line 11 with its row 3.
        m.sprites = vec![
            msprite(-12, 7, 6, 0x01),  // left margin, 4 px into the window
            msprite(252, 7, 6, 0x02),  // straddles the right edge
            msprite(-13, 7, 6, 0x03),  // under the first: loses where they overlap
            msprite(-100, 7, 6, 0x00), // beyond a 16-px margin
        ];
        let mut out = WideFrame::new(0);
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        let at = |wx: i32| out.row(11)[(wx + 16) as usize];
        let spr = |pal: usize| 0x20 + 0x10 + pal * 4 + 3;
        for wx in -12..-4 {
            assert_eq!(at(wx), spr(1) as u8, "wx {wx}: first sprite wins");
        }
        assert_eq!(
            at(-13),
            spr(3) as u8,
            "second sprite where the first is absent"
        );
        assert_eq!(at(-14), 0x0F, "margin bg: tile 5 column 2 is transparent");
        for wx in 256..260 {
            assert_eq!(at(wx), spr(2) as u8, "right margin part of the edge sprite");
        }
        assert_eq!(
            &out.row(11)[16..16 + WIDTH],
            &frame[11 * WIDTH..12 * WIDTH],
            "centre verbatim"
        );
        // Other lines, sprite-less lines and backdrop lines get none.
        for (y, why) in [(10usize, "unrecorded"), (12, "unrecorded")] {
            assert!(out.row(y)[..16].iter().all(|&p| p == 0x0F), "{why}");
        }
        let mut off = rec.clone();
        off.lines[11].mask &= !PPUMASK_SHOW_SPRITES;
        render_wide_indexed(&frame, &off, &m, &chr, &mut out);
        assert_eq!(out.row(11)[(-10 + 16) as usize], 0x0F, "sprites disabled");
        let mut backdrop = m.clone();
        backdrop.lines[11].fill = MarginFill::Backdrop;
        render_wide_indexed(&frame, &rec, &backdrop, &chr, &mut out);
        assert_eq!(out.row(11)[(-10 + 16) as usize], 0x0F, "backdrop line");
    }

    #[test]
    fn behind_margin_sprite_loses_to_opaque_margin_background() {
        let frame = pattern_frame();
        let chr = chr_image();
        let (rec, mut m) = sprite_scene();
        // fine_x 0: slot -1 covers wx -8..0; tile 5 row 3 is opaque at its
        // first and last column (wx -8 and -1).
        m.sprites = vec![msprite(-8, 7, 6, 0x21)];
        let mut out = WideFrame::new(0);
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        let at = |wx: i32| out.row(11)[(wx + 16) as usize];
        let bg = 0x20 + 1; // background palette 0, pattern 1
        let spr = 0x20 + 0x10 + 4 + 3;
        assert_eq!(at(-8), bg);
        assert_eq!(at(-1), bg);
        for wx in -7..-1 {
            assert_eq!(at(wx), spr, "wx {wx}: transparent bg shows the sprite");
        }
    }

    #[test]
    fn left_sprite_fill_is_opt_in_and_never_covers_real_sprites() {
        let frame = Box::new([0x0Fu8; WIDTH * HEIGHT]);
        let chr = chr_image();
        let (mut rec, mut m) = sprite_scene();
        // A real sprite at x 0 whose row 3 (tile 5) is opaque at x 0 and 7.
        rec.lines[11].sprite_start = 0;
        rec.lines[11].sprite_len = 1;
        rec.sprites.push(SpriteRef {
            x: 0,
            page: 2,
            tile: 5,
            fine_row: 3,
            ..SpriteRef::default()
        });
        m.sprites = vec![msprite(-4, 7, 6, 0x01), msprite(3, 7, 6, 0x01)];
        let spr = 0x20 + 0x10 + 4 + 3;
        let mut out = WideFrame::new(0);
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        assert!(
            out.row(11)[16..16 + WIDTH].iter().all(|&p| p == 0x0F),
            "off: centre verbatim"
        );
        m.fill_left_sprites = true;
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        let at = |wx: i32| out.row(11)[(wx + 16) as usize];
        assert_eq!(at(-4), spr, "margin part");
        assert_eq!(at(0), 0x0F, "real sprite pixel keeps the frame");
        for wx in 1..4 {
            assert_eq!(at(wx), spr, "wx {wx}: in-window part of a left sprite");
        }
        for wx in 4..16 {
            assert_eq!(
                at(wx),
                0x0F,
                "wx {wx}: nothing past the sprite / window rows"
            );
        }
        // Left 8 sprite columns clipped and not repainted: no fill.
        rec.lines[11].mask &= !PPUMASK_SHOW_LEFT_SPRITES;
        render_wide_indexed(&frame, &rec, &m, &chr, &mut out);
        assert_eq!(out.row(11)[16 + 1], 0x0F);
    }
}
