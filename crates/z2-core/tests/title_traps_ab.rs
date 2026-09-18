//! ROM-gated A/B for the title traps (`title_traps.rs`).
//!
//! Every registered fixed-bank title routine is run both ways from the
//! same seeded random state: a `JSR target : RTS` stub in RAM dispatches
//! the Rust port through the trap table in one game and interprets the
//! original bytes in the other, which carries no traps at all. RAM (dead
//! stack bytes included), WRAM, OAM, the MMC1 registers, the CPU
//! registers/flags and the cycle delta must agree. The trapped side also
//! carries the bank-7 helper traps (`SwapPRG`/`SwapCHR`/erase/`$D382` and
//! friends), so helper `JSR`s and the `$D382` death dispatch exercise the
//! nested trap paths; tails that stay with the interpreter (`LEC02`, the
//! `$CF26` save continuation, an unported `$D382` target) are handed over
//! through the dispatcher's PC redirect (`Game::trap_jump`), which pushes
//! nothing, so even the stale trampoline frame bytes match the ROM's.
//! Skips without `Z2_ROM` (see `LEGAL.md`: no ROM bytes are stored in the
//! tree).

#![cfg(feature = "interp")]

mod common;

use z2_core::bank7_traps::register_bank7_traps;
use z2_core::cpu::{FLAG_B, FLAG_I, FLAG_U};
use z2_core::game::Game;
use z2_core::title_traps::{register_title_traps, TITLE_TRAP_COUNT};

/// RAM address of the `JSR target : RTS` stub (inside the stack page,
/// well below the deepest frame these routines reach; clear of `$0100`,
/// which `bank7_code7` writes).
const STUB: u16 = 0x0130;
/// Seeds per routine.
const SEEDS: u64 = 200;
/// Instruction budget per call (the SRAM writer and the Link sprite draw
/// are a few thousand).
const BUDGET: u64 = 100_000;

fn load_rom() -> Option<Vec<u8>> {
    common::rom_bytes("title_traps_ab")
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

const TARGETS: &[(u16, &str)] = &[
    (0xC33C, "bank7_code7"),
    (0xC358, "bank7_Reset_Number_of_Lives__to_3_"),
    (0xC360, "LC360"),
    (0xC3A5, "bank7_code10"),
    (0xC3B5, "bank7_Load_Lives_Remaining_Screen"),
    (0xC3E6, "LC3E6"),
    (0xC3ED, "LC3ED"),
    (0xC41E, "bank7_code11"),
    (0xCA1B, "LCA1B"),
    (0xCA24, "bank7_code16"),
    (0xCA6C, "LCA6C"),
    (0xCA72, "LCA72"),
    (0xCA85, "LCA85"),
    (
        0xCB18,
        "LCB18_fill_hp_or_mp_to_full__provide_x_register__maybe",
    ),
    (0xCF05, "LCF05"),
    (0xCF21, "LCF21_SaveGameWhenChooseSAVEwhenDead__maybe"),
];

/// Random but title/death-shaped state; returns the entry `(A, X, Y, P)`.
fn seed(g: &mut Game, rng: &mut Lcg, target: u16) -> (u8, u8, u8, u8) {
    for b in g.ram.iter_mut() {
        *b = rng.next();
    }
    for b in g.wram.iter_mut() {
        *b = rng.next();
    }
    // Death dispatcher index (3-entry table), save slot (3 slots).
    g.ram[0x073D] = rng.below(3);
    g.ram[0x0772] = rng.below(3);
    // World / region / town: bias the music and Great-Palace branches.
    g.ram[0x0707] = if rng.coin() {
        rng.below(3)
    } else {
        rng.below(8)
    };
    g.ram[0x0706] = if rng.below(3) == 0 { 3 } else { rng.below(4) };
    g.ram[0x056B] = if rng.coin() { 7 } else { rng.below(8) };
    // Timers expire half the time.
    if rng.coin() {
        g.ram[0x0501] = 0;
    }
    // Start/Select edges: half the seeds change the held pair.
    g.ram[0x00F7] = rng.next();
    g.ram[0x0744] = if rng.coin() {
        g.ram[0x00F7]
    } else {
        rng.next()
    };
    g.ram[0x0488] = rng.below(2);
    if rng.below(4) == 0 {
        g.ram[0x079F] = 0xFF;
    }
    // Lives: last life a third of the time.
    g.ram[0x0700] = if rng.below(3) == 0 { 1 } else { rng.below(5) };
    g.ram[0x0783] = rng.below(9);
    g.ram[0x0784] = rng.below(9);
    // Saved PRG bank for SwapToSavedPRG (`$0769`), CHR bank for the lives
    // screen (`$076E`).
    g.ram[0x0769] = rng.below(8);
    g.ram[0x076E] = rng.below(0x20);
    let x = match target {
        // `LCB18` takes the meter in X: mostly 0/1, sometimes wild (page
        // cross on `$0783,X`).
        0xCB18 => {
            if rng.below(4) == 0 {
                rng.next()
            } else {
                rng.below(2)
            }
        }
        _ => rng.next(),
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
fn run_pair(raw: &[u8], target: u16, seed_no: u64) -> (Game, Game, bool, bool) {
    let mut t = Game::from_ines(raw).expect("Z2_ROM must be MMC1 iNES");
    let mut u = Game::from_ines(raw).expect("Z2_ROM must be MMC1 iNES");
    register_bank7_traps(&mut t);
    let helpers = t.traps.len();
    register_title_traps(&mut t);
    assert_eq!(t.traps.len(), helpers + TITLE_TRAP_COUNT);
    assert!(u.traps.is_empty(), "the plain side runs the ROM bytes only");
    let mut rng_t = Lcg(seed_no.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(target));
    let mut rng_u = Lcg(rng_t.0);
    let regs = seed(&mut t, &mut rng_t, target);
    let regs_u = seed(&mut u, &mut rng_u, target);
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
fn title_traps_match_rom_bytes_on_seeded_state() {
    let Some(raw) = load_rom() else {
        eprintln!("SKIP: Z2_ROM not set");
        return;
    };
    for &(target, name) in TARGETS {
        let mut compared = 0u64;
        for seed_no in 0..SEEDS {
            let (t, u, ok_t, ok_u) = run_pair(&raw, target, seed_no);
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
            assert_eq!(u.mmc1, t.mmc1, "{what}: mmc1");
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
