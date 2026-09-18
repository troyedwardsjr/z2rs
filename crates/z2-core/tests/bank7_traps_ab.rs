//! Bank-7 A/B tests: trapped Rust vs untrapped ASM, bit-identical state.
//!
//! Each test builds two [`Game`]s from the same ROM bytes (gated on
//! `Z2_ROM`, skipped gracefully without it), registers the full bank-7
//! trap set on one side, then runs the same `JSR target : RTS` stub (poked
//! into RAM, executed via `try_call_asm`) on both. State compared: RAM,
//! WRAM, OAM, full CPU registers, MMC1, and PPU/APU façade traffic.
//!
//! The stub lives at `$0200` unless the routine under test touches that
//! page (noted per test); stub bytes are identical on both sides, so they
//! cannot mask a divergence.

#![cfg(feature = "interp")]

mod common;

use z2_core::bank7_traps::{register_bank7_traps, BANK7_TRAP_COUNT};
use z2_core::game::Game;

/// Caller stub: `JSR target : RTS`.
fn stub_for(target: u16) -> [u8; 4] {
    [0x20, target as u8, (target >> 8) as u8, 0x60]
}

/// Load the ROM bytes, or `None` (skip) without `Z2_ROM`.
fn load_rom() -> Option<Vec<u8>> {
    common::rom_bytes("bank7_traps_ab")
}

/// `(trapped, plain)` pair from the same image.
fn pair(raw: &[u8]) -> (Game, Game) {
    let mut trapped = Game::from_ines(raw).expect("Z2_ROM must be MMC1 iNES");
    let plain = Game::from_ines(raw).expect("Z2_ROM must be MMC1 iNES");
    register_bank7_traps(&mut trapped);
    assert_eq!(
        trapped.traps.len(),
        BANK7_TRAP_COUNT,
        "trap set must install {BANK7_TRAP_COUNT} traps"
    );
    (trapped, plain)
}

/// Poke `JSR target : RTS` at `stub`, then run to `RTS`.
///
/// Registers come from the test's own `set_cpu` (this helper only pokes the
/// stub bytes); `try_call_asm(stub, ..)` starts `PC` at the stub and halts
/// when the final `RTS` restores the entry stack level.
fn call_via_stub(game: &mut Game, stub: u16, target: u16) {
    let blob = stub_for(target);
    for (i, &b) in blob.iter().enumerate() {
        game.ram[((stub as usize) + i) & 0x7FF] = b;
    }
    game.try_call_asm(stub, 1_000_000)
        .unwrap_or_else(|e| panic!("call ${target:04X} via ${stub:04X}: {e}"));
}

/// Assert full observable state matches (RAM/WRAM/OAM/regs/MMC1/bus traffic).
fn assert_same_state(a: &Game, b: &Game, what: &str) {
    assert_eq!(a.ram, b.ram, "{what}: ram");
    assert_eq!(a.wram, b.wram, "{what}: wram");
    assert_eq!(a.oam, b.oam, "{what}: oam");
    assert_eq!(a.cpu_state(), b.cpu_state(), "{what}: cpu");
    assert_eq!(a.mmc1, b.mmc1, "{what}: mmc1");
    assert_eq!(
        (a.ppu.reads, a.ppu.writes, a.ppu.last_write),
        (b.ppu.reads, b.ppu.writes, b.ppu.last_write),
        "{what}: ppu traffic"
    );
    assert_eq!(
        (a.apu.reads, a.apu.writes, a.apu.last_write),
        (b.apu.reads, b.apu.writes, b.apu.last_write),
        "{what}: apu traffic"
    );
}

fn setup<F: Fn(&mut Game)>(g: &mut Game, f: F) {
    f(g);
}

// ------------------------------------------------- MMC1 ($FF9D/$FFB1/$FFC5/$FFC9/$FFCC)

