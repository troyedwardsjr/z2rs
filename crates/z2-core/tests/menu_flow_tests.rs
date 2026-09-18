//! Menu-flow empirical mapping.
//!
//! ROM-gated end-to-end checks (skip gracefully without `Z2_ROM`):
//! drive the untrapped interpreter through title → file-select →
//! register/name-entry → game-start with scripted input, asserting the
//! RAM signatures per state. Pure-logic unit tests (no ROM) pin the
//! `title_flow`/`save` dispatch tables used to interpret the dumps.
//!
//! State machine found (live on ROM `/Volumes/Holy Drive/dev/z2rs/rom/zelda2.nes`):
//!
//! ```text
//! title ($736=3,$76C=0) --Start--> file-select ($736=0,$76C=1, fairy REGISTER)
//!   --Start--> register ($736=1,$76C=1, fairy slot 0, name grid live)
//!   --A x8--> named (SRAM backup $602C+ = tiles, $1E wraps to 0)
//!   --Select x3--> END ($78) --Start--> file-select (slot 0 occupied, $1A=$06)
//!   --Start--> LoadSlot ($772=0, lives=3, name/stats in RAM)
//!   --wait--> lives screen --> gameplay ($736=$0B sideview, area 0)
//! ```
//!
//! `$1A` bits are *empty* bits (`bank5_code26` `$B502` sets a bit iff the
//! slot name is all-`$F4`): fresh cart `$1A=$07`, slot-0-named `$1A=$06`.

#![cfg(feature = "interp")]

mod common;

use z2_core::game::Game;

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Load the pinned ROM into a reset `Game`, or `None` (skip) without it.
fn rom_game() -> Option<Game> {
    let raw = common::rom_bytes("menu_flow_tests")?;
    let mut game = Game::from_ines(&raw).ok()?;
    game.reset();
    Some(game)
}

/// One-line RAM signature for menu states.
fn sig(g: &Game) -> String {
    let r = |a: u16| g.ram[a as usize];
    let name: Vec<String> = (0..8).map(|i| format!("{:02X}", r(0x07A1 + i))).collect();
    let prog: Vec<String> = (0..8).map(|i| format!("{:02X}", r(0x0777 + i))).collect();
    // Name entry writes go to BACKUP part1 ($6002+slot*…+42), not main:
    // LB23C pointers resolve slot s → $602C/$605E/$6090 (bak1+42).
    let bak0name: Vec<String> = (0..8)
        .map(|i| format!("{:02X}", g.wram[0x602C - 0x6000 + i]))
        .collect();
    let (_, _, _, _, pc, _) = g.cpu_state();
    format!(
        "f={} pc=${:04X} m736={:02X} s76C={:02X} fairy19={:02X} y1B={:02X} pres1A={:02X} pos1E={:02X} col1F={:02X} row20={:02X} slot772={:02X} lives700={:02X} r73B={:02X} name=[{}] prog777=[{}] hdr7400={:02X} b0sram=[{}]",
        g.frame_count(),
        pc,
        r(0x0736),
        r(0x076C),
        r(0x0019),
        r(0x001B),
        r(0x001A),
        r(0x001E),
        r(0x001F),
        r(0x0020),
        r(0x0772),
        r(0x0700),
        r(0x073B),
        name.join(" "),
        prog.join(" "),
        g.wram[0x7400 - 0x6000],
        bak0name.join(" "),
    )
}

fn step_n(g: &mut Game, n: usize, input: u8) {
    for _ in 0..n {
        g.step(input);
    }
}

// Input bytes on this contract (bit0=A … bit7=Right).
const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
#[allow(dead_code)]
const UP: u8 = 0x10;
#[allow(dead_code)]
const DOWN: u8 = 0x20;
#[allow(dead_code)]
const LEFT: u8 = 0x40;
#[allow(dead_code)]
const RIGHT: u8 = 0x80;

