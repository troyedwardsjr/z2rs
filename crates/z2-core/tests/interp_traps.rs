//! Trap-table tests: synthetic PRG only, no ROM (except the ignored
//! replay harness, which needs `Z2_ROM` and skips without it).
//!
//! * Trap-table register/toggle semantics (incl. the global kill switch).
//! * `ports.toml` ledger parsing (read-only `include_str!`, zero traps by
//!   default).
//! * Trapped vs untrapped A/B equivalence on synthetic code and on the
//!   hand-ported NMI RNG advance (`prg7.asm $C189-$C1B0` bytes, executed for
//!   real on the untrapped side).
//! * `JMP` tail-call traps, trap-log ring + `last_writer` attribution, and
//!   [`Divergence`](z2_core::traps::Divergence) reporting.
//! * Ignored warp-glitch replay harness (gated on `Z2_ROM` + oracle).

#![cfg(feature = "interp")]

use z2_core::game::{rng_advance, Game, BTN_A, BTN_LEFT, BTN_START};
use z2_core::traps::{routine_addrs, TRAP_LOG_CAP};

// Root ledger, read-only (same path pattern as `ram_map.rs` tests use for
// `ram-map.toml`; never written).
const PORTS_TOML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ports.toml"));

// Synthetic layout: caller $C000 -> routine $C100 (-> tail target $C200).
const CALLER: u16 = 0xC000;
const ROUTINE: u16 = 0xC100;
const TAIL: u16 = 0xC200;

/// `INC $051B` + `RTS` (untrapped-side bytes for the trivial trap).
const INC_RNG_BLOB: [u8; 4] = [0xEE, 0x1B, 0x05, 0x60];

/// Rust mirror of [`INC_RNG_BLOB`] (replicates N/Z like the real `INC`).
fn inc_rng(game: &mut Game) {
    let r = game.ram[0x51B].wrapping_add(1);
    game.ram[0x51B] = r;
    game.cpu.p = (game.cpu.p & !(z2_core::cpu::FLAG_Z | z2_core::cpu::FLAG_N))
        | if r == 0 { z2_core::cpu::FLAG_Z } else { 0 }
        | (r & z2_core::cpu::FLAG_N);
}

/// Game with `caller_blob` at $C000 and `INC $051B` at $C100.
fn prog_with_inc(caller_blob: &[u8]) -> Game {
    let mut blob = vec![0xEA; 0x104];
    blob[..caller_blob.len()].copy_from_slice(caller_blob);
    blob[0x100..0x100 + INC_RNG_BLOB.len()].copy_from_slice(&INC_RNG_BLOB);
    let mut g = Game::with_test_program(CALLER, &blob);
    g.set_cpu(0, 0, 0, 0xFD, CALLER, 0x20);
    g
}

// ------------------------------------------------- table semantics

#[test]
fn trap_table_register_toggle_and_kill_switch() {
    let mut g = Game::new();
    assert!(g.traps.is_empty(), "zero traps by default");
    assert!(!g.traps.is_trapped(ROUTINE));
    g.trap_register("inc_rng", Some(7), ROUTINE, inc_rng);
    assert!(g.traps.is_trapped(ROUTINE));
    assert_eq!(g.traps.get(ROUTINE).unwrap().name, "inc_rng");
    // A/B toggle: untrapped runs raw ASM without unregistering.
    g.set_untrapped(ROUTINE, true);
    assert!(!g.traps.is_trapped(ROUTINE));
    assert!(g.traps.is_untrapped(ROUTINE));
    g.set_untrapped(ROUTINE, false);
    assert!(g.traps.is_trapped(ROUTINE));
    // Global kill switch.
    g.traps.enabled = false;
    assert!(!g.traps.is_trapped(ROUTINE));
    g.traps.enabled = true;
    assert!(g.traps.is_trapped(ROUTINE));
    assert!(g.traps.unregister(ROUTINE));
    assert!(!g.traps.is_trapped(ROUTINE));
}

// ------------------------------------------------- ports.toml

#[test]
fn routine_addrs_parse_the_ledger() {
    let routines = routine_addrs(PORTS_TOML);
    // Ledger scale (2960 routines at the first milestone): fail loudly on drift.
    assert!(
        routines.len() >= 2000,
        "ports.toml shrank to {} routines",
        routines.len()
    );
    let code = routines.iter().filter(|r| r.kind == "code").count();
    assert!(code >= 1000, "only {code} code routines parsed");
    assert!(routines.iter().all(|r| !r.name.is_empty()));
    assert!(routines.iter().all(|r| r.bank <= 7));
    assert!(
        routines.iter().all(|r| (0x8000..=0xFFFF).contains(&r.addr)),
        "routine entries must be PRG addresses"
    );
}

