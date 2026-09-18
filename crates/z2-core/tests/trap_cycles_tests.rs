//! Per-trap CPU cycle accounting (time-collapse fix).
//!
//! Traps were state-exact but cycle-free: `Reset_Memory_Ranges`/`Erase`
//! (~15k ASM cycles) executed in ~10 host cycles, pulling frame-4's bank-5
//! `$00`-zeroing into frame 3 (trapped f=3 `$0000` vs untrapped f=7). Each
//! trap now carries [`TrapInfo::cycles`](z2_core::traps::TrapInfo) (body +
//! final `RTS`/`RTI`, caller entry excluded); [`Game::fire_trap`] adds it,
//! so `run_budget`/`Cpu::run_cycles` frame budgets and the absolute-cycle
//! vblank cadence stay aligned. Large traps crossing a budget end interpret
//! that one call instead (split like hardware — see `split_trap_target`).
//!
//! * Synthetic (ROM-free) tests: registration shapes, JSR/JMP dispatch
//!   symmetry (no double `RTS`), split rule, `run_cycles` pacing.
//! * ROM-gated tests (skip without `Z2_ROM`): measured-vs-tabled costs for
//!   every fixed-bank boot trap (exact-excerpt `try_call_asm` deltas) and
//!   multi-frame pacing (no 39k overshoot).
//!
//! Existing A/B tests needed no updates: they assert `cpu_state()` (which
//! excludes `cycles`) and `assert_same_state` (RAM/WRAM/OAM/regs/MMC1/bus,
//! not cycles), so charging is invisible to them by design. The
//! `trapped_multiframe_is_deterministic` pacing note in `bank7_traps_ab.rs`
//! is now stale (trapped pacing matches untrapped) but its assertions
//! (determinism, crash-free, boot fires traps) still hold.

#![cfg(feature = "interp")]

mod common;

use z2_core::game::Game;
use z2_core::traps::default_trap_cycles;

// ------------------------------------------------- synthetic: registration

fn inc_rng(game: &mut Game) {
    let r = game.ram[0x51B].wrapping_add(1);
    game.ram[0x51B] = r;
    game.cpu.p = (game.cpu.p & !(z2_core::cpu::FLAG_Z | z2_core::cpu::FLAG_N))
        | if r == 0 { z2_core::cpu::FLAG_Z } else { 0 }
        | (r & z2_core::cpu::FLAG_N);
}

#[test]
fn register_keeps_compiling_and_tables_boot_costs() {
    // `trap_register` keeps its old shape (all 100+ call sites compile);
    // known boot addrs pick up tabled costs, unknown stay 0.
    let mut g = Game::new();
    g.trap_register("inc_rng", Some(7), 0xC100, inc_rng);
    assert_eq!(g.traps.get(0xC100).unwrap().cycles, 0);
    assert_eq!(default_trap_cycles(0xFF9D), 34);
    assert_eq!(default_trap_cycles(0xD281), 15294);
    assert_eq!(default_trap_cycles(0xD266), 19336);
    assert_eq!(default_trap_cycles(0x1234), 0);
    // Explicit override path for measured/variable routines.
    g.trap_register_cycles("inc_rng_c", Some(7), 0xC101, inc_rng, 12);
    assert_eq!(g.traps.get(0xC101).unwrap().cycles, 12);
}

// ------------------------------------------------- synthetic: dispatch symmetry
//
// `INC $051B` abs (6) + `RTS` (6) = 12 body. JSR entry 6 + 12 = 18;
// JMP entry 3 + 12 = 15 (no extra +6 — the cost holds the return).

fn jsr_prog(caller: u16, target: u16) -> Game {
    // caller: JSR target : RTS. Target: INC $051B : RTS.
    let mut blob = vec![0xEA; 0x104];
    blob[0] = 0x20;
    blob[1] = target as u8;
    blob[2] = (target >> 8) as u8;
    blob[3] = 0x60;
    blob[0x100] = 0xEE;
    blob[0x101] = 0x1B;
    blob[0x102] = 0x05;
    blob[0x103] = 0x60;
    let mut g = Game::with_test_program(caller, &blob);
    g.set_cpu(0, 0, 0, 0xFD, caller, 0x20);
    g
}

