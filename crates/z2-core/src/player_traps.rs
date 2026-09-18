//! Player trap shims + registration.
//!
//! `Game`-shim layer over the pure `player` / `player_magic`
//! modules. Each `fn(&mut Game)` reads its inputs from
//! `game.ram`/`game.wram`, calls a pure helper, writes the results back.
//!
//! | shim | label | addr |
//! |---|---|---|
//! | [`pl_gravity`] | `bank7_applyGravityMotion` | `$D19B` |
//! | [`pl_xy_move`] | `bank7_XY_Movements_Routine` | `$D1CE` |
//! | [`pl_death_check`] | `bank7_check_if_link_died_0494__linkdeath` | `$D3CC` |
//! | [`pl_link_hit`] | `bank7_Link_Hit_Routine` | `$E2EF` |
//! | [`pl_recoil`] | `bank7_Set_Links_Recoil` | `$E399` |
//! | [`pl_code37`] | `bank7_code37` | `$E371` |
//! | [`pl_code39`] | `bank7_code39` | `$E558` |
//! | [`pl_sword_hit`] | `bank7_Sword_Hit_Detection…` | `$E677` |
//! | [`pl_shield_box`] | `bank7_code43` | `$E975` |
//! | [`pl_sword_box`] | `bank7_code44` | `$E9A2` |
//! | [`pl_body_box`] | `bank7_code45` | `$E9D8` |
//! | [`pl_overlap`] | `bank7_idem__maybe` | `$E9F9` |
//! | [`pl_fairy_step`] | `bank7_code47` | `$EBB8` |
//!
//! Slot convention: every shim is indexed by the CPU `X` register exactly
//! like the ROM (the physics callers pass Link = 0, enemy slot + 1,
//! projectile slot + 7/`$0D`; the contact routines get the enemy slot, and
//! the box overlap `$E9F9` reloads `X` from `$10` on exit). Link's own
//! bytes are the slot-0 mirrors (`$29`/`$4D`/`$3B`/`$70`/`$57D`/…).
//!
//! Verification: the any% movie (`--trap-set bank7,player`, 3000 frames)
//! matches the bank-7-only baseline in RAM/OAM/palette; it never reaches
//! the damage shims (`$E2EF`/`$E371`/`$E399`/`$E558`/`$E677`: no enemy
//! contact or sword hit in that window), which are instead A/B-tested
//! against the ROM bytes through the trap dispatcher on seeded random
//! state (`tests/player_traps_ab.rs`, ROM-gated), together with the
//! physics and box shims. `$D3CC` is the mode-`$0B` (Side View Main)
//! table entry, dispatched by the `$D382`/`$D385` trampolines (their ports
//! tail-jump into it with [`Game::trap_jump`], the ROM bytes with
//! `JMP ($0E)`), so the shim ports the death gate exactly and hands the
//! rest of the mode routine back to the interpreter at `$D3E9` (or `$E18A`
//! on death) the same way; its own A/B runs from a `JMP` stub up to that
//! hand-off. `$EBB8` (branch-only entry at `$EC20`) never
//! dispatches. The `LD3E9` region (`$D3E9-$D545`: bank-0 spell cast,
//! meter/exp ticks, the scroll write, `Hub_Update_Routine`, Link display,
//! tail `JMP L99E6`) is only ever reached by the two branches inside
//! `$D3CC`, is not a routine entry, and is intentionally not registered
//! (see [`PLAYER_TRAPS_UNREGISTERED`]).
//!
//! Banked code (bank 0 spells `$8DC3` ff., `bank0_Fairy_Spell` `$91A4`,
//! `Thunder_Spell` `$91E6`, `Hub_Update_Routine` `$968D`) is listed in
//! [`PLAYER_TRAPS_BANKED`] with `bank = None` and intentionally *not*
//! registered: 16-bit trap keys alias across `$8000-$BFFF` (see `traps.rs`
//! M1 caveat). Same rule as `SIDEVIEW_TRAPS` in `sideview_traps.rs`.
//! Display emission (`bank7_Links_Display_Routine`, `$EBF0`) is PPU scope
//! and stays unregistered; see [`PLAYER_TRAPS_UNREGISTERED`].
//!
//! Reused read-only (never copied here): `sideview_collision` tile/coll bits
//! for the lava/water + stab-break folds, `bank7_timers` for the NMI timer
//! sweep that owns `$0500`/`$0518` reloads.

use crate::bank7_common::{adc_val, cmp_val, inner_jsr_frame, pop_word, sbc_val, set_nz};
use crate::cpu::{bus_read, bus_write, FLAG_C};
use crate::game::Game;
use crate::player as pl;
use crate::player_magic as pm;

// ---------------------------------------------------------------------------
// Trap tables.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Fixed-bank player traps actually registered by [`register_player_traps`].
pub const PLAYER_TRAPS: &[TrapEntry] = &[
    ("bank7_applyGravityMotion", Some(7), 0xD19B),
    ("bank7_XY_Movements_Routine", Some(7), 0xD1CE),
    ("bank7_check_if_link_died_0494__linkdeath", Some(7), 0xD3CC),
    ("bank7_Link_Hit_Routine", Some(7), 0xE2EF),
    ("bank7_Set_Links_Recoil", Some(7), 0xE399),
    ("bank7_code37", Some(7), 0xE371),
    ("bank7_code39", Some(7), 0xE558),
    (
        "bank7_Sword_Hit_Detection_maybe__probably_part_of_it_at_least",
        Some(7),
        0xE677,
    ),
    ("bank7_code43", Some(7), 0xE975),
    ("bank7_code44", Some(7), 0xE9A2),
    ("bank7_code45", Some(7), 0xE9D8),
    ("bank7_idem__maybe", Some(7), 0xE9F9),
    ("bank7_code47", Some(7), 0xEBB8),
];

/// Number of fixed-bank player traps actually registered.
pub const PLAYER_TRAP_COUNT: usize = 13;

/// Banked (bank 0) spell/level routines: listed, never registered (aliasing).
pub const PLAYER_TRAPS_BANKED: &[TrapEntry] = &[
    ("Spell_Casting_Routine", None, 0x8DC3),
    ("Jump_Spell", None, 0x8E58),
    ("Life_Spell", None, 0x8E5D),
    ("Spell_Spell", None, 0x8E73),
    ("Shield_Spell", None, 0x8E8D),
    ("Reflect_Spell", None, 0x8E96),
    ("spells_routines2", None, 0x8ED8),
    ("bank0_Fairy_Spell", None, 0x91A4),
    ("L91AF", None, 0x91AF),
    ("Thunder_Spell", None, 0x91E6),
    ("Hub_Update_Routine", None, 0x968D),
    ("Table_for_Magic_Needed_for_Spells", None, 0x8D7B),
    ("Table_for_Spell_effects", None, 0x8DBB),
    ("Table_for_levelup_experience_high", None, 0x9659),
    ("Table_for_levelup_experience_low", None, 0x9671),
];

/// Fixed-bank code intentionally unregistered: `bank7_Links_Display_Routine`
/// is display/PPU scope, and `LD3E9` is the body of the Side View Main mode
/// routine after the [`pl_death_check`] gate — a branch target only (never
/// a `JSR`/`JMP` destination, so a trap there could never dispatch) that
/// calls into bank 0 and writes the PPU scroll, and is left to the
/// interpreter as a whole.
pub const PLAYER_TRAPS_UNREGISTERED: &[TrapEntry] = &[
    ("bank7_Links_Display_Routine", Some(7), 0xEBF0),
    ("LD3E9_side_view_main_body", Some(7), 0xD3E9),
];

// ---------------------------------------------------------------------------
// Small ram helpers (direct mirror access; same convention as
// sideview_traps.rs).
// ---------------------------------------------------------------------------

fn r(ram: &[u8; 0x800], a: u16) -> u8 {
    ram[a as usize & 0x7FF]
}

fn w(ram: &mut [u8; 0x800], a: u16, v: u8) {
    let i = a as usize & 0x7FF;
    ram[i] = v;
}

// Cheapest-path cycle bases tabled by [`register_player_traps`]. Every shim
// keeps a per-call ledger (`cy`): each instruction on the executed path
// adds its `cpu.rs` cost (taken branches 3, +1 across a page), an inner
// `JSR` adds 6 plus the callee's base (the callee charges its own path
// extras), and the shim charges `cy - BASE`, so the total equals the ASM
// cost. Private helpers (`$DC91`, `$E5F3`, `LE942`) charge their whole cost
// themselves, so their callers ledger only the `JSR`.
const BASE_GRAVITY: u64 = 70;
const BASE_XY_MOVE: u64 = 108;
const BASE_DEATH_CHECK: u64 = 7;
const BASE_LINK_HIT: u64 = 36;
const BASE_RECOIL: u64 = 43;
const BASE_CODE37: u64 = 58;
const BASE_CODE39: u64 = 140;
const BASE_SWORD_HIT: u64 = 27;
const BASE_SHIELD_BOX: u64 = 52;
const BASE_SWORD_BOX: u64 = 71;
const BASE_BODY_BOX: u64 = 52;
const BASE_OVERLAP: u64 = 46;
const BASE_FAIRY: u64 = 14;

