//! Controller-input ports.
//!
//! | Rust fn | Label | Addr |
//! |---|---|---|
//! | [`capture_controller`] | `bank7_Controllers_Input_Capture` | `$D367` |
//! | [`read_controllers`] | `bank7_Controllers_Input` | `$D346` |
//!
//! `prg7.asm $D367-$D381` reference:
//! ```asm
//! LDX #$01 : STX $4016 : DEX : STX $4016   ; strobe 1 then 0 (latch)
//! LDX #$08
//! @Loop: LDA $4016 : LSR : ROL $F5 : LDA $4017 : LSR : ROL $F6
//!        DEX : BNE @Loop
//!        RTS
//! ```
//!
//! `prg7.asm $D346-$D366` reference: capture twice into `$F5`, repeat from
//! the top while the two reads disagree (debounce); then edge-detect pad 1
//! and pad 2 (`X = 1, 0`):
//! ```asm
//! LDA $F5,x : TAY : EOR $F7,x : AND $F5,x : STA $F5,x : STY $F7,x
//! ```
//! so `$F5/$F6` end up holding *pressed-this-frame* edges
//! (`new & ~old`) and `$F7/$F8` hold the *held* levels.
//!
//! ## Bit order
//!
//! The `ROL` loop shifts the first-polled bit (`A`) to bit 7, so the
//! assembled bytes are MSB-first: bit 7 = A, 6 = B, 5 = Select, 4 = Start,
//! 3 = Up, 2 = Down, 1 = Left, 0 = Right. (The *pad latch* order used by
//! [`Game::step`](crate::game::Game::step) is LSB-first; the two orders
//! coexist: latch bits are indices, `$F5` bits are reversed.) The port
//! replicates the `ROL` sequence exactly, so the order falls out of the
//! emulation rather than a lookup.
//!
//! All pad traffic goes through the bus (`$4016`/`$4017`), preserving the
//! strobe-latch and shift-register side effects, so trapped/untrapped A/B
//! runs (and the oracle, which latches the same FM2 bit order) agree.

use crate::bank7_common::{cmp_val, inner_jsr_frame, rol_mem, set_nz};
use crate::cpu::{bus_read, bus_write, FLAG_C, FLAG_N, FLAG_Z};
use crate::game::Game;

/// `$4016` strobe/latch port.
const JOY1: u16 = 0x4016;
/// `$4017` controller-2 read port.
const JOY2: u16 = 0x4017;
/// Assembled pad bytes (`joy1/2_pressed`, edges after [`read_controllers`]).
const PAD1_EDGE: u16 = 0x00F5;
/// Pad-2 edge byte.
const PAD2_EDGE: u16 = 0x00F6;
/// Held-level bytes (`joy1_held` at `+0`, `joy2_held` at `+1`).
const PAD1_HELD: u16 = 0x00F7;
/// Zero-page scratch used by the debounce compare.
const SCRATCH: usize = 0x000;

