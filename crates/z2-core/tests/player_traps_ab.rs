//! ROM-gated A/B for the player traps (`player_traps.rs`).
//!
//! Every registered shim that a `JSR` can reach is run both ways from the
//! same seeded random state: a `JSR target : RTS` stub in RAM dispatches
//! the Rust port through the trap table in one game and interprets the
//! original bytes in the other. RAM (dead stack bytes included), WRAM, OAM,
//! the CPU registers/flags and the cycle delta must agree. Tail jumps that
//! stay with the interpreter (`bank7_monster_death`, `bank7_get_item`) run
//! on both sides: the shim hands them to the dispatcher (`Game::trap_jump`),
//! the ROM bytes `JMP` there.
//!
//! The mode-table entry `$D3CC` (Side View Main) is reached by the `$D385`
//! trampoline's `JMP ($0E)`, and its shim only covers the death gate before
//! handing the rest of the mode routine back to the interpreter at `$D3E9`
//! (or `$E18A` on death); it gets its own pass from a `JMP $D3CC` stub run
//! up to that hand-off. Skips without `Z2_ROM` (see `LEGAL.md`: no ROM
//! bytes are stored in the tree).

#![cfg(feature = "interp")]

mod common;

use z2_core::cpu::{step_instruction, FLAG_B, FLAG_I, FLAG_U};
use z2_core::game::Game;
use z2_core::player_traps::{register_player_traps, PLAYER_TRAP_COUNT};

/// RAM address of the `JSR target : RTS` stub.
const STUB: u16 = 0x0700;
/// Side View Main entry (mode `$0B`, `$C302`).
const SIDE_VIEW_MAIN: u16 = 0xD3CC;
/// Where the `$D3CC` gate hands the interpreter the rest of the routine:
/// `LD3E9` (no death) or `LE18A` (death committed).
const GATE_EXITS: [u16; 2] = [0xD3E9, 0xE18A];
/// Seeds per routine.
const SEEDS: u64 = 400;
/// Instruction budget per call (the death/item tails are a few hundred).
const BUDGET: u64 = 50_000;

fn load_rom() -> Option<Vec<u8>> {
    common::rom_bytes("player_traps_ab")
}

/// Tiny deterministic generator (64-bit LCG, top bits).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u8
    }

    fn below(&mut self, n: u8) -> u8 {
        self.next() % n
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 != 0
    }
}

/// Which `X` the routine expects on entry.
#[derive(Clone, Copy)]
enum Kind {
    /// `$D19B`/`$D1CE`: Link (0), enemy slot + 1, projectile slot + 7/$0D.
    Physics,
    /// Contact routines: an enemy slot (mostly equal to `$10`).
    Contact,
    /// Box routines: `X` is irrelevant (the overlap reloads it from `$10`).
    Boxes,
}

const TARGETS: &[(u16, Kind, &str)] = &[
    (0xD19B, Kind::Physics, "bank7_applyGravityMotion"),
    (0xD1CE, Kind::Physics, "bank7_XY_Movements_Routine"),
    (0xE2EF, Kind::Contact, "bank7_Link_Hit_Routine"),
    (0xE399, Kind::Contact, "bank7_Set_Links_Recoil"),
    (0xE371, Kind::Contact, "bank7_code37"),
    (0xE558, Kind::Contact, "bank7_code39"),
    (0xE677, Kind::Contact, "bank7_Sword_Hit_Detection"),
    (0xE975, Kind::Boxes, "bank7_code43"),
    (0xE9A2, Kind::Boxes, "bank7_code44"),
    (0xE9D8, Kind::Boxes, "bank7_code45"),
    (0xE9F9, Kind::Boxes, "bank7_idem__maybe"),
    (0xEBB8, Kind::Boxes, "bank7_code47"),
];

