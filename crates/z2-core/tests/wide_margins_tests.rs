//! ROM-free tests for the widescreen margin provider (`wide_margins`):
//! synthetic PRG / WRAM / render records only.

use z2_core::overworld_map::{PALETTE_CODES, TILE_MAPPINGS};
use z2_core::wide_margins::*;
use z2_ppu::{
    BgTileId, FrameRecord, LineRecord, MarginFill, Margins, PPUCTRL_BG_TABLE, PPUMASK_SHOW_BG,
};

const BANK: usize = 0x4000;

#[test]
fn resolve_ring_cases() {
    assert_eq!(resolve_ring(190, 512, 700), 702);
    assert_eq!(resolve_ring(510, 512, 5), -2);
    assert_eq!(resolve_ring(14, 30, 44), 44);
    assert_eq!(resolve_ring(0, 30, 29), 30);
    assert_eq!(resolve_ring(0, 512, 256), 0, "tie -> lower");
    assert_eq!(resolve_ring(1, 512, 256), 1);
    // Around the 511/0 wrap of the ring with the camera near a page edge.
    assert_eq!(resolve_ring(507, 512, 512), 507);
    assert_eq!(resolve_ring(3, 512, 510), 515);
    assert_eq!(resolve_ring(507, 512, 0), -5);
    // RAM a frame's scroll delta ahead/behind still lands on the same instance.
    for delta in -40..=40 {
        assert_eq!(resolve_ring(700 % 512, 512, 700 + delta), 700);
    }
}

/// PRG with area bank 3 tables: groups 0/1 -> `$8600`, 2 -> `$8700`,
/// 3 -> `$8800`; each entry `i` of group `g` is `[g0|i, g1|i, g2|i, g3|i]`
/// shifted into distinct values so every lookup is unambiguous.
fn synth_prg() -> Vec<u8> {
    let mut prg = vec![0u8; 8 * BANK];
    let b3 = 3 * BANK;
    for (g, ptr) in [0x8600u16, 0x8600, 0x8700, 0x8800].into_iter().enumerate() {
        prg[b3 + 0x500 + g * 2] = ptr as u8;
        prg[b3 + 0x500 + g * 2 + 1] = (ptr >> 8) as u8;
    }
    for (base, grp) in [(0x600usize, 1u8), (0x700, 2), (0x800, 3)] {
        for i in 0..16usize {
            for q in 0..4usize {
                prg[b3 + base + i * 4 + q] = (grp << 6) | ((i as u8) << 2) | q as u8;
            }
        }
    }
    prg
}

/// Expected CHR tile for `code` quadrant `q` (0 TL, 1 BL, 2 TR, 3 BR) under
/// [`synth_prg`] (groups 0 and 1 share the `$8600` table).
fn quad(code: u8, q: u8) -> u8 {
    let grp = match code >> 6 {
        0 | 1 => 1,
        g => g,
    };
    (grp << 6) | ((code & 0x3F) << 2) | q
}

fn synth_ram(bank: u8, ground: u8) -> Box<[u8; 0x800]> {
    let mut ram = Box::new([0u8; 0x800]);
    ram[usize::from(ADDR_AREA_PRG_BANK)] = bank;
    ram[usize::from(ADDR_GROUND_TYPE)] = ground;
    ram
}

#[test]
fn sideview_source_decodes_synthetic_level_ram() {
    let prg = synth_prg();
    let mut wram = Box::new([0x40u8; 0x2000]);
    // Page 1, row 5, column 7 = $81 (group 2, index 1).
    wram[0xD0 + 5 * 16 + 7] = 0x81;
    // Page 3, row 12, column 0 = $C3 (group 3, index 3).
    wram[3 * 0xD0 + 12 * 16] = 0xC3;
    let ram = synth_ram(3, 1);
    let src = SideviewSource::from_state(&ram[..], &wram[..], &prg);
    assert_eq!((src.area_bank, src.fill_code), (3, 0x40));
    assert_eq!(SideviewSource::page_base(2), 0x1A0);
    assert_eq!(src.metatile(32 + 14, 5), 0x81);
    assert_eq!(src.metatile(32 + 15, 5), 0x81);
    assert_eq!(src.chr_tile(0x81, 0, 0), (quad(0x81, 0), 2));

    let wt = 32 + 14;
    let top = 4 + 10;
    let id = src.tile_id(wt, top, 6, 9);
    assert_eq!(
        id,
        BgTileId {
            page: 9,
            tile: quad(0x81, 0),
            pal: 2,
            fine_y: 6,
            nt: 1,
            coarse_x: 14,
            coarse_y: top,
            fetched: true,
        }
    );
    assert_eq!(src.tile_id(wt + 1, top, 0, 9).tile, quad(0x81, 2), "TR");
    assert_eq!(src.tile_id(wt, top + 1, 0, 9).tile, quad(0x81, 1), "BL");
    assert_eq!(src.tile_id(wt + 1, top + 1, 0, 9).tile, quad(0x81, 3), "BR");
    // Palette group follows the code (group 3 on the last level row).
    let last = src.tile_id(96, 4 + 24, 0, 9);
    assert_eq!((last.tile, last.pal, last.nt), (quad(0xC3, 0), 3, 1));
    // Outside level RAM: fill code $40 (group 1 entry 0).
    for out in [-1, -32, 128, 400] {
        let f = src.tile_id(out, top, 0, 9);
        assert!(f.fetched);
        assert_eq!((f.pal, f.tile >> 2), (1, (1 << 4)), "wt {out}");
    }
    // Ground type 0 -> fill $42.
    let ram0 = synth_ram(3, 0);
    let src0 = SideviewSource::from_state(&ram0[..], &wram[..], &prg);
    assert_eq!(
        src0.tile_id(-1, top, 0, 9).tile,
        quad(0x42, 2),
        "right half of $42"
    );
    // HUD rows and past the level: nothing.
    assert!(!src.tile_id(wt, 3, 0, 9).fetched);
    assert!(!src.tile_id(wt, 30, 0, 9).fetched);
}

