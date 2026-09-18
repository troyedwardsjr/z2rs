//! Per-frame render record (opt-in): what the scanline-timed renderer
//! actually fetched and chose for every visible line.
//!
//! The record is produced *beside* the indexed frame by [`crate::Ppu`] when
//! [`crate::Ppu::set_record`] is on (default off). It never feeds back into
//! rendering: pixels, sprite-0 hit timing and the overflow flag are identical
//! with the record on or off. Consumers are the widescreen compositor
//! ([`crate::wide`]) and HD graphics packs, which need tile/sprite identity
//! rather than palette indices.
//!
//! ## How a line is captured
//!
//! A line is recorded when the renderer *commits* it (the beam passed dot
//! 256, or [`crate::Ppu::finish_frame`] completed the frame). At that point:
//!
//! * [`LineRecord::tiles`] is a copy of the 34 background fetches of the line
//!   in fetch order, each carrying the CHR page, nametable and `v` bits that
//!   were live *when that tile was fetched*;
//! * [`LineRecord::v_start`] / [`LineRecord::fine_x`] describe the line
//!   start (before the tile-0 prefetch);
//! * `ctrl`, `mask`, `chr_page`, `palette`, `backdrop` and
//!   [`LineRecord::fine_x_end`] are snapshotted at commit, which is the state
//!   the sprite pass of that line used.
//!
//! ## Mid-line splits
//!
//! The catch-up renderer can split a line: a `$2000`/`$2005`/`$2006` write, a
//! nametable write or a CHR/mirroring change lands after some tiles were
//! fetched ([`crate::AccessKind::Fetch`]), or a `$2001`/palette write lands
//! after some pixels were emitted ([`crate::AccessKind::Pixel`]). The record
//! keeps the tiles exactly as rendered — tiles `0..k` from the old state and
//! the rest from the new — and raises [`LineRecord::split`] (fetch-state
//! change) or [`LineRecord::pixel_split`] (pixel-state change). On a split
//! line, window pixel `x` maps to tile `(x + fine_x) >> 3` only up to the
//! change; consumers wanting exactness should treat split lines
//! conservatively (Zelda II's only such lines are the HUD split line and
//! title/intro `$2006` lines).

use crate::state::{BG_FETCHES_PER_LINE, PPUCTRL_BG_TABLE, PPUMASK_SHOW_BG};
use crate::HEIGHT;

/// Background tile fetches recorded per line (2 prefetched + 32 on-line).
pub const RECORD_TILES_PER_LINE: usize = BG_FETCHES_PER_LINE as usize;

/// `BgTileId::page` / `LineRecord::chr_page` value meaning "no CHR page".
pub const NO_PAGE: u8 = u8::MAX;

/// Identity of one fetched background tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BgTileId {
    /// CHR asset page (0-31) backing the background pattern table at fetch
    /// time; [`NO_PAGE`] when unset.
    pub page: u8,
    /// Tile index 0-255 (nametable byte).
    pub tile: u8,
    /// Palette group 0-3 (attribute quadrant).
    pub pal: u8,
    /// Pattern row 0-7 (`v` bits 12-14 at fetch).
    pub fine_y: u8,
    /// Logical nametable 0-3 (`v` bits 10-11 at fetch).
    pub nt: u8,
    /// Coarse X 0-31 (`v` bits 0-4 at fetch).
    pub coarse_x: u8,
    /// Coarse Y 0-31 (`v` bits 5-9 at fetch).
    pub coarse_y: u8,
    /// False when rendering was disabled at fetch time (blank tile; the
    /// other fields are meaningless).
    pub fetched: bool,
}

impl BgTileId {
    /// The "nothing fetched" identity (also [`Default`]).
    pub const NONE: BgTileId = BgTileId {
        page: NO_PAGE,
        tile: 0,
        pal: 0,
        fine_y: 0,
        nt: 0,
        coarse_x: 0,
        coarse_y: 0,
        fetched: false,
    };
}

impl Default for BgTileId {
    fn default() -> Self {
        Self::NONE
    }
}