#[test]
fn jsr_trap_charges_entry_plus_body() {
    let mut g = jsr_prog(0xC000, 0xC100);
    g.trap_register_cycles("inc", Some(7), 0xC100, inc_rng, 12);
    let c0 = g.cpu.cycles;
    g.call_asm(0xC000);
    // call_asm sets PC=caller directly (no outer JSR): stub JSR 6 + body 12
    // + stub RTS 6 = 24.
    assert_eq!(g.cpu.cycles - c0, 24, "JSR 6 + body 12 + stub RTS 6");
    assert_eq!(g.ram[0x51B], 1);
}

#[test]
fn jmp_trap_charges_entry_plus_body_without_double_rts() {
    // $C100: JMP $C200 (tail call). $C200 trapped: INC $051B (12).
    let caller = [0x20, 0x00, 0xC1, 0x60];
    let mut blob = vec![0xEA; 0x204];
    blob[..caller.len()].copy_from_slice(&caller);
    blob[0x100..0x103].copy_from_slice(&[0x4C, 0x00, 0xC2]);
    blob[0x200..0x204].copy_from_slice(&[0xEE, 0x1B, 0x05, 0x60]);
    let mut g = Game::with_test_program(0xC000, &blob);
    g.set_cpu(0, 0, 0, 0xFD, 0xC000, 0x20);
    g.trap_register_cycles("inc_tail", Some(7), 0xC200, inc_rng, 12);
    let c0 = g.cpu.cycles;
    g.call_asm(0xC000);
    // Stub JSR 6 + JMP 3 + body 12 + stub RTS 6 = 27. The old dispatch added
    // an extra +6 for the emulated RTS (33); the cost already holds it.
    assert_eq!(
        g.cpu.cycles - c0,
        27,
        "JSR 6 + JMP 3 + body 12 + stub RTS 6"
    );
    assert_eq!(g.ram[0x51B], 1);
}

// ------------------------------------------------- synthetic: split rule

#[test]
fn large_trap_crossing_budget_interprets_instead_of_overshooting() {
    // Game with JSR $D281 at $C000 and the real $D281 bytes mapped at $D281
    // is ROM-dependent; synthetically: a 15294-cost trap at $C100 called
    // from $C000 with a budget that it would cross must interpret (no log,
    // cycles land at end, no 15k overshoot).
    let mut g = jsr_prog(0xC000, 0xC100);
    g.trap_register_cycles("big", Some(7), 0xC100, inc_rng, 15294);
    // Budget ends 10 cycles out: JSR 6 + 15294 would cross by ~15k.
    let end = g.cpu.cycles + 10;
    // run via the split-aware frame path: step through run_cycles(10).
    g.run_cycles(10).unwrap();
    // Interpreted (untrapped for the crossing call): no trap record, cycles
    // advance by real instructions (JSR 6 + INC 6 + RTS 6 = 18), landing
    // past `end` by instruction granularity only — not +15k.
    assert!(
        g.trap_log().is_empty(),
        "crossing call must interpret, not trap"
    );
    assert!(
        g.cpu.cycles < end + 100,
        "no atomic overshoot: cycles={} end={end}",
        g.cpu.cycles
    );
}

#[test]
fn small_trap_inside_budget_still_traps() {
    let mut g = jsr_prog(0xC000, 0xC100);
    g.trap_register_cycles("inc", Some(7), 0xC100, inc_rng, 12);
    g.run_cycles(100).unwrap();
    assert_eq!(g.trap_log().len(), 1, "fitting call must trap");
    assert_eq!(g.ram[0x51B], 1);
}

// ------------------------------------------------- ROM-gated: measured costs

fn load_rom() -> Option<Vec<u8>> {
    common::rom_bytes("trap_cycles_tests")
}

/// Untrapped `JSR target : RTS` stub at `stub`; returns total cycles
/// (JSR 6 + body+RTS + stub RTS 6). Caller salts RAM/pads per routine.
fn stub_total_cycles(raw: &[u8], stub: u16, target: u16, setup: impl Fn(&mut Game)) -> u64 {
    let mut g = Game::from_ines(raw).expect("ROM");
    setup(&mut g);
    let blob = [0x20, target as u8, (target >> 8) as u8, 0x60];
    for (i, &b) in blob.iter().enumerate() {
        g.ram[((stub as usize) + i) & 0x7FF] = b;
    }
    let c0 = g.cpu.cycles;
    g.try_call_asm(stub, 10_000_000).expect("call");
    g.cpu.cycles - c0
}