/// 75 rows of 64 columns: row `r` is four 16-wide runs of
/// `[forest, grass, desert, swamp]` rotated by `r % 4`.
fn synth_blob() -> Vec<u8> {
    let order = [0x6u8, 0x5, 0x4, 0x7];
    let mut blob = Vec::new();
    for r in 0..75 {
        for i in 0..4 {
            blob.push(0xF0 | order[(i + r) % 4]);
        }
    }
    blob
}

fn ow_wram() -> Box<[u8; 0x2000]> {
    let mut wram = Box::new([0u8; 0x2000]);
    let blob = synth_blob();
    wram[0x1C00..0x1C00 + blob.len()].copy_from_slice(&blob);
    wram
}

#[test]
fn overworld_source_decodes_synthetic_rle_with_boundaries() {
    let wram = ow_wram();
    let mut ram = Box::new([0u8; 0x800]);
    let src = OverworldSource::from_state(&ram[..], &wram[..]);
    assert_eq!(src.terrain(0, 0), 0x6);
    assert_eq!(src.terrain(16, 0), 0x5);
    assert_eq!(src.terrain(63, 0), 0x7);
    assert_eq!(src.terrain(0, 1), 0x5, "row 1 rotated");
    assert_eq!(src.terrain(40, 74), order_at(74, 40));
    assert_eq!(
        src.terrain(-1, 5),
        OW_TERRAIN_MOUNTAIN,
        "west of map, region 0"
    );
    assert_eq!(src.terrain(64, 5), OW_TERRAIN_WATER);
    assert_eq!(src.terrain(10, -1), OW_TERRAIN_WATER);
    assert_eq!(src.terrain(10, 75), OW_TERRAIN_WATER);
    ram[usize::from(ADDR_OW_REGION)] = 2;
    let east = OverworldSource::from_state(&ram[..], &wram[..]);
    assert_eq!(
        east.terrain(-1, 5),
        OW_TERRAIN_WATER,
        "west of map, region 2"
    );

    // Terrain column 16 = world tiles 32/33; column-major quadrants.
    for (wt, parity, q) in [(32, 0u8, 0usize), (32, 1, 1), (33, 0, 2), (33, 1, 3)] {
        let id = src.tile_id(wt, 0, parity, 3, 17);
        assert_eq!(id.tile, TILE_MAPPINGS[5][q], "wt {wt} parity {parity}");
        assert_eq!((id.pal, id.page, id.fine_y), (PALETTE_CODES[5] & 3, 17, 3));
        assert!(id.fetched);
    }
    let west = src.tile_id(-1, 0, 0, 0, 17);
    assert_eq!(
        west.tile, TILE_MAPPINGS[0xB][2],
        "tile -1 is the right half of column -1"
    );
}

fn order_at(r: usize, tx: usize) -> u8 {
    [0x6u8, 0x5, 0x4, 0x7][(tx / 16 + r) % 4]
}

fn line(v_start: u16, fine_x: u8) -> LineRecord {
    let mut l = LineRecord::EMPTY;
    l.valid = true;
    l.mask = PPUMASK_SHOW_BG;
    l.ctrl = PPUCTRL_BG_TABLE;
    l.chr_page = [5, 6];
    l.v_start = v_start;
    l.fine_x = fine_x;
    l
}

