//! ROM-free sideview spawn/despawn tests + gated snapshots.

#[allow(dead_code)]
#[path = "../src/sideview_spawn.rs"]
mod sideview_spawn;

mod common;

use sideview_spawn::*;

#[test]
fn spawn_gate_matches_d603() {
    // Fairy always skips.
    assert!(!spawn_gate(true, 0x02, 0x02, 0x00));
    // Single-screen always spawns.
    assert!(spawn_gate(false, 0x02, 0x02, 0x00));
    assert!(spawn_gate(false, 0x02, 0x02, 0x01));
    // Two-page: half flag selects.
    assert!(spawn_gate(false, 0x01, 0x03, 0x01));
    assert!(!spawn_gate(false, 0x01, 0x03, 0x00));
}

#[test]
fn enemy_list_walk_finds_unspawned() {
    // len 7: entries at 1..6; entry at 1 matches screens (hi 2, lo 3).
    // b0 = (hi<<4)|lo with hi in low 2 bits after the 3-rotate → use $23.
    let list = [7u8, 0x23, 0x02, 0x05, 0x23, 0x84, 0x06, 0x07];
    // scr_l & 3 = 3? 0x23 hi bits: (0x23>>4)&3 = 2. Use scr_l = 2.
    assert_eq!(find_spawn(&list, 0x02, 0x03), Some(1));
    // Spawned bit set → skip to next match at 4.
    let list2 = [7u8, 0x23, 0x82, 0x05, 0x23, 0x04, 0x06, 0x07];
    assert_eq!(find_spawn(&list2, 0x02, 0x03), Some(4));
    // No match → None.
    assert_eq!(find_spawn(&list, 0x00, 0x00), None);
    // Empty → None.
    assert_eq!(find_spawn(&[], 0x02, 0x03), None);
    // Entry decode spot-check.
    let e = decode_entry(0x23, 0x04, 0x15);
    assert_eq!((e.page_lo, e.spawned, e.id), (0x03, false, 0x15));
    assert!(decode_entry(0x00, 0x80, 0x00).spawned);
}

#[test]
fn item_slots_and_kinds() {
    assert_eq!(free_slot(&[0, 0, 0, 0, 0, 0]), Some(5));
    assert_eq!(free_slot(&[0, 1, 1, 1, 1, 1]), Some(0));
    assert_eq!(item_kind(0x05), ItemKind::PBag);
    assert_eq!(item_kind(0x12), ItemKind::Key);
    assert_eq!(item_kind(0x25), ItemKind::JarBlue);
    assert_eq!(item_kind(0x35), ItemKind::JarRed);
    assert_eq!(item_kind(0x45), ItemKind::Heart);
    assert_eq!(item_kind(0x55), ItemKind::Doll);
    assert_eq!(item_kind(0x65), ItemKind::Crystal);
    assert_eq!(item_kind(0xFF), ItemKind::Other(0xFF));
    assert_eq!(ENEMY_Y_TAB.len(), 8);
}

#[test]
fn despawn_keeps_elevator_and_myu_floor() {
    // Elevator never despawns, even far away.
    assert!(!despawn(ENEMY_ELEVATOR, 0x00, 0x00, 0x05, 0x00, 0x05, 0x00));
    // Below Myu floor never despawns.
    assert!(!despawn(0x01, 0x00, 0x00, 0x05, 0x00, 0x05, 0x00));
    // Far-left enemy despawns.
    assert!(despawn(0x10, 0x00, 0x00, 0x05, 0x80, 0x05, 0x80));
}

#[test]
fn corpus_sideview_spawn_snapshots_skip_without_corpus() {
    let Some(rd) = common::corpus_snapshots("corpus_sideview_spawn_snapshots") else {
        return;
    };
    eprintln!(
        "corpus snapshots present: {} entries",
        rd.filter_map(Result::ok).count()
    );
}
