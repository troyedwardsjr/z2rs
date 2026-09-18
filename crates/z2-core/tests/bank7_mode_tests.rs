//! ROM-free bank-7 logic tests.
//!
//! * Typed selector round-trips + dispatcher truth tables (`bank7_mode`).
//! * Mode-change detectors + reset prefix + NMI spans as direct calls on
//!   synthetic state (`Game::new` / `with_test_program` — no ROM).
//! * The task's transition chain (title → file select → overworld →
//!   sideview → death → continue) driven through the Rust dispatcher.

#![cfg(feature = "interp")]

use z2_core::bank7_dispatch::{
    detect_boot_stage_change, detect_dialog_change, detect_game_mode_change,
    store_ppu_macro_selector,
};
use z2_core::bank7_mode::{
    boot_ready, dispatch_boot_stage, dispatch_main_loop, dispatch_side_routine, route_nmi_tail,
    Action, BootStage, GameMode, NmiRoute, Selectors, SideRoutine,
};
use z2_core::bank7_nmi::{nmi_ppu_setup, nmi_prologue, NmiEntry};
use z2_core::bank7_reset::reset_prefix;
use z2_core::game::Game;

// ------------------------------------------------- selector codecs

#[test]
fn selector_codecs_roundtrip() {
    for v in 0..=255u8 {
        assert_eq!(GameMode::decode(v).encode(), v);
        assert_eq!(BootStage::decode(v).encode(), v);
        assert_eq!(SideRoutine::decode(v).encode(), v);
    }
    assert_eq!(GameMode::decode(0x0B), GameMode::SideScroll);
    assert_eq!(BootStage::decode(0x01), BootStage::Idle);
    assert_eq!(SideRoutine::decode(0x00), SideRoutine::Idle);
    assert_eq!(SideRoutine::decode(0x04), SideRoutine::Pane(0x04));
}

// ------------------------------------------------- main-loop + boot dispatch

#[test]
fn boot_wait_predicate_matches_c010_chain() {
    // ($0736 == 8 || $0736 == 14) && $076C == 1.
    let ready = |m: u8, s: u8| {
        boot_ready(Selectors {
            mode: GameMode::decode(m),
            stage: BootStage::decode(s),
            routine: SideRoutine::Idle,
        })
    };
    assert!(ready(0x08, 0x01));
    assert!(ready(0x14, 0x01));
    assert!(!ready(0x08, 0x00));
    assert!(!ready(0x00, 0x01));
    assert!(!ready(0x0B, 0x01));
    assert_eq!(
        dispatch_main_loop(Selectors {
            mode: GameMode::decode(0x08),
            stage: BootStage::decode(0x01),
            routine: SideRoutine::Idle,
        }),
        Action::LoadArea
    );
    assert_eq!(
        dispatch_main_loop(Selectors {
            mode: GameMode::Title,
            stage: BootStage::RestartCastle,
            routine: SideRoutine::Idle,
        }),
        Action::KeepWaiting
    );
}

#[test]
fn boot_stage_dispatch_names_table_entries() {
    for s in 0..7u8 {
        let sel = Selectors {
            mode: GameMode::Title,
            stage: BootStage::decode(s),
            routine: SideRoutine::Idle,
        };
        assert_eq!(dispatch_boot_stage(sel), Action::BootTable(s));
    }
    // Past the 7-entry $C2D8 table: no ASM target; wait instead of overread.
    let sel = Selectors {
        mode: GameMode::Title,
        stage: BootStage::BeyondTable(0x09),
        routine: SideRoutine::Idle,
    };
    assert_eq!(dispatch_boot_stage(sel), Action::KeepWaiting);
}

#[test]
fn side_routine_dispatch_and_flute_bypass() {
    let sel = Selectors {
        mode: GameMode::SideScroll,
        stage: BootStage::Idle,
        routine: SideRoutine::Pane(0x04),
    };
    assert_eq!(dispatch_side_routine(sel, false), Action::SideTable(0x04));
    assert_eq!(dispatch_side_routine(sel, true), Action::SideFrame);
}

