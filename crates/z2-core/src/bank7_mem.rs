//! Power-on memory-init ports.
//!
//! | Rust fn | Label | Addr |
//! |---|---|---|
//! | [`reset_memory_ranges`] | `bank7_Reset_Memory_Ranges` | `$D281` |
//! | [`clear_0300_04ff_and_zeropage`] | `bank7_Set_Memory_300_4FF_and_00_DF_to_Zero` | `$D29C` |
//! | [`remove_all_sprites`] | `bank7_Remove_All_Sprites` | `$D24C` |
//! | [`remove_sprites_keep_sprite0`] | `bank7_Remove_All_Sprites_except_Sprite0` | `$D250` |
//! | [`erase_name_table`] | `bank7_Erase_Name_Table_1` / `bank7_Erase_Name_Table_0` | `$D261` / `$D263` |
//! | [`erase_both_name_tables`] | `bank7_Erase_Name_Tables_0and1__set_scroll_to_0_0` | `$D266` |
//! | [`fill_screen`] | `bank7_Fill_Screen_with_Tile_Code_F4...` | `$D2BE` |
//!
//! `prg7.asm $D281-$D2BD` reference: three `LDX #$00 : TXA` pages cleared
//! with `STA addr,x : DEX : BNE` (each stores `$xx00` first, then
//! `$xxFF..$xx01` — all 256 bytes), then the zero-page loop
//! `LDX #$DF : STA $00,x : DEX : CPX #$FF : BNE` (clears `$00-$DF`
//! inclusive; `$E0-$FF` are cleared separately by bank 5, `prg5.asm`
//! `$A6A6`), finishing with `STX $0302 : STX $0363` (`X = $FF`, the loop
//! exit value — the PPU-queue terminator, see `bank7_ppu_queue`).
//!
//! Sprite hiding (`$D24C-$D25D`): stores `$F8` (offscreen Y) to every 4th
//! byte of `$0200-$02FF` (the sprite-Y slots). The `$D250` entry starts at
//! `Y = 4`, so sprite 0's Y (`$0200`) is preserved — the `Y` wrap from
//! `$FC + 4` to `$00` exits the loop *before* storing `$0200`.
//!
//! Name-table erase (`$D261-$D2EB`): fills PPU `$2400`/`$2000` with tile
//! `$F4` (768 + 192 bytes) then 64 `$00` bytes; see [`fill_screen`].
//! All PPU traffic goes through the bus so façade counters match the ASM.

use crate::bank7_common::{cmp_val, inc_val, set_nz};
use crate::cpu::{bus_read, bus_write, FLAG_C, FLAG_N, FLAG_Z};
use crate::game::Game;

/// Clear one 256-byte page with `A` (`LDX #$00 : TXA`, then
/// `STA page,x : DEX : BNE`). Exit: `X = 0`, `A = 0`, `Z = 1`, `N = 0`.
fn clear_page(game: &mut Game, page: u16) {
    game.cpu.x = 0;
    game.cpu.a = 0; // TXA of X = 0.
    set_nz(&mut game.cpu.p, 0);
    // Stores page+0 first, then page+FF .. page+01 (256 total).
    bus_write(game, page, 0);
    for x in (1u8..=0xFF).rev() {
        bus_write(game, page + u16::from(x), 0);
    }
    game.cpu.x = 0;
    set_nz(&mut game.cpu.p, 0);
}

/// `bank7_Reset_Memory_Ranges` (`prg7.asm $D281`): clear
/// `$0700-$07FF`, `$0600-$06FF`, `$0500-$05FF` to zero.
///
/// Falls through to [`clear_0300_04ff_and_zeropage`] in the original; the
/// Rust caller (and the trap) compose the two like the ASM does.
/// Exit: `X = $FF` (from the zero-page loop), `A = 0`; `Y` untouched.
pub fn reset_memory_ranges(game: &mut Game) {
    clear_page(game, 0x0700);
    clear_page(game, 0x0600);
    // LD293: the $0500 loop shares the same shape.
    clear_page(game, 0x0500);
    clear_0300_04ff_and_zeropage(game);
}

