//! Boot trap-table + shim tests.
//!
//! Synthetic `Game` tests always run (ROM-free fixtures); the table
//! cross-checks `BOOT_TRAPS` for uniqueness and bank discipline, and the
//! end-to-end `JSR`-dispatch tests prove the stage-table tail-call surgery
//! lands on the ROM table target with a balanced stack.

#![cfg(feature = "interp")]

use z2_core::boot_traps::{
    register_boot_traps, BOOT_TRAPS, BOOT_TRAP_COUNT, CODE5_CYCLES, CODE6_CYCLES, LC2CA_CYCLES,
};
use z2_core::game::Game;

#[test]
fn boot_trap_table_is_unique_and_bank_disciplined() {
    assert_eq!(BOOT_TRAP_COUNT, 3);
    let mut seen = std::collections::BTreeSet::new();
    let mut fixed = 0;
    for (name, bank, addr) in BOOT_TRAPS {
        assert!(!name.is_empty());
        assert!(*addr >= 0x8000, "{name} ${addr:04X} outside PRG");
        assert!(seen.insert((*name, *bank, *addr)), "duplicate {name}");
        match bank {
            Some(7) => {
                assert!((0xC000..=0xFFFF).contains(addr), "{name} not fixed-bank");
                fixed += 1;
            }
            None => assert!(
                (0x8000..0xC000).contains(addr),
                "{name} banked entry outside $8000-$BFFF"
            ),
            Some(b) => panic!("{name} unexpected bank {b}"),
        }
    }
    // 3 registered + 2 JMP-only fixed + banked data-only.
    assert_eq!(fixed, 5);
    assert!(BOOT_TRAPS.len() > fixed);
}

#[test]
fn register_boot_traps_registers_fixed_jsr_only_and_is_idempotent() {
    let mut game = Game::new();
    register_boot_traps(&mut game);
    assert_eq!(game.traps.len(), BOOT_TRAP_COUNT);
    assert!(game.traps.is_trapped(0xC2CA));
    assert!(game.traps.is_trapped(0xC2E6));
    assert!(game.traps.is_trapped(0xC31E));
    // JMP-only fixed entries stay untrapped; banked entries never register.
    assert!(!game.traps.is_trapped(0xC000));
    assert!(!game.traps.is_trapped(0xC388));
    assert!(!game.traps.is_trapped(0xA6A0));
    assert!(!game.traps.is_trapped(0xAA08));
    register_boot_traps(&mut game);
    assert_eq!(game.traps.len(), BOOT_TRAP_COUNT);
}

#[test]
fn trap_costs_are_counted() {
    // LC2CA: LDY 2 + 2x STY 8 + JSR 6 + LD174 21 + JSR 6 + $D385 43 = 86.
    assert_eq!(LC2CA_CYCLES, 86);
    // code5/code6: JSR 6 + LD168 20 + JSR 6 + $D385 43 = 75.
    assert_eq!(CODE5_CYCLES, 75);
    assert_eq!(CODE6_CYCLES, 75);
    let mut game = Game::new();
    register_boot_traps(&mut game);
    assert_eq!(game.traps.get(0xC2CA).unwrap().cycles, 86);
    assert_eq!(game.traps.get(0xC2E6).unwrap().cycles, 75);
    assert_eq!(game.traps.get(0xC31E).unwrap().cycles, 75);
}

// ---------------------------------------------------------------------------
// End-to-end trap dispatch: JSR into a trapped stage dispatch must land on
// the ROM table target with the trampoline's register/scratch effects.
// PRG pokes place synthetic tables at the real fixed-bank addresses.
// ---------------------------------------------------------------------------

/// Game with a 32 KiB zeroed PRG, `JSR target` at `$8000`, and RESET+NMI
/// vectors pointed at it. `table_base`/`entry` select one synthetic
/// table word written before the run.
fn dispatch_game(jsr_target: u16) -> Game {
    let lo = (jsr_target & 0xFF) as u8;
    let hi = (jsr_target >> 8) as u8;
    // `JSR target : NOP : NOP ...` at $8000 (execution never falls through:
    // the trap's emulated RTS lands on the table target instead).
    let mut game = Game::with_test_program(0x8000, &[0x20, lo, hi, 0xEA, 0xEA]);
    game.set_nmi_vector(0x8000);
    game.reset();
    game
}

