//! Global timers + frame counter + RNG ports.
//!
//! NMI tail block `prg7.asm $C169-$C1B0` reference:
//! ```asm
//! LDX #$0C : DEC $0500 : BPL :+          ; $C169 (stun/invincibility tick)
//! LDA #$14 : STA $0500 : LDX #$18        ; reload 20, widen the sweep
//! : LDA $0501,x : BEQ :+ : DEC $0501,x   ; $C177 (decrement each live timer)
//! : DEX : BPL :-                         ; X = $0C..0 or $18..0, ends $FF
//! INC $12                                ; $C182 (frame counter)
//! LDX #$00 : LDY #$09                    ; $C185 (RNG prologue)
//! LDA $051A : AND #$02 : STA $00         ; $C189 (carry seed bit 0)
//! LDA $051B : AND #$02 : EOR $00         ; ... (carry seed bit 1)
//! CLC : BEQ :+ : SEC
//! : ROR $051A,x : INX : DEY : BNE :-     ; 9-way LFSR shift ($C19B)
//! ```
//!
//! The RNG half already lives in [`crate::game::rng_advance`] (same bytes,
//! same observable effects); this module owns the timer half and the
//! combined entry [`tick_nmi_counters`] so the NMI port has one call site.
//! `$0500` reloads to `$14` (20) on expiry; `$0501+X` timers only decrement
//! when nonzero; `$0012` increments unconditionally every NMI.

use crate::bank7_common::set_nz;
use crate::cpu::{bus_read, bus_write};
use crate::game::{rng_advance, Game};

/// First timer cell (stun/invincibility tick, reloads to `$14`).
pub const TIMER0: u16 = 0x0500;
/// Base of the swept timer array (`$0501+X`).
pub const TIMER_BASE: u16 = 0x0501;
/// NMI frame counter (`INC $12`, `prg7.asm $C182`).
pub const FRAME_COUNTER: u16 = 0x0012;
/// Sweep width when `$0500` did not expire (X = `$0C..0`).
pub const SWEEP_NARROW: u8 = 0x0C;
/// Sweep width after a `$0500` reload (X = `$18..0`).
pub const SWEEP_WIDE: u8 = 0x18;
/// `$0500` reload value (20 frames).
pub const TIMER0_RELOAD: u8 = 0x14;

/// Timer half (`prg7.asm $C169-$C184`): `DEC $0500` (+ reload), sweep and
/// decrement `$0501+X`, `INC $12`.
///
/// Exit: `X = $FF` (final `DEX` wrap: `N = 1`, `Z = 0`), `Y` untouched,
/// `A` = `$14` iff `$0500` reloaded else preserved, `N`/`Z` from the final
/// `INC $12`... precisely: `INC` sets `N`/`Z` from `$12` (overwriting the
/// `DEX` flags), `C`/`V`/`I`/`D` untouched throughout.
pub fn tick_global_timers(game: &mut Game) {
    // LDX #$0C : DEC $0500 : BPL :+.
    game.cpu.x = SWEEP_NARROW;
    set_nz(&mut game.cpu.p, SWEEP_NARROW);
    let t0 = bus_read(game, TIMER0).wrapping_sub(1);
    bus_write(game, TIMER0, t0);
    // DEC sets N/Z; BPL branches on N = 0.
    let minus = t0 & 0x80 != 0;
    set_nz(&mut game.cpu.p, t0);
    if minus {
        // LDA #$14 : STA $0500 : LDX #$18.
        game.cpu.a = TIMER0_RELOAD;
        set_nz(&mut game.cpu.p, TIMER0_RELOAD);
        bus_write(game, TIMER0, TIMER0_RELOAD);
        game.cpu.x = SWEEP_WIDE;
        set_nz(&mut game.cpu.p, SWEEP_WIDE);
    }
    // Sweep X .. 0: LDA $0501,x : BEQ skip : DEC $0501,x / skip: DEX : BPL.
    let mut x = game.cpu.x;
    loop {
        let addr = TIMER_BASE + u16::from(x);
        let v = bus_read(game, addr);
        set_nz(&mut game.cpu.p, v); // LDA $0501,x.
        if v != 0 {
            let d = v.wrapping_sub(1); // DEC $0501,x.
            bus_write(game, addr, d);
            set_nz(&mut game.cpu.p, d);
        }
        // DEX : BPL (exit on wrap to $FF).
        x = x.wrapping_sub(1);
        set_nz(&mut game.cpu.p, x);
        if x == 0xFF {
            break;
        }
    }
    game.cpu.x = 0xFF;
    // INC $12 (N/Z from the result; C/V/I/D untouched).
    let f = bus_read(game, FRAME_COUNTER).wrapping_add(1);
    bus_write(game, FRAME_COUNTER, f);
    set_nz(&mut game.cpu.p, f);
}

/// Full NMI counter block (`prg7.asm $C169-$C1B0`): timers, frame counter,
/// RNG prologue (`LDX #$00 : LDY #$09`) and the LFSR advance.
///
/// Delegates the LFSR to [`rng_advance`], which replicates the prologue's
/// net register/flag effects (`X = 9`, `Y = 0`, `A = b0 ^ b1`, `C` from the
/// last `ROR`, `Z = 1`, `N = 0`).
pub fn tick_nmi_counters(game: &mut Game) {
    tick_global_timers(game);
    game.cpu.x = 0x00; // LDX #$00 ($C185).
    set_nz(&mut game.cpu.p, 0x00);
    game.cpu.y = 0x09; // LDY #$09 ($C187).
    set_nz(&mut game.cpu.p, 0x09);
    rng_advance(game);
}