/// `LDA abs,Y` out of the fixed bank (`$C000-$FFFF` is always bank 7):
/// returns the byte and charges the page-cross cycle the way `cpu.rs`
/// does (`+1` when `base + y` leaves the base page). The hitbox routines
/// index their two-entry tables with whatever `$17`/`$9F` hold, so the
/// read goes through the bus instead of a clamped constant.
fn abs_y(game: &mut Game, base: u16, y: u8) -> u8 {
    let addr = base.wrapping_add(u16::from(y));
    if addr & 0xFF00 != base & 0xFF00 {
        game.cpu.cycles += 1;
    }
    bus_read(game, addr)
}

/// Set or clear `C` (a `SEC`/`CLC`, or the carry parked by a `ROR`).
fn set_c(p: &mut u8, c: bool) {
    if c {
        *p |= FLAG_C;
    } else {
        *p &= !FLAG_C;
    }
}

// ---------------------------------------------------------------------------
// Shims: physics.
// ---------------------------------------------------------------------------

/// `bank7_applyGravityMotion` (bank 7 `$D19B`).
///
/// `X`-indexed on the CPU register: slot 0 = Link (bank 0 `$9561`/`$9569`),
/// enemy slot + 1 from the `$DECE` wrapper. `$00` = gravity add, `$02` =
/// max fall (staged by the callers); integrates `$29,x`/`$19,x`, the
/// counter `$03E6,x` and `$057D,x`. Exit state as the bytes leave it:
/// `Y`/`$07` = sign byte of the incoming velocity, `A` = the new velocity
/// (or `0` on the `CMP $02` clamp path, which also clears the counter),
/// flags from that `CMP` (`LDA #$00` on the clamp path).
///
/// Cycles: 70 on the positive, unclamped path (`register_player_traps`);
/// `+1` for a negative velocity (`DEY`), `+11` for the clamp re-store.
pub fn pl_gravity(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let vs = r(&game.ram, 0x057D + x);
    let sign = if vs & 0x80 != 0 { 0xFF } else { 0x00 };
    game.cpu.y = sign;
    w(&mut game.ram, 0x0007, sign);
    let mut p = game.cpu.p;
    // $D1A5 CLC : ADC $29,x : STA $29,x ; LDA $19,x : ADC $07 : STA $19,x.
    p &= !FLAG_C;
    let y = adc_val(&mut p, vs, r(&game.ram, 0x0029 + x));
    w(&mut game.ram, 0x0029 + x, y);
    let yh = adc_val(&mut p, r(&game.ram, 0x0019 + x), sign);
    w(&mut game.ram, 0x0019 + x, yh);
    // $D1B0 LDA $03E6,x : CLC : ADC $00 : STA ; LDA $057D,x : ADC #$00 : STA.
    p &= !FLAG_C;
    let ctr = adc_val(&mut p, r(&game.ram, 0x03E6 + x), r(&game.ram, 0x0000));
    w(&mut game.ram, 0x03E6 + x, ctr);
    let vs1 = adc_val(&mut p, r(&game.ram, 0x057D + x), 0x00);
    w(&mut game.ram, 0x057D + x, vs1);
    // $D1C1 CMP $02 : BNE LD1CD — terminal clamp re-stores and clears the
    // counter (`LDA #$00` leaves A = 0, Z set).
    let max_fall = r(&game.ram, 0x0002);
    cmp_val(&mut p, vs1, max_fall);
    if vs1 == max_fall {
        set_nz(&mut p, 0x00);
        w(&mut game.ram, 0x03E6 + x, 0x00);
        game.cpu.a = 0x00;
        game.cpu.cycles += 11;
    } else {
        game.cpu.a = vs1;
    }
    if sign != 0 {
        game.cpu.cycles += 1;
    }
    game.cpu.p = p;
}

/// `bank7_XY_Movements_Routine` (bank 7 `$D1CE`).
///
/// `X`-indexed on the CPU register: 0 = Link (bank 0 `$9628`), enemy slot
/// plus one from `bank7_Simple_Horizontal_Movement` (`$DEB8`), slot plus
/// 7 / `$0D` for the projectiles (`$DED4`, bank 0 `$98AB`). Splits `$70,x`
/// into `$01` (low nibble `<< 4`) and `$00` (sign-extended high nibble),
/// `Y`/`$02` = sign byte; adds `$01` into `$03D6,x` and the carry + `$00`
/// / `$02` into `$4D,x` / `$3B,x`. Returns `A = carry + $00` (the
/// whole-pixel delta; bank 0 keeps it in `$14`) with the flags of that
/// final `CLC : ADC $00`. The `ROL : PHA : ROR : … : PLA` carry hand-off
/// leaves the carry byte at `$0100+SP` (dead stack).
///
/// Cycles: 108 on the positive path (`register_player_traps`); `+2` for a
/// negative velocity (`ORA #$F0` + `DEY` replace two taken branches).
pub fn pl_xy_move(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let hspeed = r(&game.ram, 0x0070 + x);
    let (lo, hi, sign) = pl::vel_split(hspeed);
    w(&mut game.ram, 0x0001, lo);
    w(&mut game.ram, 0x0000, hi);
    w(&mut game.ram, 0x0002, sign);
    game.cpu.y = sign;
    let (sub, carry) = r(&game.ram, 0x03D6 + x).overflowing_add(lo);
    w(&mut game.ram, 0x03D6 + x, sub);
    // $D1F6 LDA #$00 : ROL : PHA : ROR — the subpixel carry parks on the
    // stack and comes back as the carry-in of the position add.
    let c = u8::from(carry);
    game.ram[0x0100 + usize::from(game.cpu.sp)] = c;
    let mut p = game.cpu.p;
    if carry {
        p |= FLAG_C;
    } else {
        p &= !FLAG_C;
    }
    let x_lo = adc_val(&mut p, r(&game.ram, 0x004D + x), hi);
    w(&mut game.ram, 0x004D + x, x_lo);
    let x_hi = adc_val(&mut p, r(&game.ram, 0x003B + x), sign);
    w(&mut game.ram, 0x003B + x, x_hi);
    // `LD247`: PLA : CLC : ADC $00 : RTS.
    p &= !FLAG_C;
    game.cpu.a = adc_val(&mut p, c, hi);
    game.cpu.p = p;
    if sign != 0 {
        game.cpu.cycles += 2;
    }
}

/// `bank7_check_if_link_died_0494__linkdeath` (bank 7 `$D3CC`): the
/// mode-`$0B` Side View Main entry (`$C302`), reached through the `$D385`
/// trampoline's `JMP ($0E)` with whatever registers the mode loop left.
///
/// Only the death gate (`$D3CC-$D3E6`) is the shim's body; the rest of the
/// mode routine (`LD3E9`: bank-0 spell cast, meter/exp ticks, scroll
/// write, `Hub_Update_Routine`, Link display, tail `JMP L99E6`) stays with
/// the interpreter, so both ROM branches to `LD3E9` become a
/// [`Game::trap_jump`] to `$D3E9`. `$0494 != 0` (kill flag) with `$050C == 0`
/// (injured timer expired) commits death via [`pm::death_check`]: `$0494 =
/// 0`, `$EC = 1` (the `INX`ed `X`), `$076C = 2`, `$2001 = 0` through the
/// bus (rendering off), then the tail `JMP LE18A` (`INC $0726` +
/// `bank7_KillAllMonsters`, interpreter) the same way. Exit `A`/`X`/flags
/// as the last load left them: `A = $0494` (`Z`) on the no-kill branch,
/// `X = $050C` on the injured branch, `A = 0`/`X = 1`/`Z` on death.
///
/// Cycles: 7 on the no-kill branch (`register_player_traps`); `+6` while
/// the injured timer runs, `+29` on the death path (its `JMP` included; no
/// `RTS` executes, the hand-off is the branch/`JMP` itself).
pub fn pl_death_check(game: &mut Game) {
    let mut p = game.cpu.p;
    // $D3CC LDA $0494 : BEQ LD3E9.
    let kill = r(&game.ram, 0x0494);
    game.cpu.a = kill;
    set_nz(&mut p, kill);
    if kill == 0 {
        game.cpu.p = p;
        game.trap_jump(0xD3E9);
        return;
    }
    // $D3D1 LDX $050C : BNE LD3E9.
    let injured = r(&game.ram, 0x050C);
    game.cpu.x = injured;
    set_nz(&mut p, injured);
    let Some((state, sound)) = pm::death_check(kill, injured) else {
        game.cpu.p = p;
        game.cpu.cycles += 6;
        game.trap_jump(0xD3E9);
        return;
    };
    // $D3D6 STX $0494 : INX : STX $EC ; LDA #$02 : STA $076C ; LDA #$00 :
    // STA $2001 ; JMP LE18A.
    w(&mut game.ram, 0x0494, injured);
    let x = injured.wrapping_add(1);
    game.cpu.x = x;
    w(&mut game.ram, 0x00EC, sound);
    w(&mut game.ram, 0x076C, state);
    game.cpu.a = 0x00;
    set_nz(&mut p, 0x00);
    game.cpu.p = p;
    bus_write(game, 0x2001, 0x00);
    game.cpu.cycles += 29;
    game.trap_jump(0xE18A);
}