#[test]
fn build_margins_sideview_end_to_end_across_ring_wrap() {
    let prg = synth_prg();
    let mut wram = Box::new([0x40u8; 0x2000]);
    // World tile 95 = page 2, metatile column 15 (right half), level row 4.
    wram[2 * 0xD0 + 4 * 16 + 15] = 0x81;
    // World tile 62 = page 1, column 15 (left half), row 4.
    wram[0xD0 + 4 * 16 + 15] = 0xC2;
    let mut ram = synth_ram(3, 1);
    ram[usize::from(ADDR_GAME_MODE)] = MODE_SIDEVIEW;
    ram[usize::from(ADDR_SCROLL_HI)] = 2;
    ram[usize::from(ADDR_SCROLL_LO)] = 0;

    let mut rec = FrameRecord::new();
    // Line 100: nt_x 1, coarse_x 31, coarse_y 12, fine_y 4, fine_x 3 -> ring 507.
    rec.lines[100] = line((4 << 12) | (1 << 10) | (12 << 5) | 31, 3);
    // Line 10: HUD row.
    rec.lines[10] = line(1 << 5, 0);
    let mut m = Margins::new(0);
    build_margins(&ram, &wram, &prg, &rec, 11, &mut m);
    assert_eq!(m.tiles, 11);
    assert_eq!(m.lines[10].fill, MarginFill::Backdrop);
    let ml = &m.lines[100];
    assert_eq!(ml.fill, MarginFill::Tiles);
    // world_left = 507 (RAM 512 is 5 px ahead) -> slot 0 = tile 63.
    assert_eq!(ml.right[0].tile, quad(0x81, 2));
    assert_eq!(
        (ml.right[0].pal, ml.right[0].page, ml.right[0].fine_y),
        (2, 6, 4)
    );
    assert_eq!(ml.right[0].coarse_x, 31);
    assert_eq!(ml.left[0].tile, quad(0xC2, 0));
    assert_eq!(ml.left[0].pal, 3);
    assert!(ml.left[11].fetched && ml.right[11].fetched, "M+1 slots");

    // Same line, camera at page 0 pixel 0: ring 507 resolves to -5.
    ram[usize::from(ADDR_SCROLL_HI)] = 0;
    build_margins(&ram, &wram, &prg, &rec, 4, &mut m);
    let ml = &m.lines[100];
    // Slot 0 = tile -1 -> left[0] = tile -2 (fill $40, left half),
    // right[0] = tile 31 (page 0 column 15 right half = $40 too).
    assert_eq!(ml.left[0].tile, quad(0x40, 0));
    assert_eq!(ml.right[0].tile, quad(0x40, 2));
    assert_eq!(ml.right[1].coarse_x, 0);
    assert_eq!(ml.right[1].nt, 1);

    // Pause pane open -> backdrop; other modes -> backdrop.
    ram[usize::from(ADDR_MENU)] = 1;
    build_margins(&ram, &wram, &prg, &rec, 4, &mut m);
    assert_eq!(m.lines[100].fill, MarginFill::Backdrop);
    ram[usize::from(ADDR_MENU)] = 0;
    ram[usize::from(ADDR_GAME_MODE)] = 0x11;
    build_margins(&ram, &wram, &prg, &rec, 4, &mut m);
    assert_eq!(m.lines[100].fill, MarginFill::Backdrop);
}

#[test]
fn build_margins_overworld_end_to_end() {
    let wram = ow_wram();
    let prg = vec![0u8; 8 * BANK];
    let mut ram = Box::new([0u8; 0x800]);
    ram[usize::from(ADDR_GAME_MODE)] = MODE_OVERWORLD;
    ram[usize::from(ADDR_OW_TILE_X)] = 29;
    ram[usize::from(ADDR_OW_PIXELS_LEFT)] = 6;
    ram[usize::from(ADDR_OW_FACING)] = 1;
    ram[usize::from(ADDR_OW_TILE_Y)] = 52;
    let mut rec = FrameRecord::new();
    // World left 330 = ring 74 = coarse 9 fine 2; NT-Y 1, coarse_y 0.
    rec.lines[0] = line((1 << 11) | 9, 2);
    // Line 17: coarse_y 3 of NT-Y 1 -> stack row 33 -> ring row 16, parity 1.
    rec.lines[17] = line((1 << 11) | (3 << 5) | (1 << 12) | 9, 2);
    let mut m = Margins::new(8);
    build_margins(&ram, &wram, &prg, &rec, 8, &mut m);
    let ml = &m.lines[0];
    assert_eq!(ml.fill, MarginFill::Tiles);
    // $73 = 52 -> top row 45 -> map row 15; wt0 = 41.
    // right[0] = wt 73 -> tx 36 (right half), left[0] = wt 40 -> tx 20.
    assert_eq!(
        ml.right[0].tile,
        TILE_MAPPINGS[usize::from(order_at(15, 36))][2]
    );
    assert_eq!(
        ml.left[0].tile,
        TILE_MAPPINGS[usize::from(order_at(15, 20))][0]
    );
    assert_eq!(
        ml.left[0].pal,
        PALETTE_CODES[usize::from(order_at(15, 20))] & 3
    );
    let ml = &m.lines[17];
    assert_eq!(
        ml.right[0].tile,
        TILE_MAPPINGS[usize::from(order_at(16, 36))][3]
    );
    assert_eq!(ml.right[0].fine_y, 1);
    // Unrecorded lines are backdrop.
    assert_eq!(m.lines[1].fill, MarginFill::Backdrop);
}
