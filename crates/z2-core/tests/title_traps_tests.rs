//! Title trap-table + port tests (register-exact pass).
//!
//! Synthetic `Game` tests always run (ROM-free): the bank-7 helper traps
//! (`SwapPRG`/`SwapCHR`/erase/`$D382`) and the title traps are registered
//! so every inner `JSR` a port makes lands on a Rust port instead of the
//! (empty) PRG image. Paths that hand control back to ROM bytes
//! (`LEC02`, the `$CF26` save continuation) are checked by the tail-jump
//! request they leave for the dispatcher (`Game::take_trap_jump`); a tail
//! into a trapped routine (`LCF05`) is fired the way the dispatcher would
//! ([`follow_tail`]). Bit-exact behaviour against the ROM bytes
//! (registers, flags, dead stack bytes, cycles) is pinned by the ROM-gated
//! `title_traps_ab` A/B. The table cross-checks `TITLE_TRAPS` for
//! uniqueness and bank discipline.

#![cfg(feature = "interp")]

use z2_core::bank7_traps::register_bank7_traps;
use z2_core::cpu::{FLAG_C, FLAG_N, FLAG_V, FLAG_Z};
use z2_core::game::Game;
use z2_core::title_traps::{
    register_title_traps, tt_die, tt_gameover_choice, tt_gameover_wait, tt_lives3, tt_lives_screen,
    tt_mode_inc, tt_new_life, tt_ready_go, tt_ready_hold, tt_ready_setup, tt_refill, tt_respawn,
    tt_save_on_save, tt_state_inc, tt_title_mode, TITLE_TRAPS, TITLE_TRAP_COUNT,
};
use z2_core::traps::TrapExit;

/// Fresh game with the bank-7 helpers and the title ports registered.
fn game() -> Game {
    let mut g = Game::new();
    register_bank7_traps(&mut g);
    register_title_traps(&mut g);
    g.cpu.cycles = 0;
    g
}

/// Resolve the tail jump a port left for the dispatcher: it must name
/// `expect`, and when `expect` is trapped its port fires like the
/// dispatcher's chain would (a plain return: nothing more to do).
fn follow_tail(g: &mut Game, expect: u16) {
    assert_eq!(g.take_trap_jump(), Some(expect), "tail jump target");
    if g.traps.is_trapped(expect) {
        assert_eq!(g.fire_trap(expect), TrapExit::Return);
    }
}

/// The dead bytes of an inner `JSR` frame that was pushed and popped at
/// the current `SP` (`push_word` writes `hi` at `SP`, `lo` at `SP - 1`).
fn dead_frame_word(g: &Game) -> u16 {
    let sp = usize::from(g.cpu.sp);
    let lo = u16::from(g.ram[0x0100 + sp - 1]);
    let hi = u16::from(g.ram[0x0100 + sp]);
    lo | (hi << 8)
}