fn salt_ram(g: &mut Game) {
    for (i, b) in g.ram.iter_mut().enumerate() {
        *b = (i.wrapping_mul(37).wrapping_add(11)) as u8;
    }
}

#[test]
fn boot_trap_costs_match_interpreter_exactly() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    // (stub, target, setup-id). Body = total - 12 (stub JSR + stub RTS).
    // All fixed-path routines must match exactly; variable-path use the
    // documented common case (same/agree/empty).
    let cases: &[(u16, u16, &str)] = &[
        (0x0200, 0xFF9D, "mmc1"),
        (0x0200, 0xFFB1, "mmc1"),
        (0x0200, 0xFFCC, "mmc1"),
        (0x0200, 0xFFC5, "mmc1-to0"),
        (0x0200, 0xFFC9, "mmc1-saved"),
        (0x0200, 0xD367, "capture"),
        (0x0200, 0xD346, "input-agree"),
        (0x0200, 0xD281, "reset-ranges"),
        (0x0200, 0xD29C, "clear-0300"),
        (0x0700, 0xD24C, "sprites-all"),
        (0x0700, 0xD250, "sprites-keep0"),
        (0x0200, 0xD2BE, "fill"),
        (0x0200, 0xD261, "erase1"),
        (0x0200, 0xD263, "erase0"),
        (0x0200, 0xD266, "erase-both"),
        (0x0200, 0xD2EC, "drain-empty"),
        (0x0200, 0xFD82, "code52"),
        (0x0400, 0xD168, "ld168-same"),
        (0x0400, 0xD174, "ld174-same"),
        (0x0400, 0xD158, "ld158"),
        (0x0400, 0xD15C, "ld15c-same"),
        (0x0400, 0xCF05, "lcf05"),
    ];
    for (stub, target, id) in cases {
        let total = stub_total_cycles(&raw, *stub, *target, |g| {
            g.set_pad(0, 0);
            match *id {
                "mmc1" => g.set_cpu(0x0F, 0x34, 0x12, 0xFD, *stub, 0x20 | 0x01),
                "mmc1-to0" => {
                    g.set_cpu(0xA5, 0x34, 0x12, 0xFD, *stub, 0x20 | 0x01);
                    g.ram[0x769] = 0x03;
                }
                "mmc1-saved" => {
                    g.set_cpu(0x5A, 0x34, 0x12, 0xFD, *stub, 0x20 | 0x01);
                    g.ram[0x769] = 0x03;
                }
                "capture" | "input-agree" => {
                    g.ram[0xF5] = 0xCC;
                    g.ram[0xF6] = 0x33;
                    g.ram[0xF7] = 0x0F;
                    g.ram[0xF8] = 0xF0;
                    g.set_cpu(0x01, 0x02, 0x03, 0xFD, *stub, 0x24);
                }
                "reset-ranges" => {
                    salt_ram(g);
                    g.set_cpu(0x42, 0x13, 0x71, 0xFD, *stub, 0x20 | 0x40);
                }
                "clear-0300" => {
                    salt_ram(g);
                    g.set_cpu(0x42, 0x13, 0x71, 0xFD, *stub, 0x20 | 0x40);
                }
                "sprites-all" | "sprites-keep0" => {
                    salt_ram(g);
                    g.set_cpu(0x42, 0x13, 0x71, 0xFD, *stub, 0x20 | 0x40);
                }
                "fill" => g.set_cpu(0x24, 0x07, 0x09, 0xFD, *stub, 0x20),
                "erase1" | "erase0" | "erase-both" => {
                    g.set_cpu(0x33, 0x07, 0x09, 0xFD, *stub, 0x20);
                    g.ram[0x73D] = 0x41;
                }
                "drain-empty" => {
                    g.set_cpu(0x99, 0x12, 0x34, 0xFD, *stub, 0x20);
                    g.ram[0x000] = 0x00;
                    g.ram[0x001] = 0x04;
                    g.ram[0x400] = 0xFF;
                }
                "code52" => {
                    g.ram[0x720] = 0x02;
                    g.ram[0x71E] = 0x1B;
                    g.ram[0x71D] = 0x24;
                    for i in 0..7usize {
                        g.ram[0x471 + i] = 0x10 + i as u8;
                    }
                    g.ram[0x7AE] = 0x01;
                    g.set_cpu(0x00, 0x00, 0x07, 0xFD, *stub, 0x20);
                }
                "ld168-same" => {
                    g.ram[0x736] = 0x02;
                    g.ram[0x737] = 0x02;
                    g.set_cpu(0xAA, 0xBB, 0xCC, 0xFD, *stub, 0x20);
                }
                "ld174-same" => {
                    g.ram[0x76C] = 0x01;
                    g.ram[0x76D] = 0x01;
                    g.set_cpu(0xAA, 0xBB, 0xCC, 0xFD, *stub, 0x20);
                }
                "ld158" => g.set_cpu(0x05, 0, 0, 0xFD, *stub, 0x20),
                "ld15c-same" => {
                    g.ram[0x738] = 0x04;
                    g.ram[0x739] = 0x04;
                    g.set_cpu(0xAA, 0xBB, 0xCC, 0xFD, *stub, 0x20);
                }
                "lcf05" => g.set_cpu(0, 0, 0, 0xFD, *stub, 0x20),
                _ => g.set_cpu(0, 0, 0, 0xFD, *stub, 0x20),
            }
        });
        let body = total.saturating_sub(12);
        let table = default_trap_cycles(*target);
        assert_eq!(
            body, table,
            "${target:04X} ({id}): measured body {body} != table {table}"
        );
    }
}

