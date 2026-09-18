//! ROM-free bank-7 tests: synthetic excerpts + direct-call logic.
//!
//! Two inline NMI blocks have no `JSR` entry, so they trap nothing; instead
//! their exact bytes are embedded here (precedent: `RNG_BLOB` in
//! `interp_traps.rs`) and run against the Rust ports:
//!
//! * `$C169-$C1B0` (timers + frame counter + RNG) vs
//!   [`tick_nmi_counters`](z2_core::bank7_timers::tick_nmi_counters).
//! * `$D382-$D397` / `$D385-$D397` (trampolines) vs
//!   [`jump_routine_073d`](z2_core::bank7_dispatch::jump_routine_073d) /
//!   [`jump_indexed_table`](z2_core::bank7_dispatch::jump_indexed_table).

#![cfg(feature = "interp")]

use z2_core::game::Game;

/// Exact `$C169-$C1B0` bytes (`prg7.asm`; timers + `INC $12` + RNG prologue
/// + 9-way `ROR` loop), plus a trailing `RTS` for `call_asm` (56 + 1).
const TIMERS_BLOB: [u8; 58] = [
    0xA2, 0x0C, // LDX #$0C ($C169)
    0xCE, 0x00, 0x05, // DEC $0500
    0x10, 0x07, // BPL +7
    0xA9, 0x14, // LDA #$14
    0x8D, 0x00, 0x05, // STA $0500
    0xA2, 0x18, // LDX #$18
    0xBD, 0x01, 0x05, // LDA $0501,x ($C177)
    0xF0, 0x03, // BEQ +3
    0xDE, 0x01, 0x05, // DEC $0501,x
    0xCA, // DEX
    0x10, 0xF5, // BPL -11
    0xEE, 0x12, 0x00, // INC $12 ($C182)
    0xA2, 0x00, // LDX #$00 ($C185)
    0xA0, 0x09, // LDY #$09
    0xAD, 0x1A, 0x05, // LDA $051A ($C189)
    0x29, 0x02, // AND #$02
    0x85, 0x00, // STA $00
    0xAD, 0x1B, 0x05, // LDA $051B
    0x29, 0x02, // AND #$02
    0x45, 0x00, // EOR $00
    0x18, // CLC
    0xF0, 0x01, // BEQ +1
    0x38, // SEC
    0x7E, 0x1A, 0x05, // ROR $051A,x ($C19B)
    0xE8, // INX
    0x88, // DEY
    0xD0, 0xF9, // BNE -7 ($C1A0)
    0x60, // RTS (harness only)
];

/// Exact `$D382-$D397` bytes (`LDA $073D` + trampoline + `JMP ($0E)`).
const TRAMP_FULL: [u8; 24] = [
    0xAD, 0x3D, 0x07, // LDA $073D ($D382)
    0x0A, // ASL ($D385)
    0xA8, // TAY
    0x68, // PLA
    0x85, 0x0C, // STA $0C
    0x68, // PLA
    0x85, 0x0D, // STA $0D
    0xC8, // INY
    0xB1, 0x0C, // LDA ($0C),y
    0x85, 0x0E, // STA $0E
    0xC8, // INY
    0xB1, 0x0C, // LDA ($0C),y
    0x85, 0x0F, // STA $0F
    0x6C, 0x0E, 0x00, // JMP ($0E)
];

/// Exact `$D385-$D397` bytes (index in entry `A`).
const TRAMP_INDEXED: [u8; 21] = [
    0x0A, // ASL
    0xA8, // TAY
    0x68, // PLA
    0x85, 0x0C, // STA $0C
    0x68, // PLA
    0x85, 0x0D, // STA $0D
    0xC8, // INY
    0xB1, 0x0C, // LDA ($0C),y
    0x85, 0x0E, // STA $0E
    0xC8, // INY
    0xB1, 0x0C, // LDA ($0C),y
    0x85, 0x0F, // STA $0F
    0x6C, 0x0E, 0x00, // JMP ($0E)
];

#[test]
fn timers_rng_excerpt_matches_port() {
    // Seeds: timer reload arm, narrow/wide sweeps, RNG carry corners.
    for seed in 0..6u8 {
        // --- Rust side (direct call, same seeds + regs) ---
        let mut a = Game::new();
        seed_state(&mut a, seed);
        a.set_cpu(0x77, 0x02, 0x05, 0xFD, 0xC000, 0x20);
        // Mirror call_asm's synthetic $FFFF return so stack bytes compare.
        a.ram[0x1FC] = 0xFF;
        a.ram[0x1FD] = 0xFF;
        z2_core::bank7_timers::tick_nmi_counters(&mut a);
        // --- ASM side (synthetic PRG, same seeds) ---
        let mut b = Game::with_test_program(0xC000, &TIMERS_BLOB);
        seed_state(&mut b, seed);
        // Entry regs mirror an NMI tail (A clobbered early anyway).
        b.set_cpu(0x77, 0x02, 0x05, 0xFD, 0xC000, 0x20);
        b.call_asm(0xC000);
        assert_eq!(a.ram, b.ram, "timers ram, seed {seed}");
        assert_eq!(a.cpu_state(), b.cpu_state(), "timers regs, seed {seed}");
    }
}

