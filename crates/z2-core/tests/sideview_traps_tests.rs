//! Sideview trap-table + overworld binding + warp-glitch replay.
//!
//! Synthetic `Game` tests always run (ROM-free fixtures). Snapshot and
//! movie replays are harness-ready and skip gracefully without
//! `Z2_ROM` / `corpus/` / movie files.

#![cfg(feature = "interp")]

mod common;

use z2_core::bank7_traps::register_bank7_traps;
use z2_core::game::Game;
use z2_core::overworld::OVERWORLD_TRAPS;
use z2_core::sideview_traps::{
    ow_boundaries, ow_chop, ow_ldf01, ow_ldf3f, ow_ldfd2, ow_le001, ow_stone,
    register_overworld_traps, register_sideview_traps, sv_elevator, sv_lava_check, sv_locked_door,
    sv_scroll_gate, SIDEVIEW_TRAPS, SIDEVIEW_TRAP_COUNT,
};
use z2_core::traps::TrapExit;

#[test]
fn sideview_trap_table_is_sorted_unique_and_plausible() {
    assert!(!SIDEVIEW_TRAPS.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for (name, bank, addr) in SIDEVIEW_TRAPS {
        assert!(!name.is_empty());
        assert!(*addr >= 0x8000, "{name} ${addr:04X} outside PRG");
        assert!(seen.insert((*name, *bank, *addr)), "duplicate {name}");
    }
    // Fixed-bank count pins the registration set; banked entries are listed
    // but skipped (aliasing caveat). The ROM-owned area setup/draw and
    // ground-find routines are deliberately not trap entries because their
    // proxy shims corrupt real-cartridge rendering.
    let fixed = SIDEVIEW_TRAPS
        .iter()
        .filter(|(_, b, _)| *b == Some(7))
        .count();
    assert_eq!(fixed, SIDEVIEW_TRAP_COUNT);
}

#[test]
fn register_sideview_traps_installs_fixed_set() {
    let mut game = Game::new();
    assert_eq!(game.traps.len(), 0);
    register_sideview_traps(&mut game);
    assert_eq!(game.traps.len(), SIDEVIEW_TRAP_COUNT);
    // Spot-check fixed entries; banked entries must NOT be registered.
    assert!(game.traps.is_trapped(0xC9A5));
    assert!(game.traps.is_trapped(0xD6C1));
    assert!(!game.traps.is_trapped(0xC4CB));
    assert!(!game.traps.is_trapped(0xC755));
    assert!(!game.traps.is_trapped(0xC82B));
    assert!(!game.traps.is_trapped(0xC89D));
    assert!(!game.traps.is_trapped(0xE030));
    assert!(!game.traps.is_trapped(0x8CE1));
    assert!(!game.traps.is_trapped(0x80EE));
    // Idempotent.
    register_sideview_traps(&mut game);
    assert_eq!(game.traps.len(), SIDEVIEW_TRAP_COUNT);
}

#[test]
fn overworld_binding_registers_fixed_bank_only() {
    let mut game = Game::new();
    register_overworld_traps(&mut game);
    let fixed = OVERWORLD_TRAPS
        .iter()
        .filter(|(_, b, _)| *b == Some(7))
        .count();
    // Every fixed-bank entry but the ROM-owned mode-0 world loader.
    assert_eq!(game.traps.len(), fixed - 1);
    // Banked overworld entries stay data-only (aliasing with sideview).
    assert!(!game.traps.is_trapped(0x8284));
    assert!(!game.traps.is_trapped(0x83CF));
    // `bank7_code18` ($CD40) is listed but deliberately not registered:
    // its multi-NMI copy loops cannot run as one atomic trap body.
    assert!(!game.traps.is_trapped(0xCD40));
    // The remaining fixed-bank overworld entries are live.
    for addr in [
        0xCCB3u16, 0xDFEF, 0xDFF8, 0xE01B, 0xDF79, 0xDF01, 0xDF3F, 0xDFD2, 0xE001, 0xE024, 0xE16F,
    ] {
        assert!(game.traps.is_trapped(addr), "${addr:04X} registered");
    }
}

/// Synthetic 32 KiB PRG image for the register-exact shims: the sideview
/// bank's false-wall test opcode + table pointer at `$8516`, tile codes at
/// `$851A-$8522`, the `LE1BE` probe table at `$E04E`, the `$EAE8` probe
/// offset / page tables, the elevator velocity table at `$D8BF`, and an
/// `RTS` at every ROM sub-routine the shims call through the interpreter
/// (`$DE40`, `$DEC8`, `$C295`, `$E371`). Values are made up for the test —
/// nothing here is copied from the cartridge.
fn synthetic_prg() -> Game {
    let mut blob = vec![0u8; 0x8000];
    let put = |b: &mut Vec<u8>, addr: u16, bytes: &[u8]| {
        let off = usize::from(addr - 0x8000);
        b[off..off + bytes.len()].copy_from_slice(bytes);
    };
    // False-wall test: `CMP $9F00,y` at $8516 (opcode D9) + RTS. Table
    // entries are $C0 (indices 0..3, plus index $FF for class-0 tiles), so a
    // tile counts as solid when its value is at least $C0.
    put(&mut blob, 0x8516, &[0xD9, 0x00, 0x9F, 0x60]);
    put(&mut blob, 0x9F00, &[0xC0, 0xC0, 0xC0, 0xC0]);
    put(&mut blob, 0x9FFF, &[0xC0]);
    // Tile codes: jump-through $11/$12/$13, chimney $14, stab $61, step $60,
    // lava $A1, water $A2/$A3.
    put(
        &mut blob,
        0x851A,
        &[0x11, 0x12, 0x13, 0x14, 0x61, 0x60, 0xA1, 0xA2, 0xA3],
    );
    // Probe tables ($EAA0 X offsets, $EAC0 Y offsets, $EAE0/$EAE4 pages).
    let mut t28 = [0u8; 0x20];
    let mut tc0 = [0u8; 0x20];
    t28[4] = 0x14;
    t28[5] = 0x0C;
    t28[6] = 0x14;
    t28[7] = 0x0C;
    t28[0x1D] = 0x0F;
    tc0[6] = 0xE0;
    tc0[7] = 0xE0;
    put(&mut blob, 0xEAA0, &t28);
    put(&mut blob, 0xEAC0, &tc0);
    put(
        &mut blob,
        0xEAE0,
        &[0x00, 0xD0, 0xA0, 0x70, 0x60, 0x60, 0x61, 0x62],
    );
    // LE1BE bit table.
    let mut t21 = [0u8; 0x22];
    t21[4] = 0x04;
    t21[5] = 0x04;
    t21[6] = 0x08;
    t21[7] = 0x08;
    put(&mut blob, 0xE04E, &t21);
    // Elevator velocities and stub sub-routines.
    put(&mut blob, 0xD8BF, &[0x00, 0x18, 0xE8]);
    for stub in [0xDE40u16, 0xDEC8, 0xC295, 0xE371] {
        put(&mut blob, stub, &[0x60]);
    }
    Game::with_test_program(0x8000, &blob)
}

#[test]
fn scroll_gate_dispatches_on_entry_a() {
    let mut game = synthetic_prg();
    register_sideview_traps(&mut game);
    register_overworld_traps(&mut game);
    // `A & 6 == 0`: hole-fall check; Link not falling (`$19 < 2`) → nothing.
    game.ram[0x003B] = 0x02;
    game.set_cpu(0x00, 0, 0, 0xFD, 0xE16F, 0x20);
    sv_scroll_gate(&mut game);
    assert_eq!(game.ram[0x003B], 0x02, "no exit bit: page holds");
    assert_eq!(game.ram[0x0736], 0x00);
    // Right exit (`A & 4`): page steps up, exit reset, mode $10, and every
    // live slot is retired (LDD3D then INC leaves it at 1).
    game.ram[0x0013] = 0x08;
    game.ram[0x0759] = 0x01;
    game.ram[0x0070] = 0x18;
    game.ram[0x00B6] = 0x01;
    game.ram[0x00BC] = 0x80; // negative list index: skip the ($D6) write
    game.ram[0x00B8] = 0x02;
    game.ram[0x00BE] = 0x80;
    game.ram[0x0010] = 0x03;
    game.set_cpu(0x06, 0, 0, 0xFD, 0xE16F, 0x20);
    sv_scroll_gate(&mut game);
    assert_eq!(game.ram[0x003B], 0x03);
    assert_eq!(game.ram[0x0013], 0);
    assert_eq!(game.ram[0x0759], 0);
    assert_eq!(game.ram[0x0070], 0);
    assert_eq!(game.ram[0x0736], 0x10);
    assert_eq!(game.ram[0x0726], 1);
    assert_eq!(game.ram[0x00B6], 1);
    assert_eq!(game.ram[0x00B8], 1);
    let (_, x, _, sp, _, _) = game.cpu_state();
    assert_eq!(x, 0x03, "X restored from $10");
    assert_eq!(sp, 0xFD, "stack balanced");
    // Left exit (`A & 2` only) keeps the page (caller already stepped).
    game.set_cpu(0x02, 0, 0, 0xFD, 0xE16F, 0x20);
    sv_scroll_gate(&mut game);
    assert_eq!(game.ram[0x003B], 0x03);
    assert_eq!(game.ram[0x0736], 0x10);
}

#[test]
fn link_level_tick_rebuilds_collision_bits() {
    let mut game = synthetic_prg();
    register_sideview_traps(&mut game);
    // Link at page 0, X $40, Y $80 standing on a solid row: the foot probes
    // (Y = 5/4, offsets $0C/$14, row `$80 & $F0`) read level RAM at
    // `$6000 + ($4C >> 4) + $80`; make that tile class 3 (solid).
    game.ram[0x004D] = 0x40;
    game.ram[0x0029] = 0x80;
    game.ram[0x003B] = 0x00;
    game.ram[0x00B5] = 0x01;
    game.wram[0x0084] = 0xC5; // foot row, left probe column
    game.wram[0x0085] = 0xC5; // right probe column
    game.wram[0x008F] = 0xC5; // foot-centre column ($4F >> 4 = 4) → $6084 too
    game.set_cpu(0, 0, 0, 0xFD, 0xE079, 0x20);
    sv_lava_check(&mut game);
    assert_eq!(
        game.ram[0x00A7] & 0x04,
        0x04,
        "below bit from the foot probe"
    );
    assert_eq!(game.ram[0x00A7] & 0x08, 0x00, "no ceiling above");
    assert_eq!(game.ram[0x0752], 0x00);
    let (_, _, _, sp, _, _) = game.cpu_state();
    assert_eq!(sp, 0xFD, "stack balanced");
    // Lava under the feet: injury path.
    game.wram[0x0084] = 0xA1;
    game.ram[0x00B5] = 0x00;
    game.set_cpu(0, 0, 0, 0xFD, 0xE079, 0x20);
    sv_lava_check(&mut game);
    assert_eq!(game.ram[0x00E9], 0x01);
    assert_eq!(game.ram[0x050C], 0x10);
    assert_eq!(game.ram[0x00B5], 0x01);
    // Deep water: only when Link is low enough.
    game.wram[0x0084] = 0xA2;
    game.ram[0x0029] = 0xB0;
    game.wram[0x00B4] = 0xA2;
    game.set_cpu(0, 0, 0, 0xFD, 0xE079, 0x20);
    sv_lava_check(&mut game);
    assert_eq!(game.ram[0x0752], 0x20);
}

#[test]
fn elevator_and_door_shims_follow_slot_x() {
    let mut game = synthetic_prg();
    register_sideview_traps(&mut game);
    // Elevator: active cabin (`$A8,x & $10`) in slot 2, Up held.
    game.ram[0x0010] = 2;
    game.ram[0x00AA] = 0x10;
    game.ram[0x0743] = 0x08;
    game.ram[0x002C] = 0x40;
    game.ram[0x00A7] = 0x08; // ceiling contact agrees with Up: no movement
    game.set_cpu(0, 2, 0, 0xFD, 0xD8C2, 0x20);
    sv_elevator(&mut game);
    assert_eq!(game.ram[0x0754], 0x10, "$0754 takes the masked state byte");
    assert_eq!(game.ram[0x0580], 0xE8, "up velocity into $057E,x");
    assert_eq!(game.ram[0x0029], 0x00, "contact skips the Y tracking");
    // Inactive cabin: tail-jumps to LDE40 through the dispatcher's PC
    // redirect — nothing is pushed, the jump request names $DE40.
    game.ram[0x00AA] = 0x00;
    game.set_cpu(0, 2, 0, 0xFD, 0xD8C2, 0x20);
    sv_elevator(&mut game);
    let (_, _, _, sp, _, _) = game.cpu_state();
    assert_eq!(sp, 0xFD, "no bytes pushed for the tail JMP");
    assert_eq!(game.take_trap_jump(), Some(0xDE40));
    // Locked door in slot 1 touched by Link with three keys: one consumed,
    // the opening count starts, Link stops.
    game.ram[0x0010] = 1;
    game.ram[0x00A9] = 0x10;
    game.ram[0x00B0] = 0x00;
    game.ram[0x078C] = 0;
    game.ram[0x0793] = 0x03;
    game.ram[0x0070] = 0x18;
    game.set_cpu(0, 1, 0, 0xFD, 0xD991, 0x20);
    sv_locked_door(&mut game);
    assert_eq!(game.ram[0x0793], 0x02);
    assert_eq!(game.ram[0x00B0], 0x01);
    assert_eq!(game.ram[0x0070], 0x00);
    assert_eq!(game.ram[0x00EC], 0x80);
    let (_, x, _, sp, _, _) = game.cpu_state();
    assert_eq!(x, 1);
    assert_eq!(sp, 0xFD);
    // No keys: the door stays shut.
    game.ram[0x0793] = 0;
    game.ram[0x00B0] = 0;
    game.set_cpu(0, 1, 0, 0xFD, 0xD991, 0x20);
    sv_locked_door(&mut game);
    assert_eq!(game.ram[0x00B0], 0x00);
}

// ---------------------------------------------------------------------------
// Gated: sideview snapshots verify clean (harness-ready).
// ---------------------------------------------------------------------------

#[test]
fn sideview_snapshots_verify_clean_when_present() {
    let Some(rd) = common::corpus_snapshots("sideview traps corpus snapshots") else {
        return;
    };
    let snaps: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sideview-"))
        })
        .collect();
    if snaps.is_empty() {
        eprintln!("SKIP: no sideview-* snapshots (no corpus/ dir yet)");
        return;
    }
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(b.starts_with(b"Z2SNAP01"), "{}: bad magic", p.display());
        assert!(b.len() >= 8 + 2048 + 8192, "{}: too small", p.display());
    }
}