#[test]
fn nmi_tail_routing_truth_table() {
    // Tail iff routine == Idle && dialog != 1 ($C151-$C167 derivation).
    for dialog in [0x00u8, 0x01, 0x02, 0x07] {
        for routine in [0x00u8, 0x01, 0x03, 0x07] {
            let got = route_nmi_tail(SideRoutine::decode(routine), dialog, false);
            let want = if routine == 0 && dialog != 1 {
                NmiRoute::EngineTail
            } else {
                NmiRoute::DialogFast
            };
            assert_eq!(got, want, "routine={routine:02X} dialog={dialog:02X}");
        }
    }
    assert_eq!(
        route_nmi_tail(SideRoutine::Idle, 0x00, true),
        NmiRoute::PauseBranch
    );
}

// ------------------------------------------------- detectors (direct calls)

#[test]
fn game_mode_change_detector_clears_riders() {
    let mut g = Game::new();
    g.ram[0x736] = 0x0B;
    g.ram[0x737] = 0x08; // changed: clears $073B/$0738/$073D.
    g.ram[0x73B] = 0x11;
    g.ram[0x738] = 0x22;
    g.ram[0x73D] = 0x33;
    g.set_cpu(0x00, 0x00, 0x09, 0xFD, 0xC000, 0x20);
    detect_game_mode_change(&mut g);
    assert_eq!(g.ram[0x737], 0x0B, "shadow follows");
    assert_eq!((g.ram[0x73B], g.ram[0x738], g.ram[0x73D]), (0, 0, 0));
    assert_eq!(g.cpu_state().0, 0x0B, "A = new mode");
    // Unchanged: shadow only, riders preserved.
    let mut g = Game::new();
    g.ram[0x736] = 0x0B;
    g.ram[0x737] = 0x0B;
    g.ram[0x73D] = 0x33;
    detect_game_mode_change(&mut g);
    assert_eq!(g.ram[0x73D], 0x33);
}

#[test]
fn boot_stage_change_detector_resets_mode() {
    let mut g = Game::new();
    g.ram[0x76C] = 0x02;
    g.ram[0x76D] = 0x01; // changed.
    g.ram[0x736] = 0x0B;
    g.ram[0x73D] = 0x05;
    for i in 0..256usize {
        g.ram[0x200 + i] = (i as u8).wrapping_add(7);
    }
    detect_boot_stage_change(&mut g);
    assert_eq!(g.ram[0x76D], 0x02);
    assert_eq!(g.ram[0x736], 0x00, "Game Mode cleared ($D18A)");
    assert_eq!(g.ram[0x73D], 0x00);
    assert_eq!(g.ram[0x200], 0xF8, "LD174 hides all sprites ($D24C)");
    assert_eq!(g.ram[0x204], 0xF8);
}

#[test]
fn dialog_detector_and_macro_store() {
    let mut g = Game::new();
    g.set_cpu(0x06, 0, 0, 0xFD, 0, 0x20);
    store_ppu_macro_selector(&mut g);
    assert_eq!(g.ram[0x725], 0x06);
    g.ram[0x738] = 0x04;
    g.ram[0x739] = 0x04;
    g.ram[0x73D] = 0x09;
    detect_dialog_change(&mut g);
    assert_eq!(g.ram[0x73D], 0x09, "unchanged dialog keeps $073D");
    g.ram[0x738] = 0x05;
    detect_dialog_change(&mut g);
    assert_eq!(g.ram[0x739], 0x05, "shadow follows");
    assert_eq!(g.ram[0x73D], 0x00, "changed dialog clears $073D (LD195)");
}

// ------------------------------------------------- reset prefix (no ROM)