// ------------------------------------------------- ROM-gated: frame pacing

fn full_game(raw: &[u8]) -> Game {
    let mut g = Game::from_ines(raw).expect("ROM");
    g.reset();
    z2_core::bank7_traps::register_bank7_traps(&mut g);
    z2_core::sideview_traps::register_sideview_traps(&mut g);
    z2_core::sideview_traps::register_overworld_traps(&mut g);
    z2_core::player_traps::register_player_traps(&mut g);
    z2_core::enemy_traps::register_enemy_traps(&mut g);
    z2_core::town_traps::register_town_traps(&mut g);
    z2_core::palace_traps::register_palace_traps(&mut g);
    z2_core::title_traps::register_title_traps(&mut g);
    g
}

#[test]
fn trapped_frames_hold_ntsc_pacing_without_overshoot() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    let mut g = full_game(&raw);
    // Power-on frame is short (FIRST_FRAME_END); frames 1+ are full NTSC.
    // No frame may overshoot by a whole routine (pre-fix frame 3 ran 39k).
    for f in 0..6 {
        let c0 = g.cpu.cycles;
        g.step(0);
        let delta = g.cpu.cycles - c0;
        let expect = if f == 0 { 27_274 } else { 29_781 };
        assert!(
            delta.abs_diff(expect) <= 3,
            "frame {f}: delta {delta} vs expect {expect} (atomic overshoot?)"
        );
    }
    assert!(!g.trap_log().is_empty(), "boot must fire traps");
}

#[test]
fn trapped_boot_matches_untrapped_through_frame_3_state() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    // The f=3 `$0000` collapse: trapped completed D281 a frame early
    // ($00=00, $0302=$FF) while the oracle/untrapped was mid-clear
    // ($00=02, $0302=00). With charging + splitting both agree.
    let mut t = full_game(&raw);
    let mut u = Game::from_ines(&raw).expect("ROM");
    t.reset();
    u.reset();
    for _ in 0..4 {
        t.step(0);
        u.step(0);
    }
    // Frame-3 end state (after 4 steps: frames 0..3): zeropage pointer and
    // queue terminators must agree — the exact bytes the live divergence
    // compared (`$0000`, `$0302`).
    assert_eq!(t.ram[0x000], u.ram[0x000], "$00 must match at f3 end");
    assert_eq!(t.ram[0x302], u.ram[0x302], "$0302 must match at f3 end");
}