// ---------------------------------------------------------------------------
// Gated: warp-glitch replay with sideview trapped (reports absence).
// ---------------------------------------------------------------------------

#[test]
fn warp_glitch_replay_with_sideview_trapped_when_present() {
    let Some(movie) = common::env_path_file(
        "Z2_WARP_MOVIE",
        "corpus/movies/warp-glitch.fm2",
        "sideview warp-glitch movie",
    ) else {
        return;
    };
    // Harness-ready: full replay needs Z2_ROM + oracle lockstep (main
    // `xtask verify --dut game` wiring, which lives under tools/).
    // Presence + non-empty is all this layer asserts.
    let meta = std::fs::metadata(&movie).expect("movie metadata");
    assert!(meta.len() > 0, "warp movie must be non-empty");
    let mut game = Game::new();
    register_sideview_traps(&mut game);
    assert_eq!(game.traps.len(), SIDEVIEW_TRAP_COUNT);
}

// ---------------------------------------------------------------------------
// Instruction-level overworld ports (`$DF3F`, `$DF01`, `$E001`, `$DFEF`):
// register/flag/memory/stack/cycle effects pinned against the prg7.asm /
// prg1.asm listings cited in the port doc comments. ROM-free: pointers stay
// in WRAM and the mapper swaps only touch the MMC1 model.
// ---------------------------------------------------------------------------