#[test]
fn reset_prefix_state_matches_hand_computation() {
    let mut g = Game::new();
    // Dirty the MMC1 shift register + regs like a warm boot would.
    g.mmc1.write(0x8000, 0x01);
    g.mmc1.write(0x8000, 0x01);
    g.set_cpu(0xAA, 0xBB, 0x55, 0x60, 0x1234, 0x20);
    reset_prefix(&mut g);
    assert_eq!(g.cpu_state().3, 0xFF, "TXS");
    assert_eq!(g.cpu_state().1, 0xFF, "X = $FF after the spin");
    assert_eq!(
        g.cpu_state().0,
        0x00,
        "A = last $2002 read (real PPU: no vblank yet)"
    );
    assert_eq!(g.cpu_state().2, 0x55, "Y preserved");
    assert_eq!(g.cpu_state().5 & 0x04, 0x04, "SEI");
    assert_eq!(g.cpu_state().5 & 0x08, 0x00, "CLD");
    assert_eq!((g.mmc1.shift, g.mmc1.count), (0, 0), "MMC1 shift cleared");
    assert_eq!(g.mmc1.prg_mode(), 3, "reset writes force PRG mode 3");
    assert_eq!(g.ppu.reads, 2, "two $2002 polls");
    assert_eq!(g.ppu.writes, 1, "one $2000 write");
}

// ------------------------------------------------- NMI spans (synthetic PRG for the $0725 table)

/// `with_test_program` at `$C000` with a 2-entry `$0725` table at `$C03D`:
/// entry 0 -> `$0302`, entry 1 -> `$0400`.
fn nmi_test_game() -> Game {
    let mut blob = vec![0xEA; 0x100];
    blob[0x3D] = 0x02;
    blob[0x3E] = 0x03; // entry 0: $0302.
    blob[0x3F] = 0x00;
    blob[0x40] = 0x04; // entry 1: $0400.
    let mut g = Game::with_test_program(0xC000, &blob);
    g.set_cpu(0x00, 0x00, 0x00, 0xFD, 0xC000, 0x20);
    g
}

#[test]
fn nmi_entry_routing_and_oam_dma() {
    // Sound path ($0100 bit 7 clear).
    let mut g = nmi_test_game();
    g.ram[0x100] = 0x00;
    assert_eq!(nmi_prologue(&mut g).0, NmiEntry::Sound);
    // Bank-5 path (bit 7 set, bit 6 clear).
    let mut g = nmi_test_game();
    g.ram[0x100] = 0x80;
    assert_eq!(nmi_prologue(&mut g).0, NmiEntry::Bank5);
    // Full path: OAM DMA copies $0200 page, scroll zeroed, masks merged.
    let mut g = nmi_test_game();
    for i in 0..256usize {
        g.ram[0x200 + i] = (i ^ 0x3C) as u8;
    }
    g.ram[0x100] = 0xC0;
    g.ram[0xFF] = 0xB0;
    g.ram[0x747] = 0x04;
    g.ram[0xFE] = 0x10;
    g.ram[0x726] = 0x00; // take the ORA #$18 arm.
    g.ram[0x768] = 0x00;
    g.ram[0x7AE] = 0x00;
    let (entry, pal) = nmi_prologue(&mut g);
    assert_eq!(entry, NmiEntry::Full);
    assert!(!pal);
    for i in 0..256usize {
        assert_eq!(g.oam[i], (i ^ 0x3C) as u8, "oam[{i}] DMA");
    }
    assert_eq!(g.ram[0xFF], 0xB0 & 0x7C | 0x04);
    assert_eq!(g.ram[0xFE], 0x10 | 0x18);
    assert_eq!(g.cpu_state().0, 0x00, "A = $07AE (BEQ gate value)");
}

