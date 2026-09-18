//! Tail-jump (PC redirect) dispatch tests for [`Game::trap_jump`].
//!
//! A trap body that ends in a tail `JMP` asks the dispatcher to continue at
//! the target instead of emulating its `RTS`; a trapped target fires in
//! turn ([`Game::fire_trap`]). Synthetic (ROM-free) programs pin the
//! contract: identical RAM (dead stack bytes included), registers and
//! cycles against the same bytes interpreted, one trap record per fired
//! body, and a stack that only ever holds what the ROM's `JSR`s pushed.

#![cfg(feature = "interp")]

mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use z2_core::cpu::{FLAG_N, FLAG_Z};
use z2_core::game::Game;
use z2_core::traps::TrapExit;

const CALLER: u16 = 0xC000;
/// `JMP $C100` relay for the `JMP`-dispatch case.
const RELAY: u16 = 0xC010;
const A: u16 = 0xC100;
const B: u16 = 0xC200;
const C: u16 = 0xC300;
const NMI_BODY: u16 = 0xC400;
const NMI_TAIL: u16 = 0xC500;

type Trap = (&'static str, u16, fn(&mut Game), u64);

/// `INC abs` semantics for the synthetic bodies (`N`/`Z` from the result).
fn inc(game: &mut Game, addr: usize) {
    let v = game.ram[addr].wrapping_add(1);
    game.ram[addr] = v;
    game.cpu.p = (game.cpu.p & !(FLAG_Z | FLAG_N)) | if v == 0 { FLAG_Z } else { 0 } | (v & FLAG_N);
}

/// `INC $0510 : JMP $C200` as a port: the `JMP` is charged here, no `RTS`.
fn body_a(game: &mut Game) {
    inc(game, 0x510);
    game.cpu.cycles += 6 + 3;
    game.trap_jump(B);
}

/// `INC $0511 : JMP $C300`.
fn body_b(game: &mut Game) {
    inc(game, 0x511);
    game.cpu.cycles += 6 + 3;
    game.trap_jump(C);
}

/// `INC $0512 : RTS` (the `RTS` is part of the body's cost).
fn body_c(game: &mut Game) {
    inc(game, 0x512);
    game.cpu.cycles += 6 + 6;
}

/// NMI handler port: `INC $0513 : JMP $C500` (the tail holds the `RTI`).
fn body_nmi(game: &mut Game) {
    inc(game, 0x513);
    game.cpu.cycles += 6 + 3;
    game.trap_jump(NMI_TAIL);
}

const TRAP_A: Trap = ("a", A, body_a, 0);
const TRAP_B: Trap = ("b", B, body_b, 0);
const TRAP_C: Trap = ("c", C, body_c, 0);

/// Program: caller `JSR $C100 : RTS`; `$C010` `JMP $C100`; `$C100`
/// `INC $0510 : JMP $C200`; `$C200` `INC $0511 : JMP $C300`; `$C300`
/// `INC $0512 : RTS`; `$C500` `NOP : RTI`. The same bytes run untrapped
/// on the plain side.
fn program(caller_target: u16) -> Game {
    let mut blob = vec![0xEA; 0x502];
    blob[..4].copy_from_slice(&[0x20, caller_target as u8, (caller_target >> 8) as u8, 0x60]);
    blob[0x010..0x013].copy_from_slice(&[0x4C, 0x00, 0xC1]);
    blob[0x100..0x106].copy_from_slice(&[0xEE, 0x10, 0x05, 0x4C, 0x00, 0xC2]);
    blob[0x200..0x206].copy_from_slice(&[0xEE, 0x11, 0x05, 0x4C, 0x00, 0xC3]);
    blob[0x300..0x304].copy_from_slice(&[0xEE, 0x12, 0x05, 0x60]);
    blob[0x500..0x502].copy_from_slice(&[0xEA, 0x40]);
    let mut g = Game::with_test_program(CALLER, &blob);
    g.set_nmi_vector(NMI_BODY);
    g.set_cpu(0x11, 0x22, 0x33, 0xFD, CALLER, 0x24);
    // Salt the stack page so untouched slots are observable.
    for (i, b) in g.ram[0x100..0x200].iter_mut().enumerate() {
        *b = i as u8;
    }
    g
}

fn run(caller_target: u16, traps: &[Trap]) -> Game {
    let mut g = program(caller_target);
    for &(name, addr, f, cycles) in traps {
        g.trap_register_cycles(name, Some(7), addr, f, cycles);
    }
    g.call_asm(CALLER);
    g
}

fn fired(g: &Game) -> Vec<String> {
    g.trap_log().iter().map(|r| r.attribution()).collect()
}

fn assert_same(trapped: &Game, plain: &Game, what: &str) {
    assert_eq!(trapped.ram, plain.ram, "{what}: ram (dead stack included)");
    assert_eq!(trapped.cpu_state(), plain.cpu_state(), "{what}: registers");
    assert_eq!(trapped.cpu.cycles, plain.cpu.cycles, "{what}: cycles");
    assert_eq!(
        (trapped.ram[0x510], trapped.ram[0x511], trapped.ram[0x512]),
        (1, 1, 1),
        "{what}: every stage ran once"
    );
}

#[test]
fn tail_jump_into_unported_code_matches_the_bytes() {
    // A trapped, its tail `JMP $C200` continues in the ROM bytes; the RTS
    // at $C300 pops the caller's frame like hardware. JSR 6 + 9 + 9 + 12 +
    // stub RTS 6 = 42 either way.
    let t = run(A, &[TRAP_A]);
    let u = run(A, &[]);
    assert_same(&t, &u, "unported tail");
    assert_eq!(fired(&t), ["a@$C100"]);
    assert!(fired(&u).is_empty());
}

#[test]
fn tail_jump_chains_through_trapped_targets() {
    let t = run(A, &[TRAP_A, TRAP_B, TRAP_C]);
    let u = run(A, &[]);
    assert_same(&t, &u, "trapped chain");
    assert_eq!(fired(&t), ["a@$C100", "b@$C200", "c@$C300"]);
}

#[test]
fn tail_jump_chain_may_end_in_unported_code() {
    let t = run(A, &[TRAP_A, TRAP_B]);
    let u = run(A, &[]);
    assert_same(&t, &u, "chain into ROM");
    assert_eq!(fired(&t), ["a@$C100", "b@$C200"]);
}

#[test]
fn jmp_dispatch_follows_tail_jumps_too() {
    // Caller `JSR $C010`, `$C010: JMP $C100`: the JMP path fires A, whose
    // chain ends in C's emulated RTS (popping the caller's frame).
    let t = run(RELAY, &[TRAP_A, TRAP_C]);
    let u = run(RELAY, &[]);
    assert_same(&t, &u, "JMP dispatch");
    assert_eq!(fired(&t), ["a@$C100", "c@$C300"]);
}

#[test]
fn untrapped_toggle_breaks_the_chain_where_asked() {
    // B registered but running as ROM bytes: A's jump interprets B, whose
    // JMP $C300 dispatches C's trap again.
    let mut g = program(A);
    for (name, addr, f, cycles) in [TRAP_A, TRAP_B, TRAP_C] {
        g.trap_register_cycles(name, Some(7), addr, f, cycles);
    }
    g.set_untrapped(B, true);
    g.call_asm(CALLER);
    let u = run(A, &[]);
    assert_same(&g, &u, "untrapped middle");
    assert_eq!(fired(&g), ["a@$C100", "c@$C300"]);
}

#[test]
fn fire_trap_reports_how_control_leaves() {
    let mut g = program(A);
    g.trap_register_cycles("a", Some(7), A, body_a, 0);
    assert_eq!(
        g.fire_trap(A),
        TrapExit::Jump(B),
        "B unported: continue there"
    );
    assert_eq!(fired(&g), ["a@$C100"]);
    assert_eq!(g.take_trap_jump(), None, "the request was consumed");
    g.trap_register_cycles("b", Some(7), B, body_b, 0);
    g.trap_register_cycles("c", Some(7), C, body_c, 0);
    assert_eq!(g.fire_trap(A), TrapExit::Return, "chain ends in C's RTS");
    assert_eq!(fired(&g), ["a@$C100", "a@$C100", "b@$C200", "c@$C300"]);
    assert_eq!(g.fire_trap(0xC0F0), TrapExit::Return, "unregistered: no-op");
}

#[test]
fn take_trap_jump_observes_and_clears_a_direct_call() {
    let mut g = program(A);
    body_a(&mut g);
    assert_eq!(g.take_trap_jump(), Some(B));
    assert_eq!(g.take_trap_jump(), None);
}

#[test]
fn vector_trap_can_tail_jump_into_its_rti() {
    let mut g = program(A);
    g.trap_register_cycles("nmi", Some(7), NMI_BODY, body_nmi, 0);
    g.cpu.request_nmi();
    // Service: frame pushed, the port runs, its tail lands on the NOP at
    // $C500 (executed by this step) with the interrupt frame intact.
    g.step_instruction().unwrap();
    assert_eq!(g.cpu_state().4, NMI_TAIL + 1, "continues at the tail");
    assert_eq!(g.cpu_state().3, 0xFD - 3, "interrupt frame still pushed");
    assert_eq!(g.ram[0x513], 1);
    assert_eq!(fired(&g), ["nmi@$C400"]);
    // The tail's own RTI pops it.
    g.step_instruction().unwrap();
    let (_, _, _, sp, pc, p) = g.cpu_state();
    assert_eq!((sp, pc, p), (0xFD, CALLER, 0x24));
}

// ------------------------------------------------- ROM-gated: the real trampolines

/// Minimal `.fm2` pad-track reader (same mapping as `interp_traps.rs`):
/// each `|command|pad0|` line's `RLDUTSBA` columns to NES bits
/// A,B,Select,Start,Up,Down,Left,Right = 0..7.
fn read_fm2_pads(text: &str) -> Vec<u8> {
    const COLS: [char; 8] = ['R', 'L', 'D', 'U', 'T', 'S', 'B', 'A'];
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = t.split('|').collect();
        if cells.len() < 3 || cells[2].trim().len() != 8 {
            continue;
        }
        let pad = cells[2].trim().as_bytes();
        let mut bits = 0u8;
        for (i, want) in COLS.iter().enumerate() {
            if pad[i] as char == *want {
                bits |= 1 << (7 - i);
            }
        }
        out.push(bits);
    }
    out
}

