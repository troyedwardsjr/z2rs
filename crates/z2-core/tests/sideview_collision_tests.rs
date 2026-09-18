//! ROM-free sideview collision-matrix tests + gated snapshots.

#[allow(dead_code)]
#[path = "../src/sideview_collision.rs"]
mod sideview_collision;

mod common;

use sideview_collision::*;

#[test]
fn collision_matrix_matches_e079() {
    // (tile, link_y, moving_down, grounded, down_held) → (attr, fx, blocks).
    let cases: [(u8, u8, TileAttr, FootFx); 7] = [
        (0x40, 0x50, TileAttr::Open, FootFx::None),
        (0x00, 0x50, TileAttr::JumpThrough, FootFx::None),
        (0x80, 0x10, TileAttr::Lava, FootFx::LavaHurt),
        (0x87, 0xA5, TileAttr::Water, FootFx::DeepWater),
        (0x87, 0xA4, TileAttr::Water, FootFx::None),
        (0x91, 0xFF, TileAttr::Water, FootFx::DeepWater),
        (0x61, 0x50, TileAttr::Breakable, FootFx::None),
    ];
    for (tile, y, attr, fx) in cases {
        assert_eq!(tile_attr(tile), attr, "tile {tile:02X}");
        assert_eq!(foot_fx(tile, y), fx, "tile {tile:02X} y {y:02X}");
    }
    // Jump-through blocks unless sinking (down + grounded + down-held).
    assert!(!jump_blocks(true, true, true));
    assert!(jump_blocks(true, true, false));
    assert!(jump_blocks(false, true, true));
    assert!(jump_blocks(true, false, true));
}

#[test]
fn probe_or_matches_table21() {
    assert_eq!(TABLE21[0], 0x01);
    assert_eq!(TABLE21[4], 0x04);
    assert_eq!(TABLE21[6], 0x08);
    assert_eq!(probe_or(0x00, 0, true), 0x01);
    assert_eq!(probe_or(0x01, 4, true), 0x05);
    assert_eq!(probe_or(0xFF, 0, false), 0xFF);
}

#[test]
fn stab_break_needs_glove_and_anim() {
    assert!(stab_breaks(0x61, 0x08, true));
    assert!(stab_breaks(0x61, 0x09, true));
    assert!(!stab_breaks(0x61, 0x08, false));
    assert!(!stab_breaks(0x61, 0x05, true));
    assert!(!stab_breaks(0x60, 0x08, true));
    assert_eq!(TILE_SHATTERED, 0x8F);
    assert_eq!(WATER_LINE_Y, 0xA5);
}

#[test]
fn corpus_sideview_collision_snapshots_skip_without_corpus() {
    let Some(rd) = common::corpus_snapshots("corpus_sideview_collision_snapshots") else {
        return;
    };
    eprintln!(
        "corpus snapshots present: {} entries",
        rd.filter_map(Result::ok).count()
    );
}
