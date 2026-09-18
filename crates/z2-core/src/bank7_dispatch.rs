//! Computed-jump trampolines + mode-change detectors.
//!
//! | Rust fn | Label | Addr |
//! |---|---|---|
//! | [`jump_indexed_table`] | `bank7_PullAddrFromTableFollowingThisJSR_withIndexOfA_then_JMP` | `$D385` |
//! | [`jump_routine_073d`] | `bank7_JmpToRoutine_at_Index_073D_...` | `$D382` |
//! | [`detect_game_mode_change`] | `LD168` | `$D168` |
//! | [`detect_boot_stage_change`] | `LD174` | `$D174` |
//! | [`store_ppu_macro_selector`] | `LD158` | `$D158` |
//! | [`detect_dialog_change`] | `LD15C` | `$D15C` |
//!
//! ## Trampoline mechanics (`prg7.asm $D382-$D397`)
//!
//! ```asm
//! $D382: LDA $073D                     ; index from Routine Index
//! $D385: ASL : TAY                     ; x2 (2 bytes/entry)
//!        PLA : STA $0C : PLA : STA $0D  ; pop the JSR return address
//!        INY                           ; +1 (JSR pushes ret = target-1)
//!        LDA ($0C),y : STA $0E         ; load LE16 target
//!        INY : LDA ($0C),y : STA $0F
//!        JMP ($0E)                     ; tail-call (no RTS here)
//! ```
//!
//! The `JSR` return is *consumed*: the target's eventual `RTS` returns to
//! the original caller. The trap port performs the same stack surgery — it
//! pops the pushed return, then hands the `JMP ($0E)` to the dispatcher as
//! a PC redirect ([`Game::trap_jump`]): the target's own trap fires when it
//! has one, otherwise the interpreter continues at the target, and nothing
//! is pushed (see the `cpu.rs` JSR/JMP trap paths and `Game::fire_trap`).
//! `$0C/$0D/$0E/$0F` scratch effects are replicated.
//!
//! The game-mode tables themselves (e.g. `$0524` at `$C220`,
//! `$076C` at `$C2CA`) are dispatched by [`crate::bank7_mode`] typed
//! matches; this module is the faithful low-level trampoline those (and
//! dozens of other call sites) use.

use crate::bank7_common::set_nz;
use crate::bank7_mem::remove_all_sprites;
use crate::cpu::bus_read;
use crate::cpu::bus_write;
use crate::game::Game;

/// Routine Index (`$073D`), the `$D382` trampoline's index source.
pub const ROUTINE_INDEX_073D: u16 = 0x073D;
/// Indirect scratch (`$0C/$0D` = popped return, `$0E/$0F` = target).
pub const INDIR_SCRATCH: u16 = 0x000C;
/// Game Mode (`$0736`) + last-seen copy (`$0737`) for [`detect_game_mode_change`].
pub const GAME_MODE: u16 = 0x0736;
/// Game Mode shadow.
pub const GAME_MODE_SHADOW: u16 = 0x0737;
/// Boot Stage (`$076C`) + shadow (`$076D`) for [`detect_boot_stage_change`].
pub const BOOT_STAGE: u16 = 0x076C;
/// Boot Stage shadow.
pub const BOOT_STAGE_SHADOW: u16 = 0x076D;
/// Cleared rider bytes on a Boot Stage change (`$073C/$073B/$0738/$073D`).
pub const STAGE_CLEAR_LO: u16 = 0x0738;
/// PPU Macro Selector (`$0725`) for [`store_ppu_macro_selector`].
pub const PPU_MACRO_SELECTOR: u16 = 0x0725;
/// Dialog-change pair (`$0738/$0739`) for [`detect_dialog_change`].
pub const DIALOG_STATE: u16 = 0x0738;

/// Read the LE16 table entry at `table_base + index * 2` through the bus
/// (tables live in PRG ROM; `$8000+` routes via MMC1).
fn table_entry(game: &mut Game, table_base: u16, index: u8) -> u16 {
    // ASM: Y = A*2, INY (return points one behind), then ($0C),y / ($0C),y+1.
    // table_base here is already ret + 1 (first table byte).
    let off = u16::from(index) * 2;
    let lo = u16::from(bus_read(game, table_base.wrapping_add(off)));
    let hi = u16::from(bus_read(game, table_base.wrapping_add(off + 1)));
    lo | (hi << 8)
}