/// Random but gameplay-shaped state; returns the entry `(A, X, Y, P)`.
fn seed(g: &mut Game, rng: &mut Lcg, kind: Kind) -> (u8, u8, u8, u8) {
    for b in g.ram.iter_mut() {
        *b = rng.next();
    }
    for b in g.wram.iter_mut() {
        *b = rng.next();
    }
    let slot = rng.below(6);
    // `$10` is what the overlap reloads into X; usually the same slot.
    g.ram[0x10] = if rng.below(4) == 0 {
        rng.below(6)
    } else {
        slot
    };
    for s in 0..6usize {
        g.ram[0xA1 + s] = rng.below(0x24);
        g.ram[0xB6 + s] = rng.below(3);
        g.ram[0x91 + s] = rng.below(0x40);
        g.ram[0x0444 + s] = rng.below(4);
        if rng.coin() {
            g.ram[0x040E + s] = 0;
        }
    }
    g.ram[0x0777] = 1 + rng.below(8);
    g.ram[0x0779] = 1 + rng.below(8);
    g.ram[0x17] = if rng.below(8) == 0 {
        rng.next()
    } else {
        rng.below(2)
    };
    g.ram[0x9F] = rng.below(3);
    g.ram[0x80] = rng.below(0x0C);
    g.ram[0xC8] = rng.below(8);
    if rng.below(3) == 0 {
        g.ram[0x0480] = 0xF8;
    }
    for addr in [0x0710usize, 0x0518, 0x070F, 0x0B, 0x00, 0x050C] {
        if rng.coin() {
            g.ram[addr] = 0;
        }
    }
    let x = match kind {
        Kind::Physics => {
            let x = match rng.below(4) {
                0 => 0,
                1 => 1 + rng.below(6),
                2 => 7 + rng.below(6),
                _ => 0x0D + rng.below(6),
            };
            // Bias the gravity clamp path (`$057D,x + carry == $02`).
            if rng.below(4) == 0 {
                g.ram[0x02] = g.ram[0x057D + usize::from(x)];
            }
            x
        }
        Kind::Contact | Kind::Boxes => slot,
    };
    let p = (rng.next() & !FLAG_B) | FLAG_U | FLAG_I;
    (rng.next(), x, rng.next(), p)
}

fn poke_stub(g: &mut Game, target: u16) {
    let blob = [0x20, target as u8, (target >> 8) as u8, 0x60];
    for (i, &b) in blob.iter().enumerate() {
        g.ram[usize::from(STUB) + i] = b;
    }
}

/// One seeded pair: `(trapped, plain)` after the call, plus the outcomes.
fn run_pair(raw: &[u8], target: u16, kind: Kind, seed_no: u64) -> (Game, Game, bool, bool) {
    let mut t = Game::from_ines(raw).expect("Z2_ROM must be MMC1 iNES");
    let mut u = Game::from_ines(raw).expect("Z2_ROM must be MMC1 iNES");
    register_player_traps(&mut t);
    assert_eq!(t.traps.len(), PLAYER_TRAP_COUNT);
    let mut rng_t = Lcg(seed_no.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(target));
    let mut rng_u = Lcg(rng_t.0);
    let regs = seed(&mut t, &mut rng_t, kind);
    let regs_u = seed(&mut u, &mut rng_u, kind);
    assert_eq!(regs, regs_u);
    poke_stub(&mut t, target);
    poke_stub(&mut u, target);
    let (a, x, y, p) = regs;
    t.set_cpu(a, x, y, 0xF0, STUB, p);
    u.set_cpu(a, x, y, 0xF0, STUB, p);
    let ok_t = t.try_call_asm(STUB, BUDGET).is_ok();
    let ok_u = u.try_call_asm(STUB, BUDGET).is_ok();
    (t, u, ok_t, ok_u)
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<(usize, u8, u8)> {
    a.iter()
        .zip(b.iter())
        .enumerate()
        .find(|(_, (x, y))| x != y)
        .map(|(i, (x, y))| (i, *x, *y))
}

#[test]
fn player_traps_match_rom_bytes_on_seeded_state() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    for &(target, kind, name) in TARGETS {
        let mut compared = 0u64;
        for seed_no in 0..SEEDS {
            let (t, u, ok_t, ok_u) = run_pair(&raw, target, kind, seed_no);
            let what = format!("{name} ${target:04X} seed {seed_no}");
            assert_eq!(ok_t, ok_u, "{what}: call outcome (trapped vs plain)");
            if !ok_u {
                continue;
            }
            if let Some((i, e, a)) = first_diff(&u.ram, &t.ram) {
                panic!("{what}: ram ${i:04X} expected {e:02X} (plain) actual {a:02X} (trapped)");
            }
            if let Some((i, e, a)) = first_diff(&u.wram, &t.wram) {
                panic!(
                    "{what}: wram ${:04X} expected {e:02X} actual {a:02X}",
                    0x6000 + i
                );
            }
            assert_eq!(u.oam, t.oam, "{what}: oam");
            assert_eq!(u.cpu_state(), t.cpu_state(), "{what}: cpu (a,x,y,sp,pc,p)");
            assert_eq!(u.cpu.cycles, t.cpu.cycles, "{what}: cycles");
            compared += 1;
        }
        assert!(
            compared > SEEDS / 2,
            "{name}: too few comparable seeds ({compared})"
        );
    }
}