fn seed_state(g: &mut Game, seed: u8) {
    g.ram[0x500] = [0x01, 0x00, 0xFF, 0x14, 0x02, 0x80][seed as usize];
    for i in 0..0x19usize {
        g.ram[0x501 + i] = (i as u8).wrapping_mul(31).wrapping_add(seed);
    }
    g.ram[0x12] = seed.wrapping_mul(0x40);
    for i in 0..9usize {
        g.ram[0x51A + i] = [0x00, 0xFF, 0x02, 0xA5, 0x5A, 0x81][(seed as usize + i) % 6];
    }
    g.ram[0x51B] = [0x00, 0x02, 0x02, 0x00, 0x02, 0xFF][seed as usize];
}

/// Synthetic layout: caller at `$C000` (`[LDA #imm :] JSR tramp : RTS`)
/// with the inline word table immediately after the `JSR` (like every real
/// `$D385` call site); the excerpt bytes live at `tramp` (`$C020`), and
/// `INC`-marker targets at `$C080`/`$C090`.
fn trampoline_prog(tramp: u16, excerpt: &[u8], table: &[u16], preset_a: Option<u8>) -> Game {
    let mut full = vec![];
    if let Some(v) = preset_a {
        full.extend_from_slice(&[0xA9, v]); // LDA #imm (index for $D385).
    }
    full.extend_from_slice(&[0x20, (tramp & 0xFF) as u8, (tramp >> 8) as u8]);
    for w in table {
        full.push(*w as u8);
        full.push((*w >> 8) as u8);
    }
    full.push(0x60); // RTS landing pad (skipped by the jump, like the ASM).
    while 0xC000 + full.len() < tramp as usize {
        full.push(0xEA);
    }
    full.extend_from_slice(excerpt);
    // Targets: INC $10 : RTS and INC $11 : RTS.
    for (i, t) in [0xC080usize, 0xC090usize].iter().enumerate() {
        let need = t - 0xC000 + 4;
        if full.len() < need {
            full.resize(need, 0xEA);
        }
        let o = t - 0xC000;
        full[o] = 0xEE;
        full[o + 1] = 0x10 + i as u8;
        full[o + 2] = 0x00;
        full[o + 3] = 0x60;
    }
    let mut g = Game::with_test_program(0xC000, &full);
    g.set_cpu(0xCC, 0xDD, 0xEE, 0xFD, 0xC000, 0x20);
    g
}

/// Compare the whole RAM mirror, dead stack bytes included.
///
/// The ASM trampoline consumes its return with `PLA/PLA` (leaving the
/// stale return bytes at `$01FA/$01FB`) and `JMP ($0E)`s to the target; the
/// trap port pops the same frame and hands the jump to the dispatcher
/// (`Game::trap_jump`), so nothing else touches the stack page and the
/// stale bytes match too.
fn assert_ram_eq(a: &Game, b: &Game, what: &str) {
    for i in 0..0x800usize {
        assert_eq!(a.ram[i], b.ram[i], "{what}: ram[${i:04X}]");
    }
}

#[test]
fn trampoline_073d_excerpt_matches_port() {
    for index in [0u8, 1, 2, 5] {
        let site = 0xC010;
        let table = [0xC080u16, 0xC090, 0xC080, 0xC090, 0xC080, 0xC090];
        // --- Rust side (trap at the excerpt site) ---
        let mut a = trampoline_prog(site, &TRAMP_FULL, &table, None);
        a.ram[0x73D] = index;
        a.trap_register(
            "d382",
            Some(7),
            site,
            z2_core::bank7_dispatch::jump_routine_073d,
        );
        a.call_asm(0xC000);
        // --- ASM side (same bytes, untrapped) ---
        let mut b = trampoline_prog(site, &TRAMP_FULL, &table, None);
        b.ram[0x73D] = index;
        b.call_asm(0xC000);
        assert_ram_eq(&a, &b, &format!("trampoline index {index}"));
        assert_eq!(
            a.cpu_state(),
            b.cpu_state(),
            "trampoline regs, index {index}"
        );
        // Target marker: even indices hit $C080 (INC $10), odd hit $C090.
        let expect = if index % 2 == 0 { 1 } else { 0 };
        assert_eq!(a.ram[0x10], expect, "index {index} must land on target 0");
        assert_eq!(a.ram[0x11], 1 - expect, "index {index} must miss target 1");
    }
}

#[test]
fn trampoline_indexed_excerpt_matches_port() {
    for index in [0u8, 1, 3] {
        let site = 0xC010;
        let table = [0xC080u16, 0xC090, 0xC080, 0xC090];
        let mut a = trampoline_prog(site, &TRAMP_INDEXED, &table, Some(index));
        a.trap_register(
            "d385",
            Some(7),
            site,
            z2_core::bank7_dispatch::jump_indexed_table,
        );
        a.call_asm(0xC000);
        let mut b = trampoline_prog(site, &TRAMP_INDEXED, &table, Some(index));
        b.call_asm(0xC000);
        assert_ram_eq(&a, &b, &format!("indexed index {index}"));
        assert_eq!(a.cpu_state(), b.cpu_state(), "indexed regs, index {index}");
    }
}