#[test]
fn ab_configure_mmc1_and_swaps() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    // (addr, A-setup): cover bit corners incl. reset-bit flows.
    for (addr, aval) in [
        (0xFF9Du16, 0x0Fu8),
        (0xFF9D, 0x00),
        (0xFF9D, 0x1F),
        (0xFFB1, 0x00),
        (0xFFB1, 0x07),
        (0xFFCC, 0x00),
        (0xFFCC, 0x07),
        (0xFFC5, 0xA5), // A ignored (LDA #0 inside).
        (0xFFC9, 0x5A), // A ignored (LDA $0769 inside).
    ] {
        let (mut t, mut u) = pair(&raw);
        for g in [&mut t, &mut u] {
            setup(g, |g| {
                g.set_cpu(aval, 0x34, 0x12, 0xFD, 0x0200, 0x20 | 0x01);
                g.ram[0x769] = 0x03; // SwapToSavedPRG source.
            });
            call_via_stub(g, 0x0200, addr);
        }
        assert_same_state(&t, &u, &format!("mmc1 ${addr:04X} A=${aval:02X}"));
        assert_eq!(t.trap_log().len(), 1, "one trap must fire");
    }
}

// ------------------------------------------------- input ($D346/$D367)

#[test]
fn ab_controller_capture_and_input() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    for pads in [
        (0x00u8, 0x00u8),
        (0xFF, 0x00),
        (0x00, 0xFF),
        (0xA5, 0x5A),
        (z2_core::game::BTN_A | z2_core::game::BTN_START, 0),
        (0xFF, 0xFF),
    ] {
        // Capture ($D367).
        let (mut t, mut u) = pair(&raw);
        for g in [&mut t, &mut u] {
            setup(g, |g| {
                g.set_pad(pads.0, pads.1);
                g.ram[0xF5] = 0xCC; // stale: fully overwritten by 8 ROLs.
                g.ram[0xF6] = 0x33;
                g.set_cpu(0x77, 0x00, 0x44, 0xFD, 0x0200, 0x20);
            });
            call_via_stub(g, 0x0200, 0xD367);
        }
        assert_same_state(&t, &u, &format!("capture pads={pads:02X?}"));
        // Assembled order is MSB-first (A at bit 7): pad byte reversed.
        assert_eq!(t.ram[0xF5], pads.0.reverse_bits());
        assert_eq!(t.ram[0xF6], pads.1.reverse_bits());
        // Debounced input ($D346), incl. held/pressed split across frames.
        let (mut t, mut u) = pair(&raw);
        for g in [&mut t, &mut u] {
            setup(g, |g| {
                g.set_pad(pads.0, pads.1);
                g.ram[0xF7] = 0x0F; // stale held: edge = new & ~old.
                g.ram[0xF8] = 0xF0;
                g.set_cpu(0x01, 0x02, 0x03, 0xFD, 0x0200, 0x24);
            });
            call_via_stub(g, 0x0200, 0xD346);
        }
        assert_same_state(&t, &u, &format!("input pads={pads:02X?}"));
        // Pressed edges = new & ~old (on the reversed-bit bytes).
        let new1 = pads.0.reverse_bits();
        assert_eq!(t.ram[0xF5], new1 & !0x0F);
        assert_eq!(t.ram[0xF7], new1);
    }
}

// ------------------------------------------------- mem clears + sprites ($D281/$D29C/$D24C/$D250)

#[test]
fn ab_memory_ranges_and_sprites() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    for addr in [0xD281u16, 0xD29C, 0xD24C, 0xD250] {
        let (mut t, mut u) = pair(&raw);
        // Stub at $0700 for the sprite routines ($0200 is their target);
        // $D281/$D29C clear $0700, so those use $0200 (untouched by them).
        let stub = if addr == 0xD24C || addr == 0xD250 {
            0x0700
        } else {
            0x0200
        };
        for g in [&mut t, &mut u] {
            setup(g, |g| {
                for (i, b) in g.ram.iter_mut().enumerate() {
                    *b = (i.wrapping_mul(37).wrapping_add(11)) as u8;
                }
                g.set_cpu(0x42, 0x13, 0x71, 0xFD, stub, 0x20 | 0x40);
            });
            call_via_stub(g, stub, addr);
        }
        assert_same_state(&t, &u, &format!("mem ${addr:04X}"));
        if addr == 0xD24C {
            // Every 4th byte of $0200 is a sprite-Y slot -> $F8; the salt
            // between slots is preserved.
            assert_eq!(t.ram[0x200], 0xF8);
            assert_eq!(t.ram[0x204], 0xF8);
            assert_eq!(t.ram[0x2FC], 0xF8);
            let salt1 = 0x201usize.wrapping_mul(37).wrapping_add(11) as u8;
            assert_eq!(t.ram[0x201], salt1);
        }
        if addr == 0xD250 {
            // Sprite 0's Y preserved (salt at $0200 is (0x200*37+11) as u8).
            let salt = 0x200usize.wrapping_mul(37).wrapping_add(11) as u8;
            assert_eq!(t.ram[0x200], salt);
            assert_eq!(t.ram[0x204], 0xF8);
            assert_eq!(t.ram[0x2FC], 0xF8);
        }
        if addr == 0xD29C {
            assert_eq!((t.ram[0x302], t.ram[0x363]), (0xFF, 0xFF));
        }
    }
}