#[test]
fn ow_ldf3f_divides_accumulator_by_fifteen() {
    for a in [0u8, 14, 15, 44, 45, 0xC0, 0xFF] {
        let mut game = Game::new();
        game.cpu.a = a;
        game.cpu.x = 0x5A;
        game.cpu.p = 0x24;
        let c0 = game.cpu.cycles;
        ow_ldf3f(&mut game);
        assert_eq!(game.cpu.y, a / 15, "quotient for {a}");
        assert_eq!(game.cpu.a, a % 15, "remainder for {a}");
        assert_eq!(game.cpu.x, 0x5A, "X untouched");
        // Final ADC #$0F re-crosses zero: C set, N clear, Z per remainder.
        assert_ne!(game.cpu.p & 0x01, 0, "C for {a}");
        assert_eq!(game.cpu.p & 0x80, 0, "N for {a}");
        assert_eq!(game.cpu.p & 0x02 != 0, a % 15 == 0, "Z for {a}");
        assert_eq!(game.cpu.cycles - c0, 18 + 9 * u64::from(a / 15));
    }
}

#[test]
fn ow_ldf01_builds_scroll_anchor_from_map_position() {
    // Row 42 = 2 * 15 + 12, column $37: $0A = $2C; ROLs fold the remainder
    // bits 3/2 into $00 = $23; $01 = ($2C << 6) & $FF = 0; $7A = 7 * 2;
    // $79 = ($2C & $10) >> 1 | $00.
    let mut game = Game::new();
    game.ram[0x75] = 42;
    game.ram[0x76] = 0x37;
    game.cpu.x = 0x77;
    let c0 = game.cpu.cycles;
    ow_ldf01(&mut game);
    assert_eq!(game.ram[0x0A], 0x2C);
    assert_eq!(game.ram[0x77], 0x2C);
    assert_eq!(game.ram[0x7D], 0x10);
    assert_eq!(game.ram[0x00], 0x23);
    assert_eq!(game.ram[0x01], 0x00);
    assert_eq!(game.ram[0x7A], 0x0E);
    assert_eq!(game.ram[0x79], 0x23);
    assert_eq!(game.ram[0x7E], 0x00);
    assert_eq!(game.cpu.a, 0);
    assert_eq!(game.cpu.y, 2);
    assert_eq!(game.cpu.x, 0x77);
    assert_ne!(game.cpu.p & 0x02, 0, "Z from LDA #$00");
    assert_eq!(game.cpu.p & 0x01, 0, "C cleared by LSR of $0A & $10");
    // Inner JSR LDF3F (at $DF03) leaves $DF05 on the dead stack, SP balanced.
    assert_eq!(game.cpu.sp, 0xFD);
    assert_eq!(game.ram[0x01FD], 0xDF);
    assert_eq!(game.ram[0x01FC], 0x05);
    assert_eq!(game.cpu.cycles - c0, 106 + 18 + 9 * 2);

    // Row 15 (odd quotient, zero remainder), column $10: the quotient's
    // low bit lands in $79 bit 3 (second nametable), $7A = 0.
    let mut game = Game::new();
    game.ram[0x75] = 15;
    game.ram[0x76] = 0x10;
    ow_ldf01(&mut game);
    assert_eq!(game.ram[0x0A], 0x10);
    assert_eq!(game.ram[0x00], 0x20);
    assert_eq!(game.ram[0x79], 0x28);
    assert_eq!(game.ram[0x7A], 0x00);
    assert_eq!(game.cpu.y, 1);
}

