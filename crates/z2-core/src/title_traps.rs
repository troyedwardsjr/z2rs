//! Title/save/death/game-over trap ports + registration (register-exact
//! lockstep pass).
//!
//! Every fixed-bank entry below is a register-exact port of the `prg7.asm`
//! bytes: the memory writes, `A`/`X`/`Y`/`P` at the exit `RTS` (or tail
//! `JMP`) and the cycle clock all match the ROM, so the port can stand in
//! for the bytes under lockstep. Bank-7 helpers (`SwapPRG`, `SwapCHR`,
//! `Erase_Name_Tables_0and1`, the `$D382` trampoline, `LCB18`, `$C358`)
//! go through their own traps via [`jsr_sub`]; tail jumps (`LEC02`,
//! `LCF05`) and the SRAM save continuation at `$CF26` hand control to the
//! dispatcher with [`Game::trap_jump`], which fires the target's trap when
//! it has one and interprets from there otherwise, nothing pushed.
//!
//! Most of these routines are mode-table targets reached through the
//! `$D382`/`$D385` trampolines (`JMP ($0E)`); the trampoline ports reach
//! them the same way, so they fire in the default configuration as well as
//! with the trampolines interpreted (`--untrap D382,D385`). `LCF05`,
//! `LCB18`, `$C358` and `LC360` are also direct `JSR`/`JMP` targets.
//!
//! | shim | label | addr |
//! |---|---|---|
//! | [`tt_title_mode`] | `bank7_code7` | `$C33C` |
//! | [`tt_lives3`] | `bank7_Reset_Number_of_Lives__to_3_` | `$C358` |
//! | [`tt_state_inc`] | `LC360` | `$C360` |
//! | [`tt_ready_setup`] | `bank7_code10` | `$C3A5` |
//! | [`tt_lives_screen`] | `bank7_Load_Lives_Remaining_Screen` | `$C3B5` |
//! | [`tt_ready_hold`] | `LC3E6` | `$C3E6` |
//! | [`tt_ready_go`] | `LC3ED` | `$C3ED` |
//! | [`tt_respawn`] | `bank7_code11` | `$C41E` |
//! | [`tt_death_dispatch`] | `LCA1B` | `$CA1B` |
//! | [`tt_die`] | `bank7_code16` | `$CA24` |
//! | [`tt_new_life`] | `LCA6C` | `$CA6C` |
//! | [`tt_gameover_wait`] | `LCA72` | `$CA72` |
//! | [`tt_gameover_choice`] | `LCA85` | `$CA85` |
//! | [`tt_refill`] | `LCB18_fill_hp_or_mp_to_full__provide_x_register__maybe` | `$CB18` |
//! | [`tt_mode_inc`] | `LCF05` | `$CF05` |
//! | [`tt_save_on_save`] | `LCF21_SaveGameWhenChooseSAVEwhenDead__maybe` | `$CF21` |
//!
//! Cycle accounting: every port charges its own per-path cost through
//! [`cyc`] (base + taken-branch + page-cross extras, `cpu.rs` model) and is
//! registered with an explicit cost of `0`; the one exception is `LCF05`,
//! whose flat 12 lives in [`crate::traps::default_trap_cycles`] because it
//! is also a boot-path tail-call target pinned by the cycle tests.
//!
//! Banked code (every bank-0 `$8000-$BFFF` and bank-5 `$8000-$BFFF`
//! entry) is listed in [`TITLE_TRAPS`] with `bank = None` and
//! intentionally *not* registered: 16-bit trap keys alias across
//! `$8000-$BFFF` (see `traps.rs` M1 caveat), and bank 0 vs bank 5 share
//! the window. Same rule as the banked entries in
//! [`register_sideview_traps`](crate::sideview_traps::register_sideview_traps)
//! and [`register_town_traps`](crate::town_traps::register_town_traps):
//! only `Some(7)` (fixed-bank) entries are registered today.

use crate::bank7_common::{
    adc_val, cmp_val, inc_val, jsr_sub, pop_word, push_word, sbc_val, set_nz,
};
use crate::cpu::{bus_read, bus_write, FLAG_C, FLAG_N, FLAG_V, FLAG_Z};
use crate::game::Game;
use crate::traps::TrapExit;