/// Poke one table word and run the `JSR` at `$8000` through the trap.
fn run_jsr(game: &mut Game, table_base: u16, index: u8, target: u16) {
    let off = (table_base - 0x8000) as usize + usize::from(index) * 2;
    game.prg[off] = (target & 0xFF) as u8;
    game.prg[off + 1] = (target >> 8) as u8;
    game.step_instruction().unwrap(); // JSR (trap fires, lands on target)
}

#[test]
fn lc2ca_no_change_dispatches_table2() {
    let mut game = dispatch_game(0xC2CA);
    register_boot_traps(&mut game);
    // table2[1] = bank7_code5 is trapped too; keep it as ROM bytes here so
    // the landing PC is observable (the chain is covered separately).
    game.set_untrapped(0xC2E6, true);
    // Steady state: no stage change (shadow matches), mode 0.
    game.ram[0x76C] = 0x01;
    game.ram[0x76D] = 0x01;
    game.ram[0x726] = 0x09; // nonzero: LC2CA zeroes it
    game.ram[0x729] = 0x09;
    run_jsr(&mut game, 0xC2D8, 1, 0xC2E6);
    let (_, _, _, _, pc, _) = game.cpu_state();
    assert_eq!(pc, 0xC2E6, "tail-call lands on table2[1]");
    // LDY/STY prefix + LD174 no-change effects.
    assert_eq!(game.ram[0x727], 0x00);
    assert_eq!(game.ram[0x729], 0x00);
    assert_eq!(game.ram[0x76D], 0x01);
    // Trampoline effects: Y = A*2+2 (both INYs), A = target hi,
    // $0C-$0F scratch.
    // The skipped inner JSR $D385 would have pushed the byte before the
    // inline table, $C2D7; the outer caller return remains on the stack.
    assert_eq!(game.cpu_state().2, 4); // index 1 -> 1*2+2
    assert_eq!(game.cpu_state().0, 0xC2);
    assert_eq!(game.ram[0x0C], 0xD7);
    assert_eq!(game.ram[0x0D], 0xC2);
    assert_eq!(game.ram[0x0E], 0xE6);
    assert_eq!(game.ram[0x0F], 0xC2);
    assert_eq!(
        game.trap_log().last().map(|r| r.attribution()),
        Some("LC2CA@$C2CA".to_string())
    );
}

#[test]
fn lc2ca_stage_change_clears_and_hides_sprites() {
    let mut game = dispatch_game(0xC2CA);
    register_boot_traps(&mut game);
    // table2[2] = bank7_code6 is trapped too (see above).
    game.set_untrapped(0xC31E, true);
    // Stage 0 → 2 change: LD174 hides sprites + clears riders.
    game.ram[0x76C] = 0x02;
    game.ram[0x76D] = 0x00;
    game.ram[0x736] = 0x08;
    game.ram[0x200] = 0x11; // OAM page: hidden by Remove_All_Sprites
    run_jsr(&mut game, 0xC2D8, 2, 0xC31E); // table2[2]
    let (_, _, _, _, pc, _) = game.cpu_state();
    assert_eq!(pc, 0xC31E);
    assert_eq!(game.ram[0x200], 0xF8, "sprites hidden on stage change");
    assert_eq!(game.ram[0x736], 0x00, "mode cleared on stage change");
    assert_eq!(game.ram[0x73B], 0x00);
    assert_eq!(game.ram[0x73D], 0x00);
    assert_eq!(game.cpu_state().0, 0xC3, "A = target hi byte");
}

#[test]
fn code5_and_code6_dispatch_their_tables() {
    for (addr, base, target) in [
        (0xC2E6u16, 0xC2ECu16, 0xD000u16),
        (0xC31Eu16, 0xC324u16, 0xD100u16),
    ] {
        let mut game = dispatch_game(addr);
        register_boot_traps(&mut game);
        // No mode change: $0736 == $0737.
        game.ram[0x736] = 0x05;
        game.ram[0x737] = 0x05;
        let sp0 = game.cpu_state().3;
        run_jsr(&mut game, base, 5, target);
        let (a, _, y, sp, pc, _) = game.cpu_state();
        assert_eq!(pc, target, "table target for {addr:04X}");
        assert_eq!(y, 5 * 2 + 2); // index 5 -> Y = 5*2+2
        assert_eq!(a, (target >> 8) as u8);
        assert_eq!(sp, sp0.wrapping_sub(2), "outer JSR frame is retained");
    }
}