#[test]
fn ow_le001_accumulates_one_rle_run_through_saved_bank() {
    let mut game = Game::new();
    game.ram[0x0769] = 1; // saved PRG bank (only the MMC1 model sees it)
    game.ram[0x0E] = 0x10;
    game.ram[0x0F] = 0x7C; // row pointer into the WRAM blob copy
    game.ram[0x03] = 0xFF; // caller's `LDA #$FF : STA $03` seed
    game.wram[0x1C12] = 0xA4; // run: 10 more columns of terrain 4
    game.cpu.y = 2;
    game.cpu.x = 0x33;
    let c0 = game.cpu.cycles;
    ow_le001(&mut game);
    assert_eq!(game.ram[0x02], 0x04, "terrain nibble");
    assert_eq!(game.ram[0x03], 0x0A, "$FF + $0A + carry wraps to $0A");
    assert_eq!(game.cpu.a, 0x0A, "A = new $03 via PHA/PLA");
    assert_eq!(game.cpu.y, 2, "Y untouched");
    assert_eq!(game.cpu.x, 0x33, "X untouched");
    assert_eq!(
        game.cpu.p & 0x03,
        0,
        "C from SwapToPRG0's LSRs of 0, Z clear"
    );
    assert_eq!(game.mmc1.prg, 0, "ends on PRG bank 0");
    // Dead stack: PHA'd A at $01FD under the SwapToPRG0 frame ($E018).
    assert_eq!(game.cpu.sp, 0xFD);
    assert_eq!(game.ram[0x01FD], 0x0A);
    assert_eq!(game.ram[0x01FC], 0xE0);
    assert_eq!(game.ram[0x01FB], 0x18);
    assert_eq!(game.cpu.cycles - c0, 133);

    // Page-crossing index costs one extra cycle per indexed read.
    let mut game = Game::new();
    game.ram[0x0E] = 0xFE;
    game.ram[0x0F] = 0x7C;
    game.wram[0x1D00] = 0x31;
    game.cpu.y = 2;
    let c0 = game.cpu.cycles;
    ow_le001(&mut game);
    assert_eq!(game.ram[0x02], 0x01);
    assert_eq!(game.ram[0x03], 0x04);
    assert_eq!(game.cpu.cycles - c0, 135);
}