#[test]
fn title_trap_table_is_unique_and_bank_disciplined() {
    assert_eq!(TITLE_TRAP_COUNT, 16);
    let mut seen = std::collections::BTreeSet::new();
    let mut fixed = 0;
    for (name, bank, addr) in TITLE_TRAPS {
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
    assert_eq!(fixed, TITLE_TRAP_COUNT);
}

#[test]
fn register_title_traps_registers_fixed_only_and_is_idempotent() {
    let mut game = Game::new();
    register_title_traps(&mut game);
    assert_eq!(game.traps.len(), TITLE_TRAP_COUNT);
    register_title_traps(&mut game);
    assert_eq!(game.traps.len(), TITLE_TRAP_COUNT);
    // Every port charges itself except LCF05, which keeps its tabled 12.
    for (_, bank, addr) in TITLE_TRAPS {
        if bank.is_none() {
            continue;
        }
        let cycles = game.traps.get(*addr).expect("registered").cycles;
        assert_eq!(cycles, if *addr == 0xCF05 { 12 } else { 0 }, "${addr:04X}");
    }
}

#[test]
fn title_mode_swaps_to_bank5_and_resets_the_stage() {
    let mut g = game();
    g.ram[0x076C] = 0x01;
    g.cpu.p = FLAG_C | FLAG_N | FLAG_V;
    tt_title_mode(&mut g);
    assert_eq!(g.ram[0x076C], 0x00);
    assert_eq!(g.ram[0x0100], 0x80);
    // SwapPRG(5) then SwapCHR(5 >> 4 = 0): the PRG bank is 5, CHR bank 0.
    assert_eq!(g.mmc1.prg, 5);
    assert_eq!(g.mmc1.chr0, 0);
    assert_eq!(g.cpu.a, 0x00);
    assert_eq!(g.cpu.p & (FLAG_Z | FLAG_N), FLAG_Z);
    // 32 own cycles + two 34-cycle swaps.
    assert_eq!(g.cpu.cycles, 32 + 34 + 34);
}

#[test]
fn lives_state_and_mode_ports_match_the_bytes() {
    let mut g = game();
    g.ram[0x0700] = 0x01;
    g.ram[0x0760] = 0xFE;
    g.ram[0x076C] = 0xFF;
    tt_lives3(&mut g);
    assert_eq!(g.ram[0x0700], 0x03);
    assert_eq!(g.ram[0x0760], 0xFF);
    assert_eq!(g.ram[0x076C], 0x00);
    assert_eq!(g.cpu.a, 0x03);
    assert_eq!(g.cpu.p & (FLAG_Z | FLAG_N), FLAG_Z, "flags from INC $076C");
    assert_eq!(g.cpu.cycles, 24);

    let mut g = game();
    g.ram[0x076C] = 0x7F;
    tt_state_inc(&mut g);
    assert_eq!(g.ram[0x076C], 0x80);
    assert_eq!(g.cpu.p & (FLAG_Z | FLAG_N), FLAG_N);
    assert_eq!(g.cpu.cycles, 12);

    let mut g = game();
    tt_mode_inc(&mut g);
    assert_eq!(g.ram[0x0736], 0x01);
    assert_eq!(g.cpu.cycles, 0, "LCF05 cost lives in the trap table");

    let mut g = game();
    tt_new_life(&mut g);
    assert_eq!(g.ram[0x076C], 0x06);
    assert_eq!(g.cpu.a, 0x06);
    assert_eq!(g.cpu.cycles, 12);
}

#[test]
fn ready_setup_and_hold_bump_the_dialog_index() {
    let mut g = game();
    g.ram[0x0760] = 0x33;
    g.ram[0x0726] = 0x01;
    g.ram[0x0738] = 0x00;
    tt_ready_setup(&mut g);
    assert_eq!(g.ram[0x0725], 0x0C);
    assert_eq!(g.ram[0x0760], 0x00);
    assert_eq!(g.ram[0x0726], 0x02);
    assert_eq!(g.ram[0x0738], 0x01);
    assert_eq!(g.cpu.a, 0x00);
    assert_eq!(g.cpu.p & (FLAG_Z | FLAG_N), 0, "flags from INC $0738 = 1");
    assert_eq!(g.cpu.cycles, 33);

    let mut g = game();
    g.ram[0x0726] = 0x05;
    g.ram[0x0738] = 0x01;
    tt_ready_hold(&mut g);
    assert_eq!(g.ram[0x0726], 0x00);
    assert_eq!(g.ram[0x0738], 0x02);
    assert_eq!(g.cpu.a, 0x00);
    assert_eq!(g.cpu.cycles, 21);
}

#[test]
fn lives_screen_stages_the_packet_through_the_helper_traps() {
    let mut g = game();
    for (i, b) in (0..20u8).zip(0x958..0x96C) {
        g.wram[b] = 0xC0 + i;
    }
    for i in 0..8usize {
        g.ram[0x07A1 + i] = 0xDA + i as u8;
    }
    g.ram[0x0700] = 0x03;
    g.ram[0x076E] = 0x0E;
    g.ram[0x0738] = 0x01;
    tt_lives_screen(&mut g);
    assert_eq!(g.mmc1.chr0, 0x0E, "SwapCHR($076E)");
    assert_eq!(&g.ram[0x0302..0x0305], &[0xC0, 0xC1, 0xC2]);
    assert_eq!(
        &g.ram[0x0305..0x030D],
        &[0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF, 0xE0, 0xE1]
    );
    assert_eq!(
        &g.ram[0x030D..0x0316],
        &[0xCB, 0xCC, 0xCD, 0xCE, 0xCF, 0xD0, 0xD1, 0xD3, 0xD3]
    );
    assert_eq!(g.ram[0x0314], 0xD3, "lives digit tile");
    assert_eq!(g.ram[0x0501], 0x70);
    assert_eq!(g.ram[0x0738], 0x02);
    assert_eq!(g.cpu.a, 0x70);
    assert_eq!(g.cpu.y, 0xFF);
    assert_eq!(g.cpu.p & FLAG_C, 0, "3 + $D0 does not carry");
    // 440 own cycles + SwapCHR 34 + Erase_Name_Tables_0and1 19336.
    assert_eq!(g.cpu.cycles, 440 + 34 + 19336);
}

#[test]
fn ready_go_poses_link_and_hands_lec02_to_the_interpreter() {
    // Timer running: no mode bump, pose, tail JMP LEC02 (re-targeted RTS).
    let mut g = game();
    let sp0 = g.cpu.sp;
    g.ram[0x0501] = 0x10;
    g.ram[0x0736] = 0x04;
    g.ram[0x0013] = 0x07;
    tt_ready_go(&mut g);
    assert_eq!(g.ram[0x0736], 0x04);
    assert_eq!(g.ram[0x0029], 0x50);
    assert_eq!(g.ram[0x00CC], 0x78);
    assert_eq!(g.ram[0x0080], 0x03);
    assert_eq!(g.ram[0x009F], 0x01);
    for a in [0x00C8usize, 0x0013, 0x0011, 0x0090] {
        assert_eq!(g.ram[a], 0x00, "${a:04X}");
    }
    assert_eq!(g.cpu.a, 0x00);
    assert_eq!(g.cpu.p & (FLAG_C | FLAG_Z | FLAG_N), FLAG_C | FLAG_Z);
    assert_eq!(g.cpu.sp, sp0, "nothing pushed for the tail JMP");
    follow_tail(&mut g, 0xEC02);
    assert_eq!(g.cpu.cycles, 4 + 3 + 34 + 3);
    // Timer expired: $0736++ first.
    let mut g = game();
    g.ram[0x0501] = 0x00;
    g.ram[0x0736] = 0x04;
    tt_ready_go(&mut g);
    assert_eq!(g.ram[0x0736], 0x05);
    assert_eq!(g.cpu.cycles, 4 + 2 + 6 + 34 + 3);
}

#[test]
fn respawn_selects_music_once_the_timer_expires() {
    // Overworld (world 0) doubles the music code: 4.
    let mut g = game();
    g.ram[0x0501] = 0x00;
    g.ram[0x0707] = 0x00;
    tt_respawn(&mut g);
    assert_eq!(g.ram[0x075F], 0x04);
    assert_eq!(g.ram[0x076C], 0x01);
    assert_eq!(g.ram[0x076D], 0x01);
    assert_eq!(g.ram[0x0736], 0x07);
    assert_eq!(g.ram[0x0029], 0x50);
    follow_tail(&mut g, 0xEC02);
    assert_eq!(g.cpu.cycles, 4 + 2 + 2 + 4 + 2 + 1 + 2 + 23 + 34 + 3);
    // Town 7 in world 1/2 also doubles; other towns keep 2.
    let mut g = game();
    g.ram[0x0707] = 0x02;
    g.ram[0x056B] = 0x07;
    tt_respawn(&mut g);
    assert_eq!(g.ram[0x075F], 0x04);
    let mut g = game();
    g.ram[0x0707] = 0x02;
    g.ram[0x056B] = 0x03;
    tt_respawn(&mut g);
    assert_eq!(g.ram[0x075F], 0x02);
    // Palaces (world >= 3) keep 2.
    let mut g = game();
    g.ram[0x0707] = 0x05;
    g.ram[0x056B] = 0x07;
    tt_respawn(&mut g);
    assert_eq!(g.ram[0x075F], 0x02);
    // Timer running: only the pose + LEC02.
    let mut g = game();
    g.ram[0x0501] = 0x20;
    g.ram[0x075F] = 0x09;
    tt_respawn(&mut g);
    assert_eq!(g.ram[0x075F], 0x09);
    assert_eq!(g.ram[0x0736], 0x00);
    assert_eq!(g.ram[0x0029], 0x50);
    assert_eq!(g.cpu.cycles, 4 + 3 + 3 + 34 + 3);
}

#[test]
fn die_decrements_or_games_over() {
    // Lives remain: refill both meters via the LCB18 trap, clears, $76C = 6.
    let mut g = game();
    g.ram[0x0700] = 0x03;
    g.ram[0x0783] = 0x04;
    g.ram[0x0784] = 0x04;
    g.ram[0x074C] = 0x11;
    g.ram[0x00DE] = 0x22;
    for i in 0..6usize {
        g.ram[0x05C3 + i] = 0x33;
    }
    tt_die(&mut g);
    assert_eq!(g.ram[0x0700], 0x02);
    assert_eq!(g.ram[0x076C], 0x06);
    assert_eq!(g.ram[0x0773], 127);
    assert_eq!(g.ram[0x0774], 127);
    assert_eq!(g.ram[0x074C], 0x00);
    assert_eq!(g.ram[0x00DE], 0x00);
    assert_eq!(&g.ram[0x05C3..0x05C9], &[0; 6]);
    assert_eq!(g.cpu.a, 0x06);
    assert_eq!(g.cpu.x, 0xFF);
    assert_eq!(g.cpu.y, 0xFF);
    // LDX 2 + 2x(JSR 6 + LCB18 29 + DEX 2 + BPL 3/2) + clears 21 + LDY 2 +
    // loop 59 + DEC 6 + BNE taken 3 + LCA6C 12.
    assert_eq!(
        g.cpu.cycles,
        2 + (6 + 29 + 2 + 3) + (6 + 29 + 2 + 2) + 21 + 2 + 59 + 6 + 3 + 12
    );
    // Last life: game-over branch ($E9 = 2, $501 = $F0, $725 = 9, XP = 0).
    let mut g = game();
    g.ram[0x0700] = 0x01;
    g.ram[0x0775] = 0x12;
    g.ram[0x0726] = 0x01;
    g.ram[0x073D] = 0x01;
    tt_die(&mut g);
    assert_eq!(g.ram[0x0700], 0x00);
    assert_eq!(g.ram[0x00E9], 0x02);
    assert_eq!(g.ram[0x0501], 0xF0);
    assert_eq!(g.ram[0x0726], 0x02);
    assert_eq!(g.ram[0x073D], 0x02);
    assert_eq!(g.ram[0x0725], 0x09);
    assert_eq!(g.ram[0x0775], 0x00);
    assert_eq!(g.ram[0x0776], 0x00);
    assert_eq!(g.ram[0x076C], 0x00, "no new life");
    assert_eq!(g.cpu.a, 0x00);
    // The inner JSR LCA17 leaves its return ($CA5C) as dead stack bytes.
    assert_eq!(dead_frame_word(&g), 0xCA5C);
}

#[test]
fn gameover_wait_advances_on_start_or_timeout() {
    let mut g = game();
    g.ram[0x0736] = 0x06;
    g.ram[0x0726] = 0x01;
    g.ram[0x00F7] = 0x10; // Start held
    g.ram[0x0501] = 0x40;
    tt_gameover_wait(&mut g);
    follow_tail(&mut g, 0xCF05);
    assert_eq!(g.ram[0x0736], 0x07);
    assert_eq!(g.ram[0x0726], 0x00);
    assert_eq!(g.cpu.a, 0x10);
    // Body 13 (BNE base included) + BNE taken 1 + JMP 3 + LCF05 (tabled 12).
    assert_eq!(g.cpu.cycles, 13 + 1 + 3 + 12);
    let mut g = game();
    g.ram[0x0736] = 0x06;
    g.ram[0x00F7] = 0x00;
    g.ram[0x0501] = 0x40;
    tt_gameover_wait(&mut g);
    assert_eq!(g.ram[0x0736], 0x06);
    assert_eq!(g.cpu.a, 0x40);
    // Body 13 (BNE not taken) + LDA abs 4 + BNE taken 3 + RTS 6.
    assert_eq!(g.cpu.cycles, 13 + 4 + 3 + 6);
    let mut g = game();
    g.ram[0x0736] = 0x06;
    g.ram[0x00F7] = 0x00;
    g.ram[0x0501] = 0x00;
    tt_gameover_wait(&mut g);
    follow_tail(&mut g, 0xCF05);
    assert_eq!(g.ram[0x0736], 0x07);
    // Body 13 + LDA abs 4 + BNE not taken 2 + JMP 3 + LCF05 (tabled 12).
    assert_eq!(g.cpu.cycles, 13 + 4 + 2 + 3 + 12);
}

#[test]
fn gameover_choice_select_toggles_continue_save() {
    // Fresh press of Select (held $20 vs prev $00) flips $488 (LCAF7).
    let mut g = game();
    for b in 0x974..0x97B {
        g.wram[b] = (b & 0xFF) as u8;
    }
    g.ram[0x00F7] = 0x20;
    g.ram[0x0744] = 0x00;
    tt_gameover_choice(&mut g);
    assert_eq!(g.ram[0x0000], 0x80, "scratch $00 = ($F7 & $30) << 2");
    assert_eq!(g.ram[0x0488], 0x01);
    assert_eq!(g.ram[0x00EF], 0x10);
    // The $6974 packet, with the $FA marker dropped at $0305 + 1 * 2.
    assert_eq!(
        &g.ram[0x0302..0x0309],
        &[0x74, 0x75, 0x76, 0x77, 0x78, 0xFA, 0x7A]
    );
    assert_eq!(g.ram[0x0305 + 2], 0xFA);
    assert_eq!(g.cpu.a, 0xFA);
    assert_eq!(g.cpu.y, 0x02);
    // Compare 27 + BIT/BVC 5 + BVC taken 1, BPL 2 + LDA/STA/LDY 7 + copy
    // 103 (the `BPL LCAFF` back-branch crosses a page: 6 x 4 + 2 on top of
    // 7 x 11) + toggle/marker 27.
    assert_eq!(g.cpu.cycles, 27 + 5 + 3 + 7 + 103 + 27);
    // No change: no-op (BEQ LCAF6).
    g.ram[0x0744] = 0x20;
    g.ram[0x0488] = 0x01;
    g.cpu.cycles = 0;
    tt_gameover_choice(&mut g);
    assert_eq!(g.ram[0x0488], 0x01);
    assert_eq!(g.cpu.cycles, 27 + 7);
    // Releasing Select is a change, but LCAF7's BPL bails out (nothing
    // toggles) — unlike the old shim, which flipped $488 on release.
    let mut g = game();
    g.ram[0x00F7] = 0x00;
    g.ram[0x0744] = 0x20;
    g.ram[0x0488] = 0x01;
    tt_gameover_choice(&mut g);
    assert_eq!(g.ram[0x0488], 0x01);
    assert_eq!(g.ram[0x00EF], 0x00);
    assert_eq!(g.cpu.p & FLAG_V, 0);
    // Compare 27 + BIT/BVC 5 + BVC taken 1, BPL 2 + BPL taken 1, RTS 6.
    assert_eq!(g.cpu.cycles, 27 + 5 + 3 + 7);
}

#[test]
fn gameover_choice_start_continues_or_saves() {
    // CONTINUE ($488 = 0): continues++, wipe $E0-$EF/$7C0-$7FF only,
    // castle respawn ($76C = 0, $75F = 1). $700-$7BF survives the LCAB1
    // walk (exits at Y = $00); lives = 3 comes from LC34F later.
    let mut g = game();
    g.ram[0x00F7] = 0x10;
    g.ram[0x0744] = 0x00;
    g.ram[0x0488] = 0x00;
    g.ram[0x079F] = 0x02;
    g.ram[0x0706] = 0x00;
    g.ram[0x0707] = 0x00;
    g.ram[0x0700] = 0x00;
    g.ram[0x07C0] = 0x09;
    g.ram[0x07FF] = 0x09;
    g.ram[0x07BF] = 0x09;
    g.ram[0x00E0] = 0x09;
    g.ram[0x00EF] = 0x09;
    g.ram[0x00DF] = 0x09;
    g.ram[0x0775] = 0x55;
    tt_gameover_choice(&mut g);
    assert_eq!(g.ram[0x079F], 0x03);
    assert_eq!(g.ram[0x076C], 0x00);
    assert_eq!(g.ram[0x075F], 0x01);
    assert_eq!(g.ram[0x07C0], 0x00);
    assert_eq!(g.ram[0x07FF], 0x00);
    assert_eq!(g.ram[0x07BF], 0x09);
    assert_eq!(g.ram[0x00E0], 0x00);
    assert_eq!(g.ram[0x00EF], 0x00);
    assert_eq!(g.ram[0x00DF], 0x09);
    assert_eq!(g.ram[0x0775], 0x00);
    assert_eq!(g.cpu.a, 0x00);
    assert_eq!(g.cpu.x, 0xFF);
    assert_eq!(g.cpu.y, 0x01);
    // The inner JSR $CF30 leaves $CAD2 as dead stack bytes.
    assert_eq!(dead_frame_word(&g), 0xCAD2);
    assert_eq!(
        g.cpu.cycles,
        27 + 5 + 8 + 6 + 4 + 143 + 2 + 639 + 6 + 1 + 16 + 6 + 22 + 4 + 7 + 14
    );
    // Continues saturate at $FF.
    let mut g = game();
    g.ram[0x00F7] = 0x10;
    g.ram[0x079F] = 0xFF;
    tt_gameover_choice(&mut g);
    assert_eq!(g.ram[0x079F], 0xFF);
    // Great Palace (region 3 * 5 + world 0 = $0F): lives 3 via the $C358
    // trap, area codes 0, $76C = 1, $75F = 2.
    let mut g = game();
    g.ram[0x00F7] = 0x10;
    g.ram[0x0706] = 0x03;
    g.ram[0x0707] = 0x00;
    g.ram[0x0561] = 0x07;
    g.ram[0x0701] = 0x07;
    g.ram[0x075C] = 0x07;
    tt_gameover_choice(&mut g);
    assert_eq!(g.ram[0x0700], 0x03);
    assert_eq!(g.ram[0x0561], 0x00);
    assert_eq!(g.ram[0x0701], 0x00);
    assert_eq!(g.ram[0x075C], 0x00);
    assert_eq!(g.ram[0x076C], 0x01);
    assert_eq!(g.ram[0x075F], 0x02);
    assert_eq!(g.cpu.a, 0x01);
    assert_eq!(g.cpu.y, 0x02);
    // SAVE ($488 = 1): flash timer + $736++ (LCF05 trap).
    let mut g = game();
    g.ram[0x00F7] = 0x10;
    g.ram[0x0744] = 0x00;
    g.ram[0x0488] = 0x01;
    g.ram[0x0736] = 0x06;
    tt_gameover_choice(&mut g);
    follow_tail(&mut g, 0xCF05);
    assert_eq!(g.ram[0x07B0], 0x40);
    assert_eq!(g.ram[0x0736], 0x07);
    assert_eq!(g.cpu.a, 0x40);
    assert_eq!(
        g.cpu.cycles,
        27 + 5 + 8 + 6 + 4 + 143 + 2 + 639 + 6 + 9 + 12
    );
}

#[test]
fn refill_uses_the_caller_x_register() {
    let mut g = game();
    g.ram[0x0783] = 0x05;
    g.ram[0x0784] = 0x06;
    g.ram[0x0773] = 0x11;
    g.ram[0x0774] = 0x22;
    g.cpu.x = 0x01;
    tt_refill(&mut g);
    assert_eq!(g.ram[0x0773], 0x11, "X = 1 only refills the life meter");
    assert_eq!(g.ram[0x0774], 6 * 32 - 1);
    assert_eq!(g.cpu.a, 6 * 32 - 1);
    assert_eq!(g.cpu.x, 0x01);
    assert_eq!(g.cpu.p & FLAG_C, FLAG_C, "SBC without borrow");
    assert_eq!(g.cpu.cycles, 29);
    g.cpu.x = 0x00;
    g.cpu.cycles = 0;
    tt_refill(&mut g);
    assert_eq!(g.ram[0x0773], 5 * 32 - 1);
    assert_eq!(g.cpu.cycles, 29);
    // Indexing across the page boundary costs the extra read cycle.
    let mut g = game();
    g.cpu.x = 0x7D;
    g.ram[0x0800 & 0x7FF] = 0x00;
    tt_refill(&mut g);
    assert_eq!(g.cpu.cycles, 30);
}

#[test]
fn save_on_save_swaps_to_bank5_and_resumes_the_writer_in_rom() {
    let mut g = game();
    let sp0 = g.cpu.sp;
    tt_save_on_save(&mut g);
    assert_eq!(g.mmc1.prg, 5);
    assert_eq!(g.cpu.sp, sp0, "nothing pushed for the hand-off");
    follow_tail(&mut g, 0xCF26);
    assert_eq!(g.cpu.cycles, 2 + 6 + 34);
}