/// `bank7_PullAddrFromTableFollowingThisJSR_...` (`prg7.asm $D385`).
///
/// Entry `A` = table index. Pops the return address the interpreter
/// pushed for the trapping `JSR`, loads `table[ret + 1 + A * 2]`, and
/// tail-jumps there ([`Game::trap_jump`]: the target's trap fires when it
/// has one, else the interpreter continues at it). Effects: `$0C/$0D` =
/// popped return, `$0E/$0F` = target, `A` = target high byte, `Y = A * 2 +
/// 2` (+flags); the `JSR` frame is consumed and nothing is pushed.
pub fn jump_indexed_table(game: &mut Game) {
    // ASL : TAY.
    let a = game.cpu.a;
    let doubled = a.wrapping_mul(2);
    game.cpu.y = doubled;
    // ASL flags: C = old bit 7, N/Z from the result.
    if a & 0x80 != 0 {
        game.cpu.p |= crate::cpu::FLAG_C;
    } else {
        game.cpu.p &= !crate::cpu::FLAG_C;
    }
    set_nz(&mut game.cpu.p, doubled);
    // PLA : STA $0C : PLA : STA $0D.
    let ret = crate::bank7_common::pop_word(game);
    game.ram[INDIR_SCRATCH as usize] = ret as u8;
    game.ram[INDIR_SCRATCH as usize + 1] = (ret >> 8) as u8;
    // INY (Y = A*2 + 1; the pushed return points one byte behind the
    // table, so ($0C),y lands on table[A * 2]).
    game.cpu.y = game.cpu.y.wrapping_add(1);
    set_nz(&mut game.cpu.p, game.cpu.y);
    // LDA ($0C),y : STA $0E : INY : LDA ($0C),y : STA $0F.
    // ($0C) = ret; table starts at ret + 1 = ret + (A*2+1) - A*2.
    let y = game.cpu.y;
    let lo = bus_read(game, ret.wrapping_add(u16::from(y)));
    game.cpu.a = lo;
    set_nz(&mut game.cpu.p, lo);
    game.ram[INDIR_SCRATCH as usize + 2] = lo;
    game.cpu.y = y.wrapping_add(1);
    set_nz(&mut game.cpu.p, game.cpu.y);
    let hi = bus_read(game, ret.wrapping_add(u16::from(game.cpu.y)));
    game.cpu.a = hi;
    set_nz(&mut game.cpu.p, hi);
    game.ram[INDIR_SCRATCH as usize + 3] = hi;
    let target = u16::from(lo) | (u16::from(hi) << 8);
    // JMP ($0E): continue at the target once this body returns.
    game.trap_jump(target);
}

/// `bank7_JmpToRoutine_at_Index_073D_...` (`prg7.asm $D382`).
///
/// `LDA $073D`, then the `$D385` body above. `N`/`Z` come from the `$073D`
/// load (overwritten by the `ASL` like the original).
pub fn jump_routine_073d(game: &mut Game) {
    let v = bus_read(game, ROUTINE_INDEX_073D);
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
    jump_indexed_table(game);
}

/// Table-entry reader used by the typed dispatchers in `bank7_mode`
/// (same `table[index * 2]` layout, explicit base instead of the JSR
/// return trick).
pub fn read_jump_table(game: &mut Game, table_base: u16, index: u8) -> u16 {
    table_entry(game, table_base, index)
}

