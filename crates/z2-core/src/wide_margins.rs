//! Widescreen margin provider: decode the scenery just outside the NES
//! window from level data, anchored on the PPU render record.
//!
//! The NES only keeps the 256 visible pixels (plus a little prefetch) in its
//! nametables, and the columns beyond the window are streamed lazily (up to
//! ~8 frames stale in sideview). Margins are therefore decoded from the
//! game's own level data after each frame:
//!
//! * **Sideview** (mode `$0B`): level RAM in WRAM, `page * $D0 + row * 16 +
//!   col` (4 pages x 13 rows x 16 metatile columns). Each metatile code
//!   maps to four CHR tiles through the area bank's pointer table at
//!   `$8500` (bank `ram[$0769]`, read from the raw PRG image — bank 0 is
//!   mapped at `$8000` during play), column-major `[TL, BL, TR, BR]`; the
//!   palette group is `code >> 6`. Level row `r` covers nametable tile rows
//!   `4 + 2r` / `5 + 2r`; HUD rows 0-3 get backdrop margins.
//! * **Overworld** (mode `$05`): the RLE map in WRAM `$7C00`, one terrain
//!   metatile per 16x16 cell ([`overworld_map::TILE_MAPPINGS`] column-major,
//!   [`overworld_map::PALETTE_CODES`]). Out-of-map cells are water, or
//!   mountain left of column 0 in regions `$0706 < 2`.
//!
//! RAM is end-of-frame and can differ from the scroll the PPU actually used
//! by the frame's scroll delta, so the horizontal position comes from the
//! record's per-line ring coordinate and RAM only picks the ring instance
//! ([`resolve_ring`]). Everything else (title, lives screen, loaders, pause
//! pane, lines with background off) gets backdrop margins.
//!
//! Pure functions: they compile without the `interp` feature.

use z2_ppu::{BgTileId, FrameRecord, LineRecord, MarginFill, Margins, MARGIN_SLOTS};

use crate::overworld_map;

/// Game mode (`$0736`).
pub const ADDR_GAME_MODE: u16 = 0x0736;
/// Area PRG bank holding the `$8500` metatile tables.
pub const ADDR_AREA_PRG_BANK: u16 = 0x0769;
/// Ground type: 0 selects fill code `$42`, else `$40`.
pub const ADDR_GROUND_TYPE: u16 = 0x010C;
/// Pause / spell pane open when nonzero (nametable-only overlay).
pub const ADDR_MENU: u16 = 0x0524;
/// Sideview camera page.
pub const ADDR_SCROLL_HI: u16 = 0x072A;
/// Sideview camera pixel within the page.
pub const ADDR_SCROLL_LO: u16 = 0x072C;
/// Overworld Link tile row (`$1E`-based).
pub const ADDR_OW_TILE_Y: u16 = 0x0073;
/// Overworld Link tile column.
pub const ADDR_OW_TILE_X: u16 = 0x0074;
/// Pixels left in the current overworld step.
pub const ADDR_OW_PIXELS_LEFT: u16 = 0x007D;
/// Overworld facing (1 right, 2 left, 4 down, 8 up).
pub const ADDR_OW_FACING: u16 = 0x0562;
/// Overworld region index.
pub const ADDR_OW_REGION: u16 = 0x0706;
/// Overworld mode.
pub const MODE_OVERWORLD: u8 = 0x05;
/// Sideview play mode.
pub const MODE_SIDEVIEW: u8 = 0x0B;
/// Pointer table (4 LE words, one per palette group) in the area bank.
pub const METATILE_PTR_TABLE: u16 = 0x8500;
/// Level RAM bytes per page.
pub const LEVEL_PAGE_LEN: usize = 0xD0;
/// Metatile columns per level row.
pub const LEVEL_ROW_LEN: usize = 16;
/// Level rows per page.
pub const LEVEL_ROWS: usize = 13;
/// Level pages.
pub const LEVEL_PAGES: usize = 4;
/// Nametable tile row of level row 0.
pub const LEVEL_TOP_TILE_ROW: u8 = 4;
/// Sideview horizontal ring (two nametables side by side).
pub const SIDEVIEW_RING: i32 = 512;
/// Overworld horizontal ring (one 32-column nametable).
pub const OVERWORLD_RING: i32 = 256;
/// Overworld vertical ring in terrain rows (two stacked nametables).
pub const OVERWORLD_ROW_RING: i32 = 30;
/// Terrain nibble: mountain.
pub const OW_TERRAIN_MOUNTAIN: u8 = 0x0B;
/// Terrain nibble: water.
pub const OW_TERRAIN_WATER: u8 = 0x0C;
/// World tile columns covered by level RAM (4 pages x 32).
pub const SIDEVIEW_WORLD_TILES: i32 = 128;