/// Every fixed-bank trap group, in the verify driver's order.
fn register_all_groups(g: &mut Game) {
    z2_core::bank7_traps::register_bank7_traps(g);
    z2_core::sideview_traps::register_sideview_traps(g);
    z2_core::sideview_traps::register_overworld_traps(g);
    z2_core::player_traps::register_player_traps(g);
    z2_core::enemy_traps::register_enemy_traps(g);
    z2_core::town_traps::register_town_traps(g);
    z2_core::palace_traps::register_palace_traps(g);
    z2_core::title_traps::register_title_traps(g);
    z2_core::boot_traps::register_boot_traps(g);
}

/// A marked mode-table entry: address, counting wrapper, registered name.
type Marked = (u16, fn(&mut Game), &'static str);

/// Fire counters for the marked mode-table entries (one test uses them).
static HITS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

fn hit_lives_screen(g: &mut Game) {
    HITS[0].fetch_add(1, Ordering::Relaxed);
    z2_core::title_traps::tt_lives_screen(g);
}
fn hit_respawn(g: &mut Game) {
    HITS[1].fetch_add(1, Ordering::Relaxed);
    z2_core::title_traps::tt_respawn(g);
}
fn hit_go_outside(g: &mut Game) {
    HITS[2].fetch_add(1, Ordering::Relaxed);
    z2_core::sideview_traps::sv_go_outside(g);
}
fn hit_side_view_main(g: &mut Game) {
    HITS[3].fetch_add(1, Ordering::Relaxed);
    z2_core::player_traps::pl_death_check(g);
}

/// Mode-table entries reached only through the `$D382`/`$D385`
/// trampolines' `JMP ($0E)` (title, sideview and player groups), each
/// re-registered as a counting wrapper around its real port with the
/// port's own cost. Before the PC redirect these never fired in the
/// default configuration (the trampoline ports landed on them through a
/// pushed return, not a dispatch).
const MARKED: [Marked; 4] = [
    (
        0xC3B5,
        hit_lives_screen,
        "bank7_Load_Lives_Remaining_Screen",
    ),
    (0xC41E, hit_respawn, "bank7_code11"),
    (0xCCB3, hit_go_outside, "bank7_go_outside"),
    (
        0xD3CC,
        hit_side_view_main,
        "bank7_check_if_link_died_0494__linkdeath",
    ),
];

/// Replaying the warpless movie with every group registered reaches the
/// game proper, and the mode-table ports fire through the trampoline
/// ports' tail jumps. Needs `Z2_ROM` and the corpus (`Z2_MOVIES`, or
/// `Z2_CORPUS/movies`, holding `warpless.fm2`); skips otherwise.
#[test]
fn rom_movie_reaches_mode_table_ports_through_the_trampolines() {
    let Some(rom) = common::rom_path("rom_movie_reaches_mode_table_ports") else {
        return;
    };
    let movies = common::var_present("Z2_MOVIES")
        .or_else(|| common::var_present("Z2_CORPUS").map(|c| format!("{c}/movies")));
    let Some(movies) = movies else {
        eprintln!(
            "skipping rom_movie_reaches_mode_table_ports: neither Z2_MOVIES nor Z2_CORPUS is set"
        );
        return;
    };
    let Some(movie) = common::file_present(
        &std::path::Path::new(&movies).join("warpless.fm2"),
        "rom_movie_reaches_mode_table_ports",
    ) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&movie) else {
        eprintln!(
            "skipping rom_movie_reaches_mode_table_ports: cannot read {}",
            movie.display()
        );
        return;
    };
    let pads = read_fm2_pads(&text);
    let raw = std::fs::read(rom).expect("read Z2_ROM");
    let mut g = Game::from_ines(&raw).expect("Z2_ROM must be MMC1 iNES");
    register_all_groups(&mut g);
    for (addr, wrapper, name) in MARKED {
        let info = *g.traps.get(addr).expect("marked entry is registered");
        assert_eq!(info.name, name, "port at ${addr:04X}");
        g.traps
            .register_with_cycles(info.name, info.bank, addr, wrapper, info.cycles);
    }
    g.reset();
    let mut first_hit = None;
    for (f, &pad) in pads.iter().enumerate().take(3000) {
        g.step(pad);
        if HITS.iter().any(|h| h.load(Ordering::Relaxed) > 0) {
            first_hit = Some(f);
            break;
        }
    }
    assert_eq!(g.exec_errors, 0, "interpreter faults during the replay");
    let hits: Vec<(u16, u32)> = MARKED
        .iter()
        .zip(HITS.iter())
        .map(|((addr, _, _), h)| (*addr, h.load(Ordering::Relaxed)))
        .filter(|(_, n)| *n > 0)
        .collect();
    assert!(
        !hits.is_empty(),
        "no marked mode-table port fired in the first 3000 movie frames"
    );
    eprintln!(
        "mode-table ports fired via the trampolines by frame {}: {hits:04X?}",
        first_hit.unwrap()
    );
}