// ------------------------------------------------- erase + fill ($D261/$D263/$D266/$D2BE)

#[test]
fn ab_erase_and_fill() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    for (addr, aval) in [
        (0xD261u16, 0x11u8),
        (0xD263, 0x22),
        (0xD266, 0x33),
        (0xD2BE, 0x24),
    ] {
        let (mut t, mut u) = pair(&raw);
        for g in [&mut t, &mut u] {
            setup(g, |g| {
                g.set_cpu(aval, 0x07, 0x09, 0xFD, 0x0200, 0x20);
                g.ram[0x73D] = 0x41;
            });
            call_via_stub(g, 0x0200, addr);
        }
        assert_same_state(&t, &u, &format!("erase ${addr:04X}"));
        if addr == 0xD266 {
            assert_eq!((t.ram[0xFC], t.ram[0xFD], t.ram[0x746]), (0, 0, 0));
            assert_eq!(t.ram[0x73D], 0x42, "$073D increments (LD27D)");
        }
    }
}

// ------------------------------------------------- PPU queue drain ($D2EC) + code52 ($FD82)

/// Craft a two-entry macro at $0400: normal entry + $4C redirect, then $FF.
fn craft_queue(g: &mut Game) {
    // Entry 1 at $0400: hi=$20 lo=$05 ctrl=$03 + 3 tiles ($03 & $3F = 3).
    let e1 = [0x20u8, 0x05, 0x03, 0x41, 0x42, 0x43];
    for (i, &b) in e1.iter().enumerate() {
        g.ram[0x400 + i] = b;
    }
    // $4C redirect at $0406 -> $0500.
    g.ram[0x406] = 0x4C;
    g.ram[0x407] = 0x00;
    g.ram[0x408] = 0x05;
    // Entry 2 at $0500: hi=$21 lo=$00 ctrl=$01 + 1 tile, then $FF.
    let e2 = [0x21u8, 0x00, 0x01, 0x77, 0xFF];
    for (i, &b) in e2.iter().enumerate() {
        g.ram[0x500 + i] = b;
    }
    g.ram[0xFF] = 0xB0; // sprite-bank mirror (drain merges bit 2).
    g.ram[0x000] = 0x00;
    g.ram[0x001] = 0x04; // pointer -> $0400.
}

#[test]
fn ab_ppu_queue_drain() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    let (mut t, mut u) = pair(&raw);
    for g in [&mut t, &mut u] {
        setup(g, |g| {
            craft_queue(g);
            g.set_cpu(0x99, 0x12, 0x34, 0xFD, 0x0200, 0x20);
        });
        call_via_stub(g, 0x0200, 0xD2EC);
    }
    assert_same_state(&t, &u, "LD2EC drain");
    // Pointer advanced past both entries to the $FF terminator ($0504).
    assert_eq!((t.ram[0], t.ram[1]), (0x04, 0x05));
    // Entry 1: $2006 x2 + $2000 + $2007 x3 = 6; entry 2: 2 + 1 + 1 = 4.
    assert_eq!(t.ppu.writes, 10, "drain must emit exactly 10 PPU writes");
}

#[test]
fn ab_sideview_palette_loader() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    let (mut t, mut u) = pair(&raw);
    for g in [&mut t, &mut u] {
        setup(g, |g| {
            g.ram[0x720] = 0x02;
            g.ram[0x71E] = 0x1B;
            g.ram[0x71D] = 0x24;
            for i in 0..7usize {
                g.ram[0x471 + i] = 0x10 + i as u8;
            }
            g.ram[0x7AE] = 0x01;
            g.set_cpu(0x00, 0x00, 0x07, 0xFD, 0x0200, 0x20);
        });
        call_via_stub(g, 0x0200, 0xFD82);
    }
    assert_same_state(&t, &u, "code52 palette");
    assert_eq!(&t.ram[0x471..0x478], &[0; 7], "rows zeroed after upload");
    assert_eq!(t.ram[0x3A3], 0x07, "entry Y stored");
    assert_eq!(t.ram[0x7AE], 0x00, "$07AE cleared");
}