/// One sprite the line's evaluation chose (after the 8-sprite limit), in
/// OAM order (= priority order, first wins).
///
/// Every chosen sprite is recorded whether or not any of its pixels are
/// opaque or on screen, so HD art can replace fully transparent sprites too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpriteRef {
    /// OAM slot 0-63.
    pub oam_index: u8,
    /// Left edge x (OAM byte 3; 8-bit, so sprites never reach the margins).
    pub x: u8,
    /// CHR asset page backing the pattern table this sprite row reads.
    pub page: u8,
    /// Resolved pattern tile (8x16: top `tile & $FE` or bottom `+1` half
    /// already selected, V-flip included).
    pub tile: u8,
    /// Pattern row 0-7 within `tile`, V-flip already applied.
    pub fine_row: u8,
    /// Row within the whole sprite (0-7 or 0-15) before flipping.
    pub row_in_sprite: u8,
    /// Sprite palette 0-3 (`$3F11 + pal * 4`).
    pub pal: u8,
    /// Horizontal flip (OAM attr bit 6).
    pub flip_h: bool,
    /// Vertical flip (OAM attr bit 7; `fine_row`/`tile` already account for it).
    pub flip_v: bool,
    /// Behind background (OAM attr bit 5).
    pub behind: bool,
    /// Drawn in 8x16 mode (`PPUCTRL` bit 5).
    pub tall: bool,
}

/// Everything recorded for one visible scanline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRecord {
    /// The line was recorded this frame (false for lines rendered before the
    /// record was enabled, or not yet rendered).
    pub valid: bool,
    /// `v` at the line's first tile fetch (before the tile-0 prefetch).
    pub v_start: u16,
    /// Fine X at pixel 0.
    pub fine_x: u8,
    /// Fine X when the line committed (differs on a mid-line `$2005` write).
    pub fine_x_end: u8,
    /// `$2000` at line commit.
    pub ctrl: u8,
    /// `$2001` at line commit.
    pub mask: u8,
    /// CHR asset pages of pattern slots 0/1 at line commit.
    pub chr_page: [u8; 2],
    /// Palette RAM snapshot at commit (raw 32 bytes; the four backdrop
    /// cells are kept equal in both halves, see [`LineRecord::palette_entry`]).
    pub palette: [u8; 32],
    /// Index used for transparent background pixels at commit (backdrop or
    /// the rendering-off palette hijack, greyscale applied).
    pub backdrop: u8,
    /// A fetch-state change (`$2000`/`$2005`/`$2006`, nametable write,
    /// CHR/mirroring change) landed after the line started fetching.
    pub split: bool,
    /// A `$2001` or palette write landed after pixels were already emitted.
    pub pixel_split: bool,
    /// The 34 fetched tiles in fetch order; tile `k` covers window pixels
    /// `[8k - fine_x, 8k - fine_x + 8)`.
    pub tiles: [BgTileId; RECORD_TILES_PER_LINE],
    /// Start of this line's sprites in [`FrameRecord::sprites`].
    pub sprite_start: u16,
    /// Number of chosen sprites on this line.
    pub sprite_len: u8,
    /// More than 8 sprites covered the line (Faithful8 dropout happened).
    pub sprite_overflow: bool,
}

impl LineRecord {
    /// An unrecorded line (backdrop `$0F`, no pages, nothing fetched).
    pub const EMPTY: LineRecord = LineRecord {
        valid: false,
        v_start: 0,
        fine_x: 0,
        fine_x_end: 0,
        ctrl: 0,
        mask: 0,
        chr_page: [NO_PAGE; 2],
        palette: [0; 32],
        backdrop: 0x0F,
        split: false,
        pixel_split: false,
        tiles: [BgTileId::NONE; RECORD_TILES_PER_LINE],
        sprite_start: 0,
        sprite_len: 0,
        sprite_overflow: false,
    };

    /// Palette entry 0-31 with the `$3F10/14/18/1C -> $3F00/04/08/0C` fold
    /// (same rule as [`crate::Ppu::palette_entry`]).
    #[must_use]
    pub fn palette_entry(&self, i: usize) -> u8 {
        let mut idx = i & 0x1F;
        if idx & 0x13 == 0x10 {
            idx -= 0x10;
        }
        self.palette[idx]
    }
}

impl Default for LineRecord {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Record of one rendered frame. The copy returned by
/// [`crate::Ppu::frame_record`] is complete and matches the frame the last
/// [`crate::Ppu::finish_frame`] returned.
#[derive(Debug, Clone)]
pub struct FrameRecord {
    /// One entry per visible scanline.
    pub lines: Box<[LineRecord; HEIGHT]>,
    /// Chosen sprites of all lines, concatenated in line order.
    pub sprites: Vec<SpriteRef>,
    /// Lines recorded this frame (240 for a frame recorded throughout).
    pub lines_done: u16,
}

impl Default for FrameRecord {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameRecord {
    /// Empty record (lines built on the heap; sprites pre-reserved).
    #[must_use]
    pub fn new() -> Self {
        let lines: Box<[LineRecord]> = vec![LineRecord::EMPTY; HEIGHT].into_boxed_slice();
        let lines: Box<[LineRecord; HEIGHT]> = match lines.try_into() {
            Ok(b) => b,
            Err(_) => unreachable!("vec has HEIGHT lines"),
        };
        Self {
            lines,
            sprites: Vec::with_capacity(512),
            lines_done: 0,
        }
    }

