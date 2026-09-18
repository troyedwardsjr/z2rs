//! Shared ALU-flag helpers for the bank-7 ports.
//!
//! Every helper mirrors one 6502 flag rule used by the routines in
//! `bank7_*`: only `N`/`Z`/`C`/`V` are ever touched here (`I`/`D` survive,
//! `B`/`U` are push/pull conventions owned by `cpu.rs`). Callers cite the
//! exact `prg7.asm` address whose instruction semantics they replicate.

use crate::cpu::{FLAG_C, FLAG_N, FLAG_V, FLAG_Z};
use crate::game::{Game, CALL_ASM_BUDGET};
use crate::traps::TrapExit;

/// `N`/`Z` from a result byte (LDA/LDX/LDY/AND/EOR/ORA/TAX/...).
pub(crate) fn set_nz(p: &mut u8, v: u8) {
    if v == 0 {
        *p |= FLAG_Z;
    } else {
        *p &= !FLAG_Z;
    }
    if v & FLAG_N != 0 {
        *p |= FLAG_N;
    } else {
        *p &= !FLAG_N;
    }
}

/// Memory `ROL` with an explicit carry-in: returns `(result, carry_out)`
/// and sets `N`/`Z` from the result (caller sets `C` from `carry_out`).
pub(crate) fn rol_mem(p: &mut u8, carry_in: bool, v: u8) -> (u8, bool) {
    let out = v & 0x80 != 0;
    let r = (v << 1) | u8::from(carry_in);
    set_nz(p, r);
    (r, out)
}

/// `INC` on a byte: wraps, sets `N`/`Z` (`C` untouched).
pub(crate) fn inc_val(p: &mut u8, v: u8) -> u8 {
    let r = v.wrapping_add(1);
    set_nz(p, r);
    r
}

/// `CMP`/`CPX`/`CPY` (register vs immediate/memory): `C` = reg >= rhs.
pub(crate) fn cmp_val(p: &mut u8, reg: u8, rhs: u8) {
    let res = reg.wrapping_sub(rhs);
    if reg >= rhs {
        *p |= FLAG_C;
    } else {
        *p &= !FLAG_C;
    }
    if res == 0 {
        *p |= FLAG_Z;
    } else {
        *p &= !FLAG_Z;
    }
    if res & FLAG_N != 0 {
        *p |= FLAG_N;
    } else {
        *p &= !FLAG_N;
    }
}

/// Binary `ADC` (the 2A03 has no decimal mode; `D` ignored, like `cpu.rs`).
pub(crate) fn adc_val(p: &mut u8, a: u8, rhs: u8) -> u8 {
    let c = u8::from(*p & FLAG_C != 0);
    let sum = u16::from(a) + u16::from(rhs) + u16::from(c);
    let res = sum as u8;
    if sum > 0xFF {
        *p |= FLAG_C;
    } else {
        *p &= !FLAG_C;
    }
    if ((a ^ res) & (rhs ^ res) & 0x80) != 0 {
        *p |= FLAG_V;
    } else {
        *p &= !FLAG_V;
    }
    set_nz(p, res);
    res
}

/// `SBC` as `ADC(!rhs)` with the incoming carry (`SEC` = borrow none).
pub(crate) fn sbc_val(p: &mut u8, a: u8, rhs: u8) -> u8 {
    adc_val(p, a, !rhs)
}

// ------------------------------------------------- inner-JSR stack framing
//
// A trapped routine whose ASM body `JSR`s another routine leaves that inner
// return address as stale stack bytes (e.g. `$D34F` at `$01F8/$01F9` after
// `bank7_Controllers_Input`). The Rust ports call the inner Rust body
// directly, so they replicate the framing traffic explicitly — push the
// same return the ASM `JSR` would (`site + 2`), run the body, pop — keeping
// trapped runs bit-identical down to dead stack bytes. Order mirrors
// `cpu.rs::push16`/`pop16` exactly (high byte at `SP`, then low).
//
// A body may leave the frame the way the ROM does: with its `RTS` (the
// frame is popped here) or with a tail jump (`Game::trap_jump`) — the
// target then runs inside the frame, trapped or interpreted, until an
// `RTS` pops it (`continue_in_frame`). `jsr_sub` is the same frame around
// a routine outside the port: its trap when it has one, the ROM bytes
// otherwise, never a synthetic return address.

/// Push a 16-bit return address (mirrors `cpu.rs::push16`).
pub(crate) fn push_word(game: &mut Game, ret: u16) {
    game.ram[0x0100 + usize::from(game.cpu.sp)] = (ret >> 8) as u8;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
    game.ram[0x0100 + usize::from(game.cpu.sp)] = ret as u8;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// Pop a 16-bit return address (mirrors `cpu.rs::pop16`).
pub(crate) fn pop_word(game: &mut Game) -> u16 {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    let lo = u16::from(game.ram[0x0100 + usize::from(game.cpu.sp)]);
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    let hi = u16::from(game.ram[0x0100 + usize::from(game.cpu.sp)]);
    lo | (hi << 8)
}

/// Run `body` inside one inner-`JSR` frame for `site` (a `JSR site` in the
/// original pushes `site + 2`; the matching `RTS` pops it back, leaving the
/// bytes stale). Balanced: `SP` is unchanged afterwards. A tail jump the
/// body requests ([`Game::trap_jump`]) runs inside the frame
/// ([`continue_in_frame`]).
pub(crate) fn inner_jsr_frame(game: &mut Game, site: u16, body: fn(&mut Game)) {
    let sp_ret = game.cpu.sp;
    push_word(game, site.wrapping_add(2));
    body(game);
    match game.take_trap_jump() {
        None => {
            let _ = pop_word(game);
        }
        Some(target) => continue_in_frame(game, target, sp_ret),
    }
}

/// `JSR target` from a ported body, the way the interpreter's `JSR` runs
/// it: the `site + 2` return frame is pushed, a trapped target fires (tail
/// jumps included, see [`continue_in_frame`]) and an unported one
/// interprets until its `RTS` pops the frame — nothing synthetic lands on
/// the stack, so dead stack bytes match the ROM's. The caller charges the
/// `JSR` (6); the body charges itself.
pub(crate) fn jsr_sub(game: &mut Game, site: u16, target: u16) {
    let sp_ret = game.cpu.sp;
    push_word(game, site.wrapping_add(2));
    continue_in_frame(game, target, sp_ret);
}

/// Continue at `target` inside a `JSR` frame whose `RTS` restores `sp_ret`:
/// a trapped target fires (chaining through trapped tail jumps) and the
/// frame is popped afterwards; an unported target — or the unported end of
/// the chain — is interpreted until that `RTS` pops the frame itself. A
/// callee that consumed the frame (`PLA : PLA : JMP`, `SP` already back at
/// `sp_ret`) turns the transfer into a non-local exit from the port: its
/// jump is handed on to the port's own dispatcher (and the port must return
/// at once), a plain return leaves the stack alone.
fn continue_in_frame(game: &mut Game, target: u16, sp_ret: u8) {
    let target = if game.traps.is_trapped(target) {
        match game.fire_trap(target) {
            TrapExit::Return => {
                if game.cpu.sp != sp_ret {
                    let _ = pop_word(game);
                }
                return;
            }
            TrapExit::Jump(next) => next,
        }
    } else {
        target
    };
    if game.cpu.sp == sp_ret {
        game.trap_jump(target);
        return;
    }
    if let Err(e) = game.run_until_sp(target, sp_ret, CALL_ASM_BUDGET) {
        panic!("jsr frame continuation at ${target:04X}: {e}");
    }
}