/// `bank7_Set_Memory_300_4FF_and_00_DF_to_Zero` (`prg7.asm $D29C`).
///
/// Clears `$0400-$04FF`, `$0300-$03FF`, then `$00-$DF`; writes the loop-exit
/// `X = $FF` to `$0302`/`$0363` (PPU-queue terminator + text pointer).
/// Exit: `A = 0`, `X = $FF`, `Z = 1`, `C = 1` (final `CPX #$FF`), `N = 0`.
pub fn clear_0300_04ff_and_zeropage(game: &mut Game) {
    clear_page(game, 0x0400);
    clear_page(game, 0x0300);
    // LDX #$DF : STA $00,x : DEX : CPX #$FF : BNE (stores $DF .. $00).
    game.cpu.x = 0xDF;
    set_nz(&mut game.cpu.p, 0xDF);
    for x in (0u8..=0xDF).rev() {
        bus_write(game, u16::from(x), 0);
        if x == 0 {
            break;
        }
    }
    game.cpu.x = 0xFF;
    // CPX #$FF with X = $FF: Z = 1, C = 1, N = 0.
    cmp_val(&mut game.cpu.p, 0xFF, 0xFF);
    // STX $0302 : STX $0363 (no flags).
    bus_write(game, 0x0302, 0xFF);
    bus_write(game, 0x0363, 0xFF);
}

/// Sprite-Y hide loop shared by `$D24C`/`$D250`: store `$F8` to every 4th
/// byte of `$0200` starting at `start`, wrapping `Y` to 0 to stop.
/// Exit: `Y = 0`, `A = $F8`, `Z = 1`, `N = 0` (final `INY` wrap).
fn hide_sprites_from(game: &mut Game, start: u8) {
    game.cpu.a = 0xF8; // LDA #$F8: N = 1, Z = 0 (overwritten below).
    set_nz(&mut game.cpu.p, 0xF8);
    let mut y = start;
    loop {
        bus_write(game, 0x0200 + u16::from(y), 0xF8);
        // Four INYs per store in the original (LD254 unrolled x4 effect =
        // step 4 with per-step flags; only the last INY's flags survive).
        let (next_y, wrapped) = y.overflowing_add(4);
        y = next_y;
        if wrapped {
            break;
        }
    }
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
}

/// `bank7_Remove_All_Sprites` (`prg7.asm $D24C`): hide all 64 sprites.
///
/// `LDY #$00 : BEQ LD252` (always taken) then the `$F8` loop.
pub fn remove_all_sprites(game: &mut Game) {
    game.cpu.y = 0; // LDY #$00.
    set_nz(&mut game.cpu.p, 0);
    // BEQ LD252 (taken; no flags).
    hide_sprites_from(game, 0);
}

/// `bank7_Remove_All_Sprites_except_Sprite0` (`prg7.asm $D250`).
///
/// `LDY #$04`: the wrap exits before `$0200` is stored, preserving sprite 0.
pub fn remove_sprites_keep_sprite0(game: &mut Game) {
    game.cpu.y = 4; // LDY #$04.
    set_nz(&mut game.cpu.p, 4);
    hide_sprites_from(game, 4);
}