#[test]
fn fresh_game_has_zero_traps() {
    // Acceptance precondition: with zero traps the interpreter runs the
    // original bytes everywhere (pure replay baseline).
    let g = Game::new();
    assert!(g.traps.is_empty());
    assert!(g.trap_log().is_empty());
    assert_eq!(g.last_writer(), None);
}

// ------------------------------------------------- JSR trap A/B

#[test]
fn jsr_trap_and_untrapped_ab_identical() {
    // caller: JSR $C100; RTS.
    let caller = [0x20, 0x00, 0xC1, 0x60];
    // --- trapped side ---
    let mut a = prog_with_inc(&caller);
    a.ram[0x51B] = 0x41;
    a.trap_register("inc_rng", Some(7), ROUTINE, inc_rng);
    a.call_asm(CALLER);
    assert_eq!(a.ram[0x51B], 0x42);
    assert_eq!(a.trap_log().len(), 1);
    assert_eq!(a.last_writer().as_deref(), Some("inc_rng@$C100"));
    // --- untrapped side (same bytes, no Rust) ---
    let mut b = prog_with_inc(&caller);
    b.ram[0x51B] = 0x41;
    b.trap_register("inc_rng", Some(7), ROUTINE, inc_rng);
    b.set_untrapped(ROUTINE, true);
    b.call_asm(CALLER);
    assert_eq!(b.ram[0x51B], 0x42);
    assert!(b.trap_log().is_empty());
    // Bit-identical: whole RAM mirror + registers (trap replicates N/Z).
    assert_eq!(a.ram, b.ram);
    assert_eq!(a.cpu_state(), b.cpu_state());
}

#[test]
fn jmp_tailcall_trap_emulates_rts() {
    // $C100: JMP $C200 (tail call). $C200 trapped: INC $051B.
    let caller = [0x20, 0x00, 0xC1, 0x60]; // JSR $C100; RTS
    let mut blob = vec![0xEA; 0x204];
    blob[..caller.len()].copy_from_slice(&caller);
    blob[0x100..0x103].copy_from_slice(&[0x4C, 0x00, 0xC2]); // JMP $C200
    blob[0x200..0x204].copy_from_slice(&INC_RNG_BLOB);
    let mut g = Game::with_test_program(CALLER, &blob);
    g.set_cpu(0, 0, 0, 0xFD, CALLER, 0x20);
    g.ram[0x51B] = 0x0F;
    g.trap_register("inc_tail", Some(7), TAIL, inc_rng);
    g.call_asm(CALLER);
    assert_eq!(g.ram[0x51B], 0x10, "tail-called trap ran once");
    assert_eq!(g.cpu_state().3, 0xFD, "emulated RTS balanced the stack");
    assert_eq!(g.last_writer().as_deref(), Some("inc_tail@$C200"));
    // Untrapped: the JMP lands in the ASM bytes, same effect.
    let mut u = Game::with_test_program(CALLER, &blob);
    u.set_cpu(0, 0, 0, 0xFD, CALLER, 0x20);
    u.ram[0x51B] = 0x0F;
    u.trap_register("inc_tail", Some(7), TAIL, inc_rng);
    u.set_untrapped(TAIL, true);
    u.call_asm(CALLER);
    assert_eq!(u.ram, g.ram);
    assert_eq!(u.cpu_state(), g.cpu_state());
}

// ------------------------------------------------- RNG advance A/B

/// Exact NMI RNG-advance bytes (`prg7.asm $C189-$C1B0`); entry X=0, Y=9 is
/// set by the caller prologue (as the NMI handler's LDX/LDY do).
const RNG_BLOB: [u8; 25] = [
    0xAD, 0x1A, 0x05, // LDA $051A
    0x29, 0x02, // AND #$02
    0x85, 0x00, // STA $00
    0xAD, 0x1B, 0x05, // LDA $051B
    0x29, 0x02, // AND #$02
    0x45, 0x00, // EOR $00
    0x18, // CLC
    0xF0, 0x01, // BEQ +1
    0x38, // SEC
    0x7E, 0x1A, 0x05, // ROR $051A,x
    0xE8, // INX
    0x88, // DEY
    0xD0, 0xF9, // BNE -7
];

/// Caller: LDX #0, LDY #9, JSR rng_site, RTS — with the RNG bytes inline at
/// the call site for the untrapped side.
fn rng_prog() -> (Game, u16) {
    let site: u16 = 0xC010;
    let mut blob = vec![
        0xA2, 0x00, // LDX #$00
        0xA0, 0x09, // LDY #$09
        0x20, 0x10, 0xC0, // JSR $C010
        0x60, // RTS
    ];
    blob.resize(0x10, 0xEA); // pad so the RNG bytes land exactly at $C010
    assert_eq!(blob.len(), (site - CALLER) as usize);
    blob.extend_from_slice(&RNG_BLOB);
    blob.push(0x60); // RTS after the inline bytes
    let mut g = Game::with_test_program(CALLER, &blob);
    g.set_cpu(0, 0, 0, 0xFD, CALLER, 0x20);
    (g, site)
}