// ---------------------------------------------------------------------------
// Trap table.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Registration table for `main` to wire into the trap dispatcher.
///
/// Fixed-bank (`Some(7)`) entries are safe today; `None` entries are
/// data-only (aliasing caveat above) and skipped by
/// [`register_title_traps`].
pub const TITLE_TRAPS: &[TrapEntry] = &[
    // Fixed bank: registered.
    ("bank7_code7", Some(7), 0xC33C),
    ("bank7_Reset_Number_of_Lives__to_3_", Some(7), 0xC358),
    ("LC360", Some(7), 0xC360),
    ("bank7_code10", Some(7), 0xC3A5),
    ("bank7_Load_Lives_Remaining_Screen", Some(7), 0xC3B5),
    ("LC3E6", Some(7), 0xC3E6),
    ("LC3ED", Some(7), 0xC3ED),
    ("bank7_code11", Some(7), 0xC41E),
    ("LCA1B", Some(7), 0xCA1B),
    ("bank7_code16", Some(7), 0xCA24),
    ("LCA6C", Some(7), 0xCA6C),
    ("LCA72", Some(7), 0xCA72),
    ("LCA85", Some(7), 0xCA85),
    (
        "LCB18_fill_hp_or_mp_to_full__provide_x_register__maybe",
        Some(7),
        0xCB18,
    ),
    ("LCF05", Some(7), 0xCF05),
    ("LCF21_SaveGameWhenChooseSAVEwhenDead__maybe", Some(7), 0xCF21),
    // Bank 0: game-over text/palettes, manual save, new-game init.
    ("Tables_for_Game_Over_screen_text", None, 0x8000),
    ("L8001", None, 0x8001),
    ("L800E", None, 0x800E),
    (
        "Tables_for_Ganon_Shadow_Tile_Mapping_and_Palette_Mapping",
        None,
        0x801F,
    ),
    ("bank0_Return_of_Ganon_screen_Palettes", None, 0x80C9),
    ("bank0_Manual_Save_Game_Routine_UP_AND_A", None, 0xA19C),
    ("LA1A5", None, 0xA1A5),
    (
        "bank0_Table_For_PPU_Instructions_On_GameOverScreen_45_loaded_in_6957",
        None,
        0xA99C,
    ),
    ("startup_init_begin_game", None, 0xAA08),
    ("LAA28", None, 0xAA28),
    // Bank 5: title / file-select / SRAM / ending.
    ("bank5_PowerON__Reset_Memory", None, 0xA6A0),
    ("LA6D9", None, 0xA6D9),
    ("bank5_code19", None, 0xA6F0),
    ("bank5_code20", None, 0xA70F),
    ("LA72E", None, 0xA72E),
    ("LA737", None, 0xA737),
    ("bank5_Intro_Sprites", None, 0xA7C1),
    ("LAB6D", None, 0xAB6D),
    ("bank5_table_intro_screen_text", None, 0xA932),
    ("LAA08", None, 0xAA08),
    ("bank5_routines_related_to_Ending_sequence", None, 0x8B50),
    ("L8B69", None, 0x8B69),
    ("bank5_Ending_Text_Zelda_", None, 0x8DDE),
    ("L921C", None, 0x921C),
    ("L9248", None, 0x9248),
    ("L9255", None, 0x9255),
    ("bank5_Pointer_table_for_End_Credits", None, 0x9259),
    ("bank5_End_Credits", None, 0x927D),
    ("bank5_code22", None, 0xB24E),
    ("bank5_Load_Saved_Games_Data", None, 0xB261),
    ("LB28B", None, 0xB28B),
    ("LB2AA", None, 0xB2AA),
    ("LB2B4", None, 0xB2B4),
    (
        "function_reset_link_stats_to_beginning_values",
        None,
        0xB2CA,
    ),
    ("bank5_Load_Initial_Item_Presence_Bits", None, 0xB2D7),
    ("LB303", None, 0xB303),
    ("LB319", None, 0xB319),
    ("LB384", None, 0xB384),
    ("bank5_code23", None, 0xB3DF),
    ("LB3F4", None, 0xB3F4),
    ("bank5_code24", None, 0xB40A),
    ("LB412", None, 0xB412),
    ("bank5_code25", None, 0xB425),
    ("LB443", None, 0xB443),
    ("bank5_elimination_mode", None, 0xB462),
    ("bank5_code26", None, 0xB502),
    ("bank5_Display_Saved_Games_Names", None, 0xB529),
    ("LB5B5", None, 0xB5B5),
    (
        "bank5_Related_to_AttackMagicLife_Levels__and_convert_numbers_to_Tile_Mappings____Load_Attack_Magic_Life_Levels",
        None,
        0xB5D8,
    ),
    ("LB623", None, 0xB623),
    ("LB63C", None, 0xB63C),
    ("LB678", None, 0xB678),
    ("LB692", None, 0xB692),
    ("LB6AC", None, 0xB6AC),
    ("LB8AE", None, 0xB8AE),
    ("LB8D2", None, 0xB8D2),
    ("LB911", None, 0xB911),
    ("LB931_stage", None, 0xB931),
    ("bank5_code27", None, 0xB960),
    ("LB978", None, 0xB978),
    ("LB99D", None, 0xB99D),
    ("LB9A7", None, 0xB9A7),
    ("LB9CA", None, 0xB9CA),
    ("LB9F8", None, 0xB9F8),
    ("LBA00", None, 0xBA00),
    ("LBA13", None, 0xBA13),
    ("LBA18", None, 0xBA18),
    ("LBA40", None, 0xBA40),
    ("LBA6C", None, 0xBA6C),
    ("LBAB8", None, 0xBAB8),
    ("bank5_pointer_table7", None, 0xBAC5),
    ("bank5_Beginning_Values", None, 0xBAE3),
    ("bank5_table_for_blanking_out_name", None, 0xBB0D),
    (
        "bank5_Initial_Item_Presence_Bits_600_61F__West_Hyrule",
        None,
        0xBB15,
    ),
    ("bank5_Tables_for_Selection_Screen_Text_", None, 0xBC19),
    ("bank5_Table_for_Letters_Tile_Mappings", None, 0xBD75),
];

/// Number of fixed-bank title traps actually registered.
pub const TITLE_TRAP_COUNT: usize = 16;