#[test]
fn nmi_ppu_setup_drains_and_restores_scroll() {
    let mut g = nmi_test_game();
    // $0725 = 0 -> pointer $0302 -> $FF terminator (immediate drain exit).
    g.ram[0x725] = 0x00;
    g.ram[0x302] = 0xFF;
    g.ram[0x768] = 0x00; // take the scroll-restore arm.
    g.ram[0xFF] = 0xB0;
    g.ram[0xFD] = 0x11;
    g.ram[0xFC] = 0x22;
    g.ram[0xFE] = 0x30;
    nmi_ppu_setup(&mut g);
    assert_eq!(
        (g.ram[0], g.ram[1]),
        (0x02, 0x03),
        "$00/$01 from table entry 0"
    );
    // Latch quad ($2006 x4) + scroll restore ($2000/$2005 x2... $2005 twice)
    // + final $FE -> $2001: 8 PPU writes total on this path.
    let writes = g.ppu.writes;
    assert_eq!(writes, 8, "macro + latch + restore writes, got {writes}");
    assert_eq!(g.ppu.last_write, Some((0x2001, 0x30)), "final $FE -> $2001");
    assert_eq!(g.ram[0x725], 0x00, "selector stored back");
    assert_eq!(g.ppu.model().scroll(), (0x11, 0x22), "$FD is X, $FC is Y");
}

// ------------------------------------------------- transition chain (synthetic scenario)

/// Drive the task's transition chain through the Rust dispatcher with
/// synthetic RAM states: title → file select → overworld → sideview →
/// death → continue. Each stage asserts the routing decision; bodies live
/// in other banks (0/5/6), so the chain checks *decisions*, while the
/// detector tests above check the RAM side effects of changing selectors.
#[test]
fn mode_transition_chain_snapshot_style() {
    let mut g = Game::new();
    // Title (boot wait): mode $08, stage Idle -> LoadArea once NMI posts it.
    g.ram[0x736] = 0x08;
    g.ram[0x76C] = 0x01;
    g.ram[0x524] = 0x00;
    let sel = Selectors::read(&g);
    assert_eq!(dispatch_main_loop(sel), Action::LoadArea);
    // File select behaves as an overworld-ish mode here: unknown modes keep
    // waiting until their loader posts a terminal (no spurious LoadArea).
    g.ram[0x736] = 0x02; // (synthetic stand-in for a bank-0 menu mode)
    assert_eq!(dispatch_main_loop(Selectors::read(&g)), Action::KeepWaiting);
    // Overworld steady state: stage Idle, routine Idle -> engine tail.
    g.ram[0x736] = 0x08;
    assert_eq!(
        route_nmi_tail(SideRoutine::Idle, 0x00, false),
        NmiRoute::EngineTail
    );
    // Sideview: mode $0B routes the pause pane + side frame.
    g.ram[0x736] = 0x0B;
    g.ram[0x524] = 0x04;
    let sel = Selectors::read(&g);
    assert_eq!(sel.mode, GameMode::SideScroll);
    assert_eq!(
        route_nmi_tail(SideRoutine::Pane(0x04), 0x00, true),
        NmiRoute::PauseBranch
    );
    assert_eq!(dispatch_side_routine(sel, false), Action::SideTable(0x04));
    // Death: stage Die ($02) names boot-table entry 2 (bank-0 death path).
    g.ram[0x76C] = 0x02;
    assert_eq!(
        dispatch_boot_stage(Selectors::read(&g)),
        Action::BootTable(0x02)
    );
    // Continue: stage LivesRestart ($06) -> entry 6, then back to Idle.
    g.ram[0x76C] = 0x06;
    assert_eq!(
        dispatch_boot_stage(Selectors::read(&g)),
        Action::BootTable(0x06)
    );
    // Detector side effects fire on the way back (stage 6 -> 1).
    g.ram[0x76D] = 0x06;
    g.ram[0x76C] = 0x01;
    g.ram[0x736] = 0x0B;
    detect_boot_stage_change(&mut g);
    assert_eq!(g.ram[0x736], 0x00, "continue resets Game Mode (LD174)");
}