const PRG_BANK_LEN: usize = 0x4000;

/// Byte at CPU `addr` with PRG `bank` mapped at `$8000-$BFFF` and the last
/// bank fixed at `$C000-$FFFF` (MMC1 PRG mode 3). Anything else, or a byte
/// outside the image, reads 0.
#[must_use]
pub fn prg_read(prg: &[u8], bank: u8, addr: u16) -> u8 {
    let off = match addr {
        0x8000..=0xBFFF => usize::from(bank) * PRG_BANK_LEN + usize::from(addr - 0x8000),
        0xC000..=0xFFFF => match prg.len().checked_sub(PRG_BANK_LEN) {
            Some(last) => last + usize::from(addr - 0xC000),
            None => return 0,
        },
        _ => return 0,
    };
    prg.get(off).copied().unwrap_or(0)
}

/// Little-endian word at `addr` (second byte at `addr + 1`, wrapping).
#[must_use]
pub fn prg_read16(prg: &[u8], bank: u8, addr: u16) -> u16 {
    u16::from(prg_read(prg, bank, addr))
        | (u16::from(prg_read(prg, bank, addr.wrapping_add(1))) << 8)
}

/// The value congruent to `ring_pos` modulo `ring` nearest to `estimate`
/// (ties resolve to the lower value).
#[must_use]
pub fn resolve_ring(ring_pos: i32, ring: i32, estimate: i32) -> i32 {
    let base = estimate - (estimate - ring_pos).rem_euclid(ring);
    let up = base + ring;
    if estimate - base <= up - estimate {
        base
    } else {
        up
    }
}

/// Sideview level decoder over WRAM level RAM and the area bank tables.
#[derive(Debug, Clone, Copy)]
pub struct SideviewSource<'a> {
    /// Full WRAM (`$6000-$7FFF`).
    pub wram: &'a [u8],
    /// Raw PRG image (16 KiB banks, last bank fixed).
    pub prg: &'a [u8],
    /// Area PRG bank (`$0769`).
    pub area_bank: u8,
    /// Metatile used outside level RAM (`$40`, or `$42` for ground type 0).
    pub fill_code: u8,
}

impl<'a> SideviewSource<'a> {
    /// Build from post-frame RAM: bank = `ram[$0769]`, fill code from
    /// `ram[$010C]`.
    #[must_use]
    pub fn from_state(ram: &[u8], wram: &'a [u8], prg: &'a [u8]) -> Self {
        let fill_code = if ram.get(usize::from(ADDR_GROUND_TYPE)).copied().unwrap_or(0) == 0 {
            0x42
        } else {
            0x40
        };
        Self {
            wram,
            prg,
            area_bank: ram
                .get(usize::from(ADDR_AREA_PRG_BANK))
                .copied()
                .unwrap_or(0),
            fill_code,
        }
    }

    /// WRAM offset of level page `page` (0-3): `page * $D0`.
    #[must_use]
    pub fn page_base(page: u8) -> usize {
        usize::from(page & 3) * LEVEL_PAGE_LEN
    }

    /// Metatile code at world tile column `wt` (8-px units; page `wt >> 5`,
    /// metatile column `(wt >> 1) & 15`) and level row `r` (0-12). Outside
    /// `0..128` columns or 13 rows: the fill code.
    #[must_use]
    pub fn metatile(&self, wt: i32, r: usize) -> u8 {
        if !(0..SIDEVIEW_WORLD_TILES).contains(&wt) || r >= LEVEL_ROWS {
            return self.fill_code;
        }
        let page = (wt >> 5) as u8;
        let col = ((wt >> 1) & 15) as usize;
        let off = Self::page_base(page) + r * LEVEL_ROW_LEN + col;
        self.wram.get(off).copied().unwrap_or(self.fill_code)
    }