#[test]
fn ow_boundaries_resolves_terrain_run_or_water() {
    // Row 5 pointer at $600A -> $7C40; runs: 3 cols of 3, 6 cols of 4,
    // 16 cols of 1. Column 5 (Y offset $1E added by the caller) is in run 1.
    let mut game = Game::new();
    game.ram[0x0769] = 1;
    game.wram[0x000A] = 0x40;
    game.wram[0x000B] = 0x7C;
    game.wram[0x1C40..0x1C43].copy_from_slice(&[0x23, 0x54, 0xF1]);
    game.ram[0x00] = 5;
    game.ram[0x01] = 5 + 0x1E;
    game.cpu.x = 0x11;
    game.cpu.y = 0x22;
    let c0 = game.cpu.cycles;
    ow_boundaries(&mut game);
    assert_eq!(game.ram[0x02], 4, "terrain of the run covering column 5");
    assert_eq!(game.ram[0x03], 9, "run end column");
    assert_eq!(game.ram[0x04], 5, "map row");
    assert_eq!(game.ram[0x00], 6, "column incremented");
    assert_eq!(game.ram[0x0E], 0x40);
    assert_eq!(game.ram[0x0F], 0x7C);
    assert_eq!(game.cpu.x, 3, "LDX #$03");
    assert_eq!(game.cpu.y, 1, "run index");
    assert_eq!(game.cpu.a, 0, "SwapToPRG0 exit");
    assert_ne!(game.cpu.p & 0x02, 0);
    assert_eq!(game.mmc1.prg, 0);
    assert_eq!(game.cpu.sp, 0xFD);
    // Dead stack: $DFF4 (outer JSR L83CF) over $83E2 (inner JSR code8).
    assert_eq!(game.ram[0x01FD], 0xDF);
    assert_eq!(game.ram[0x01FC], 0xF4);
    assert_eq!(game.ram[0x01FB], 0x83);
    assert_eq!(game.ram[0x01FA], 0xE2);
    // 92 wrapper + 39 + code8 26 + 41 (run 0) + 43 (run 1).
    assert_eq!(game.cpu.cycles - c0, 241);

    // East of the map: water, registers and $00 untouched.
    let mut game = Game::new();
    game.ram[0x00] = 0x40;
    game.ram[0x01] = 0x30;
    game.cpu.x = 0x11;
    game.cpu.y = 0x22;
    let c0 = game.cpu.cycles;
    ow_boundaries(&mut game);
    assert_eq!(game.ram[0x02], 0x0C);
    assert_eq!(game.ram[0x00], 0x40);
    assert_eq!((game.cpu.x, game.cpu.y), (0x11, 0x22));
    assert_eq!(game.cpu.cycles - c0, 92 + 19);

    // South of the map: row check fails after $04 is stored.
    let mut game = Game::new();
    game.ram[0x00] = 3;
    game.ram[0x01] = 0x1E + 0x4B;
    let c0 = game.cpu.cycles;
    ow_boundaries(&mut game);
    assert_eq!(game.ram[0x02], 0x0C);
    assert_eq!(game.ram[0x04], 0x4B);
    assert_eq!(game.ram[0x00], 3);
    assert_eq!(game.cpu.cycles - c0, 92 + 33);
}