// ---------------------------------------------------------------------------
// Shims: damage.
//
// Instruction-exact ports of the contact routines. They are `X`-indexed on
// the CPU register like the ROM, and re-read `X` after every box overlap
// (`$E9F9` reloads it from `$10`). Inner `JSR`s go through
// `inner_jsr_frame` so the dead stack bytes match; `PHA`/`PLA` pairs move
// `SP` for the same reason. Tail jumps into routines that stay with the
// interpreter (`bank7_monster_death`, `bank7_get_item`) go through
// [`Game::trap_jump`].
// ---------------------------------------------------------------------------

/// `PHA`: park `v` at `$0100+SP` and drop `SP` (mirrors `cpu.rs::push`).
fn pha(game: &mut Game, v: u8) {
    game.ram[0x0100 + usize::from(game.cpu.sp)] = v;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// `PLA`: raise `SP` and read the byte back (flags are the caller's).
fn pla(game: &mut Game) -> u8 {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    game.ram[0x0100 + usize::from(game.cpu.sp)]
}

/// `bank7_Determine_Enemy_Facing_Direction_relative_to_Link` (`$DC91`),
/// inlined for the recoil paths (the enemy group owns the registered
/// port; this private copy keeps the trap self-contained). `Y = 1`, then
/// `Link.x + 8` (16-bit; the carry-in is the caller's flag — no `CLC`)
/// minus `enemy.x` (`$4E,x`/`$3C,x`) into `$0F`/`$0E`; a negative result
/// bumps `Y` to 2. `$60,x = Y`, returns `Y - 1` with `A` = the high-byte
/// difference. Charges its whole cost (51, `+1` for the `INY`).
fn facing_dc91(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let mut p = game.cpu.p;
    let mut cy: u64 = 51;
    let mut y: u8 = 0x01;
    set_nz(&mut p, y);
    // LDA $4D : ADC #$08 : PHA ; LDA $3B : ADC #$00 : STA $0E ; PLA.
    let lo = adc_val(&mut p, r(&game.ram, 0x004D), 0x08);
    game.ram[0x0100 + usize::from(game.cpu.sp)] = lo;
    let hi = adc_val(&mut p, r(&game.ram, 0x003B), 0x00);
    w(&mut game.ram, 0x000E, hi);
    set_nz(&mut p, lo);
    // SBC $4E,x : STA $0F ; LDA $0E : SBC $3C,x : BPL LDCAA : INY.
    let dlo = sbc_val(&mut p, lo, r(&game.ram, 0x004E + x));
    w(&mut game.ram, 0x000F, dlo);
    set_nz(&mut p, hi);
    let dhi = sbc_val(&mut p, hi, r(&game.ram, 0x003C + x));
    if dhi & 0x80 != 0 {
        y = y.wrapping_add(1);
        set_nz(&mut p, y);
        cy += 1;
    }
    // LDCAA STY $60,x : DEY : RTS.
    w(&mut game.ram, 0x0060 + x, y);
    y = y.wrapping_sub(1);
    set_nz(&mut p, y);
    game.cpu.a = dhi;
    game.cpu.y = y;
    game.cpu.p = p;
    game.cpu.cycles += cy;
}

/// `bank7_check_if_shield_protects_from_sword_hit` (`$E5F3`): the enemy
/// weapon box into `$04-$07` from sprite slot `Y = $91,x` — `$05 =
/// $0210,y + 9`, `$07 = 1`, `$04 = $0213,y`, `$06 = $0E`, the width clipped
/// at the screen edge like `code43` (`$04 + $06` with the carry of the
/// `+ 9`). Charges its whole cost (47, `+7` clipped).
fn check_shield_e5f3(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let mut p = game.cpu.p;
    let mut cy: u64 = 47;
    let y = r(&game.ram, 0x0091 + x);
    set_nz(&mut p, y);
    let sy = abs_y(game, 0x0210, y);
    set_c(&mut p, false);
    let by = adc_val(&mut p, sy, 0x09);
    w(&mut game.ram, 0x0005, by);
    w(&mut game.ram, 0x0007, 0x01);
    let bx = abs_y(game, 0x0213, y);
    w(&mut game.ram, 0x0004, bx);
    w(&mut game.ram, 0x0006, 0x0E);
    set_nz(&mut p, bx);
    let mut a = adc_val(&mut p, bx, 0x0E);
    if p & FLAG_C != 0 {
        a = bx ^ 0xFF;
        set_nz(&mut p, a);
        w(&mut game.ram, 0x0006, a);
        cy += 7;
    }
    game.cpu.a = a;
    game.cpu.y = y;
    game.cpu.p = p;
    game.cpu.cycles += cy;
}

/// `LE942` (bank 7 `$E942`; the disassembly lists its bytes as data): the
/// enemy's own box into `$04-$07`. `Y = $6E1D[$A1,x] * 4` indexes the
/// four-byte `$E8FA` table: `$04 = $CD + [0]`, `$06 = [1]`, `$05 = $2A,x +
/// [2]`, `$07 = [3]`, width clipped at the screen edge (`$04 + $06` with
/// the carry of the Y add). Charges its whole cost (68, `+7` clipped, plus
/// the page-crossing table reads).
fn enemy_box_e942(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let mut p = game.cpu.p;
    let mut cy: u64 = 68;
    let code = r(&game.ram, 0x00A1 + x);
    set_nz(&mut p, code);
    let mut v = abs_y(game, 0x6E1D, code);
    set_nz(&mut p, v);
    for _ in 0..2 {
        set_c(&mut p, v & 0x80 != 0);
        v <<= 1;
        set_nz(&mut p, v);
    }
    let y = v;
    let t0 = abs_y(game, 0xE8FA, y);
    let t1 = abs_y(game, 0xE8FB, y);
    let t2 = abs_y(game, 0xE8FC, y);
    let t3 = abs_y(game, 0xE8FD, y);
    // LDA $CD : CLC : ADC [0] : STA $04 ; LDA [1] : STA $06.
    set_c(&mut p, false);
    let bx = adc_val(&mut p, r(&game.ram, 0x00CD), t0);
    w(&mut game.ram, 0x0004, bx);
    w(&mut game.ram, 0x0006, t1);
    // LDA $2A,x : CLC : ADC [2] : STA $05 ; LDA [3] : STA $07.
    set_c(&mut p, false);
    let by = adc_val(&mut p, r(&game.ram, 0x002A + x), t2);
    w(&mut game.ram, 0x0005, by);
    w(&mut game.ram, 0x0007, t3);
    // LDA $04 : ADC $06 : BCC ; LDA $04 : EOR #$FF : STA $06 ; RTS.
    set_nz(&mut p, bx);
    let mut a = adc_val(&mut p, bx, t1);
    if p & FLAG_C != 0 {
        a = bx ^ 0xFF;
        set_nz(&mut p, a);
        w(&mut game.ram, 0x0006, a);
        cy += 7;
    }
    game.cpu.a = a;
    game.cpu.y = y;
    game.cpu.p = p;
    game.cpu.cycles += cy;
}

/// `bank7_Link_Hit_Routine` (bank 7 `$E2EF`).
///
/// `Y = $A1,x` (enemy code). Unless Link is immune (`$0518 != 0`) the exp
/// loss `$05E8` is booked from the enemy's steal bit (`$6DD5,y & $10`: 0,
/// else `$0A`, or `$14` for Moa). `$0C = $6DF9,y & $0F` (damage code); an
/// immune Link exits `CLC`. Otherwise: hurt sound `$E9 = 1`, `$00 = 1`,
/// `bank7_Set_Links_Recoil`, damage `LE2AE[code * 8 + $0779]` (halved
/// under the shield spell `$070F`), `$0774 -= damage` with `$074F |= $40`;
/// on underflow `$0774 = 0` and `INC $0494` (death); then `$050C = $20`,
/// `$0518 = 4`, `$0400 = 0`, `$A7 &= $FB`, `$057D = $FE`, `SEC`.
///
/// Cycles: base 36 (immune path); the vulnerable path adds 112 plus the
/// recoil call, `+5`/`+6` for the steal bit (Moa), `+1` shield spell,
/// `+11` death.
pub fn pl_link_hit(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let mut p = game.cpu.p;
    let mut cy: u64 = 0;
    // $E2EF LDY $A1,x ; LDA $0518 : BNE LE308 (crosses into $E3xx).
    let y0 = r(&game.ram, 0x00A1 + x);
    set_nz(&mut p, y0);
    let immune = r(&game.ram, 0x0518);
    set_nz(&mut p, immune);
    cy += 4 + 4;
    if immune != 0 {
        cy += 4;
    } else {
        // LDA $6DD5,y : AND #$10 : BEQ LE305 (crosses) ; LDA #$0A : CPY
        // #$06 : BNE LE305 ; LDA #$14 ; LE305 STA $05E8.
        cy += 2;
        let steal = abs_y(game, 0x6DD5, y0) & 0x10;
        set_nz(&mut p, steal);
        cy += 4 + 2;
        let loss = if steal == 0 {
            cy += 4;
            steal
        } else {
            set_nz(&mut p, 0x0A);
            cmp_val(&mut p, y0, 0x06);
            cy += 2 + 2 + 2;
            if y0 == 0x06 {
                set_nz(&mut p, 0x14);
                cy += 2 + 2;
                0x14
            } else {
                cy += 3;
                0x0A
            }
        };
        w(&mut game.ram, 0x05E8, loss);
        cy += 4;
    }
    // LE308 LDA $6DF9,y : AND #$0F : STA $0C ; LDA $0518 : BNE LE368.
    let dmg = abs_y(game, 0x6DF9, y0) & 0x0F;
    set_nz(&mut p, dmg);
    w(&mut game.ram, 0x000C, dmg);
    set_nz(&mut p, immune);
    cy += 4 + 2 + 3 + 4;
    if immune != 0 {
        // LE368 CLC : RTS.
        set_c(&mut p, false);
        cy += 3 + 2 + 6;
        game.cpu.a = immune;
        game.cpu.y = y0;
        game.cpu.p = p;
        game.cpu.cycles += cy - BASE_LINK_HIT;
        return;
    }
    // LDA #$01 : STA $E9 : STA $00 ; JSR bank7_Set_Links_Recoil.
    set_nz(&mut p, 0x01);
    w(&mut game.ram, 0x00E9, 0x01);
    w(&mut game.ram, 0x0000, 0x01);
    game.cpu.a = 0x01;
    game.cpu.p = p;
    inner_jsr_frame(game, 0xE31A, pl_recoil);
    p = game.cpu.p;
    cy += 2 + 2 + 3 + 3 + 6 + BASE_RECOIL;
    // LDA $0C : ASL : ASL : ASL : ADC $0779 : TAY ; LDA LE2AE,y.
    let mut v = r(&game.ram, 0x000C);
    set_nz(&mut p, v);
    for _ in 0..3 {
        set_c(&mut p, v & 0x80 != 0);
        v <<= 1;
        set_nz(&mut p, v);
    }
    let idx = adc_val(&mut p, v, r(&game.ram, 0x0779));
    let mut damage = abs_y(game, 0xE2AE, idx);
    set_nz(&mut p, damage);
    cy += 3 + 2 + 2 + 2 + 4 + 2 + 4;
    // LDY $070F : BEQ LE32F : LSR ; LE32F STA $0C.
    let y = r(&game.ram, 0x070F);
    set_nz(&mut p, y);
    cy += 4;
    if y == 0 {
        cy += 3;
    } else {
        set_c(&mut p, damage & 0x01 != 0);
        damage >>= 1;
        set_nz(&mut p, damage);
        cy += 2 + 2;
    }
    w(&mut game.ram, 0x000C, damage);
    cy += 3;
    // LDA $0774 : SEC : SBC $0C : STA $0774 ; LDA $074F : ORA #$40 : STA
    // $074F ; BCS LE34C ; LDA #$00 : STA $0774 : INC $0494.
    set_c(&mut p, true);
    let hp = sbc_val(&mut p, r(&game.ram, 0x0774), damage);
    w(&mut game.ram, 0x0774, hp);
    let pane = r(&game.ram, 0x074F) | 0x40;
    set_nz(&mut p, pane);
    w(&mut game.ram, 0x074F, pane);
    cy += 4 + 2 + 3 + 4 + 4 + 2 + 4;
    if p & FLAG_C != 0 {
        cy += 3;
    } else {
        set_nz(&mut p, 0x00);
        w(&mut game.ram, 0x0774, 0x00);
        let k = r(&game.ram, 0x0494).wrapping_add(1);
        set_nz(&mut p, k);
        w(&mut game.ram, 0x0494, k);
        cy += 2 + 2 + 4 + 6;
    }
    // LE34C timers, sword frame, collision bits, knock-up ; SEC : RTS.
    w(&mut game.ram, 0x050C, 0x20);
    w(&mut game.ram, 0x0518, 0x04);
    w(&mut game.ram, 0x0400, 0x00);
    let a7 = r(&game.ram, 0x00A7) & 0xFB;
    set_nz(&mut p, a7);
    w(&mut game.ram, 0x00A7, a7);
    set_nz(&mut p, 0xFE);
    w(&mut game.ram, 0x057D, 0xFE);
    set_c(&mut p, true);
    cy += 2 + 4 + 2 + 4 + 2 + 4 + 3 + 2 + 3 + 2 + 4 + 2 + 6;
    game.cpu.a = 0xFE;
    game.cpu.y = y;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_LINK_HIT;
}

/// `bank7_Set_Links_Recoil` (bank 7 `$E399`).
///
/// `$0D = $A7 & 3` (Link's L/R collision bits), then `$DC91` re-faces slot
/// `X` toward Link with the old `$60,x` parked on the stack and restored;
/// `Y` = facing (1/2). When `Y != $0D`, Link's `$70` takes `LE36C[Y]`
/// (`$04`/`$FC`), or `LE36C[Y + 2]` (`$0D`/`$F3`) when `$00 != 0` (the
/// Link-hit caller sets `$00 = 1`; `code37` clears it). Exit: `A` = the
/// stored velocity (the restored facing on the suppressed path), `Y` as
/// selected, flags from the last load/compare.
///
/// Cycles: base 43 (suppressed path; `$DC91` charged separately); `+12`
/// for the `$00 = 0` select, `+15` with the `INY INY`.
pub fn pl_recoil(game: &mut Game) {
    let x = u16::from(game.cpu.x);
    let mut p = game.cpu.p;
    let mut cy: u64 = 0;
    // $E399 LDA $A7 : AND #$03 : STA $0D.
    let rel = r(&game.ram, 0x00A7) & 0x03;
    set_nz(&mut p, rel);
    w(&mut game.ram, 0x000D, rel);
    cy += 3 + 2 + 3;
    // LDA $60,x : PHA : JSR $DC91 : PLA : STA $60,x.
    let face = r(&game.ram, 0x0060 + x);
    set_nz(&mut p, face);
    game.cpu.a = face;
    game.cpu.p = p;
    pha(game, face);
    inner_jsr_frame(game, 0xE3A2, facing_dc91);
    let old = pla(game);
    cy += 4 + 3 + 6 + 4 + 4;
    p = game.cpu.p;
    set_nz(&mut p, old);
    w(&mut game.ram, 0x0060 + x, old);
    // INY : CPY $0D : BEQ LE3B8.
    let mut y = game.cpu.y.wrapping_add(1);
    set_nz(&mut p, y);
    cmp_val(&mut p, y, rel);
    cy += 2 + 3;
    if y == rel {
        cy += 3 + 6;
        game.cpu.a = old;
        game.cpu.y = y;
        game.cpu.p = p;
        game.cpu.cycles += cy - BASE_RECOIL;
        return;
    }
    // LDA $00 : BEQ LE3B3 : INY : INY.
    let z = r(&game.ram, 0x0000);
    set_nz(&mut p, z);
    cy += 2 + 3;
    if z == 0 {
        cy += 3;
    } else {
        y = y.wrapping_add(2);
        set_nz(&mut p, y);
        cy += 2 + 2 + 2;
    }
    // LE3B3 LDA LE36C,y : STA $70 : RTS.
    let hs = abs_y(game, 0xE36C, y);
    set_nz(&mut p, hs);
    w(&mut game.ram, 0x0070, hs);
    cy += 4 + 3 + 6;
    game.cpu.a = hs;
    game.cpu.y = y;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_RECOIL;
}

/// `bank7_code37` (bank 7 `$E371`): enemy-side recoil after a sword or
/// shield contact.
///
/// `$00 = 0`; when the blade register `$0B` is 0 (sword, not a
/// projectile) the enemy gets `$0502 = $18` and Link recoils
/// (`bank7_Set_Links_Recoil`). Then `$DC91` re-faces slot `X` (old `$60,x`
/// parked/restored) and `LE369[Y + 2]` (`$F4`/`$0C`) becomes the enemy's
/// knockback `$043E,x`, doubled (`ASL`) while its hit state `$040E,x`
/// runs. Exit: `A` = the stored knockback, `Y` = the hit state, `N`/`Z`
/// from the `LDY` (or the `ASL`).
///
/// Cycles: base 58 (`$0B != 0`, no hit state; `$DC91` charged
/// separately); `+11` plus the recoil call when `$0B = 0`, `+1` for the
/// `ASL`.
pub fn pl_code37(game: &mut Game) {
    let mut p = game.cpu.p;
    let mut cy: u64 = 0;
    // $E371 LDA #$00 : STA $00 ; LDA $0B : BNE LE381.
    set_nz(&mut p, 0x00);
    w(&mut game.ram, 0x0000, 0x00);
    let blade = r(&game.ram, 0x000B);
    set_nz(&mut p, blade);
    game.cpu.a = blade;
    cy += 2 + 3 + 3;
    if blade == 0 {
        // LDA #$18 : STA $0502 ; JSR bank7_Set_Links_Recoil.
        set_nz(&mut p, 0x18);
        w(&mut game.ram, 0x0502, 0x18);
        game.cpu.a = 0x18;
        game.cpu.p = p;
        inner_jsr_frame(game, 0xE37E, pl_recoil);
        p = game.cpu.p;
        cy += 2 + 2 + 4 + 6 + BASE_RECOIL;
    } else {
        cy += 3;
    }
    // LE381 LDA $60,x : PHA : JSR $DC91 : PLA : STA $60,x : INY : INY.
    let x = u16::from(game.cpu.x);
    let face = r(&game.ram, 0x0060 + x);
    set_nz(&mut p, face);
    game.cpu.a = face;
    game.cpu.p = p;
    pha(game, face);
    inner_jsr_frame(game, 0xE384, facing_dc91);
    let old = pla(game);
    p = game.cpu.p;
    set_nz(&mut p, old);
    w(&mut game.ram, 0x0060 + x, old);
    let y = game.cpu.y.wrapping_add(2);
    set_nz(&mut p, y);
    cy += 4 + 3 + 6 + 4 + 4 + 2 + 2;
    // LE38C LDA LE369,y : LDY $040E,x : BEQ LE395 : ASL ; LE395 STA
    // $043E,x : RTS.
    let mut kb = abs_y(game, 0xE369, y);
    set_nz(&mut p, kb);
    let hit = abs_y(game, 0x040E, x as u8);
    set_nz(&mut p, hit);
    cy += 4 + 4;
    if hit == 0 {
        cy += 3;
    } else {
        set_c(&mut p, kb & 0x80 != 0);
        kb <<= 1;
        set_nz(&mut p, kb);
        cy += 2 + 2;
    }
    w(&mut game.ram, 0x043E + x, kb);
    cy += 5 + 6;
    game.cpu.a = kb;
    game.cpu.y = hit;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_CODE37;
}

/// `bank7_code39` (bank 7 `$E558`): enemy weapon vs Link's shield/body.
///
/// With the reflect spell off and an enemy code `< $17`, the weapon box
/// (`$E5F3`) is tested against Link's body box (`code45`); a hit means the
/// shield took it: `$EC = 2`, `$0B = 0`, tail `JMP bank7_code37`. Otherwise
/// (or for codes `>= $17`) the weapon box is tested against the shield box
/// (`code43`); a hit runs `bank7_Link_Hit_Routine`, and when it landed
/// (`SEC`) on a strong enemy (`$0444,x >= 2`) Link's fall speed doubles
/// (`ASL $057D`) and `$70` takes `$E556[facing]` (`$18`/`$E8`) after a
/// `$DC91` re-face. `X` is whatever the last overlap reloaded from `$10`,
/// as in the ROM.
///
/// Cycles: base 140 (code `>= $17`, shield miss; `$E5F3`/`$DC91` charged
/// separately); the other paths ledger their instructions plus the callee
/// bases.
pub fn pl_code39(game: &mut Game) {
    let mut p = game.cpu.p;
    let mut cy: u64 = 0;
    // $E558 LDA $0710 : BNE LE563 ; LDA $A1,x : CMP #$17 : BCS LE579.
    let reflect = r(&game.ram, 0x0710);
    set_nz(&mut p, reflect);
    game.cpu.a = reflect;
    cy += 4;
    let body_test = if reflect != 0 {
        cy += 3;
        true
    } else {
        let x = u16::from(game.cpu.x);
        let code = r(&game.ram, 0x00A1 + x);
        set_nz(&mut p, code);
        cmp_val(&mut p, code, 0x17);
        game.cpu.a = code;
        cy += 2 + 4 + 2;
        if code >= 0x17 {
            cy += 3;
            false
        } else {
            cy += 2;
            true
        }
    };
    game.cpu.p = p;
    if body_test {
        // LE563 JSR $E5F3 : JSR code45 : JSR idem : BCC LE579.
        inner_jsr_frame(game, 0xE563, check_shield_e5f3);
        inner_jsr_frame(game, 0xE566, pl_body_box);
        inner_jsr_frame(game, 0xE569, pl_overlap);
        cy += 6 + 6 + BASE_BODY_BOX + 6 + BASE_OVERLAP;
        if game.cpu.p & FLAG_C != 0 {
            // LDA #$02 : STA $EC ; LDA #$00 : STA $0B ; JMP bank7_code37.
            p = game.cpu.p;
            w(&mut game.ram, 0x00EC, 0x02);
            set_nz(&mut p, 0x00);
            w(&mut game.ram, 0x000B, 0x00);
            game.cpu.a = 0x00;
            game.cpu.p = p;
            cy += 2 + 2 + 3 + 2 + 3 + 3;
            game.cpu.cycles += (cy + BASE_CODE37) - BASE_CODE39;
            pl_code37(game);
            return;
        }
        cy += 3;
    }
    // LE579 JSR $E5F3 : JSR code43 : JSR idem : BCC LE5F2 (RTS).
    inner_jsr_frame(game, 0xE579, check_shield_e5f3);
    inner_jsr_frame(game, 0xE57C, pl_shield_box);
    inner_jsr_frame(game, 0xE57F, pl_overlap);
    cy += 6 + 6 + BASE_SHIELD_BOX + 6 + BASE_OVERLAP;
    if game.cpu.p & FLAG_C == 0 {
        cy += 3 + 6;
        game.cpu.cycles += cy - BASE_CODE39;
        return;
    }
    // JSR bank7_Link_Hit_Routine : BCC LE59B (RTS).
    inner_jsr_frame(game, 0xE584, pl_link_hit);
    cy += 2 + 6 + BASE_LINK_HIT;
    if game.cpu.p & FLAG_C == 0 {
        cy += 3 + 6;
        game.cpu.cycles += cy - BASE_CODE39;
        return;
    }
    // LDA $0444,x : CMP #$02 : BCC LE59B (RTS).
    let x = u16::from(game.cpu.x);
    p = game.cpu.p;
    let vul = abs_y(game, 0x0444, x as u8);
    set_nz(&mut p, vul);
    cmp_val(&mut p, vul, 0x02);
    game.cpu.a = vul;
    cy += 2 + 4 + 2;
    if vul < 0x02 {
        cy += 3 + 6;
        game.cpu.p = p;
        game.cpu.cycles += cy - BASE_CODE39;
        return;
    }
    // ASL $057D ; JSR $DC91 ; LDA $E556,y : STA $70 ; RTS.
    let vs = r(&game.ram, 0x057D);
    set_c(&mut p, vs & 0x80 != 0);
    let vs2 = vs << 1;
    set_nz(&mut p, vs2);
    w(&mut game.ram, 0x057D, vs2);
    game.cpu.p = p;
    inner_jsr_frame(game, 0xE593, facing_dc91);
    p = game.cpu.p;
    let hs = abs_y(game, 0xE556, game.cpu.y);
    set_nz(&mut p, hs);
    w(&mut game.ram, 0x0070, hs);
    cy += 2 + 6 + 6 + 4 + 3 + 6;
    game.cpu.a = hs;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_CODE39;
}

/// `LE6A6` miss exit of the sword hit: `$A8,x &= $DF`, `CLC`, `RTS`.
fn sword_miss_exit(game: &mut Game, mut p: u8, mut cy: u64) {
    let x = u16::from(game.cpu.x);
    let st = r(&game.ram, 0x00A8 + x) & 0xDF;
    set_nz(&mut p, st);
    w(&mut game.ram, 0x00A8 + x, st);
    set_c(&mut p, false);
    cy += 4 + 2 + 4 + 2 + 6;
    game.cpu.a = st;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_SWORD_HIT;
}

/// `LE6AC` exit of the sword hit: `CLC`, `RTS` (nothing hit).
fn sword_clc_exit(game: &mut Game, mut p: u8, mut cy: u64) {
    set_c(&mut p, false);
    cy += 2 + 6;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_SWORD_HIT;
}

/// `LE654` deflection: `$EC = 2`, `$0B = 0`, the caller's frame is dropped
/// (`PLA PLA`, so the exit returns to the caller's caller), then a down
/// stab bounces Link (`$057D = $FE`, `RTS`) and anything else tails into
/// `bank7_code37`.
fn sword_deflect_le654(game: &mut Game, mut p: u8, mut cy: u64) {
    w(&mut game.ram, 0x00EC, 0x02);
    set_nz(&mut p, 0x00);
    w(&mut game.ram, 0x000B, 0x00);
    let _ = pop_word(game);
    let anim = r(&game.ram, 0x0080);
    set_nz(&mut p, anim);
    cmp_val(&mut p, anim, 0x09);
    game.cpu.a = anim;
    cy += 2 + 3 + 2 + 3 + 4 + 4 + 3 + 2;
    if anim == pl::ANIM_DOWN_STAB {
        set_nz(&mut p, 0xFE);
        w(&mut game.ram, 0x057D, 0xFE);
        game.cpu.a = 0xFE;
        cy += 2 + 2 + 4 + 6;
        game.cpu.p = p;
        game.cpu.cycles += cy - BASE_SWORD_HIT;
        return;
    }
    cy += 3 + 3;
    game.cpu.p = p;
    game.cpu.cycles += (cy + BASE_CODE37) - BASE_SWORD_HIT;
    pl_code37(game);
}

/// `LE755` item/jar tail of the sword hit: `$AF,x & $7F` in `8..$0E` or
/// `$10`/`$11` re-targets the return at `bank7_get_item` (`$E771`, stays
/// with the interpreter); otherwise `LE769` clears `$040E,x`/`$ED`.
fn sword_item_le755(game: &mut Game, mut p: u8, mut cy: u64) {
    let x = u16::from(game.cpu.x);
    let v = r(&game.ram, 0x00AF + x) & 0x7F;
    set_nz(&mut p, v);
    cmp_val(&mut p, v, 0x08);
    game.cpu.a = v;
    cy += 4 + 2 + 2;
    let to_item = if v < 0x08 {
        cy += 3;
        false
    } else {
        cmp_val(&mut p, v, 0x0E);
        cy += 2 + 2;
        if v < 0x0E {
            cy += 3;
            true
        } else {
            cmp_val(&mut p, v, 0x10);
            cy += 2 + 2;
            if v == 0x10 {
                cy += 3;
                true
            } else {
                cmp_val(&mut p, v, 0x11);
                cy += 2 + 2;
                if v == 0x11 {
                    cy += 3;
                    true
                } else {
                    cy += 2;
                    false
                }
            }
        }
    };
    if to_item {
        game.cpu.p = p;
        game.trap_jump(0xE771);
        game.cpu.cycles += cy - BASE_SWORD_HIT;
        return;
    }
    // LE769 LDA #$00 : STA $040E,x : STA $ED : RTS.
    set_nz(&mut p, 0x00);
    w(&mut game.ram, 0x040E + x, 0x00);
    w(&mut game.ram, 0x00ED, 0x00);
    cy += 2 + 5 + 3 + 6;
    game.cpu.a = 0x00;
    game.cpu.p = p;
    game.cpu.cycles += cy - BASE_SWORD_HIT;
}

/// `bank7_Sword_Hit_Detection_maybe…` (bank 7 `$E677`): Link's sword vs
/// enemy slot `X`. Entry `$E677` only — the `LE694`/`LE6A1`/`LE726`/`LE6E8`
/// mid-entries other callers `JSR` to stay with the interpreter, so the
/// `$0B != 0` projectile paths only they reach are not ported (`$0B` is
/// cleared at `$E690` on this entry).
///
/// Gates: sword out (`$0480 != $F8`), enemy not sword-immune (`$6E41[code]
/// & $10`), slot live (`$B6,x = 1`); then the sword box (`code44`), a
/// `CLC` exit for elevators/locked doors, the enemy box (`LE942`) and the
/// overlap. A miss clears `$A8,x & $DF`, `CLC`. A hit on a fire-immune
/// enemy (`$6DD5[code] & $20`) deflects ([`sword_deflect_le654`]); a red
/// jar already in hit state exits `CLC`. Otherwise an up stab from below
/// hovers Link (`$057D = 0`; from above it is a miss), `$A8,x |= $20` (an
/// enemy already flagged exits with the flags as the ROM leaves them), a
/// down stab bounces (`$057D = $FE`), `$040E,x = $30`, `$ED = $10`, HP
/// `$C2,x -= LE66C[$0777]`. Survivors: `SEC` (jars go to
/// [`sword_item_le755`], down stabs skip the `code37` recoil). Dead: jars
/// to [`sword_item_le755`], everything else re-targets the return at
/// `bank7_monster_death` (`$E880`, stays with the interpreter).
///
/// Cycles: base 27 (sword retracted); the other paths ledger their
/// instructions plus the callee bases (`code44`, overlap, `code37`).
pub fn pl_sword_hit(game: &mut Game) {
    let mut p = game.cpu.p;
    let mut cy: u64 = 0;
    let mut x = u16::from(game.cpu.x);
    // $E677 LDA $0480 : CMP #$F8 : BEQ LE6A6.
    let sy = r(&game.ram, 0x0480);
    set_nz(&mut p, sy);
    cmp_val(&mut p, sy, pl::SWORD_RETRACTED);
    game.cpu.a = sy;
    cy += 4 + 2;
    if sy == pl::SWORD_RETRACTED {
        cy += 3;
        return sword_miss_exit(game, p, cy);
    }
    // LDY $A1,x ; LDA $6E41,y : AND #$10 : BNE LE6A6.
    cy += 2;
    let code = r(&game.ram, 0x00A1 + x);
    set_nz(&mut p, code);
    game.cpu.y = code;
    let imm = abs_y(game, 0x6E41, code) & 0x10;
    set_nz(&mut p, imm);
    game.cpu.a = imm;
    cy += 4 + 4 + 2;
    if imm != 0 {
        cy += 3;
        return sword_miss_exit(game, p, cy);
    }
    // LDA $B6,x : CMP #$01 : BNE LE6A6.
    cy += 2;
    let live = r(&game.ram, 0x00B6 + x);
    set_nz(&mut p, live);
    cmp_val(&mut p, live, 0x01);
    game.cpu.a = live;
    cy += 4 + 2;
    if live != 0x01 {
        cy += 3;
        return sword_miss_exit(game, p, cy);
    }
    cy += 2;
    // JSR code44 ; LDA #$00 : STA $0B.
    game.cpu.p = p;
    inner_jsr_frame(game, 0xE68D, pl_sword_box);
    p = game.cpu.p;
    set_nz(&mut p, 0x00);
    w(&mut game.ram, 0x000B, 0x00);
    game.cpu.a = 0x00;
    cy += 6 + BASE_SWORD_BOX + 2 + 3;
    // LE694 LDY $A1,x : CPY #$13 : BEQ LE6AC : CPY #$02 : BEQ LE6AC.
    let code = r(&game.ram, 0x00A1 + x);
    set_nz(&mut p, code);
    game.cpu.y = code;
    cmp_val(&mut p, code, 0x13);
    cy += 4 + 2;
    if code == 0x13 {
        cy += 3;
        return sword_clc_exit(game, p, cy);
    }
    cmp_val(&mut p, code, 0x02);
    cy += 2 + 2;
    if code == 0x02 {
        cy += 3;
        return sword_clc_exit(game, p, cy);
    }
    cy += 2;
    // JSR LE942 ; LE6A1 JSR idem : BCS LE6AE.
    game.cpu.p = p;
    inner_jsr_frame(game, 0xE69E, enemy_box_e942);
    inner_jsr_frame(game, 0xE6A1, pl_overlap);
    p = game.cpu.p;
    x = u16::from(game.cpu.x);
    cy += 6 + 6 + BASE_OVERLAP;
    if p & FLAG_C == 0 {
        cy += 2;
        return sword_miss_exit(game, p, cy);
    }
    cy += 3;
    // LE6AE LDY $A1,x ; LDA $0B : BNE LE6BE ; LDA $6DD5,y : AND #$20 : BEQ
    // LE6BE ; JMP LE654.
    let code = r(&game.ram, 0x00A1 + x);
    set_nz(&mut p, code);
    game.cpu.y = code;
    set_nz(&mut p, 0x00);
    cy += 4 + 3 + 2;
    let fire = abs_y(game, 0x6DD5, code) & 0x20;
    set_nz(&mut p, fire);
    game.cpu.a = fire;
    cy += 4 + 2;
    if fire != 0 {
        cy += 2 + 3;
        return sword_deflect_le654(game, p, cy);
    }
    cy += 3;
    // LE6BE LDA $0B : BNE ; CPY #$01 : BNE LE6CB ; LDA $040E,x : BNE LE6AC.
    set_nz(&mut p, 0x00);
    game.cpu.a = 0x00;
    cmp_val(&mut p, code, 0x01);
    cy += 3 + 2 + 2;
    if code == 0x01 {
        cy += 2;
        let hit = abs_y(game, 0x040E, x as u8);
        set_nz(&mut p, hit);
        game.cpu.a = hit;
        cy += 4;
        if hit != 0 {
            cy += 3;
            return sword_clc_exit(game, p, cy);
        }
        cy += 2;
    } else {
        cy += 3;
    }
    // LE6CB LDA $0B : BEQ LE6F3.
    set_nz(&mut p, 0x00);
    game.cpu.a = 0x00;
    cy += 3 + 3;
    // LE6F3 LDA $80 : CMP #$08 : BNE LE708 (crosses) ; LDA $0B : BNE ; LDA
    // $29 : CMP $2A,x : BCC LE6A6 (crosses) ; LDA #$00 : STA $057D.
    let anim = r(&game.ram, 0x0080);
    set_nz(&mut p, anim);
    cmp_val(&mut p, anim, pl::ANIM_UP_STAB);
    game.cpu.a = anim;
    cy += 3 + 2;
    if anim == pl::ANIM_UP_STAB {
        cy += 2;
        set_nz(&mut p, 0x00);
        game.cpu.a = 0x00;
        cy += 3 + 2;
        let ly = r(&game.ram, 0x0029);
        set_nz(&mut p, ly);
        cmp_val(&mut p, ly, r(&game.ram, 0x002A + x));
        game.cpu.a = ly;
        cy += 3 + 4;
        if p & FLAG_C == 0 {
            cy += 4;
            return sword_miss_exit(game, p, cy);
        }
        cy += 2;
        set_nz(&mut p, 0x00);
        w(&mut game.ram, 0x057D, 0x00);
        game.cpu.a = 0x00;
        cy += 2 + 4;
    } else {
        cy += 4;
    }
    // LE708 LDA $A8,x : AND #$20 : BNE LE6AD (crosses; RTS) ; LDA $A8,x :
    // ORA #$20 : STA $A8,x.
    let st = r(&game.ram, 0x00A8 + x);
    let already = st & 0x20;
    set_nz(&mut p, already);
    game.cpu.a = already;
    cy += 4 + 2;
    if already != 0 {
        cy += 4 + 6;
        game.cpu.p = p;
        game.cpu.cycles += cy - BASE_SWORD_HIT;
        return;
    }
    cy += 2;
    let st2 = st | 0x20;
    set_nz(&mut p, st2);
    w(&mut game.ram, 0x00A8 + x, st2);
    game.cpu.a = st2;
    cy += 4 + 2 + 4;
    // LDA $80 : CMP #$09 : BNE LE723 ; LDA $0B : BNE ; LDA #$FE : STA $057D.
    set_nz(&mut p, anim);
    cmp_val(&mut p, anim, pl::ANIM_DOWN_STAB);
    game.cpu.a = anim;
    cy += 3 + 2;
    if anim == pl::ANIM_DOWN_STAB {
        cy += 2;
        set_nz(&mut p, 0x00);
        cy += 3 + 2;
        set_nz(&mut p, 0xFE);
        w(&mut game.ram, 0x057D, 0xFE);
        game.cpu.a = 0xFE;
        cy += 2 + 4;
    } else {
        cy += 3;
    }
    // LE723 LDY $0777 ; LE726 LDA #$30 : STA $040E,x ; LDA #$10 : STA $ED ;
    // LDA $C2,x : SEC : SBC LE66C,y : STA $C2,x ; BEQ LE74C : BCC LE74C.
    let lvl = r(&game.ram, 0x0777);
    set_nz(&mut p, lvl);
    game.cpu.y = lvl;
    w(&mut game.ram, 0x040E + x, 0x30);
    w(&mut game.ram, 0x00ED, 0x10);
    let hp = r(&game.ram, 0x00C2 + x);
    set_nz(&mut p, hp);
    set_c(&mut p, true);
    let power = abs_y(game, 0xE66C, lvl);
    let hp2 = sbc_val(&mut p, hp, power);
    w(&mut game.ram, 0x00C2 + x, hp2);
    game.cpu.a = hp2;
    cy += 4 + 2 + 5 + 2 + 3 + 4 + 2 + 4 + 4;
    let dead = hp2 == 0 || p & FLAG_C == 0;
    if !dead {
        // LDA $A1,x : CMP #$01 : BEQ LE755 ; LDA $80 : CMP #$09 : BEQ LE74A ;
        // JSR code37 ; LE74A SEC : RTS.
        cy += 2 + 2;
        let code = r(&game.ram, 0x00A1 + x);
        set_nz(&mut p, code);
        cmp_val(&mut p, code, 0x01);
        game.cpu.a = code;
        cy += 4 + 2;
        if code == 0x01 {
            cy += 3;
            return sword_item_le755(game, p, cy);
        }
        cy += 2;
        set_nz(&mut p, anim);
        cmp_val(&mut p, anim, pl::ANIM_DOWN_STAB);
        game.cpu.a = anim;
        cy += 3 + 2;
        if anim == pl::ANIM_DOWN_STAB {
            cy += 3;
        } else {
            cy += 2;
            game.cpu.p = p;
            inner_jsr_frame(game, 0xE747, pl_code37);
            p = game.cpu.p;
            cy += 6 + BASE_CODE37;
        }
        set_c(&mut p, true);
        cy += 2 + 6;
        game.cpu.p = p;
        game.cpu.cycles += cy - BASE_SWORD_HIT;
        return;
    }
    // LE74C LDY $A1,x : CPY #$01 : BEQ LE755 ; JMP bank7_monster_death.
    cy += if hp2 == 0 { 3 } else { 2 + 3 };
    let code = r(&game.ram, 0x00A1 + x);
    set_nz(&mut p, code);
    game.cpu.y = code;
    cmp_val(&mut p, code, 0x01);
    cy += 4 + 2;
    if code == 0x01 {
        cy += 3;
        return sword_item_le755(game, p, cy);
    }
    cy += 2 + 3;
    game.cpu.p = p;
    game.trap_jump(0xE880);
    game.cpu.cycles += cy - BASE_SWORD_HIT;
}

// ---------------------------------------------------------------------------
// Shims: hitboxes.
// ---------------------------------------------------------------------------

/// `bank7_code43` (bank 7 `$E975`): shield box into `$00-$03` scratch.
///
/// `$00 = $CC + 9`, `$02 = $0D`, `Y = $17`, `$01 = $29 + LE971[y]`,
/// `$03 = LE973[y]` (ROM tables, read through the bus). The closing
/// `LDA $00 : ADC $02` (carry from the Y add) clips the width to the
/// screen edge on overflow (`$02 = $00 ^ $FF`). Exit: `A` = that sum (or
/// the clipped width), `Y = $17`, flags from the sum (`N`/`Z` from the
/// `EOR` on the clip path).
///
/// Cycles: 52 unclipped (`register_player_traps`); `+7` on the clip path.
pub fn pl_shield_box(game: &mut Game) {
    let mut p = game.cpu.p;
    // $E975 LDA $CC : CLC : ADC #$09 : STA $00 ; LDA #$0D : STA $02.
    p &= !FLAG_C;
    let x0 = adc_val(&mut p, r(&game.ram, 0x00CC), pl::SHIELD_DX);
    w(&mut game.ram, 0x0000, x0);
    w(&mut game.ram, 0x0002, pl::SHIELD_W);
    // $E980 LDY $17 ; LDA $29 : CLC : ADC LE971,y : STA $01 ; LDA LE973,y
    // : STA $03.
    let y = r(&game.ram, 0x0017);
    game.cpu.y = y;
    let dy = abs_y(game, 0xE971, y);
    let h = abs_y(game, 0xE973, y);
    p &= !FLAG_C;
    let y0 = adc_val(&mut p, r(&game.ram, 0x0029), dy);
    w(&mut game.ram, 0x0001, y0);
    w(&mut game.ram, 0x0003, h);
    // $E98F LDA $00 : ADC $02 : BCC LE99B ; LDA $00 : EOR #$FF : STA $02.
    let sum = adc_val(&mut p, x0, pl::SHIELD_W);
    if p & FLAG_C != 0 {
        let clip = x0 ^ 0xFF;
        set_nz(&mut p, clip);
        w(&mut game.ram, 0x0002, clip);
        game.cpu.a = clip;
        game.cpu.cycles += 7;
    } else {
        game.cpu.a = sum;
    }
    game.cpu.p = p;
}

/// `bank7_code44` (bank 7 `$E9A2`): sword box into `$00-$03` scratch.
///
/// `Y = 1` when the blade `$047E` sits left of `$CC` (`CMP $CC : BCS`),
/// `$00 = $047E + table26[y]` (`$F8`/`$02`), `$02 = $0E`; `Y = 1` again
/// for the stab frames (`$80` = 8/9), `$01 = $0480 + LE99E[y]`
/// (`$07`/`$00`), `$03 = LE9A0[y]` (`$03`/`$03`). Exit: `A = $03`
/// (`N`/`Z` from it, `C`/`V` from the `$0480` add), `Y` = stab flag; the
/// `PHA`/`PLA` leaves `$047E` at `$0100+SP`.
///
/// Cycles: 71 for the up-stab frame with the blade right of Link
/// (`register_player_traps`); `+1` blade left, `+2` non-stab frames, `+3`
/// down-stab (`CMP #$09` path).
pub fn pl_sword_box(game: &mut Game) {
    let mut p = game.cpu.p;
    // $E9A2 LDY #$00 ; LDA $047E : PHA : CMP $CC : BCS LE9AD : INY ; PLA.
    let sx = r(&game.ram, 0x047E);
    game.ram[0x0100 + usize::from(game.cpu.sp)] = sx;
    let cc = r(&game.ram, 0x00CC);
    cmp_val(&mut p, sx, cc);
    let side = u8::from(sx < cc);
    if side != 0 {
        game.cpu.cycles += 1;
    }
    set_nz(&mut p, sx);
    // CLC : ADC bank7_table26,y : STA $00 ; LDA #$0E : STA $02.
    let dx = abs_y(game, 0xE99C, side);
    p &= !FLAG_C;
    let x0 = adc_val(&mut p, sx, dx);
    w(&mut game.ram, 0x0000, x0);
    w(&mut game.ram, 0x0002, 0x0E);
    // $E9B8 LDY #$00 ; LDA $80 : CMP #$08 : BEQ : CMP #$09 : BNE : INY.
    let anim = r(&game.ram, 0x0080);
    let stab = match anim {
        pl::ANIM_UP_STAB => 1,
        pl::ANIM_DOWN_STAB => {
            game.cpu.cycles += 3;
            1
        }
        _ => {
            game.cpu.cycles += 2;
            0
        }
    };
    game.cpu.y = stab;
    // LDA $0480 : CLC : ADC LE99E,y : STA $01 ; LDA LE9A0,y : STA $03.
    let dy = abs_y(game, 0xE99E, stab);
    let h = abs_y(game, 0xE9A0, stab);
    p &= !FLAG_C;
    let y0 = adc_val(&mut p, r(&game.ram, 0x0480), dy);
    w(&mut game.ram, 0x0001, y0);
    w(&mut game.ram, 0x0003, h);
    set_nz(&mut p, h);
    game.cpu.a = h;
    game.cpu.p = p;
}

/// `bank7_code45` (bank 7 `$E9D8`): body box into `$00-$03` scratch.
///
/// `Y = $9F - 1` indexes `table27` (`$0E` right / `$FF` left; a zero
/// `$9F` reads `$EAD3` like the hardware), `$00 = $CC + 8 + table27[y]`,
/// `$02 = 5`, `Y = $17`, `$01 = $29 + LE9D6[y]` (`$11`/`$02`), `$03 =
/// $0C`. Exit: `A = $0C`, `Y = $17`, `C`/`V` from the Y add.
///
/// Cycles: 52 (`register_player_traps`); `+1` when the `table27` read
/// crosses the page (`$9F = 0`).
pub fn pl_body_box(game: &mut Game) {
    let mut p = game.cpu.p;
    // $E9D8 LDY $9F : DEY ; LDA $CC : CLC : ADC #$08 : CLC : ADC
    // bank7_table27,y : STA $00 ; LDA #$05 : STA $02.
    let fy = r(&game.ram, 0x009F).wrapping_sub(1);
    let dx = abs_y(game, 0xE9D4, fy);
    p &= !FLAG_C;
    let a = adc_val(&mut p, r(&game.ram, 0x00CC), 0x08);
    p &= !FLAG_C;
    let x0 = adc_val(&mut p, a, dx);
    w(&mut game.ram, 0x0000, x0);
    w(&mut game.ram, 0x0002, pl::BODY_W);
    // LDY $17 ; LDA $29 : CLC : ADC LE9D6,y : STA $01 ; LDA #$0C : STA $03.
    let sy = r(&game.ram, 0x0017);
    game.cpu.y = sy;
    let dy = abs_y(game, 0xE9D6, sy);
    p &= !FLAG_C;
    let y0 = adc_val(&mut p, r(&game.ram, 0x0029), dy);
    w(&mut game.ram, 0x0001, y0);
    w(&mut game.ram, 0x0003, pl::BODY_H);
    set_nz(&mut p, pl::BODY_H);
    game.cpu.a = pl::BODY_H;
    game.cpu.p = p;
}

/// `bank7_idem__maybe` (bank 7 `$E9F9`): overlap of the `$00-$03` box
/// against `$04-$07`, Y axis first (`X = 1`), then X (`X = 0`).
///
/// Per axis: `$0F = $04,x - $00,x + $06,x`; the axis fails when
/// `$02,x + $06,x < $0F` (8-bit wrap, `CMP : BCC`). Result is the carry
/// (`C = 1` overlap, `C = 0` miss — callers branch on it); `X` is
/// reloaded from `$10`, `A` holds the last size sum, `$0F` the last
/// distance.
///
/// Cycles: 46 for a Y-axis miss (`register_player_traps`); `+40` for an
/// X-axis miss, `+43` for an overlap (the `BPL LE9FB` back branch crosses
/// from `$EA11` into `$E9xx`, so it costs 4 when taken).
pub fn pl_overlap(game: &mut Game) {
    let mut p = game.cpu.p;
    let mut x: u8 = 0x01;
    set_nz(&mut p, x);
    let a = loop {
        let xi = u16::from(x);
        // LE9FB LDA $04,x : SEC : SBC $00,x : CLC : ADC $06,x : STA $0F.
        p |= FLAG_C;
        let d = sbc_val(&mut p, r(&game.ram, 0x0004 + xi), r(&game.ram, xi));
        p &= !FLAG_C;
        let lim = adc_val(&mut p, d, r(&game.ram, 0x0006 + xi));
        w(&mut game.ram, 0x000F, lim);
        // LDA $02,x : CLC : ADC $06,x : CMP $0F : BCC LEA11.
        p &= !FLAG_C;
        let sum = adc_val(&mut p, r(&game.ram, 0x0002 + xi), r(&game.ram, 0x0006 + xi));
        cmp_val(&mut p, sum, lim);
        if p & FLAG_C == 0 {
            if x == 0 {
                game.cpu.cycles += 40;
            }
            break sum;
        }
        // DEX : BPL LE9FB (taken: 4, page-crossing).
        x = x.wrapping_sub(1);
        set_nz(&mut p, x);
        if x & 0x80 != 0 {
            game.cpu.cycles += 43;
            break sum;
        }
    };
    // LEA11 LDX $10 : RTS.
    let s = r(&game.ram, 0x0010);
    game.cpu.x = s;
    set_nz(&mut p, s);
    game.cpu.a = a;
    game.cpu.p = p;
}

/// `bank7_code47` (bank 7 `$EBB8`): fairy sprite anchor.
///
/// Swamp passages (`$C8 & 6 != 0`) return at once. Otherwise `Y = ($12 &
/// $38) >> 3` indexes the float table; `$0208 = $29 + 8 + float` and
/// `$020B = $CC + $0C` chain their carries (no `CLC`), the tile is
/// `$0209 = $68 + (($12 & 4) >> 1)`, and `$020A` = 1 or `($050C >> 1) & 3`
/// while the hurt timer runs. Exit: `A`/`Y` as the last `LDY`/`LDA`/`TAY` left
/// them, flags likewise. Only reached by the `BNE` at `$EC20` in the ROM
/// (branches do not dispatch traps), so this stays inert on the movie.
///
/// Cycles: 14 on the swamp exit (`register_player_traps`); `+60` for the
/// anchor path, `+5` more while `$050C != 0`.
pub fn pl_fairy_step(game: &mut Game) {
    let mut p = game.cpu.p;
    // $EBB8 LDA $C8 : AND #$06 : BNE LEBEF.
    let c8 = r(&game.ram, 0x00C8) & 0x06;
    set_nz(&mut p, c8);
    game.cpu.a = c8;
    if c8 != 0 {
        game.cpu.p = p;
        return;
    }
    game.cpu.cycles += 60;
    // LDA $12 : AND #$38 : LSR : LSR : LSR : TAY (bits 0-2 are clear, so
    // the carry out of the shifts is 0).
    let f = r(&game.ram, 0x0012);
    let fy = (f & 0x38) >> 3;
    game.cpu.y = fy;
    set_c(&mut p, false);
    // LDA $29 : ADC #$08 : ADC table,y : STA $0208.
    let a = adc_val(&mut p, r(&game.ram, 0x0029), 0x08);
    let float = abs_y(game, 0xEBB0, fy);
    let ay = adc_val(&mut p, a, float);
    w(&mut game.ram, 0x0208, ay);
    // LDA $CC : ADC #$0C : STA $020B.
    let ax = adc_val(&mut p, r(&game.ram, 0x00CC), 0x0C);
    w(&mut game.ram, 0x020B, ax);
    // LDA $12 : AND #$04 : LSR : ADC #$68 : STA $0209 (LSR carry is 0).
    set_c(&mut p, false);
    let tile = adc_val(&mut p, (f & 0x04) >> 1, 0x68);
    w(&mut game.ram, 0x0209, tile);
    // LDY #$01 ; LDA $050C : BEQ LEBEC ; LSR : AND #$03 : TAY ; STY $020A.
    let t = r(&game.ram, 0x050C);
    set_nz(&mut p, t);
    let mut fl = 0x01;
    let mut out = t;
    if t != 0 {
        set_c(&mut p, t & 0x01 != 0);
        out = (t >> 1) & 0x03;
        set_nz(&mut p, out);
        fl = out;
        game.cpu.cycles += 5;
    }
    w(&mut game.ram, 0x020A, fl);
    game.cpu.y = fl;
    game.cpu.a = out;
    game.cpu.p = p;
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every fixed-bank player trap on `game`. Idempotent.
pub fn register_player_traps(game: &mut Game) {
    // Cycle costs are the cheapest-path bases counted from `prg7.asm`
    // under the `cpu.rs` model (`BASE_*` above); every other path is
    // ledgered inside the shim, page-crossing table reads included.
    game.trap_register_cycles(
        "bank7_applyGravityMotion",
        Some(7),
        0xD19B,
        pl_gravity,
        BASE_GRAVITY,
    );
    game.trap_register_cycles(
        "bank7_XY_Movements_Routine",
        Some(7),
        0xD1CE,
        pl_xy_move,
        BASE_XY_MOVE,
    );
    game.trap_register_cycles(
        "bank7_check_if_link_died_0494__linkdeath",
        Some(7),
        0xD3CC,
        pl_death_check,
        BASE_DEATH_CHECK,
    );
    game.trap_register_cycles(
        "bank7_Link_Hit_Routine",
        Some(7),
        0xE2EF,
        pl_link_hit,
        BASE_LINK_HIT,
    );
    game.trap_register_cycles(
        "bank7_Set_Links_Recoil",
        Some(7),
        0xE399,
        pl_recoil,
        BASE_RECOIL,
    );
    game.trap_register_cycles("bank7_code37", Some(7), 0xE371, pl_code37, BASE_CODE37);
    game.trap_register_cycles("bank7_code39", Some(7), 0xE558, pl_code39, BASE_CODE39);
    game.trap_register_cycles(
        "bank7_Sword_Hit_Detection_maybe__probably_part_of_it_at_least",
        Some(7),
        0xE677,
        pl_sword_hit,
        BASE_SWORD_HIT,
    );
    game.trap_register_cycles(
        "bank7_code43",
        Some(7),
        0xE975,
        pl_shield_box,
        BASE_SHIELD_BOX,
    );
    game.trap_register_cycles(
        "bank7_code44",
        Some(7),
        0xE9A2,
        pl_sword_box,
        BASE_SWORD_BOX,
    );
    game.trap_register_cycles("bank7_code45", Some(7), 0xE9D8, pl_body_box, BASE_BODY_BOX);
    game.trap_register_cycles(
        "bank7_idem__maybe",
        Some(7),
        0xE9F9,
        pl_overlap,
        BASE_OVERLAP,
    );
    game.trap_register_cycles("bank7_code47", Some(7), 0xEBB8, pl_fairy_step, BASE_FAIRY);
}
