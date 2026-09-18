//! Reset-vector + power-on init ports.
//!
//! | Rust fn | Label | Addr | Trapped? |
//! |---|---|---|---|
//! | [`reset_prefix`] | `bank7_reset` (first half) | `$FF70-$FF8D` | no (see below) |
//! | — | `ConfigureMMC1`/`SwapCHR`/`SwapPRG` | `$FF9D`/`$FFB1`/`$FFCC` | yes, via `bank7_mmc1` |
//! | — | `bank7_PowerON_code` | `$C000` | no (`JMP`-only entry) |
//!
//! `prg7.asm $FF70-$FF9A` reference:
//! ```asm
//! SEI : CLD : LDX #$00 : STX $2000 : INX
//! @PPUSpin: LDA $2002 : BPL @PPUSpin : DEX : BEQ @PPUSpin
//! TXS
//! STX $8000 : STX $A000 : STX $C000 : STX $E000   ; X = $FF: MMC1 reset
//! LDA #$0F : JSR ConfigureMMC1 : JSR SwapCHR
//! LDA #$07 : JSR SwapPRG : JMP bank7_PowerON_code
//! ```
//!
//! ## Why the vector itself is untrapped
//!
//! `Cpu::reset` fires a vector trap *then* sets `PC` to the vector, so a
//! `$FF70` trap would run the Rust body *and* re-execute the ASM bytes
//! (double execution, plus register mismatch at fire time since the ASM
//! prologue owns `X`/`SP`). JSR-target traps emulate their return and stay
//! balanced; the no-return reset vector cannot. The prologue below is
//! therefore a plain library port (verified against hand-computed state),
//! while every *called* subroutine traps normally. `bank7_PowerON_code`
//! (`$C000`) is likewise untrapped: it is entered by `JMP`, and a `JMP`
//! trap would pop a return address that was never pushed.

use crate::bank7_common::set_nz;
use crate::cpu::{bus_read, bus_write};
use crate::game::Game;

/// PPU warm-up read count on the stubbed bus (`$2002` always ready, so the
/// `@PPUSpin` loop runs exactly twice: `X = 1 -> 0 -> $FF`).
pub const PPU_SPIN_READS: u64 = 2;

/// First half of `bank7_reset` (`prg7.asm $FF70-$FF8D`): `SEI`/`CLD`,
/// PPU warm-up spin, `TXS`, and the four MMC1 shift-register resets
/// (`STX` with `X = $FF`, i.e. bit 7 set).
///
/// Exit: `I` set, `D` clear, `X = $FF`, `SP = $FF`, MMC1 shift cleared
/// with `ctrl |= $0C` (see [`crate::cpu::Mmc1::write`]), `A`/`Y`
/// preserved, `N`/`Z` from the final `DEX` (`X = $FF`: `N = 1`, `Z = 0`).
/// PPU façade: one `$2000` write (`0`) + two `$2002` reads.
pub fn reset_prefix(game: &mut Game) {
    // SEI : CLD.
    game.cpu.p |= crate::cpu::FLAG_I;
    game.cpu.p &= !crate::cpu::FLAG_D;
    // LDX #$00 : STX $2000 : INX.
    game.cpu.x = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x2000, 0);
    game.cpu.x = 1;
    set_nz(&mut game.cpu.p, 1);
    // @PPUSpin: LDA $2002 : BPL @PPUSpin (stub: always ready, never taken).
    // DEX : BEQ @PPUSpin — two passes (X = 1 -> 0 -> $FF).
    for _ in 0..PPU_SPIN_READS {
        let v = bus_read(game, 0x2002);
        game.cpu.a = v;
        set_nz(&mut game.cpu.p, v);
        // BPL not taken (stub bit 7 set).
    }
    game.cpu.x = 0; // DEX #1 (1 -> 0): BEQ taken.
    set_nz(&mut game.cpu.p, 0);
    game.cpu.x = 0xFF; // DEX #2 (0 -> $FF): BEQ not taken, N = 1.
    set_nz(&mut game.cpu.p, 0xFF);
    // TXS.
    game.cpu.sp = 0xFF;
    // STX $8000 : STX $A000 : STX $C000 : STX $E000 (MMC1 reset writes).
    bus_write(game, 0x8000, 0xFF);
    bus_write(game, 0xA000, 0xFF);
    bus_write(game, 0xC000, 0xFF);
    bus_write(game, 0xE000, 0xFF);
}