    /// CHR tile and palette group of metatile `code` for `half` (0 left,
    /// 1 right) and `sub_row` (0 top, 1 bottom). Raw indexing as the ROM
    /// (no bounds clamp on the variable-length tables).
    #[must_use]
    pub fn chr_tile(&self, code: u8, half: u8, sub_row: u8) -> (u8, u8) {
        let group = code >> 6;
        let ptr = prg_read16(
            self.prg,
            self.area_bank,
            METATILE_PTR_TABLE + u16::from(group) * 2,
        );
        let entry = ptr.wrapping_add(u16::from(code & 0x3F) * 4);
        let idx = u16::from(half & 1) * 2 + u16::from(sub_row & 1);
        let tile = prg_read(self.prg, self.area_bank, entry.wrapping_add(idx));
        (tile, group)
    }

    /// Identity of world tile column `wt` on a line at nametable row
    /// `coarse_y` / pattern row `fine_y`, using CHR `page`. HUD rows
    /// (`coarse_y < 4`) and rows past the level return an unfetched id.
    #[must_use]
    pub fn tile_id(&self, wt: i32, coarse_y: u8, fine_y: u8, page: u8) -> BgTileId {
        if !(LEVEL_TOP_TILE_ROW..30).contains(&coarse_y) {
            return BgTileId::NONE;
        }
        let rel = coarse_y - LEVEL_TOP_TILE_ROW;
        let r = usize::from(rel >> 1);
        let code = self.metatile(wt, r);
        let (tile, pal) = self.chr_tile(code, (wt & 1) as u8, rel & 1);
        BgTileId {
            page,
            tile,
            pal,
            fine_y: fine_y & 7,
            nt: ((wt >> 5) & 1) as u8,
            coarse_x: (wt & 31) as u8,
            coarse_y,
            fetched: true,
        }
    }
}

/// Overworld decoder over the WRAM RLE map.
#[derive(Debug, Clone, Copy)]
pub struct OverworldSource<'a> {
    /// RLE blob (`wram[$7C00..$8000]`).
    pub blob: &'a [u8],
    /// Row start offsets into `blob`.
    pub rows: [u16; overworld_map::ROW_COUNT],
    /// Region index (`$0706`).
    pub region_index: u8,
}

impl<'a> OverworldSource<'a> {
    /// Build from post-frame RAM/WRAM.
    #[must_use]
    pub fn from_state(ram: &[u8], wram: &'a [u8]) -> Self {
        let lo = overworld_map::WRAM_RLE_BASE - 0x6000;
        let hi = (overworld_map::WRAM_RLE_END - 0x6000).min(wram.len());
        let blob = wram.get(lo..hi).unwrap_or(&[]);
        Self {
            blob,
            rows: overworld_map::build_row_offsets(blob),
            region_index: ram.get(usize::from(ADDR_OW_REGION)).copied().unwrap_or(0),
        }
    }

    /// Terrain nibble at terrain column `tx` and map row `r` (0-74).
    /// `tx < 0`: mountain in regions < 2, else water; `tx >= 64` or `r`
    /// outside the map: water.
    #[must_use]
    pub fn terrain(&self, tx: i32, r: i32) -> u8 {
        if !(0..overworld_map::ROW_COUNT as i32).contains(&r) {
            return OW_TERRAIN_WATER;
        }
        if tx < 0 {
            return if self.region_index < 2 {
                OW_TERRAIN_MOUNTAIN
            } else {
                OW_TERRAIN_WATER
            };
        }
        if tx >= overworld_map::MAP_W as i32 {
            return OW_TERRAIN_WATER;
        }
        let start = usize::from(self.rows[r as usize]);
        overworld_map::tile_at(self.blob, start, tx as u8, (r + 0x1E) as u8) as u8
    }

    /// Identity of world tile column `wt` (8-px units) in map row `r`,
    /// tile row `parity` (0 top, 1 bottom) of the metatile.
    #[must_use]
    pub fn tile_id(&self, wt: i32, r: i32, parity: u8, fine_y: u8, page: u8) -> BgTileId {
        let t = usize::from(self.terrain(wt.div_euclid(2), r) & 0x0F);
        let half = (wt & 1) as usize;
        BgTileId {
            page,
            tile: overworld_map::TILE_MAPPINGS[t][half * 2 + usize::from(parity & 1)],
            pal: overworld_map::PALETTE_CODES[t] & 3,
            fine_y: fine_y & 7,
            nt: 0,
            coarse_x: (wt & 31) as u8,
            coarse_y: 0,
            fetched: true,
        }
    }
}