// ---------------------------------------------------------------------------
// Register-exact micro helpers (same conventions as `sideview_traps.rs`:
// the port keeps A/X/Y/P and the cycle clock aligned with the ROM bytes).
// ---------------------------------------------------------------------------

/// `LDA`-style register load: sets `N`/`Z`.
fn lda(game: &mut Game, v: u8) {
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
}

/// `LDX`-style register load: sets `N`/`Z`.
fn ldx(game: &mut Game, v: u8) {
    game.cpu.x = v;
    set_nz(&mut game.cpu.p, v);
}

/// `LDY`-style register load: sets `N`/`Z`.
fn ldy(game: &mut Game, v: u8) {
    game.cpu.y = v;
    set_nz(&mut game.cpu.p, v);
}

/// `STA addr` (no flags).
fn sta(game: &mut Game, addr: u16) {
    let a = game.cpu.a;
    bus_write(game, addr, a);
}

/// `ASL A`: `C` = old bit 7, `N`/`Z` from the result.
fn asl_a(game: &mut Game) {
    let a = game.cpu.a;
    if a & 0x80 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    game.cpu.a = a << 1;
    set_nz(&mut game.cpu.p, a << 1);
}

/// `LSR A`: `C` = old bit 0, `N` cleared, `Z` from the result.
fn lsr_a(game: &mut Game) {
    let a = game.cpu.a;
    if a & 1 != 0 {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    game.cpu.a = a >> 1;
    set_nz(&mut game.cpu.p, a >> 1);
}

/// `BIT zp`: `N`/`V` copy memory bits 7/6, `Z` = `(A & mem) == 0`.
fn bit_mem(p: &mut u8, a: u8, m: u8) {
    *p = (*p & !(FLAG_N | FLAG_V)) | (m & (FLAG_N | FLAG_V));
    if a & m == 0 {
        *p |= FLAG_Z;
    } else {
        *p &= !FLAG_Z;
    }
}

/// `INC abs` through the bus: wraps, `N`/`Z` from the new value.
fn inc_mem(game: &mut Game, addr: u16) {
    let v = bus_read(game, addr);
    let v = inc_val(&mut game.cpu.p, v);
    bus_write(game, addr, v);
}

/// `DEC abs` through the bus: wraps, `N`/`Z` from the new value.
fn dec_mem(game: &mut Game, addr: u16) -> u8 {
    let v = bus_read(game, addr).wrapping_sub(1);
    set_nz(&mut game.cpu.p, v);
    bus_write(game, addr, v);
    v
}

/// Page-cross extra cycle for an indexed read (`base` vs `base + idx`).
fn cross(base: u16, idx: u8) -> u64 {
    let ea = base.wrapping_add(u16::from(idx));
    u64::from((base & 0xFF00) != (ea & 0xFF00))
}

/// Charge `n` CPU cycles (per-path counting, `cpu.rs` cost model:
/// base + taken-branch, page-cross extras only where computed).
fn cyc(game: &mut Game, n: u64) {
    game.cpu.cycles += n;
}

/// `LDA src,y : STA dst,y : DEY : BPL` copy of `count` bytes, entered
/// with `Y = count - 1` already loaded by the caller. Exit `Y = $FF`,
/// `A` = the byte at `src`, `N` set. Cost: `count * 14 - 1` (+ page-cross
/// extras on the `LDA abs,Y` reads, + `branch_cross` per taken `BPL` when
/// the loop's back-branch crosses a page, as `$CB06 → $CAFF` does).
fn copy_desc(game: &mut Game, src: u16, dst: u16, count: u8, branch_cross: u64) {
    for i in (0..count).rev() {
        let v = bus_read(game, src.wrapping_add(u16::from(i)));
        lda(game, v);
        bus_write(game, dst.wrapping_add(u16::from(i)), v);
        ldy(game, i.wrapping_sub(1));
        let bpl = if i == 0 { 2 } else { 3 + branch_cross };
        cyc(game, 4 + cross(src, i) + 5 + 2 + bpl);
    }
}

// ---------------------------------------------------------------------------
// Shared bodies.
// ---------------------------------------------------------------------------

/// `LC3F5` (`$C3F5-$C40F`): Link's pose for the lives screen
/// (`$29 = $50`, `$CC = $78`, `$80 = 3`, `$9F = 1`, `$C8/$13/$11/$90 =
/// 0`), then `JMP LEC02` (Link sprite draw, interpreted). Exit `A = 0`,
/// `C = 1` (both `LSR`s shift a 1 out; `LEC02`'s `SBC #$04` depends on
/// it), `Z = 1`, `N = 0`. 34 cycles + the `JMP` (3).
fn respawn_pose(game: &mut Game) {
    lda(game, 0x50);
    bus_write(game, 0x0029, 0x50);
    lda(game, 0x78);
    bus_write(game, 0x00CC, 0x78);
    lda(game, 0x03);
    bus_write(game, 0x0080, 0x03);
    lsr_a(game);
    bus_write(game, 0x009F, 0x01);
    lsr_a(game);
    for a in [0x00C8u16, 0x0013, 0x0011, 0x0090] {
        bus_write(game, a, 0);
    }
    // 13 instructions (34) + JMP LEC02 (3) = 37.
    cyc(game, 2 + 3 + 2 + 3 + 2 + 3 + 2 + 3 + 2 + 3 + 3 + 3 + 3 + 3);
    game.trap_jump(0xEC02);
}

/// `bank7_FUNCTION_CONVERT_706_and_707_to_Rx5plusW` (`$CF30`):
/// `LDA $0706 : ASL : ASL : ADC $0706 : ADC $0707 : RTS` — region * 5 +
/// world, with the second `ASL`'s carry feeding the first `ADC`. Exit
/// `A` = the sum, `C`/`V`/`N`/`Z` from the last `ADC`. 22 cycles, RTS
/// included (the caller charges the `JSR`).
fn region_x5_plus_world(game: &mut Game) {
    let region = bus_read(game, 0x0706);
    lda(game, region);
    asl_a(game);
    asl_a(game);
    let a = adc_val(&mut game.cpu.p, game.cpu.a, region);
    game.cpu.a = a;
    let world = bus_read(game, 0x0707);
    let a = adc_val(&mut game.cpu.p, game.cpu.a, world);
    game.cpu.a = a;
    cyc(game, 4 + 2 + 2 + 4 + 4 + 6);
}

// ---------------------------------------------------------------------------
// Fixed-bank ports.
// ---------------------------------------------------------------------------

/// `bank7_code7` (bank 7 `$C33C`): title mode entry.
///
/// `LDA #$05 : JSR SwapPRG : JSR SwapCHR : LDA #$80 : STA $0100 : LDA #$00
/// : STA $076C : RTS`. `SwapCHR` sees `A = 5 >> 4 = 0` (SwapPRG's four
/// `LSR`s). Exit `A = 0`, `Z = 1`, `N = 0`. 32 cycles + the two swaps.
pub fn tt_title_mode(game: &mut Game) {
    lda(game, 0x05);
    cyc(game, 2 + 6);
    jsr_sub(game, 0xC33E, 0xFFCC);
    cyc(game, 6);
    jsr_sub(game, 0xC341, 0xFFB1);
    lda(game, 0x80);
    sta(game, 0x0100);
    lda(game, 0x00);
    sta(game, 0x076C);
    cyc(game, 2 + 4 + 2 + 4 + 6);
}

/// `bank7_Reset_Number_of_Lives__to_3_` (bank 7 `$C358`): `LDA #$03 :
/// STA $0700 : INC $0760`, falling into `LC360` (`INC $076C : RTS`).
/// Exit `A = 3`, `N`/`Z` from the new `$076C`. 24 cycles.
pub fn tt_lives3(game: &mut Game) {
    lda(game, 0x03);
    sta(game, 0x0700);
    inc_mem(game, 0x0760);
    inc_mem(game, 0x076C);
    cyc(game, 2 + 4 + 6 + 6 + 6);
}

/// `LC360` (bank 7 `$C360`): `INC $076C : RTS`. 12 cycles.
pub fn tt_state_inc(game: &mut Game) {
    inc_mem(game, 0x076C);
    cyc(game, 6 + 6);
}

/// `bank7_code10` (bank 7 `$C3A5`): lives-screen setup.
///
/// `LDA #$0C : STA $0725 : LDA #$00 : STA $0760 : INC $0726 : JMP LC3E2`
/// (`INC $0738 : RTS`). Exit `A = 0`, `N`/`Z` from the new `$0738`.
/// 33 cycles.
pub fn tt_ready_setup(game: &mut Game) {
    lda(game, 0x0C);
    sta(game, 0x0725);
    lda(game, 0x00);
    sta(game, 0x0760);
    inc_mem(game, 0x0726);
    inc_mem(game, 0x0738);
    cyc(game, 2 + 4 + 2 + 4 + 6 + 3 + 6 + 6);
}

/// `bank7_Load_Lives_Remaining_Screen` (bank 7 `$C3B5`).
///
/// `LDA $076E : JSR SwapCHR : JSR Erase_Name_Tables_0and1`, then stages
/// the lives packet: `$6958..+20 → $0302`, the name `$07A1..+8 → $0305`,
/// `$0314 = $0700 + $D0` (`CLC : ADC`), `$0501 = $70`, and `INC $0738`.
/// Exit `A = $70`, `Y = $FF`, `N`/`Z` from the new `$0738`, `C`/`V` from
/// the `ADC`. 440 cycles + the swap and the erase.
pub fn tt_lives_screen(game: &mut Game) {
    let chr = bus_read(game, 0x076E);
    lda(game, chr);
    cyc(game, 4 + 6);
    jsr_sub(game, 0xC3B8, 0xFFB1);
    cyc(game, 6);
    jsr_sub(game, 0xC3BB, 0xD266);
    // LDY #$13 : LC3C0 loop.
    ldy(game, 0x13);
    cyc(game, 2);
    copy_desc(game, 0x6958, 0x0302, 0x14, 0);
    // LDY #$07 : LC3CB loop.
    ldy(game, 0x07);
    cyc(game, 2);
    copy_desc(game, 0x07A1, 0x0305, 0x08, 0);
    // LDA $0700 : CLC : ADC #$D0 : STA $0314 : LDA #$70 : STA $0501 :
    // INC $0738 : RTS.
    let lives = bus_read(game, 0x0700);
    lda(game, lives);
    game.cpu.p &= !FLAG_C;
    let tile = adc_val(&mut game.cpu.p, lives, 0xD0);
    game.cpu.a = tile;
    sta(game, 0x0314);
    lda(game, 0x70);
    sta(game, 0x0501);
    inc_mem(game, 0x0738);
    cyc(game, 4 + 2 + 2 + 4 + 2 + 4 + 6 + 6);
}

/// `LC3E6` (bank 7 `$C3E6`): `LDA #$00 : STA $0726 : BEQ LC3E2`
/// (`INC $0738 : RTS`). Exit `A = 0`, `N`/`Z` from the new `$0738`.
/// 21 cycles.
pub fn tt_ready_hold(game: &mut Game) {
    lda(game, 0x00);
    sta(game, 0x0726);
    inc_mem(game, 0x0738);
    cyc(game, 2 + 4 + 3 + 6 + 6);
}

/// `LC3ED` (bank 7 `$C3ED`): `LDA $0501 : BNE LC3F5 : INC $0736`, then
/// the [`respawn_pose`] (which tail-jumps to `LEC02`).
pub fn tt_ready_go(game: &mut Game) {
    let timer = bus_read(game, 0x0501);
    lda(game, timer);
    cyc(game, 4);
    if timer != 0 {
        cyc(game, 3);
    } else {
        inc_mem(game, 0x0736);
        cyc(game, 2 + 6);
    }
    respawn_pose(game);
}

/// `bank7_code11` (bank 7 `$C41E`): respawn once `$0501` expires.
///
/// `LDA $0501 : BNE LC446`; else `LDA #$02`, doubled to 4 (`ASL`) when
/// `$0707 == 0` or (`$0707 < 3` and `$056B == 7`), `STA $075F`, then
/// `$076C = $076D = 1`, `$0736 = 7`. `LC446: JMP LC3F5` — the
/// [`respawn_pose`] and `LEC02` on every path.
pub fn tt_respawn(game: &mut Game) {
    let timer = bus_read(game, 0x0501);
    lda(game, timer);
    cyc(game, 4);
    if timer != 0 {
        // BNE LC446 taken, JMP LC3F5.
        cyc(game, 3 + 3);
        respawn_pose(game);
        return;
    }
    cyc(game, 2);
    // LDA #$02 : LDY $0707 : BEQ LC435
    lda(game, 0x02);
    let world = bus_read(game, 0x0707);
    ldy(game, world);
    cyc(game, 2 + 4 + 2);
    let mut double = false;
    if world == 0 {
        cyc(game, 1);
        double = true;
    } else {
        // CPY #$03 : BCS LC436
        cmp_val(&mut game.cpu.p, world, 0x03);
        cyc(game, 2 + 2);
        if world >= 3 {
            cyc(game, 1);
        } else {
            // LDY $056B : CPY #$07 : BNE LC436
            let town = bus_read(game, 0x056B);
            ldy(game, town);
            cmp_val(&mut game.cpu.p, town, 0x07);
            cyc(game, 4 + 2 + 2);
            if town != 7 {
                cyc(game, 1);
            } else {
                double = true;
            }
        }
    }
    if double {
        // LC435: ASL
        asl_a(game);
        cyc(game, 2);
    }
    // LC436: STA $075F : LDA #$01 : STA $076C : STA $076D : LDA #$07 :
    // STA $0736 : (LC446) JMP LC3F5
    sta(game, 0x075F);
    lda(game, 0x01);
    sta(game, 0x076C);
    sta(game, 0x076D);
    lda(game, 0x07);
    sta(game, 0x0736);
    cyc(game, 4 + 2 + 4 + 4 + 2 + 4 + 3);
    respawn_pose(game);
}

/// `LCA1B` (bank 7 `$CA1B`): death dispatcher on `$073D` through the
/// `$D382` trampoline (`[bank0_unknown1 $8140, bank7_code16 $CA24, LCA72
/// $CA72]`).
///
/// The `JSR $D382` frame (`$CA1D`) is pushed like the ROM's; the
/// trampoline (its trap, or the same port inline when it is running
/// untrapped) consumes it and tail-jumps to the table target
/// ([`Game::trap_jump`]): a trapped target has already fired by the time
/// the trampoline's trap returns, an unported one is handed on to this
/// routine's dispatcher so the interpreter resumes there with only the
/// caller's frame on the stack. `$073D > 2` walks past the table exactly
/// like the hardware (`$01A2`, stack garbage).
pub fn tt_death_dispatch(game: &mut Game) {
    cyc(game, 6);
    push_word(game, 0xCA1D);
    if game.traps.is_trapped(0xD382) {
        if let TrapExit::Jump(target) = game.fire_trap(0xD382) {
            game.trap_jump(target);
        }
    } else {
        // Same port inline: its jump request stays pending for the
        // dispatcher of this routine.
        crate::bank7_dispatch::jump_routine_073d(game);
        cyc(game, 47);
    }
}

/// `bank7_code16` (bank 7 `$CA24`): death entry.
///
/// Refills both meters (`LDX #$01 : JSR LCB18 : DEX : BPL`), clears
/// `$074C $0524 $0525 $DE $048D $05C3..$05C8`, then `DEC $0700`: lives
/// left → `LCA6C` (`$076C = 6`, exit `A = 6`); none → game over (`$E9 =
/// 2`, `$0501 = $F0`, `$0726++`, `$073D++`, `JSR LCA17` with `A = 9` →
/// `$0725`, XP words zeroed, exit `A = 0`). `X = Y = $FF` on exit.
pub fn tt_die(game: &mut Game) {
    ldx(game, 0x01);
    cyc(game, 2);
    loop {
        cyc(game, 6);
        jsr_sub(game, 0xCA26, 0xCB18);
        let x = game.cpu.x.wrapping_sub(1);
        ldx(game, x);
        cyc(game, 2 + 2);
        if x & 0x80 != 0 {
            break;
        }
        cyc(game, 1);
    }
    // LDA #$00 : STA $074C : STA $0524 : STA $0525 : STA $DE : STA $048D
    lda(game, 0x00);
    sta(game, 0x074C);
    sta(game, 0x0524);
    sta(game, 0x0525);
    sta(game, 0x00DE);
    sta(game, 0x048D);
    cyc(game, 2 + 4 + 4 + 4 + 3 + 4);
    // LDY #$05 : LCA3E: STA $05C3,y : DEY : BPL LCA3E
    ldy(game, 0x05);
    cyc(game, 2);
    for i in (0..=5u8).rev() {
        bus_write(game, 0x05C3 + u16::from(i), 0);
        ldy(game, i.wrapping_sub(1));
        cyc(game, 5 + 2 + if i == 0 { 2 } else { 3 });
    }
    // DEC $0700 : BNE LCA6C
    let lives = dec_mem(game, 0x0700);
    cyc(game, 6 + 2);
    if lives != 0 {
        cyc(game, 1);
        lda(game, 0x06);
        sta(game, 0x076C);
        cyc(game, 2 + 4 + 6);
        return;
    }
    // Game over: LDA #$02 : STA $E9 : LDA #$F0 : STA $0501 : INC $0726 :
    // INC $073D : LDA #$09 : JSR LCA17 (STA $0725 : RTS)
    lda(game, 0x02);
    sta(game, 0x00E9);
    lda(game, 0xF0);
    sta(game, 0x0501);
    inc_mem(game, 0x0726);
    inc_mem(game, 0x073D);
    lda(game, 0x09);
    cyc(game, 2 + 3 + 2 + 4 + 6 + 6 + 2);
    cyc(game, 6);
    push_word(game, 0xCA5C);
    sta(game, 0x0725);
    cyc(game, 4 + 6);
    let _ = pop_word(game);
    // LDA #$00 : STA $0775 : STA $0776 : STA $0756 : STA $0755 : RTS
    lda(game, 0x00);
    sta(game, 0x0775);
    sta(game, 0x0776);
    sta(game, 0x0756);
    sta(game, 0x0755);
    cyc(game, 2 + 4 + 4 + 4 + 4 + 6);
}

/// `LCA6C` (bank 7 `$CA6C`): `LDA #$06 : STA $076C : RTS`. 12 cycles.
pub fn tt_new_life(game: &mut Game) {
    lda(game, 0x06);
    sta(game, 0x076C);
    cyc(game, 2 + 4 + 6);
}

/// `LCA72` (bank 7 `$CA72`): game-over wait.
///
/// `LDA #$00 : STA $0726 : LDA $F7 : AND #$10 : BNE LCA82 : LDA $0501 :
/// BNE LCA71 (RTS) : LCA82: JMP LCF05`. Start held or `$0501` expired →
/// `LCF05` (`$0736++`). Exit `A` = the `AND` result or the timer.
pub fn tt_gameover_wait(game: &mut Game) {
    lda(game, 0x00);
    sta(game, 0x0726);
    let held = bus_read(game, 0x00F7);
    lda(game, held);
    lda(game, held & 0x10);
    cyc(game, 2 + 4 + 3 + 2 + 2);
    if held & 0x10 != 0 {
        cyc(game, 1 + 3);
        game.trap_jump(0xCF05);
        return;
    }
    let timer = bus_read(game, 0x0501);
    lda(game, timer);
    cyc(game, 4 + 2);
    if timer != 0 {
        cyc(game, 1 + 6);
        return;
    }
    cyc(game, 3);
    game.trap_jump(0xCF05);
}

/// `LCA85` (bank 7 `$CA85`): game-over Start/Select handler.
///
/// Fires on a change in held Start/Select (`($F7 & $30) << 2` vs the
/// same of `$0744`, kept in `$00`; `BEQ` → `RTS`). `BIT $00 : BVC` picks
/// the Select path (`LCAF7`) when Start is not held now: it returns
/// unless Select *is* held now (`BPL LCAF6`), then blips `$EF = $10`,
/// re-stages the `$6974` packet at `$0302`, flips `$0488` and drops the
/// `$FA` marker at `$0305 + $0488 * 2`. The Start path bumps `$079F`
/// (saturating), wipes `$E0-$EF` and `$07C0-$07FF`, then either saves
/// (`$0488 != 0`: `$07B0 = $40`, `JMP LCF05`) or continues (`LCAC4`: XP
/// words zeroed, `JSR $CF30`; `== $0F` → Great Palace restart via `JSR
/// $C358`, `$0561/$0701/$075C = 0`, `$076C = 1`, `$075F = 2`; else
/// `$076C = 0`, `$075F = 1`).
pub fn tt_gameover_choice(game: &mut Game) {
    // LDA $F7 : AND #$30 : ASL : ASL : STA $00
    let held = bus_read(game, 0x00F7);
    lda(game, held);
    lda(game, held & 0x30);
    asl_a(game);
    asl_a(game);
    let cur = game.cpu.a;
    bus_write(game, 0x0000, cur);
    // LDA $0744 : AND #$30 : ASL : ASL : CMP $00 : BEQ LCAF6
    let prev = bus_read(game, 0x0744);
    lda(game, prev);
    lda(game, prev & 0x30);
    asl_a(game);
    asl_a(game);
    let old = game.cpu.a;
    cmp_val(&mut game.cpu.p, old, cur);
    cyc(game, 3 + 2 + 2 + 2 + 3 + 4 + 2 + 2 + 2 + 3 + 2);
    if old == cur {
        cyc(game, 1 + 6);
        return;
    }
    // BIT $00 : BVC LCAF7_gameoverscreen_select_button_pressed
    bit_mem(&mut game.cpu.p, old, cur);
    cyc(game, 3 + 2);
    if cur & FLAG_V == 0 {
        // Select path. LCAF7: BPL LCAF6 (RTS)
        cyc(game, 1 + 2);
        if cur & FLAG_N == 0 {
            cyc(game, 1 + 6);
            return;
        }
        // LDA #$10 : STA $EF : LDY #$06 : LCAFF loop ($6974,y → $0302,y).
        // The `BPL LCAFF` at $CB06 branches back across the page ($CAFF):
        // every taken iteration costs 4.
        lda(game, 0x10);
        sta(game, 0x00EF);
        ldy(game, 0x06);
        cyc(game, 2 + 3 + 2);
        copy_desc(game, 0x6974, 0x0302, 0x07, 1);
        // LDA $0488 : EOR #$01 : STA $0488 : ASL : TAY : LDA #$FA :
        // STA $0305,y : RTS
        let sel = bus_read(game, 0x0488);
        lda(game, sel);
        lda(game, sel ^ 0x01);
        sta(game, 0x0488);
        asl_a(game);
        let y = game.cpu.a;
        ldy(game, y);
        lda(game, 0xFA);
        bus_write(game, 0x0305u16.wrapping_add(u16::from(y)), 0xFA);
        cyc(game, 4 + 2 + 4 + 2 + 2 + 2 + 5 + 6);
        return;
    }
    // Start path: LDA $079F : CMP #$FF : BEQ LCAA6 : INC $079F
    let continues = bus_read(game, 0x079F);
    lda(game, continues);
    cmp_val(&mut game.cpu.p, continues, 0xFF);
    cyc(game, 4 + 2 + 2);
    if continues == 0xFF {
        cyc(game, 1);
    } else {
        inc_mem(game, 0x079F);
        cyc(game, 6);
    }
    // LCAA6: LDX #$0F : LDA #$00 : LCAAA: STA $E0,x : DEX : BPL LCAAA
    ldx(game, 0x0F);
    lda(game, 0x00);
    cyc(game, 2 + 2);
    for i in (0..=0x0Fu8).rev() {
        bus_write(game, 0x00E0 + u16::from(i), 0);
        ldx(game, i.wrapping_sub(1));
        cyc(game, 4 + 2 + if i == 0 { 2 } else { 3 });
    }
    // LDY #$C0 : LCAB1: STA $0700,y : INY : BNE LCAB1
    ldy(game, 0xC0);
    cyc(game, 2);
    for y in 0xC0..=0xFFu8 {
        bus_write(game, 0x0700 + u16::from(y), 0);
        ldy(game, y.wrapping_add(1));
        cyc(game, 5 + 2 + if y == 0xFF { 2 } else { 3 });
    }
    // LDA $0488 : BEQ LCAC4
    let sel = bus_read(game, 0x0488);
    lda(game, sel);
    cyc(game, 4 + 2);
    if sel != 0 {
        // SAVE: LDA #$40 : STA $07B0 : JMP LCF05
        lda(game, 0x40);
        sta(game, 0x07B0);
        cyc(game, 2 + 4 + 3);
        game.trap_jump(0xCF05);
        return;
    }
    cyc(game, 1);
    // LCAC4: STA $0775 : STA $0776 : STA $0756 : STA $0755 (A = 0)
    sta(game, 0x0775);
    sta(game, 0x0776);
    sta(game, 0x0756);
    sta(game, 0x0755);
    cyc(game, 4 + 4 + 4 + 4);
    // JSR bank7_FUNCTION_CONVERT_706_and_707_to_Rx5plusW : CMP #$0F : BEQ
    cyc(game, 6);
    push_word(game, 0xCAD2);
    region_x5_plus_world(game);
    let _ = pop_word(game);
    let code = game.cpu.a;
    cmp_val(&mut game.cpu.p, code, 0x0F);
    cyc(game, 2 + 2);
    if code == 0x0F {
        // Great Palace restart: JSR $C358 : LDA #$00 : STA $0561 :
        // STA $0701 : STA $075C : LDA #$01 : LDY #$02
        cyc(game, 1 + 6);
        jsr_sub(game, 0xCADE, 0xC358);
        lda(game, 0x00);
        sta(game, 0x0561);
        sta(game, 0x0701);
        sta(game, 0x075C);
        lda(game, 0x01);
        ldy(game, 0x02);
        cyc(game, 2 + 4 + 4 + 4 + 2 + 2);
    } else {
        // LDA #$00 : LDY #$01 : JMP LCAF0
        lda(game, 0x00);
        ldy(game, 0x01);
        cyc(game, 2 + 2 + 3);
    }
    // LCAF0: STA $076C : STY $075F : RTS
    sta(game, 0x076C);
    let y = game.cpu.y;
    bus_write(game, 0x075F, y);
    cyc(game, 4 + 4 + 6);
}

/// `LCB18_fill_hp_or_mp_to_full__provide_x_register__maybe` (bank 7
/// `$CB18`): refill meter `X` (0 = magic, 1 = life) from its container
/// count: `LDA $0783,x : ASL ×5 : SEC : SBC #$01 : STA $0773,x : RTS`.
/// Exit `A` = the units, `C`/`V`/`N`/`Z` from the `SBC`, `X` untouched.
/// 29 cycles (+1 when `$0783 + X` crosses the page).
pub fn tt_refill(game: &mut Game) {
    let x = game.cpu.x;
    let containers = bus_read(game, 0x0783u16.wrapping_add(u16::from(x)));
    lda(game, containers);
    for _ in 0..5 {
        asl_a(game);
    }
    game.cpu.p |= FLAG_C;
    let units = sbc_val(&mut game.cpu.p, game.cpu.a, 0x01);
    game.cpu.a = units;
    bus_write(game, 0x0773u16.wrapping_add(u16::from(x)), units);
    cyc(game, 4 + cross(0x0783, x) + 10 + 2 + 2 + 5 + 6);
}

/// `LCF05` (bank 7 `$CF05`): `INC $0736 : RTS`. Cost (12) comes from the
/// central table (see the module docs), not charged here.
pub fn tt_mode_inc(game: &mut Game) {
    inc_mem(game, 0x0736);
}

/// `LCF21_SaveGameWhenChooseSAVEwhenDead__maybe` (bank 7 `$CF21`):
/// `LDA #$05 : JSR SwapPRG`, then the SRAM writer `JSR LB9CA` (bank 5,
/// data-driven), `JSR SwapToSavedPRG`, `INC $0738 : RTS`.
///
/// The bank switch is ported; the rest resumes in ROM at `$CF26`
/// ([`Game::trap_jump`]) so the bank-5 writer runs the original bytes.
pub fn tt_save_on_save(game: &mut Game) {
    lda(game, 0x05);
    cyc(game, 2 + 6);
    jsr_sub(game, 0xCF23, 0xFFCC);
    // Fall-through into the ROM bytes: no JMP to charge, nothing pushed.
    game.trap_jump(0xCF26);
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every fixed-bank title trap on `game`.
///
/// Banked (`None`) entries are skipped (aliasing caveat). Idempotent.
/// Every port charges its own cycles (explicit cost `0`) except `LCF05`,
/// which keeps its tabled 12.
pub fn register_title_traps(game: &mut Game) {
    game.trap_register_cycles("bank7_code7", Some(7), 0xC33C, tt_title_mode, 0);
    game.trap_register_cycles(
        "bank7_Reset_Number_of_Lives__to_3_",
        Some(7),
        0xC358,
        tt_lives3,
        0,
    );
    game.trap_register_cycles("LC360", Some(7), 0xC360, tt_state_inc, 0);
    game.trap_register_cycles("bank7_code10", Some(7), 0xC3A5, tt_ready_setup, 0);
    game.trap_register_cycles(
        "bank7_Load_Lives_Remaining_Screen",
        Some(7),
        0xC3B5,
        tt_lives_screen,
        0,
    );
    game.trap_register_cycles("LC3E6", Some(7), 0xC3E6, tt_ready_hold, 0);
    game.trap_register_cycles("LC3ED", Some(7), 0xC3ED, tt_ready_go, 0);
    game.trap_register_cycles("bank7_code11", Some(7), 0xC41E, tt_respawn, 0);
    game.trap_register_cycles("LCA1B", Some(7), 0xCA1B, tt_death_dispatch, 0);
    game.trap_register_cycles("bank7_code16", Some(7), 0xCA24, tt_die, 0);
    game.trap_register_cycles("LCA6C", Some(7), 0xCA6C, tt_new_life, 0);
    game.trap_register_cycles("LCA72", Some(7), 0xCA72, tt_gameover_wait, 0);
    game.trap_register_cycles("LCA85", Some(7), 0xCA85, tt_gameover_choice, 0);
    game.trap_register_cycles(
        "LCB18_fill_hp_or_mp_to_full__provide_x_register__maybe",
        Some(7),
        0xCB18,
        tt_refill,
        0,
    );
    game.trap_register("LCF05", Some(7), 0xCF05, tt_mode_inc);
    game.trap_register_cycles(
        "LCF21_SaveGameWhenChooseSAVEwhenDead__maybe",
        Some(7),
        0xCF21,
        tt_save_on_save,
        0,
    );
}