/// `JMP target` stub: the way the `$D385` trampoline's `JMP ($0E)` reaches
/// a mode-table entry (a `JMP` dispatches the trap like a `JSR`, then the
/// emulated `RTS` pops what the shim pushed).
fn poke_jmp_stub(g: &mut Game, target: u16) {
    let blob = [0x4C, target as u8, (target >> 8) as u8];
    for (i, &b) in blob.iter().enumerate() {
        g.ram[usize::from(STUB) + i] = b;
    }
}

/// Step until the PC sits on one of [`GATE_EXITS`] (a handful of
/// instructions either way); `false` on a fault or when it never gets there.
fn run_to_gate_exit(g: &mut Game) -> bool {
    for _ in 0..32 {
        if GATE_EXITS.contains(&g.cpu.pc) {
            return true;
        }
        if step_instruction(g).is_err() {
            return false;
        }
    }
    false
}

/// The `$D3CC` gate both ways from a `JMP` stub, up to the hand-off. The
/// shim hands the interpreter the rest through the dispatcher's PC redirect
/// (`Game::trap_jump`), which touches nothing on the stack, so the whole
/// RAM mirror — dead stack bytes included — must agree, cycles too.
#[test]
fn side_view_main_gate_matches_rom_bytes_on_seeded_state() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    let mut paths = [0u64; 3];
    for seed_no in 0..SEEDS {
        let mut t = Game::from_ines(&raw).expect("Z2_ROM must be MMC1 iNES");
        let mut u = Game::from_ines(&raw).expect("Z2_ROM must be MMC1 iNES");
        register_player_traps(&mut t);
        let mut rng_t =
            Lcg(seed_no.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(SIDE_VIEW_MAIN));
        let mut rng_u = Lcg(rng_t.0);
        let regs = seed(&mut t, &mut rng_t, Kind::Boxes);
        let regs_u = seed(&mut u, &mut rng_u, Kind::Boxes);
        assert_eq!(regs, regs_u);
        // Kill flag off / on, injured timer running / expired: all three
        // gate paths (no kill, injured, death).
        let kill = if rng_t.coin() {
            0
        } else {
            1 + rng_t.below(0xFF)
        };
        let injured = if rng_t.coin() {
            0
        } else {
            1 + rng_t.below(0xFF)
        };
        for g in [&mut t, &mut u] {
            g.ram[0x0494] = kill;
            g.ram[0x050C] = injured;
        }
        paths[match (kill, injured) {
            (0, _) => 0,
            (_, 0) => 2,
            _ => 1,
        }] += 1;
        poke_jmp_stub(&mut t, SIDE_VIEW_MAIN);
        poke_jmp_stub(&mut u, SIDE_VIEW_MAIN);
        let (a, x, y, p) = regs;
        t.set_cpu(a, x, y, 0xF0, STUB, p);
        u.set_cpu(a, x, y, 0xF0, STUB, p);
        let what = format!("Side View Main gate ${SIDE_VIEW_MAIN:04X} seed {seed_no}");
        assert!(
            run_to_gate_exit(&mut u),
            "{what}: plain never reached the hand-off"
        );
        assert!(
            run_to_gate_exit(&mut t),
            "{what}: trapped never reached the hand-off"
        );
        assert_eq!(u.cpu_state(), t.cpu_state(), "{what}: cpu (a,x,y,sp,pc,p)");
        assert_eq!(u.cpu.cycles, t.cpu.cycles, "{what}: cycles");
        if let Some((i, e, a)) = first_diff(&u.ram, &t.ram) {
            panic!("{what}: ram ${i:04X} expected {e:02X} (plain) actual {a:02X} (trapped)");
        }
        if let Some((i, e, a)) = first_diff(&u.wram, &t.wram) {
            panic!(
                "{what}: wram ${:04X} expected {e:02X} actual {a:02X}",
                0x6000 + i
            );
        }
        assert_eq!(u.oam, t.oam, "{what}: oam");
    }
    assert!(
        paths.iter().all(|&n| n > SEEDS / 16),
        "gate paths under-sampled (no-kill/injured/death): {paths:?}"
    );
}