// ------------------------------------------------- detectors ($D168/$D174/$D158/$D15C)

#[test]
fn ab_change_detectors() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    // (addr, setup): changed + unchanged arms for the Gore detectors.
    let cases: [(u16, [u8; 4], &str); 6] = [
        (0xD168, [0x02, 0x02, 0, 0], "LD168 same"),
        (0xD168, [0x03, 0x02, 0, 0], "LD168 changed"),
        (0xD174, [0x01, 0x01, 0, 0], "LD174 same"),
        (0xD174, [0x02, 0x01, 0, 0], "LD174 changed"),
        (0xD158, [0x05, 0, 0, 0], "LD158 store"),
        (0xD15C, [0x04, 0x09, 0, 0], "LD15C changed"),
    ];
    for (addr, vals, what) in cases {
        let (mut t, mut u) = pair(&raw);
        for g in [&mut t, &mut u] {
            setup(g, |g| {
                match addr {
                    0xD168 => {
                        g.ram[0x736] = vals[0];
                        g.ram[0x737] = vals[1];
                    }
                    0xD174 => {
                        g.ram[0x76C] = vals[0];
                        g.ram[0x76D] = vals[1];
                    }
                    0xD158 => {
                        g.set_cpu(vals[0], 0, 0, 0xFD, 0x0400, 0x20);
                    }
                    _ => {
                        g.ram[0x738] = vals[0];
                        g.ram[0x739] = vals[1];
                    }
                }
                if addr != 0xD158 {
                    g.set_cpu(0xAA, 0xBB, 0xCC, 0xFD, 0x0400, 0x20);
                }
            });
            call_via_stub(g, 0x0400, addr);
        }
        assert_same_state(&t, &u, what);
    }
    // LD174-changed side effects (checked on the trapped side).
    let (mut t, _) = pair(&raw);
    t.ram[0x76C] = 0x02;
    t.ram[0x76D] = 0x01;
    t.set_cpu(0, 0, 0, 0xFD, 0x0400, 0x20);
    call_via_stub(&mut t, 0x0400, 0xD174);
    assert_eq!(t.ram[0x736], 0x00, "stage change resets Game Mode");
    assert_eq!(t.ram[0x73D], 0x00, "stage change resets Routine Index");
}

// ------------------------------------------------- multi-frame determinism
//
// NOTE (pacing): traps model state, not cycles (precedent: `rng_advance` in
// `game.rs`), so a trapped run executes more routines per fixed-cycle frame
// than pure interpretation and the two do NOT stay at the same code point.
// Cross-comparing them frame-by-frame would punish the trap design, not the
// ports. This test asserts what traps must guarantee instead: identical
// trapped replays are bit-identical, frames advance crash-free, and boot
// fires bank-7 traps.

#[test]
fn trapped_multiframe_is_deterministic() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    let mut t1 = Game::from_ines(&raw).expect("Z2_ROM must be MMC1 iNES");
    let mut t2 = Game::from_ines(&raw).expect("Z2_ROM must be MMC1 iNES");
    register_bank7_traps(&mut t1);
    register_bank7_traps(&mut t2);
    t1.reset();
    t2.reset();
    for _ in 0..6u64 {
        t1.step(0);
        t2.step(0);
    }
    assert_eq!(t1.frame_count(), 6);
    assert_eq!(t2.frame_count(), 6);
    assert_same_state(&t1, &t2, "two trapped 6-frame replays");
    assert!(
        !t1.trap_log().is_empty(),
        "boot must fire bank-7 traps (Reset_Memory_Ranges/Controllers_Input/...)"
    );
    // Untrapped baseline still runs crash-free (own pacing, own phases).
    let mut u = Game::from_ines(&raw).expect("Z2_ROM must be MMC1 iNES");
    u.reset();
    for _ in 0..6u64 {
        u.step(0);
    }
    assert_eq!(u.frame_count(), 6);
}