/// One `LDA $401x : LSR` step: returns the shifted-out pad bit in `C` and
/// leaves `A = 0` (`LSR` of a `0`/`1` read), `N = 0`, `Z = 1`.
fn poll_bit(game: &mut Game, port: u16) -> bool {
    let v = bus_read(game, port);
    // LSR (accumulator) of a 0/1 value: C = bit 0, A = 0.
    let c = v & 1 != 0;
    if c {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    game.cpu.a = 0;
    game.cpu.p &= !FLAG_N;
    game.cpu.p |= FLAG_Z;
    c
}

/// `bank7_Controllers_Input_Capture` (`prg7.asm $D367`).
///
/// Strobe-latches both pads, then shifts 8 bits from each serial port into
/// `$F5`/`$F6` via `ROL`. Fully overwrites both bytes (eight left-rotates
/// flush any prior value), so the result is independent of entry `$F5/$F6`.
///
/// Exit: `X = 0`, `Y` untouched, `A = 0`, `N = 0`, `Z = 1`, `C` = last
/// pad-2 bit (Right). `V`/`I`/`D` untouched.
pub fn capture_controller(game: &mut Game) {
    // LDX #$01 : STX $4016 : DEX : STX $4016 (strobe on, then off = latch).
    game.cpu.x = 1;
    set_nz(&mut game.cpu.p, 1);
    bus_write(game, JOY1, 1);
    game.cpu.x = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, JOY1, 0);
    // LDX #$08.
    game.cpu.x = 8;
    set_nz(&mut game.cpu.p, 8);
    for _ in 0..8u8 {
        // LDA $4016 : LSR : ROL $F5.
        let c0 = poll_bit(game, JOY1);
        let cur = game.ram[PAD1_EDGE as usize & 0x7FF];
        let (r, out) = rol_mem(&mut game.cpu.p, c0, cur);
        game.ram[PAD1_EDGE as usize & 0x7FF] = r;
        if out {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        // LDA $4017 : LSR : ROL $F6.
        let c1 = poll_bit(game, JOY2);
        let cur = game.ram[PAD2_EDGE as usize & 0x7FF];
        let (r, out) = rol_mem(&mut game.cpu.p, c1, cur);
        game.ram[PAD2_EDGE as usize & 0x7FF] = r;
        if out {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        // DEX : BNE @Loop.
        game.cpu.x = game.cpu.x.wrapping_sub(1);
        set_nz(&mut game.cpu.p, game.cpu.x);
    }
    // Loop exits with X = 0 (last DEX 1 -> 0 sets Z = 1, N = 0); A = 0
    // from the final LSR. Both match the ASM fall-through to RTS.
}

/// `bank7_Controllers_Input` (`prg7.asm $D346`).
///
/// Debounced read + edge detect (see module docs). Exit: `X = $FF`
/// (final `DEX` 0 -> FF sets `N = 1`, `Z = 0`), `A` = pad-1 pressed edges,
/// `Y` = pad-1 held levels. `$00` holds the first capture's `$F5` of the
/// final debounce round. `C`/`V`/`I`/`D` are whatever the last `ROL`/`EOR`
/// sequence left (`EOR`/`AND` set `N`/`Z`, then `DEX` overwrites them).
pub fn read_controllers(game: &mut Game) {
    // Debounce: capture until two consecutive $F5 reads agree.
    // (`LDA $F5 : STA $00` then `LDA $F5 : CMP $00 : BNE bank7_Controllers_Input`;
    // the CMP sets C/Z/N even on the agreeing pass — replicated, not skipped.)
    // Each capture runs inside its inner-JSR frame (`JSR $D367` at `$D346`
    // pushing `$D348`, at `$D34D` pushing `$D34F`) so stale stack bytes match.
    let mut rounds: u64 = 0;
    loop {
        inner_jsr_frame(game, 0xD346, capture_controller);
        game.ram[SCRATCH] = game.ram[PAD1_EDGE as usize & 0x7FF];
        inner_jsr_frame(game, 0xD34D, capture_controller);
        let cur = game.ram[PAD1_EDGE as usize];
        let prev = game.ram[SCRATCH];
        game.cpu.a = cur;
        set_nz(&mut game.cpu.p, cur);
        cmp_val(&mut game.cpu.p, cur, prev);
        rounds += 1;
        if cur == prev {
            break;
        }
    }
    // The tabled cost covers one agreeing round; every disagreeing round
    // before it costs another 2x(JSR 6 + capture 235) + LDA 3 + STA 3 +
    // LDA 3 + CMP 3 + BNE taken 3 = 497.
    game.cpu.cycles += 497 * (rounds - 1);
    // Edge detect for X = 1 then 0 (pad 2, then pad 1).
    let mut x = 1u8;
    loop {
        // LDA $F5,x : TAY : EOR $F7,x : AND $F5,x : STA $F5,x : STY $F7,x.
        let edge_addr = (PAD1_EDGE + u16::from(x)) as usize & 0x7FF;
        let held_addr = (PAD1_HELD + u16::from(x)) as usize & 0x7FF;
        let pressed = game.ram[edge_addr];
        game.cpu.a = pressed;
        set_nz(&mut game.cpu.p, pressed);
        game.cpu.y = pressed;
        set_nz(&mut game.cpu.p, pressed);
        let held = game.ram[held_addr];
        game.cpu.a ^= held;
        set_nz(&mut game.cpu.p, game.cpu.a);
        game.cpu.a &= game.ram[edge_addr];
        set_nz(&mut game.cpu.p, game.cpu.a);
        game.ram[edge_addr] = game.cpu.a;
        game.ram[held_addr] = game.cpu.y;
        // DEX : BPL @Loop (exits when X wraps 0 -> $FF).
        x = x.wrapping_sub(1);
        game.cpu.x = x;
        set_nz(&mut game.cpu.p, x);
        if x == 0xFF {
            break;
        }
    }
}