    /// Forget the recorded frame, keeping allocations.
    pub fn clear(&mut self) {
        for l in self.lines.iter_mut() {
            l.valid = false;
        }
        self.sprites.clear();
        self.lines_done = 0;
    }

    /// Line `y` (panics when `y >= 240`).
    #[must_use]
    pub fn line(&self, y: usize) -> &LineRecord {
        &self.lines[y]
    }

    /// Sprites chosen for line `y` (empty for unrecorded lines).
    #[must_use]
    pub fn sprites_on(&self, y: usize) -> &[SpriteRef] {
        let l = &self.lines[y];
        if !l.valid {
            return &[];
        }
        let start = usize::from(l.sprite_start);
        let end = (start + usize::from(l.sprite_len)).min(self.sprites.len());
        &self.sprites[start.min(end)..end]
    }

    /// CHR page backing the background pattern table of `line`.
    #[must_use]
    pub fn bg_page(line: &LineRecord) -> u8 {
        line.chr_page[usize::from(line.ctrl & PPUCTRL_BG_TABLE != 0)]
    }

    /// Background enabled on `line` (`PPUMASK` bit 3).
    #[must_use]
    pub fn show_bg(line: &LineRecord) -> bool {
        line.mask & PPUMASK_SHOW_BG != 0
    }

    /// Horizontal position of pixel 0 on the 512-px two-nametable ring:
    /// `(nt_x << 8) | (coarse_x << 3) | fine_x` (0-511).
    #[must_use]
    pub fn ring_x(line: &LineRecord) -> u16 {
        (((line.v_start >> 10) & 1) << 8) | ((line.v_start & 0x1F) << 3) | u16::from(line.fine_x)
    }

    /// Coarse Y (tile row) at line start.
    #[must_use]
    pub fn coarse_y(line: &LineRecord) -> u8 {
        ((line.v_start >> 5) & 0x1F) as u8
    }

    /// Vertical nametable bit at line start.
    #[must_use]
    pub fn nt_y(line: &LineRecord) -> u8 {
        ((line.v_start >> 11) & 1) as u8
    }

    /// Fine Y (pattern row) at line start.
    #[must_use]
    pub fn fine_y(line: &LineRecord) -> u8 {
        ((line.v_start >> 12) & 7) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_helpers_decode_v_start() {
        let mut l = LineRecord::EMPTY;
        // fine_y 5, nt 3 (nt_y 1, nt_x 1), coarse_y 17, coarse_x 9.
        l.v_start = (5 << 12) | (3 << 10) | (17 << 5) | 9;
        l.fine_x = 6;
        assert_eq!(FrameRecord::ring_x(&l), 256 + 9 * 8 + 6);
        assert_eq!(FrameRecord::coarse_y(&l), 17);
        assert_eq!(FrameRecord::nt_y(&l), 1);
        assert_eq!(FrameRecord::fine_y(&l), 5);
        l.chr_page = [4, 9];
        l.ctrl = PPUCTRL_BG_TABLE;
        assert_eq!(FrameRecord::bg_page(&l), 9);
        l.ctrl = 0;
        assert_eq!(FrameRecord::bg_page(&l), 4);
        assert!(!FrameRecord::show_bg(&l));
        l.mask = PPUMASK_SHOW_BG;
        assert!(FrameRecord::show_bg(&l));
    }

    #[test]
    fn palette_entry_folds_sprite_backdrop_mirrors() {
        let mut l = LineRecord::EMPTY;
        l.palette[0x04] = 0x21;
        l.palette[0x14] = 0x3F; // raw cell ignored through the fold
        l.palette[0x15] = 0x16;
        assert_eq!(l.palette_entry(0x14), 0x21);
        assert_eq!(l.palette_entry(0x15), 0x16);
    }

    #[test]
    fn clear_keeps_capacity_and_invalidates() {
        let mut r = FrameRecord::new();
        r.lines[3].valid = true;
        r.lines[3].sprite_len = 1;
        r.sprites.push(SpriteRef::default());
        r.lines_done = 4;
        let cap = r.sprites.capacity();
        assert_eq!(r.sprites_on(3).len(), 1);
        r.clear();
        assert_eq!(r.sprites.capacity(), cap);
        assert_eq!(r.lines_done, 0);
        assert!(r.sprites_on(3).is_empty());
    }
}