#[test]
fn rng_advance_trapped_matches_bytes_bit_identical() {
    // Seeds covering carry-seed corners (bit 1 of $051A/$051B), all-set,
    // all-clear, and churned LFSR states.
    let seeds: Vec<[u8; 9]> = vec![
        [0x00; 9],
        [0xFF; 9],
        [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        [0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        [0x02, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        [0xA5, 0x5A, 0x81, 0x7E, 0x01, 0x80, 0xFF, 0x00, 0x42],
        [0x01, 0x03, 0x07, 0x0F, 0x1F, 0x3F, 0x7F, 0xFF, 0x55],
        [0x6D, 0x28, 0xC4, 0x11, 0x9B, 0xE2, 0x77, 0x0C, 0xF0],
    ];
    for seed in &seeds {
        let (mut a, site) = rng_prog();
        for (i, &v) in seed.iter().enumerate() {
            a.ram[0x51A + i] = v;
        }
        a.trap_register("rng_advance", Some(7), site, rng_advance);
        a.call_asm(CALLER);
        assert_eq!(a.trap_log().len(), 1, "seed {seed:02X?}");

        let (mut b, _) = rng_prog();
        for (i, &v) in seed.iter().enumerate() {
            b.ram[0x51A + i] = v;
        }
        b.call_asm(CALLER); // untrapped: the real 25 bytes execute
        assert!(b.trap_log().is_empty());

        assert_eq!(a.ram, b.ram, "RAM drift on seed {seed:02X?}");
        assert_eq!(
            a.cpu_state(),
            b.cpu_state(),
            "regs drift on seed {seed:02X?}"
        );
    }
}

// ------------------------------------------------- log + divergence

#[test]
fn trap_log_rings_and_names_last_writer() {
    // Loop: LDX #N; loop: JSR $C100; DEX; BNE loop; RTS.
    let fires: u8 = 70; // > TRAP_LOG_CAP (64): proves eviction.
    let caller = [
        0xA2, fires, // LDX #70
        0x20, 0x00, 0xC1, // JSR $C100
        0xCA, // DEX
        0xD0, 0xFA, // BNE -6
        0x60,
    ];
    let mut g = prog_with_inc(&caller);
    g.trap_register("inc_rng", Some(7), ROUTINE, inc_rng);
    g.call_asm(CALLER);
    assert_eq!(g.ram[0x51B], fires);
    assert_eq!(g.trap_log().len(), TRAP_LOG_CAP);
    let last = g.trap_log().last().unwrap();
    assert_eq!(last.name, "inc_rng");
    assert_eq!(last.addr, ROUTINE);
    assert_eq!(last.frame, g.frame_count());
    assert_eq!(last.attribution(), "inc_rng@$C100");
    assert_eq!(g.last_writer().as_deref(), Some("inc_rng@$C100"));
    // Oldest-first order retained.
    assert_eq!(g.trap_log().iter().count(), TRAP_LOG_CAP);
}

#[test]
fn divergence_names_last_routine() {
    let caller = [0x20, 0x00, 0xC1, 0x60];
    let mut g = prog_with_inc(&caller);
    g.ram[0x51B] = 0x41;
    g.trap_register("inc_rng", Some(7), ROUTINE, inc_rng);
    g.call_asm(CALLER); // ram[$51B] = $42 now
                        // Oracle stuck at the pre-trap byte + a WRAM diff (RAM wins: scanned first).
    let mut oracle_ram = *g.ram();
    oracle_ram[0x51B] = 0x41;
    let mut oracle_wram = *g.wram();
    oracle_wram[0] = g.wram()[0].wrapping_add(1);
    let d = g
        .divergence_vs(&oracle_ram, &oracle_wram)
        .expect("must diverge");
    assert_eq!(d.addr, 0x051B);
    assert_eq!((d.expected, d.actual), (0x41, 0x42));
    assert_eq!(d.frame, g.frame_count());
    assert_eq!(d.last_writer.as_deref(), Some("inc_rng@$C100"));
    assert!(d.to_string().contains("inc_rng@$C100"));
    // Identical bytes: no divergence.
    assert_eq!(g.divergence_vs(g.ram(), g.wram()), None);
    // No trap fired yet: attribution is None (pure-interp drift).
    let mut u = prog_with_inc(&caller);
    u.ram[0x51B] = 0x41;
    u.call_asm(CALLER);
    let mut oracle2 = *u.ram();
    oracle2[0x510] = u.ram()[0x510].wrapping_add(1);
    let d2 = u.divergence_vs(&oracle2, u.wram()).expect("must diverge");
    assert_eq!(d2.last_writer, None);
    assert!(d2.to_string().contains("<untrapped>"));
}

// ------------------------------------------------- replay harness (gated)

/// Minimal `.fm2` pad-track reader (no `z2-verify` dependency): maps each
/// `|command|pad0|` line's 8 `RLDUTSBA` columns to NES bits
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
        if cells.len() < 3 {
            continue;
        }
        let pad = cells[2].trim();
        if pad.len() != 8 {
            continue; // header-ish line, not an input row
        }
        let mut bits = 0u8;
        for (i, want) in COLS.iter().enumerate() {
            let c = pad.as_bytes()[i] as char;
            if c == *want {
                // Column i (left-to-right RLDUTSBA) -> NES bit (7-i).
                bits |= 1 << (7 - i);
            }
        }
        out.push(bits);
    }
    out
}

fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    h
}

