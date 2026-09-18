//! ROM-free transition tests + gated warpless-segment checks.
//!
//! Compiles standalone (`rustc --edition 2021 --test`) and under cargo.

#[path = "../src/overworld_transition.rs"]
mod overworld_transition;

mod common;

use overworld_transition::*;

// Small strided fixture: slot 61 town at (0x34,0x17), slot 41 dock at
// (0x20,0x30), rest empty (mirrors the West lanes' first entries).
fn fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; AREA_COUNT];
    let mut x = vec![0u8; AREA_COUNT];
    let mut m = vec![0u8; AREA_COUNT];
    let w = vec![0u8; AREA_COUNT];
    y[61] = 0x34;
    x[61] = 0x17;
    m[61] = 0x80;
    y[DOCK_SLOT as usize] = 0x20;
    x[DOCK_SLOT as usize] = 0x30;
    (y, x, m, w)
}

#[test]
fn scan_finds_topmost_match_and_masks_bits() {
    let (mut y, mut x, m, w) = fixture();
    // External-bit + high X bits are masked before comparing.
    y[60] = 0x80 | 0x34;
    x[60] = 0xC0 | 0x17;
    // Scan runs top-down: slot 61 beats slot 60.
    assert_eq!(find_key_area(&y, &x, &m, &w, 0x34, 0x17), Some(61));
    assert_eq!(find_key_area(&y, &x, &m, &w, 0x20, 0x30), Some(41));
    // (0x01,0x02) is empty everywhere (most lanes are (0,0), so (0,0) would hit).
    assert_eq!(find_key_area(&y, &x, &m, &w, 0x01, 0x02), None);
    y[61] = 0x00;
    assert_eq!(find_key_area(&y, &x, &m, &w, 0x34, 0x17), Some(60));
}

#[test]
fn area_group_locs_match_extract_tables() {
    assert_eq!(area_group_loc(AreaGroup::West), (0x462F, 0x861F));
    assert_eq!(area_group_loc(AreaGroup::DeathMountain), (0x610C, 0xA000));
    assert_eq!(area_group_loc(AreaGroup::East), (0x862F, 0x861F));
    assert_eq!(area_group_loc(AreaGroup::MazeIsland), (0xA10C, 0xA000));
    assert_eq!(AREA_STRIDE, 63);
    assert_eq!(AREA_SCAN_TOP, 0x3D);
}

#[test]
fn return_path_restores_and_nudges() {
    // Plain return: masked bytes pass through.
    assert_eq!(return_tiles(0x80 | 0x34, 0xC0 | 0x17), (0x34, 0x17));
    // Hole edge: Y=0 + X=$3D → Y=$51.
    assert_eq!(return_tiles(0x00, 0x3D), (HOLE_EDGE_Y, HOLE_EDGE_X));
    // Non-palace areas: no nudge.
    assert_eq!(return_nudge(10, 20, false, 1, 1, false), (10, 20));
    // Palace-bit + horizontal facing: +X, side match keeps it…
    assert_eq!(return_nudge(10, 20, true, 1, 1, false), (10, 21));
    // …side mismatch backs off two (net −X).
    assert_eq!(return_nudge(10, 20, true, 1, 2, false), (10, 19));
    // Vertical facing: +Y, kept when the shifted pick matches…
    assert_eq!(return_nudge(10, 20, true, 4, 1, false), (11, 20));
    // …backed off two when it does not (external: pick = facing >> 2).
    assert_eq!(return_nudge(10, 20, true, 8, 1, true), (9, 20));
}

#[test]
fn enemy_copy_layout_is_four_chunks() {
    assert_eq!(ENEMY_SRC_OFFSETS, [0x88A0, 0x89A0, 0x8AA0, 0x8BA0]);
    assert_eq!(ENEMY_DST_OFFSETS, [0x7000, 0x7100, 0x7200, 0x7300]);
    let srcs: Vec<Vec<u8>> = (0..4).map(|i| vec![0xA0 + i as u8; 256]).collect();
    let refs: [&[u8]; 4] = [&srcs[0], &srcs[1], &srcs[2], &srcs[3]];
    let mut sram = vec![0u8; SRAM_ENEMY_LEN];
    assert_eq!(copy_enemy_data(&mut sram, refs), 1024);
    assert_eq!(&sram[0x000..0x100], &srcs[0][..]);
    assert_eq!(&sram[0x300..0x400], &srcs[3][..]);
}

#[test]
fn raft_spots_and_modes_match_listing() {
    assert_eq!(RAFT_SPOT_X, [0x3D, 0x07]);
    assert_eq!(RAFT_SPOT_Y, [0x4D, 0x34]);
    assert_eq!(RAFT_MODE, 0x17);
    assert_eq!(HIDDEN_PALACE_SPOT, (2, 0x64, 0x2D));
    assert_eq!(PATCH_TABLE_LEN_WORDS, 4);
    let (dm, fx, clr) = sideview_entry_effects();
    assert_eq!((dm, fx, clr), (1, 0x06, 0));
}

#[test]
fn address_consts_match_ram_map() {
    assert_eq!(ADDR_AREA_INDEX, 0x0748);
    assert_eq!(ADDR_ENCOUNTER_TYPE, 0x075A);
    assert_eq!(ADDR_FAIRY_FLAG, 0x0759);
    assert_eq!(ADDR_OUTSIDE, 0x0709);
    assert_eq!(ADDR_GAME_MODE, 0x0736);
    assert_eq!(ADDR_RAFT, 0x0787);
    assert_eq!(ADDR_HAMMER, 0x078B);
    assert_eq!(ADDR_RAFT_DIR, 0x07A9);
    assert_eq!(SRAM_ENEMY_BASE, 0x7000);
    assert_eq!(WRAM_AREA_BASE, 0x6A00);
    assert_eq!(AREA_EXTERNAL_BIT, 0x80);
    // Lantern check on the remaining lane constants.
    assert_eq!((AREA_COUNT, AREA_STRIDE, SRAM_ENEMY_LEN), (63, 63, 1024));
}

#[test]
fn warpless_segments_harness_ready_without_corpus() {
    // Same gate shape as the map tests: presence-only until main wires
    // replay. Always passes; documents the acceptance hook.
    if let Some(dir) = common::env_dir(
        "Z2_CORPUS",
        "corpus/movies",
        "warpless_segments_harness_ready",
    ) {
        eprintln!(
            "corpus movies present at {} (replay is main's hook)",
            dir.display()
        );
    }
}