/// How one line's margins are decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginPolicy {
    /// Solid backdrop.
    Backdrop,
    /// Sideview level RAM.
    Sideview,
    /// Overworld RLE map.
    Overworld,
}

/// Per-line policy: unrecorded / background-off lines, the pause pane
/// (`menu != 0` in sideview), sideview HUD rows (`coarse_y < 4`) and every
/// mode but `$0B`/`$05` get [`MarginPolicy::Backdrop`].
#[must_use]
pub fn margin_policy(mode: u8, menu: u8, line: &LineRecord) -> MarginPolicy {
    if !line.valid || !FrameRecord::show_bg(line) {
        return MarginPolicy::Backdrop;
    }
    let cy = FrameRecord::coarse_y(line);
    match mode {
        MODE_SIDEVIEW if menu == 0 && (LEVEL_TOP_TILE_ROW..30).contains(&cy) => {
            MarginPolicy::Sideview
        }
        MODE_OVERWORLD if cy < 30 => MarginPolicy::Overworld,
        _ => MarginPolicy::Backdrop,
    }
}

/// World pixel of window x = 0 on a sideview line (ring instance nearest
/// the RAM camera).
#[must_use]
pub fn sideview_world_left(ram: &[u8; 0x800], line: &LineRecord) -> i32 {
    let est = (i32::from(ram[usize::from(ADDR_SCROLL_HI)]) << 8)
        | i32::from(ram[usize::from(ADDR_SCROLL_LO)]);
    resolve_ring(i32::from(FrameRecord::ring_x(line)), SIDEVIEW_RING, est)
}

/// World pixel of window x = 0 on an overworld line (ring instance nearest
/// `16 * ($74 - 8)` adjusted by the in-progress horizontal step).
#[must_use]
pub fn overworld_world_left(ram: &[u8; 0x800], line: &LineRecord) -> i32 {
    let ring_x = i32::from((line.v_start & 0x1F) << 3) | i32::from(line.fine_x & 7);
    let pl = i32::from(ram[usize::from(ADDR_OW_PIXELS_LEFT)]);
    let est = 16 * (i32::from(ram[usize::from(ADDR_OW_TILE_X)]) - 8)
        + match ram[usize::from(ADDR_OW_FACING)] {
            1 => -pl,
            2 => pl,
            _ => 0,
        };
    resolve_ring(ring_x, OVERWORLD_RING, est)
}

/// Overworld map row (0-based RLE row, may be out of range) and metatile
/// tile-row parity of line `y`.
///
/// The row within the 30-row ring comes from the record (`nt_y * 30 +
/// coarse_y`, halved); RAM only picks the instance: the top visible row is
/// `$73 - 7` and line `y` lies about `y / 16` rows below it. The vertical
/// fine-scroll phase is ignored on purpose — [`resolve_ring`] absorbs up to
/// +/-14 rows of error.
#[must_use]
pub fn overworld_row(ram: &[u8; 0x800], line: &LineRecord, y: usize) -> (i32, u8) {
    let stack = i32::from(FrameRecord::nt_y(line)) * 30 + i32::from(FrameRecord::coarse_y(line));
    let ring_row = stack >> 1;
    let est = i32::from(ram[usize::from(ADDR_OW_TILE_Y)]) - 7 + (y as i32 >> 4);
    let r73 = resolve_ring(ring_row, OVERWORLD_ROW_RING, est);
    (r73 - 0x1E, (stack & 1) as u8)
}