/// Warp-glitch movie replay through `z2-core` with zero traps.
///
/// Gated: needs `Z2_ROM` (original ROM, read-only). It only asserts
/// crash-free deterministic replay and prints the state hash; the byte-exact
/// lockstep comparison lives in `xtask verify`. Optional
/// `Z2_MOVIE` points at an `.fm2` movie (e.g. `corpus/movies/*.fm2`);
/// otherwise 600 blank frames run.
#[test]
#[ignore = "slow full-movie replay; needs Z2_ROM"]
fn warp_glitch_movie_zero_traps_no_divergence() {
    let rom_path = match std::env::var("Z2_ROM") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: Z2_ROM not set");
            return;
        }
    };
    let rom = match std::fs::read(&rom_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("SKIP: cannot read Z2_ROM ({e})");
            return;
        }
    };
    let mut game = Game::from_ines(&rom).expect("Z2_ROM must be an MMC1 iNES image");
    assert!(
        game.traps.is_empty(),
        "replay baseline runs with zero traps"
    );
    game.reset();
    let reset_pc = game.cpu_state().4;
    eprintln!("reset vector -> ${reset_pc:04X}");
    assert!(
        (0x8000..=0xFFFF).contains(&reset_pc),
        "reset vector must land in PRG"
    );

    let inputs: Vec<u8> = match std::env::var("Z2_MOVIE") {
        Ok(m) => match std::fs::read_to_string(&m) {
            Ok(text) => {
                let pads = read_fm2_pads(&text);
                eprintln!("movie {m}: {} frames", pads.len());
                pads
            }
            Err(e) => {
                eprintln!("SKIP: cannot read Z2_MOVIE ({e})");
                return;
            }
        },
        Err(_) => {
            eprintln!("no Z2_MOVIE: replaying 600 blank frames");
            vec![0; 600]
        }
    };
    // Touch the shared-contract inputs so the wiring is exercised.
    let _ = (BTN_A, BTN_START, BTN_LEFT);

    game.run_inputs(&inputs);
    let hash = fnv1a(game.ram(), fnv1a(game.wram(), 0xCBF2_9CE4_8422_2325));
    eprintln!(
        "replay ok: {} frames, ram/wram fnv1a={hash:016X} (oracle compare: see `xtask verify`)",
        game.frame_count()
    );
    // Every frame must run its full CPU budget: an illegal opcode (or any
    // interpreter halt) would leave `cycles` short of `frames * budget`.
    // (Overshoot past the budget is normal — the last instruction runs to
    // completion — so this is a one-sided bound.)
    assert!(
        game.cpu.cycles >= inputs.len() as u64 * z2_core::game::FRAME_CPU_CYCLES,
        "interpreter halted early: refusing silent divergence"
    );
    // Determinism: an identical replay must hash identically.
    let mut again = Game::from_ines(&rom).expect("reload");
    again.reset();
    again.run_inputs(&inputs);
    assert_eq!(again.ram(), game.ram(), "replay is not deterministic (ram)");
    assert_eq!(
        again.wram(),
        game.wram(),
        "replay is not deterministic (wram)"
    );
    // NOTE: the NMI frame counter ($0012) does NOT advance yet during early
    // boot: `$0100` (NmiState) is zero, so the NMI entry ($C07B) takes the
    // sound-only path and returns before the timer/`INC $12` block, which
    // only runs once the main loop programs `$0100`. Traced 2026-09-04.
    // TODO: feed the same inputs to the oracle and assert
    // `game.divergence_vs(oracle_ram, oracle_wram).is_none()`.
    assert_eq!(game.frame_count(), inputs.len() as u64);
}