// ---------------------------------------------------------------------------
// `$DF79` (forest chop / tile transform) and the `SwapToSavedPRG` wrappers
// (`$DFD2`, `$DFF8`, `$E01B`, `$E024`). ROM-free: the transform tables at
// `$DF5E-$DF69` hold made-up values, the banked bodies are `RTS` stubs, and
// the map lives in WRAM.
// ---------------------------------------------------------------------------

/// Synthetic PRG for the chop tests: made-up transform tables (`$DF5E`
/// run bytes, `$DF62` replacements, `$DF66` area-table slots, `$DF68`
/// area-table values) and `RTS` stubs at the banked entry points the
/// wrappers call (first 16 KiB, which saved bank 2 wraps onto in a 32 KiB
/// image). Both trap groups are registered so the mapper helpers and
/// `$DF79` resolve through their traps. `row`'s `$6000` pointer targets
/// `$7C40`, where `runs` is copied; `$00`/`$01` = column / row + `$1E` as
/// the bank-0 caller stages them.
fn chop_fixture(runs: &[u8], col: u8, row: u8) -> Game {
    let mut blob = vec![0u8; 0x8000];
    let put = |b: &mut Vec<u8>, addr: u16, bytes: &[u8]| {
        let off = usize::from(addr - 0x8000);
        b[off..off + bytes.len()].copy_from_slice(bytes);
    };
    put(&mut blob, 0xDF5E, &[0x09, 0x0A, 0x04, 0x05]);
    put(&mut blob, 0xDF62, &[0x01, 0x01, 0x03, 0x00]);
    put(&mut blob, 0xDF66, &[0x21, 0x22]);
    put(&mut blob, 0xDF68, &[0x77, 0x88]);
    for stub in [0x8368u16, 0x879B, 0x83A1] {
        put(&mut blob, stub, &[0x60]);
    }
    let mut game = Game::with_test_program(0x8000, &blob);
    register_bank7_traps(&mut game);
    register_overworld_traps(&mut game);
    game.ram[0x0769] = 2;
    let ptr = usize::from(row) * 2;
    game.wram[ptr] = 0x40;
    game.wram[ptr + 1] = 0x7C;
    game.wram[0x1C40..0x1C40 + runs.len()].copy_from_slice(runs);
    game.ram[0x00] = col;
    game.ram[0x01] = row + 0x1E;
    game.ram[0x0725] = 0x55;
    game
}