/// `LD168` (`prg7.asm $D168`): Game-Mode change detector.
///
/// ```asm
/// LDA $0736 : CMP $0737 : STA $0737 : BNE LD18D : RTS
/// LD18D: LDY #$00 : STY $073B : STY $0738  (then falls into LD195:
///        LDY #$00 : STY $073D : RTS)
/// ```
/// On change, `$073B`/`$0738`/`$073D` reset to 0. Exit `A` = new `$0736`.
pub fn detect_game_mode_change(game: &mut Game) {
    let cur = bus_read(game, GAME_MODE);
    game.cpu.a = cur;
    set_nz(&mut game.cpu.p, cur);
    let shadow = bus_read(game, GAME_MODE_SHADOW);
    crate::bank7_common::cmp_val(&mut game.cpu.p, cur, shadow);
    bus_write(game, GAME_MODE_SHADOW, cur);
    if cur == shadow {
        return; // BEQ... BNE not taken: RTS.
    }
    // Changed path costs 37 (BNE taken 3 + LD18D 10 + LD195 6 + RTS 6 on
    // top of the 12-cycle compare) against the tabled same-path 20.
    game.cpu.cycles += 17;
    // LD18D: LDY #$00 : STY $073B : STY $0738.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x073B, 0);
    bus_write(game, STAGE_CLEAR_LO, 0);
    // LD195: LDY #$00 : STY $073D : RTS.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, ROUTINE_INDEX_073D, 0);
}

/// `LD174` (`prg7.asm $D174`): Boot-Stage change detector.
///
/// On `$076C != $076D`: hide all sprites, clear `$073C`/`$0736`, then the
/// `LD18D` tail (`$073B`/`$0738`/`$073D` = 0). Exit `A` = new `$076C`;
/// `Y = 0` on every path (`LDY #$00` runs in both arms).
pub fn detect_boot_stage_change(game: &mut Game) {
    let cur = bus_read(game, BOOT_STAGE);
    game.cpu.a = cur;
    set_nz(&mut game.cpu.p, cur);
    let shadow = bus_read(game, BOOT_STAGE_SHADOW);
    crate::bank7_common::cmp_val(&mut game.cpu.p, cur, shadow);
    bus_write(game, BOOT_STAGE_SHADOW, cur);
    if cur == shadow {
        return; // BEQ LD19A: RTS.
    }
    // Changed path: BEQ not taken 2 + JSR 6 + Remove_All_Sprites 1036 +
    // LDA 4 + LDY 2 + 2x STY 8 + LD18D 10 + LD195 6 + RTS 6 = 1092 total
    // against the tabled same-path 21.
    game.cpu.cycles += 1071;
    // JSR bank7_Remove_All_Sprites (at $D17F, pushing $D181).
    crate::bank7_common::inner_jsr_frame(game, 0xD17F, remove_all_sprites);
    // LDA $076C : LDY #$00 : STY $073C : STY $0736.
    let cur = bus_read(game, BOOT_STAGE);
    game.cpu.a = cur;
    set_nz(&mut game.cpu.p, cur);
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x073C, 0);
    bus_write(game, GAME_MODE, 0);
    // LD18D tail.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x073B, 0);
    bus_write(game, STAGE_CLEAR_LO, 0);
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, ROUTINE_INDEX_073D, 0);
}

/// `LD158` (`prg7.asm $D158`): `STA $0725` (PPU Macro Selector), `RTS`.
pub fn store_ppu_macro_selector(game: &mut Game) {
    let a = game.cpu.a;
    bus_write(game, PPU_MACRO_SELECTOR, a);
}

/// `LD15C` (`prg7.asm $D15C`): dialog-state change detector
/// (`$0738` vs `$0739`); on change runs the `LD195` tail (`$073D` = 0).
/// Exit `A` = new `$0738`.
pub fn detect_dialog_change(game: &mut Game) {
    let cur = bus_read(game, DIALOG_STATE);
    game.cpu.a = cur;
    set_nz(&mut game.cpu.p, cur);
    let shadow = bus_read(game, DIALOG_STATE + 1);
    crate::bank7_common::cmp_val(&mut game.cpu.p, cur, shadow);
    bus_write(game, DIALOG_STATE + 1, cur);
    if cur == shadow {
        return; // BEQ... BNE not taken: RTS.
    }
    // Changed path: BNE taken 3 + LD195 6 + RTS 6 = 27 against the
    // tabled same-path 20.
    game.cpu.cycles += 7;
    // BNE LD195: LDY #$00 : STY $073D : RTS.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, ROUTINE_INDEX_073D, 0);
}