#[test]
fn code5_mode_change_resets_routine_bytes() {
    let mut game = dispatch_game(0xC2E6);
    register_boot_traps(&mut game);
    // Mode 4 → 5 change: LD168 resets $073B/$0738/$073D.
    game.ram[0x736] = 0x05;
    game.ram[0x737] = 0x04;
    game.ram[0x73B] = 0x07;
    game.ram[0x73D] = 0x07;
    run_jsr(&mut game, 0xC2EC, 5, 0xD000);
    assert_eq!(game.ram[0x737], 0x05, "shadow follows");
    assert_eq!(game.ram[0x73B], 0x00);
    assert_eq!(game.ram[0x73D], 0x00);
}

#[test]
fn dispatched_tail_call_is_stack_balanced() {
    // The selected target must return through the outer JSR frame. The trap
    // dispatch first leaves that frame below the target handoff, then the
    // target's RTS restores the original stack pointer.
    let mut game = dispatch_game(0xC2CA);
    register_boot_traps(&mut game);
    game.ram[0x76C] = 0x01;
    game.ram[0x76D] = 0x01;
    let sp0 = game.cpu_state().3;
    run_jsr(&mut game, 0xC2D8, 1, 0xD234);
    let (_, _, _, sp, pc, _) = game.cpu_state();
    assert_eq!(pc, 0xD234);
    assert_eq!(sp, sp0.wrapping_sub(2));
    assert_eq!((game.ram[0x0E], game.ram[0x0F]), (0x34, 0xD2));
    game.prg[(0xD234 - 0xC000) as usize + 0x4000] = 0x60; // selected target returns
    game.step_instruction().unwrap();
    let (_, _, _, sp, pc, _) = game.cpu_state();
    assert_eq!(sp, sp0);
    assert_eq!(pc, 0x8003, "target RTS returns to the outer caller");
}

#[test]
fn lc2ca_chains_into_the_trapped_stage_dispatch() {
    // table2[1] = $C2E6 is itself trapped (bank7_code5): the LC2CA port's
    // tail jump fires it in turn, and its own tail jump lands on the
    // game-mode table target — one JSR, two trap records, only the outer
    // frame on the stack (the ROM's two trampoline frames are consumed).
    let mut game = dispatch_game(0xC2CA);
    register_boot_traps(&mut game);
    game.ram[0x76C] = 0x01;
    game.ram[0x76D] = 0x01;
    game.ram[0x736] = 0x05;
    game.ram[0x737] = 0x05;
    // code5's table: entry 5 -> $D000.
    let off = (0xC2EC - 0x8000) as usize + 5 * 2;
    game.prg[off] = 0x00;
    game.prg[off + 1] = 0xD0;
    let sp0 = game.cpu_state().3;
    let c0 = game.cpu.cycles;
    run_jsr(&mut game, 0xC2D8, 1, 0xC2E6);
    let (a, _, y, sp, pc, _) = game.cpu_state();
    assert_eq!(pc, 0xD000, "chain lands on code5's table target");
    assert_eq!(sp, sp0.wrapping_sub(2), "only the outer JSR frame remains");
    assert_eq!(
        (a, y),
        (0xD0, 5 * 2 + 2),
        "registers from the second trampoline"
    );
    assert_eq!(
        (game.ram[0x0C], game.ram[0x0D]),
        (0xEB, 0xC2),
        "$0C/$0D from the second JSR $D385"
    );
    assert_eq!((game.ram[0x0E], game.ram[0x0F]), (0x00, 0xD0));
    let fired: Vec<String> = game.trap_log().iter().map(|r| r.attribution()).collect();
    assert_eq!(fired, ["LC2CA@$C2CA", "bank7_code5@$C2E6"]);
    assert_eq!(game.cpu.cycles - c0, 6 + LC2CA_CYCLES + CODE5_CYCLES);
}