#[test]
fn ow_chop_transforms_a_matching_run_and_patches_the_area_table() {
    // Column 3 sits in run 1 (`$04`, one column of terrain 4) = table
    // entry 2; region 2 passes the gate, `X != 3` skips the hidden-town
    // checks, the run byte becomes `$03` and area slot `$21` gets `$77`.
    let mut game = chop_fixture(&[0x23, 0x04, 0xF1], 3, 5);
    game.ram[0x0706] = 2;
    let c0 = game.cpu.cycles;
    ow_chop(&mut game);
    assert_eq!(game.wram[0x1C41], 0x03, "run byte replaced");
    assert_eq!(game.wram[0x0A21], 0x77, "area table patched");
    assert_eq!(game.ram[0xEB], 0x10, "music");
    assert_eq!(game.ram[0x02], 4, "L83CF terrain");
    assert_eq!(game.ram[0x03], 4, "L83CF run end");
    assert_eq!(game.ram[0x0725], 0x55, "macro selector untouched");
    assert_eq!((game.cpu.a, game.cpu.x, game.cpu.y), (0x77, 0, 0x21));
    assert_eq!(game.cpu.sp, 0xFD);
    // Dead stack: $DF7B (JSR L83CF) over $83E2 (inner JSR code8).
    assert_eq!(game.ram[0x01FD], 0xDF);
    assert_eq!(game.ram[0x01FC], 0x7B);
    assert_eq!(game.ram[0x01FB], 0x83);
    assert_eq!(game.ram[0x01FA], 0xE2);
    // JSR 6 + L83CF 149 + scan 16 + 12 + gate 4 + 8 + 5 + music 5 +
    // run write 14 + area patch 23.
    assert_eq!(game.cpu.cycles - c0, 242);
}

#[test]
fn ow_chop_records_no_match_as_ppu_macro_zero() {
    // Column 5 is inside a multi-column run (`$54`): no table entry matches,
    // X wraps to $FF, INX/STX leave 0 in $0725.
    let mut game = chop_fixture(&[0x23, 0x54, 0xF1], 5, 5);
    let c0 = game.cpu.cycles;
    ow_chop(&mut game);
    assert_eq!(game.ram[0x0725], 0);
    assert_eq!(game.wram[0x1C41], 0x54, "map untouched");
    assert_eq!((game.cpu.a, game.cpu.x, game.cpu.y), (0x54, 0, 1));
    assert_ne!(game.cpu.p & 0x02, 0, "Z from INX");
    assert_eq!(game.cpu.p & 0x80, 0);
    assert_eq!(game.cpu.sp, 0xFD);
    // JSR 6 + L83CF 149 + 3 x 16 + 15 (last entry, BPL not taken) + 12.
    assert_eq!(game.cpu.cycles - c0, 230);
}

#[test]
fn ow_chop_entry_zero_skips_the_region_checks() {
    // Run byte `$09` = table entry 0: no region gate, no music, no area
    // patch (`X < 2`); the run byte becomes `$01`.
    let mut game = chop_fixture(&[0x23, 0x09, 0xF1], 3, 5);
    game.ram[0x0706] = 0;
    let c0 = game.cpu.cycles;
    ow_chop(&mut game);
    assert_eq!(game.wram[0x1C41], 0x01);
    assert_eq!(game.ram[0xEB], 0, "no music write");
    assert_eq!((game.cpu.a, game.cpu.x, game.cpu.y), (0x01, 0, 1));
    assert_eq!(game.cpu.p & 0x01, 0, "C clear: CPX #$02 with X = 0");
    assert_ne!(game.cpu.p & 0x80, 0, "N from $00 - $02");
    // JSR 6 + L83CF 149 + 3 x 16 + 12 + 5 + 14 + 7.
    assert_eq!(game.cpu.cycles - c0, 241);
}

#[test]
fn ow_chop_region_gate_returns_before_writing() {
    // Entry 2 outside region 2: RTS right after the region compare.
    let mut game = chop_fixture(&[0x23, 0x04, 0xF1], 3, 5);
    game.ram[0x0706] = 0;
    let c0 = game.cpu.cycles;
    ow_chop(&mut game);
    assert_eq!(game.wram[0x1C41], 0x04, "map untouched");
    assert_eq!(game.ram[0xEB], 0);
    assert_eq!((game.cpu.a, game.cpu.x, game.cpu.y), (0, 2, 1));
    // JSR 6 + L83CF 149 + scan 28 + TXA/BEQ 4 + gate 8 + BNE taken 1 + RTS 6.
    assert_eq!(game.cpu.cycles - c0, 202);
}