/// `bank7_Fill_Screen_...` (`prg7.asm $D2BE`): fill one name table.
///
/// Entry `A` = PPU high byte (`$24` for table 1, `$20` for table 0).
/// Writes `$F4` 768 + 192 times then `$00` 64 times starting at
/// `A:$00` (`$2006`/`$2006`), after priming `$2000` from `Y = $30`.
/// Reads `$2002` once (`LDY $2002`: `N`/`V` from the status byte).
///
/// Exit: `A = 0`, `X = 0`, `Y = 0`, `Z = 1`, `N = 0`, `C = 1` (final
/// `CPY #$C0`; `INY`/`STA` leave `C` alone). `V` is untouched throughout
/// (`LDY`/`LDX`/`INX`/`DEX`/`INY`/`CPY` never modify `V`).
pub fn fill_screen(game: &mut Game) {
    // LDY $2002 (absolute read; N/V from PPUSTATUS, Z from the value).
    let status = bus_read(game, 0x2002);
    game.cpu.y = status;
    // BIT-like N/V/Z: LDY sets N/Z only (V untouched by LDY!) — careful:
    // LDY does NOT touch V. V keeps its entry value. The ASM relies on
    // that; replicate exactly (no V change here).
    set_nz(&mut game.cpu.p, status);
    // LDY #$30 : STY $2000 : LDY #$00.
    game.cpu.y = 0x30;
    set_nz(&mut game.cpu.p, 0x30);
    bus_write(game, 0x2000, 0x30);
    game.cpu.y = 0x00;
    set_nz(&mut game.cpu.p, 0x00);
    // STA $2006 (entry A) : STY $2006 (low = 0).
    let hi = game.cpu.a;
    bus_write(game, 0x2006, hi);
    bus_write(game, 0x2006, 0x00);
    // LDX #$03 : LDA #$F4.
    game.cpu.x = 3;
    set_nz(&mut game.cpu.p, 3);
    game.cpu.a = 0xF4;
    set_nz(&mut game.cpu.p, 0xF4);
    // LD2D2: 3 pages x 256 $2007 writes of $F4.
    for _ in 0..3u8 {
        for _ in 0..256u16 {
            bus_write(game, 0x2007, 0xF4);
            game.cpu.y = game.cpu.y.wrapping_add(1);
        }
        // INY 256 times wraps Y to its entry value; final INY (FF->00)
        // leaves Z = 1, N = 0. DEX then BNE.
        set_nz(&mut game.cpu.p, game.cpu.y);
        game.cpu.x = game.cpu.x.wrapping_sub(1);
        set_nz(&mut game.cpu.p, game.cpu.x);
    }
    // LD2DB: STA $2007 : INY : CPY #$C0 : BNE (192 writes, Y 0 -> $C0).
    for _ in 0..0xC0u16 {
        bus_write(game, 0x2007, 0xF4);
        game.cpu.y = game.cpu.y.wrapping_add(1);
        set_nz(&mut game.cpu.p, game.cpu.y);
        cmp_val(&mut game.cpu.p, game.cpu.y, 0xC0);
    }
    // LDA #$00 : LD2E5: STA $2007 : INY : BNE (64 writes, Y $C0 -> 0).
    game.cpu.a = 0x00;
    set_nz(&mut game.cpu.p, 0x00);
    for _ in 0..0x40u16 {
        bus_write(game, 0x2007, 0x00);
        game.cpu.y = game.cpu.y.wrapping_add(1);
        set_nz(&mut game.cpu.p, game.cpu.y);
    }
    // Final flags: Y = 0 (Z = 1, N = 0); C = 1 from the last CPY #$C0
    // (INY/STA leave C alone); V untouched since entry.
    game.cpu.p |= FLAG_C;
    game.cpu.p &= !(FLAG_N | FLAG_Z);
    game.cpu.p |= FLAG_Z;
}

/// `bank7_Erase_Name_Table_1` (`prg7.asm $D261`): `LDA #$24`, tail-fill.
pub fn erase_name_table_1(game: &mut Game) {
    game.cpu.a = 0x24; // N = 0, Z = 0.
    set_nz(&mut game.cpu.p, 0x24);
    fill_screen(game);
}

/// `bank7_Erase_Name_Table_0` (`prg7.asm $D263`): `LDA #$20`, tail-fill.
pub fn erase_name_table_0(game: &mut Game) {
    game.cpu.a = 0x20;
    set_nz(&mut game.cpu.p, 0x20);
    fill_screen(game);
}

/// `bank7_Erase_Name_Tables_0and1__set_scroll_to_0_0` (`prg7.asm $D266`).
///
/// Erases table 1, then table 0 (`LD269`), zeroes `$FC`/`$FD`/`$0746` and
/// both scroll latches, then `INC $073D` (`LD27D`).
pub fn erase_both_name_tables(game: &mut Game) {
    // JSR $D261 (at $D266, pushing $D269) : erase table 1.
    crate::bank7_common::inner_jsr_frame(game, 0xD266, erase_name_table_1);
    // LD269: LDA #$20 : JSR $D263 (at $D26B, pushing $D26D) : erase table 0.
    game.cpu.a = 0x20;
    set_nz(&mut game.cpu.p, 0x20);
    crate::bank7_common::inner_jsr_frame(game, 0xD26B, erase_name_table_0);
    // LDA #$00 : STA $FC : STA $FD : STA $0746 : STA $2005 : STA $2005.
    game.cpu.a = 0x00;
    set_nz(&mut game.cpu.p, 0x00);
    bus_write(game, 0x00FC, 0x00);
    bus_write(game, 0x00FD, 0x00);
    bus_write(game, 0x0746, 0x00);
    bus_write(game, 0x2005, 0x00);
    bus_write(game, 0x2005, 0x00);
    // LD27D: INC $073D (Routine Index).
    let v = bus_read(game, 0x073D);
    let r = inc_val(&mut game.cpu.p, v);
    bus_write(game, 0x073D, r);
}
