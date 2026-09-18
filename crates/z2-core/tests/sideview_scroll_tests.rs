//! ROM-free sideview scroll/entry/exit/elevator tests + gated snapshots.

#[allow(dead_code)]
#[path = "../src/sideview_scroll.rs"]
mod sideview_scroll;

mod common;

use sideview_scroll::*;

#[test]
fn scroll_window_splits_pairs() {
    let s = Scroll {
        hi: 0x02,
        hi2: 0x03,
        lo: 0x10,
        lo2: 0x20,
    };
    assert_eq!(s.left(), 0x0210);
    assert_eq!(s.right(), 0x0320);
    assert!(!scroll_frozen(0));
    assert!(scroll_frozen(1));
}

#[test]
fn entry_anchor_and_decode() {
    let a = entry_anchor(1);
    assert_eq!((a.link_x, a.r34, a.r35), (0x70, 0x0B, 0x06));
    assert_eq!((a.page, a.scroll_hi, a.r32, a.r33), (1, 1, 0, 2));
    // $075C = 0 wraps $0732 to $FF (caller maps $FF → 3 on elevator path).
    assert_eq!(entry_anchor(0).r32, 0xFF);
    assert_eq!(decode_entry(0x00), (0x00, 0x00, 0x00));
    assert_eq!(decode_entry(0xFF), (0x3F, 0x03, 0x01));
    // map $2C → area $2C, enter ($2C<<2)&3 = 0, dir 0.
    assert_eq!(decode_entry(0x2C), (0x2C, 0x00, 0x00));
}

#[test]
fn side_exit_wall_vs_room() {
    assert_eq!(
        side_exit_step(0xFC, false, 0x10),
        SideExitStep::Wall { area_index: 0x10 }
    );
    assert_eq!(
        side_exit_step(0xFF, false, 0x10),
        SideExitStep::Wall { area_index: 0x13 }
    );
    assert_eq!(
        side_exit_step(0x14, false, 0x00),
        SideExitStep::Room {
            area: 0x05,
            page: 0x00,
            dir: 0x00
        }
    );
    // Elevator ride still computes the target (shim skips the store).
    assert!(matches!(
        side_exit_step(0x14, true, 0x00),
        SideExitStep::Room { area: 5, .. }
    ));
}

#[test]
fn door_and_elevator_exits() {
    assert_eq!(door_exit_step(0x14), (0x05, 0x00, 0x00));
    assert_eq!(door_exit_step(0x07), (0x01, 0x03, 0x01));
    assert_eq!(elevator_exit_step(0x14, false), (0x05, 0x00));
    assert_eq!(elevator_exit_step(0x14, true), (0x05, 0x00));
    assert_eq!(elev_r32(0), 0x03);
    assert_eq!(elev_r32(2), 0x01);
    assert_eq!(ELEV_MIDDLE_PAGE, 0x04);
}

#[test]
fn elevator_up_down() {
    // $0743 >> 2 indexes 00/18/E8.
    assert_eq!(elev_vel_index(0x00), 0);
    assert_eq!(elev_vel_index(0x04), 1);
    assert_eq!(elev_vel_index(0x08), 2);
    assert_eq!(elev_velocity(0x04), 0x18);
    assert_eq!(elev_velocity(0x08), 0xE8);
    assert_eq!(elev_velocity(0x00), 0x00);
    assert!(!elev_at_floor(0xD7));
    assert!(elev_at_floor(0xD8));
    assert_eq!(elev_link_y(0x10), 0x18);
    assert_eq!(page_step(2, true), 3);
    assert_eq!(page_step(0, false), 0xFF);
}

#[test]
fn corpus_sideview_scroll_snapshots_skip_without_corpus() {
    let Some(rd) = common::corpus_snapshots("sideview scroll corpus snapshots") else {
        return;
    };
    let mut n = 0;
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.file_name()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.starts_with("sideview-"))
        {
            let b = std::fs::read(&p).expect("read snapshot");
            assert!(b.starts_with(b"Z2SNAP01"), "{}: bad magic", p.display());
            n += 1;
        }
    }
    if n == 0 {
        eprintln!("SKIP: no sideview-* snapshots in corpus");
    }
}