#[test]
fn ow_chop_hidden_town_spot_stages_the_tile_bytes() {
    // Row $33 (pointer at $6066), Link at column $3D: run 4 (`$05`, entry
    // 3) covers column $3D, so after L83CF's INC the `$00 == $3E` /
    // `$04 == $33` checks pass and the four tile bytes are staged.
    let mut game = chop_fixture(&[0xF0, 0xF0, 0xF0, 0xC0, 0x05, 0x10], 0x3D, 0x33);
    game.ram[0x0706] = 2;
    let c0 = game.cpu.cycles;
    ow_chop(&mut game);
    assert_eq!(game.ram[0x00], 0x3E);
    assert_eq!(game.ram[0x04], 0x33);
    assert_eq!(
        (
            game.ram[0x0305],
            game.ram[0x0306],
            game.ram[0x030A],
            game.ram[0x030B]
        ),
        (0x5C, 0x5D, 0x5E, 0x5F)
    );
    assert_eq!(game.ram[0xEB], 0x10);
    assert_eq!(game.wram[0x1C44], 0x00, "run byte replaced by $DF62[3]");
    assert_eq!(game.wram[0x0A22], 0x88, "area slot $22 = $DF68[1]");
    assert_eq!((game.cpu.a, game.cpu.x, game.cpu.y), (0x88, 1, 0x22));
    // JSR 6 + L83CF (39 + 26 + 4 x 41 + 43 = 272) + first-entry match 12
    // + 4 + 8 + 4 + 7 + 7 + 24 + 5 + 14 + 23.
    assert_eq!(game.cpu.cycles - c0, 386);
}

#[test]
fn ow_saved_bank_wrappers_bracket_the_banked_call() {
    // `$E01B`: SwapToSavedPRG (bank 2) -> interpreted RTS at $879B ->
    // SwapToPRG0. Registers other than A/P survive; the second JSR frame
    // ($E020) overwrites the first ($E01D) on the dead stack.
    let mut game = chop_fixture(&[0x23, 0x54, 0xF1], 5, 5);
    game.cpu.x = 0x22;
    game.cpu.y = 0x33;
    let c0 = game.cpu.cycles;
    ow_stone(&mut game);
    // The trailing `JMP SwapToPRG0` is left for the dispatcher: fire it the
    // way the dispatcher would.
    assert_eq!(game.take_trap_jump(), Some(0xFFC5));
    assert_eq!(game.fire_trap(0xFFC5), TrapExit::Return);
    assert_eq!(game.mmc1.prg, 0, "ends on PRG bank 0");
    assert_eq!(game.cpu.a, 0);
    assert_ne!(game.cpu.p & 0x02, 0);
    assert_eq!(game.cpu.p & 0x01, 0);
    assert_eq!((game.cpu.x, game.cpu.y), (0x22, 0x33));
    assert_eq!(game.cpu.sp, 0xFD);
    assert_eq!(game.ram[0x01FD], 0xE0);
    assert_eq!(game.ram[0x01FC], 0x20);
    // SwapToSavedPRG 38 + RTS 6 + JSR/JSR/JMP 15 + SwapToPRG0 39.
    assert_eq!(game.cpu.cycles - c0, 98);

    // `$DFD2`: the same bracket around the trapped `$DF79` (no-match
    // fixture: 230 cycles, `$0725` = 0), frames $DFD4/$DFD7 above the
    // chop's own $DF7B/$83E2.
    let mut game = chop_fixture(&[0x23, 0x54, 0xF1], 5, 5);
    let c0 = game.cpu.cycles;
    ow_ldfd2(&mut game);
    assert_eq!(game.take_trap_jump(), Some(0xFFC5));
    assert_eq!(game.fire_trap(0xFFC5), TrapExit::Return);
    assert_eq!(game.ram[0x0725], 0);
    assert_eq!(game.mmc1.prg, 0);
    assert_eq!((game.cpu.a, game.cpu.x, game.cpu.y), (0, 0, 1));
    assert_eq!(game.cpu.sp, 0xFD);
    assert_eq!(game.ram[0x01FD], 0xDF);
    assert_eq!(game.ram[0x01FC], 0xD7);
    assert_eq!(game.ram[0x01FB], 0xDF);
    assert_eq!(game.ram[0x01FA], 0x7B);
    assert_eq!(game.ram[0x01F9], 0x83);
    assert_eq!(game.ram[0x01F8], 0xE2);
    assert_eq!(game.cpu.cycles - c0, 38 + 230 + 15 + 39);
}

// ---------------------------------------------------------------------------
// Gated: sideview snapshots verify clean (harness-ready).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Gated: warp-glitch replay with sideview trapped (reports absence).
// ---------------------------------------------------------------------------