/// Fill `out` for `tiles` per side from the frame `record` and the
/// post-frame RAM/WRAM/PRG. Pure; `tiles` is clamped by [`Margins`].
pub fn build_margins(
    ram: &[u8; 0x800],
    wram: &[u8; 0x2000],
    prg: &[u8],
    record: &FrameRecord,
    tiles: u8,
    out: &mut Margins,
) {
    out.set_tiles(tiles);
    let m = usize::from(out.tiles);
    let slots = (m + 1).min(MARGIN_SLOTS);
    let mode = ram[usize::from(ADDR_GAME_MODE)];
    let menu = ram[usize::from(ADDR_MENU)];
    let sv = SideviewSource::from_state(ram, wram, prg);
    // The RLE row table walk is only worth doing on overworld frames.
    let ow = (mode == MODE_OVERWORLD).then(|| OverworldSource::from_state(ram, wram));
    for (y, ml) in out.lines.iter_mut().enumerate() {
        let rec = record.line(y);
        let policy = margin_policy(mode, menu, rec);
        ml.fill = MarginFill::Backdrop;
        let page = FrameRecord::bg_page(rec);
        let fine_y = FrameRecord::fine_y(rec);
        match (policy, &ow) {
            (MarginPolicy::Sideview, _) => {
                let wt0 = sideview_world_left(ram, rec) >> 3;
                let cy = FrameRecord::coarse_y(rec);
                for j in 0..slots {
                    let ji = j as i32;
                    ml.left[j] = sv.tile_id(wt0 - 1 - ji, cy, fine_y, page);
                    ml.right[j] = sv.tile_id(wt0 + 32 + ji, cy, fine_y, page);
                }
                ml.fill = MarginFill::Tiles;
            }
            (MarginPolicy::Overworld, Some(src)) => {
                let wt0 = overworld_world_left(ram, rec) >> 3;
                let (r, parity) = overworld_row(ram, rec, y);
                for j in 0..slots {
                    let ji = j as i32;
                    ml.left[j] = src.tile_id(wt0 - 1 - ji, r, parity, fine_y, page);
                    ml.right[j] = src.tile_id(wt0 + 32 + ji, r, parity, fine_y, page);
                }
                ml.fill = MarginFill::Tiles;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z2_ppu::PPUMASK_SHOW_BG;

    #[test]
    fn prg_window_and_fixed_bank() {
        let mut prg = vec![0u8; 8 * PRG_BANK_LEN];
        prg[3 * PRG_BANK_LEN + 0x500] = 0x34;
        prg[3 * PRG_BANK_LEN + 0x501] = 0x12;
        prg[7 * PRG_BANK_LEN + 0x2AE0] = 0x77;
        assert_eq!(prg_read(&prg, 3, 0x8500), 0x34);
        assert_eq!(prg_read16(&prg, 3, 0x8500), 0x1234);
        assert_eq!(prg_read(&prg, 3, 0xEAE0), 0x77);
        assert_eq!(prg_read(&prg, 0, 0xEAE0), 0x77, "fixed bank ignores bank");
        assert_eq!(prg_read(&prg, 3, 0x6000), 0);
        assert_eq!(prg_read(&prg, 200, 0x8000), 0, "outside image");
        assert_eq!(prg_read(&[], 0, 0xC000), 0);
    }

    #[test]
    fn policy_table() {
        let mut l = LineRecord::EMPTY;
        l.valid = true;
        l.mask = PPUMASK_SHOW_BG;
        let at = |l: &LineRecord, cy: u16| {
            let mut l = *l;
            l.v_start = cy << 5;
            l
        };
        assert_eq!(margin_policy(0x0B, 0, &at(&l, 4)), MarginPolicy::Sideview);
        assert_eq!(margin_policy(0x0B, 0, &at(&l, 29)), MarginPolicy::Sideview);
        assert_eq!(margin_policy(0x0B, 0, &at(&l, 3)), MarginPolicy::Backdrop);
        assert_eq!(margin_policy(0x0B, 1, &at(&l, 10)), MarginPolicy::Backdrop);
        assert_eq!(margin_policy(0x05, 0, &at(&l, 0)), MarginPolicy::Overworld);
        assert_eq!(margin_policy(0x05, 3, &at(&l, 0)), MarginPolicy::Overworld);
        assert_eq!(margin_policy(0x05, 0, &at(&l, 30)), MarginPolicy::Backdrop);
        for mode in [0x00, 0x01, 0x09, 0x10, 0x11, 0x13] {
            assert_eq!(margin_policy(mode, 0, &at(&l, 10)), MarginPolicy::Backdrop);
        }
        let mut off = at(&l, 10);
        off.mask = 0;
        assert_eq!(margin_policy(0x0B, 0, &off), MarginPolicy::Backdrop);
        let mut inval = at(&l, 10);
        inval.valid = false;
        assert_eq!(margin_policy(0x05, 0, &inval), MarginPolicy::Backdrop);
    }
}