/// Drive title → register screen (fairy on slot 0, name grid live).
fn to_register(g: &mut Game) {
    step_n(g, 30, 0x00); // title intro runs itself
    step_n(g, 5, START); // title Start edge → file-select
    step_n(g, 20, 0x00); // settle: parks REGISTER ($1A=$07 all empty)
    step_n(g, 5, START); // file-select Start → register screen
    step_n(g, 20, 0x00); // settle: parks slot 0 ($1A=$0F with latch)
}

// ---------------------------------------------------------------------------
// Pure-logic pins (no ROM).
// ---------------------------------------------------------------------------

#[test]
fn empty_bits_match_code26_polarity() {
    use z2_core::save::{empty_bits, slot_empty, NAME_BLANK};
    // bank5_code26 $B502: bit set ⟺ name all-$F4 (empty).
    let empty = [NAME_BLANK; 8];
    let mut named = [NAME_BLANK; 8];
    named[3] = 0xDA;
    assert!(slot_empty(&empty));
    assert!(!slot_empty(&named));
    assert_eq!(empty_bits([&empty, &empty, &empty]), 0x07);
    assert_eq!(empty_bits([&named, &empty, &empty]), 0x06);
    assert_eq!(empty_bits([&named, &named, &named]), 0x00);
}

#[test]
fn file_select_dispatch_pins() {
    use z2_core::title_flow::{file_start, FileStart};
    assert_eq!(file_start(3), FileStart::Register);
    assert_eq!(file_start(4), FileStart::Elimination);
    assert_eq!(file_start(0), FileStart::LoadSlot(0));
}

#[test]
fn settle_fns_match_hardware_empty_bit_polarity() {
    use z2_core::title_flow::{elim_settle, file_settle, register_settle};
    // Fresh cart ($1A=$07, all empty): file-select parks REGISTER…
    assert_eq!(file_settle(0, 0x07), (3, 0x90));
    // …register (with $08 latch) parks slot 0…
    assert_eq!(register_settle(0, 0x08 | 0x07), (0, 0x30));
    // …elimination parks END (nothing erasable).
    assert_eq!(elim_settle(0, 0x07), (3, Some(0x78)));
    // Slot 0 named ($1A=$06, bit 0 clear = occupied): file-select parks
    // slot 0, register skips it to slot 1, elim parks it (erasable).
    assert_eq!(file_settle(0, 0x06), (0, 0x40));
    assert_eq!(register_settle(0, 0x08 | 0x06), (1, 0x48));
    assert_eq!(elim_settle(0, 0x06), (0, Some(0x30)));
}

#[test]
fn register_dispatch_pins() {
    use z2_core::title_flow::{register_start, RegisterStart};
    assert_eq!(register_start(3), RegisterStart::Back);
    assert_eq!(register_start(0), RegisterStart::EnterName(0));
}

#[test]
fn name_grid_nav_pins() {
    use z2_core::save::{grid_down, grid_left, grid_right, grid_up, letter_index};
    assert_eq!(letter_index(0, 0), 0);
    assert_eq!(letter_index(10, 3), 43);
    assert_eq!(grid_right(10, 0), (0, 1));
    assert_eq!(grid_left(0, 0), (10, 3));
    assert_eq!(grid_up(3, 0), (3, 3));
    assert_eq!(grid_down(2, 3), (2, 0));
}

// ---------------------------------------------------------------------------
// ROM-gated empirical mapping (dumps via --nocapture).
// ---------------------------------------------------------------------------

#[test]
fn title_start_reaches_file_select_at_register() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    step_n(&mut g, 30, 0x00);
    assert_eq!((g.ram[0x736], g.ram[0x76C]), (0x03, 0x00), "title intro");
    step_n(&mut g, 5, START);
    step_n(&mut g, 20, 0x00);
    // File-select: mode 0, stage 1, fairy REGISTER ($90), $1A=$07 all empty.
    assert_eq!((g.ram[0x736], g.ram[0x76C]), (0x00, 0x01));
    assert_eq!((g.ram[0x19], g.ram[0x1B], g.ram[0x1A]), (0x03, 0x90, 0x07));
    eprintln!("file-select: {}", sig(&g));
}

