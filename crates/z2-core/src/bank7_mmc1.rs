//! MMC1 bank-switch ports.
//!
//! Routines (all fixed bank `$FF9D-$FFDF`, always mapped):
//!
//! | Rust fn | Label | Addr | Notes |
//! |---|---|---|---|
//! | [`configure_mmc1`] | `ConfigureMMC1` | `$FF9D` | 5 serial writes to `$8000` |
//! | [`swap_chr`] | `SwapCHR` | `$FFB1` | 5 serial writes to `$A000` |
//! | [`swap_to_prg0`] | `SwapToPRG0` | `$FFC5` | `A=0`, falls into `SwapPRG` |
//! | [`swap_to_saved_prg`] | `SwapToSavedPRG` | `$FFC9` | `A=PRG_bank($0769)`, falls into `SwapPRG` |
//! | [`swap_prg`] | `SwapPRG` | `$FFCC` | 5 serial writes to `$E000` |
//!
//! `prg7.asm` reference (e.g. `$FF9D-$FFB0`):
//! `STA $8000 : LSR : STA $8000 : LSR : ...` (five `STA`s, four `LSR`s).
//! Every mapper effect goes through the shared [`crate::cpu::Mmc1`] model
//! via the bus — the serial protocol is reused, never reimplemented.
//! Register traffic is identical to the ASM (five `bus_write`s per call),
//! so trapped/untrapped A/B runs match bit-for-bit.

use crate::bank7_common::set_nz;
use crate::cpu::{bus_read, bus_write, FLAG_C, FLAG_N, FLAG_Z};
use crate::game::Game;

/// MMC1 control register address (`$8000-$9FFF`).
pub const MMC1_CTRL_ADDR: u16 = 0x8000;
/// MMC1 CHR bank 0 address (`$A000-$BFFF`).
pub const MMC1_CHR0_ADDR: u16 = 0xA000;
/// MMC1 PRG bank address (`$E000-$FFFF`).
pub const MMC1_PRG_ADDR: u16 = 0xE000;
/// Saved PRG bank latch (`PRG_bank`, mirrors header bank for area returns).
pub const PRG_BANK_SAVE: u16 = 0x0769;

/// Serial-commit helper: five bus writes of `value >> 0..5` (LSB first),
/// replicating `STA reg : LSR : ...` (`prg7.asm $FF9D`, `$FFB1`, `$FFCC`).
///
/// Entry `A` is consumed; exit `A = value >> 4` with flags from the fourth
/// `LSR` (`N = 0` always, `Z` per result, `C` = old bit 4 of `value`).
/// `V`/`I`/`D` are untouched; `X`/`Y` are untouched.
fn serial_commit(game: &mut Game, reg: u16, value: u8) {
    let mut a = value;
    for i in 0..5u8 {
        bus_write(game, reg, a);
        if i < 4 {
            // LSR (accumulator): C = old bit 0, N cleared, Z per result.
            if a & 1 != 0 {
                game.cpu.p |= FLAG_C;
            } else {
                game.cpu.p &= !FLAG_C;
            }
            a >>= 1;
            game.cpu.p &= !FLAG_N;
            if a == 0 {
                game.cpu.p |= FLAG_Z;
            } else {
                game.cpu.p &= !FLAG_Z;
            }
        }
    }
    game.cpu.a = a;
}

/// `ConfigureMMC1` (`prg7.asm $FF9D`): commit entry `A` to MMC1 control.
///
/// Used by reset (`LDA #$0F`, `$FF8D`) and area setup (`LDA #$0E`,
/// `$C4D0`), among others. See [`serial_commit`] for flag behavior.
pub fn configure_mmc1(game: &mut Game) {
    let a = game.cpu.a;
    serial_commit(game, MMC1_CTRL_ADDR, a);
}

/// `SwapCHR` (`prg7.asm $FFB1`): commit entry `A` to MMC1 CHR bank 0.
pub fn swap_chr(game: &mut Game) {
    let a = game.cpu.a;
    serial_commit(game, MMC1_CHR0_ADDR, a);
}

/// `SwapPRG` (`prg7.asm $FFCC`): commit entry `A` to the MMC1 PRG register.
///
/// Zelda II runs PRG mode 3 (fix-last at `$C000`, switch `$8000`), so this
/// selects the `$8000-$BFFF` bank. See [`serial_commit`].
pub fn swap_prg(game: &mut Game) {
    let a = game.cpu.a;
    serial_commit(game, MMC1_PRG_ADDR, a);
}

/// `SwapToPRG0` (`prg7.asm $FFC5`): `LDA #$00 : BEQ SwapPRG`.
///
/// `LDA #$00` sets `N = 0`, `Z = 1`; the `BEQ` is always taken.
pub fn swap_to_prg0(game: &mut Game) {
    game.cpu.a = 0;
    game.cpu.p &= !FLAG_N;
    game.cpu.p |= FLAG_Z;
    swap_prg(game);
}

/// `SwapToSavedPRG` (`prg7.asm $FFC9`): `LDA PRG_bank($0769)`, then `SwapPRG`.
///
/// Restores the area bank after sound/engine excursions into bank 6/0.
/// `N`/`Z` come from the loaded `$0769` value.
pub fn swap_to_saved_prg(game: &mut Game) {
    let v = bus_read(game, PRG_BANK_SAVE);
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
    swap_prg(game);
}
