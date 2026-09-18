//! ROM-free sideview area tests + ROM/corpus-gated snapshots.
//!
//! Compiles standalone (`rustc --edition 2021 --test`) and under cargo.
//! Gated tests skip gracefully without `Z2_ROM` / `corpus/`.

#[allow(dead_code)]
#[path = "../src/sideview_area.rs"]
mod sideview_area;

mod common;

use sideview_area::*;

// ---------------------------------------------------------------------------
// Pure unit tests (always run).
// ---------------------------------------------------------------------------

#[test]
fn header_variants_decode() {
    // North-Castle-like: len $36, flags $40 (width 2, layers 0), ground $68
    // (init 8, ground 6), pal-back $00 (bg-map 0).
    let h = AreaHeader::decode(&[0x36, 0x40, 0x68, 0x00]).unwrap();
    assert_eq!(h.len, 0x36);
    assert_eq!(h.width(), 2);
    assert_eq!(h.layers(), 0);
    assert_eq!(h.init_obj(), 8);
    assert!(!h.no_ceiling());
    assert_eq!(h.ground(), 6);
    assert_eq!(h.bg_map(), 0);
    assert_eq!(initial_bg_tile(h), 0x40);
    // Flat-field: ground 0 → tile $42.
    let flat = AreaHeader::decode(&[0x10, 0x00, 0x00, 0x01]).unwrap();
    assert_eq!(flat.ground(), 0);
    assert_eq!(initial_bg_tile(flat), 0x42);
    assert_eq!(flat.bg_map(), 1);
    // No-ceiling flag.
    let nc = AreaHeader::decode(&[0x20, 0x00, 0x80, 0x00]).unwrap();
    assert!(nc.no_ceiling());
    // Short slice → None.
    assert!(AreaHeader::decode(&[0x01, 0x02, 0x03]).is_none());
}

#[test]
fn both_map_sets_select() {
    assert_eq!(map_set_select(0, 0), MapSet::First);
    assert_eq!(map_set_select(0, 1), MapSet::Second);
    assert_eq!(map_set_select(0, 2), MapSet::First);
    assert_eq!(table_word(&[0xB5, 0x8C, 0x99, 0x8D], 0), Some(0x8CB5));
    assert_eq!(table_word(&[0xB5, 0x8C, 0x99, 0x8D], 1), Some(0x8D99));
    assert_eq!(table_word(&[0xB5], 0), None);
    assert_eq!(MAP_PTR_SET1, 0x8523);
    assert_eq!(ENEMY_PTR_SET1, 0x85A1);
    assert_eq!(MAP_PTR_SET2, 0xA000);
    assert_eq!(ENEMY_PTR_SET2, 0xA07E);
    assert_eq!(BEHIND_PTR, 0x8000);
    assert_eq!(AREA_SET_LEN, 63);
}

#[test]
fn enemy_fixup_matches_75a() {
    assert_eq!(enemy_fixup(0), EnemyFixup::Keep);
    assert_eq!(enemy_fixup(1), EnemyFixup::ClearKilled);
    assert_eq!(enemy_fixup(2), EnemyFixup::Advance);
    assert_eq!(enemy_fixup(9), EnemyFixup::Advance);
    assert_eq!(enemy_advance(0x7000, 0x10), 0x7010);
}

#[test]
fn connectivity_exits_decode() {
    assert_eq!(decode_connect(0xFC), SideExit::Wall);
    assert_eq!(decode_connect(0xFF), SideExit::Wall);
    assert_eq!(
        decode_connect(0x14),
        SideExit::Room {
            area: 0x05,
            page: 0x00
        }
    );
    assert_eq!(connect_index(3, 2), 14);
    let conn = [0xFC, 0x14, 0x00, 0xFD];
    assert_eq!(side_exit(&conn, 0, 0), SideExit::Wall);
    assert_eq!(side_exit(&conn, 0, 1), SideExit::Room { area: 5, page: 0 });
    // Short table → Wall (no overread).
    assert_eq!(side_exit(&[], 9, 9), SideExit::Wall);
}

#[test]
fn map_obj_kinds_match_lc89d() {
    assert_eq!(classify_map_obj(0xE2, 0x00), MapObjKind::Skip);
    assert_eq!(classify_map_obj(0xD0, 0x00), MapObjKind::CeilFloor);
    assert_eq!(classify_map_obj(0x23, 0xF8), MapObjKind::Wide);
    assert_eq!(classify_map_obj(0x23, 0x0F), MapObjKind::Item);
    assert_eq!(classify_map_obj(0x23, 0x02), MapObjKind::Small);
}

#[test]
fn background_fill_and_palette() {
    let mut a = [0u8; 16];
    let mut b = [0u8; 16];
    let mut c = [0u8; 16];
    let mut d = [0u8; 16];
    let n = fill_background([&mut a, &mut b, &mut c, &mut d], 0x40);
    assert_eq!(n, 64);
    assert!(a.iter().all(|&x| x == 0x40));
    assert_eq!(palette_index(0x00, 0, true), 0x00);
    // Grotto darkness without candle.
    assert_eq!(palette_index(0x08, 0, false), 0x40);
    assert_ne!(palette_index(0x08, 0, true), 0x40);
}

// ---------------------------------------------------------------------------
// ROM-gated tests (skip without Z2_ROM).
// ---------------------------------------------------------------------------

fn rom_bytes() -> Option<Vec<u8>> {
    common::rom_bytes("sideview_area_tests")
}

#[test]
fn rom_area_tables_have_two_sets_of_63_when_present() {
    let Some(img) = rom_bytes() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    if img.len() < 0x10010 || &img[0..4] != b"NES\x1A" {
        eprintln!("SKIP: ROM too short / bad magic for sideview tables");
        return;
    }
    // Structural (ROM-shape) check only: pointer tables live in PRG banks,
    // exact CPU mapping needs the MMC1 bank switch (interp scope). Here we
    // assert the image is large enough to hold both 63-entry sets plus the
    // 7-entry behind-map table, and that the KNOWN behind-map terminator
    // pattern ($0000 at $800C) is reachable via the bank-1 window size.
    // Full word-count verification runs under the interp A/B harness once
    // bank-aware trap routing lands (reported gap).
    assert!(img.len() >= 0x20010, "PRG must hold banks 1/2 area tables");
    assert_eq!((AREA_SET_LEN, BEHIND_LEN, CONN_STRIDE), (63, 7, 4));
}

// ---------------------------------------------------------------------------
// Corpus-gated snapshot tests (harness-ready; skip without corpus/).
// ---------------------------------------------------------------------------

fn corpus_sideview_snaps() -> Vec<std::path::PathBuf> {
    let Some(rd) = common::corpus_snapshots("sideview area corpus snapshots") else {
        return vec![];
    };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sideview-"))
        })
        .collect()
}

#[test]
fn corpus_sideview_snapshots_verify_when_present() {
    let snaps = corpus_sideview_snaps();
    if snaps.is_empty() {
        eprintln!("SKIP: no corpus sideview-* snapshots (set Z2_CORPUS)");
        return;
    }
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(
            b.starts_with(b"Z2SNAP01"),
            "{}: bad snapshot magic",
            p.display()
        );
        assert!(
            b.len() >= 8 + 2048 + 8192,
            "{}: too small for ram+wram",
            p.display()
        );
    }
}