#[test]
fn register_screen_takes_name_entry_to_backup_sram() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    to_register(&mut g);
    // Register screen: mode 1, fairy slot 0 ($30), grid live at 0,0,0.
    assert_eq!((g.ram[0x736], g.ram[0x76C]), (0x01, 0x01));
    assert_eq!((g.ram[0x19], g.ram[0x1B]), (0x00, 0x30));
    assert_eq!((g.ram[0x1E], g.ram[0x1F], g.ram[0x20]), (0x00, 0x00, 0x00));
    // 8x A at grid (0,0) = tile $DA: backup name fills, $1E wraps to 0.
    for _ in 0..8 {
        step_n(&mut g, 2, A);
        step_n(&mut g, 8, 0x00);
    }
    assert_eq!(g.ram[0x1E], 0x00, "$1E wraps after 8 letters");
    assert_eq!(
        &g.wram[0x602C - 0x6000..0x602C - 0x6000 + 8],
        &[0xDA; 8],
        "backup name = 8x 'A' tile"
    );
    eprintln!("named: {}", sig(&g));
}

#[test]
fn end_plus_start_returns_to_file_select_with_slot_occupied() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    to_register(&mut g);
    for _ in 0..8 {
        step_n(&mut g, 2, A);
        step_n(&mut g, 8, 0x00);
    }
    // Select x3: fairy 0 → 1 → 2 → END ($78).
    for _ in 0..3 {
        step_n(&mut g, 2, SELECT);
        step_n(&mut g, 8, 0x00);
    }
    assert_eq!((g.ram[0x19], g.ram[0x1B]), (0x03, 0x78), "END cursor");
    // Start at END: commit-all (hdr $A5) then back to file-select (mode 0).
    step_n(&mut g, 5, START);
    step_n(&mut g, 30, 0x00);
    assert_eq!((g.ram[0x736], g.ram[0x76C]), (0x00, 0x01));
    // Slot 0 now occupied: bit 0 clear ($1A=$06), fairy parks slot 0.
    assert_eq!(g.ram[0x1A], 0x06);
    assert_eq!((g.ram[0x19], g.ram[0x1B]), (0x00, 0x40));
    eprintln!("file-select occupied: {}", sig(&g));
}

#[test]
fn start_on_named_slot_reaches_sideview_gameplay() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    to_register(&mut g);
    for _ in 0..8 {
        step_n(&mut g, 2, A);
        step_n(&mut g, 8, 0x00);
    }
    for _ in 0..3 {
        step_n(&mut g, 2, SELECT);
        step_n(&mut g, 8, 0x00);
    }
    step_n(&mut g, 5, START);
    step_n(&mut g, 30, 0x00);
    // Start on occupied slot 0 → LoadSlot: lives=3, name + base stats live.
    step_n(&mut g, 5, START);
    step_n(&mut g, 60, 0x00);
    assert_eq!(g.ram[0x700], 0x03, "lives reset to 3");
    assert_eq!(&g.ram[0x7A1..0x7A1 + 8], &[0xDA; 8], "name loaded");
    assert_eq!(&g.ram[0x777..0x777 + 3], &[0x01; 3], "atk/mag/life = 1");
    eprintln!("loading: {}", sig(&g));
    // Lives screen → respawn → sideview gameplay ($0B) in North Castle.
    step_n(&mut g, 1100, 0x00);
    assert_eq!(g.ram[0x736], 0x0B, "sideview gameplay mode");
    assert_eq!(g.ram[0x748], 0x00, "area 0 (North Castle)");
    eprintln!("gameplay: {}", sig(&g));
}
