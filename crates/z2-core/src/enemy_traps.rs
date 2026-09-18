//! Enemy trap ports + registration (lockstep rework).
//!
//! Every registered `fn(&mut Game)` below replicates the *observable*
//! effects of one fixed-bank enemy routine: memory writes, the
//! `A`/`X`/`Y`/`SP`/`P` state its `RTS` (or tail `JMP`) leaves behind, and
//! the 6502 cycles it consumed (charged in-port, see
//! [Cycle charging](#cycle-charging)). The pure `enemy` / `enemy_ai` /
//! `enemy_boss` modules stay the ROM-free models; the ports here are the
//! bit-exact `Game`-facing layer the interpreter substitutes at `JSR`/`JMP`.
//!
//! # Control-flow conventions
//!
//! * **Tail jumps** (`JMP target` / `JMP ($0E)` ending a routine):
//!   [`Game::trap_jump`] asks the dispatcher to continue at `target` once
//!   the port returns — firing `target`'s own trap when it has one, so the
//!   trap table keeps replacing routines on every `JSR`/`JMP`-shaped
//!   transfer, and interpreting from there otherwise; nothing is pushed.
//! * **Non-local exits** (`bank7_Enemy_Stops_when_Hit`: `PLA : PLA : JMP`):
//!   the port pops the `JSR` frame the interpreter pushed for it, exactly
//!   like the original, then tail-jumps.
//! * **Hand-offs**: a family routine whose body calls an unported routine
//!   mid-way (display `LDE3D`/`LDE40`, `bank7_Link_Hit_Routine`, `LD848`)
//!   ports everything up to that call, pushes the continuation return the
//!   ASM `JSR` would have pushed, and tail-jumps into the callee; the
//!   interpreter resumes the ASM body after the callee's `RTS`. Net state is
//!   identical to the original at every instruction boundary.
//! * **Inner `JSR`s** to routines ported in this file run the Rust body
//!   inside a replicated stack frame ([`inner_jsr_frame`]) so dead stack bytes
//!   match the interpreter.
//!
//! # Cycle charging
//!
//! Every port counts the instructions it replaces under the `cpu.rs` cost
//! model (page-cross extras on `abs,y`/`abs,x`/`(zp),y` reads and on taken
//! branches included) and adds the total to `cpu.cycles` itself; the traps
//! register with a static cost of 0 (`trap_register_cycles(.., 0)`) because
//! all of them are variable-path. Tail `JMP`s count 3 (`abs`) / 5 (`ind`)
//! with no `RTS`; plain returns count the `RTS` (6).
//!
//! | port | label | addr |
//! |---|---|---|
//! | [`en_every_frame`] | `bank7_enemy_every_frame_routine` | `$D6CA` |
//! | [`en_link_collision`] | `bank7_Link_Collision_Detection` | `$D6C1` |
//! | [`en_stun_gate`] | `bank7_Enemy_Stops_when_Hit` | `$DA02` |
//! | [`en_facing`] | `bank7_Determine_Enemy_Facing_Direction_relative_to_Link` | `$DC91` |
//! | [`en_flip`] | `bank7_Change_Enemy_Facing_Direction_and_X_Velocity` | `$E8EB` |
//! | [`en_remove`] | `bank7_remove_enemy_or_item` | `$DD47` |
//! | [`en_kill_all`] | `bank7_KillAllMonsters` | `$E18F` |
//! | [`en_death_exp`] | `bank7_monster_death_give_exp` | `$DDEC` |
//! | [`en_death`] | `bank7_monster_death` | `$E880` |
//! | [`en_spawn_proj`] | `bank7_Spawn_New_Projectile` | `$DBCE` |
//! | [`en_spawn_bubble`] | `bank7_spawn_new_bubble_or_rock` | `$DBFB` |
//! | [`en_proj_disintegrate`] | `LE6E8` shield-deflect write | `$E6E8` |
//! | [`en_walker`] | `bank7_Enemy_Routines1_Bot` (→ `Myu` hand-off) | `$DA0C` |
//! | [`en_flyer`] | `bank7_Enemy_Routines1_Deeler` | `$D6DF` |
//! | [`en_generator`] | `bank7_Enemy_Routines1_Raising_Bubbles` | `$DC15` |
//! | [`en_shooter`] | `bank7_Enemy_Routines1_Octorok` (→ `LD848` hand-off) | `$D888` |
//! | [`en_elevator`] | `bank7_Enemy_Routines1_Elevator` | `$D8C2` |
//! | [`en_locked_door`] | `bank7_Enemy_Routines1_Locked_Door` (→ `LDE40` hand-off) | `$D991` |
//! | [`en_simple_h`] | `bank7_Simple_Horizontal_Movement` | `$DEB8` |
//! | [`en_simple_v`] | `bank7_Simple_Vertical_Movement` | `$DEC8` |
//! | [`en_gravity`] | `bank7_Gravity` | `$DEBE` |
//! | [`en_sword_hit`] | `bank7_Sword_Hit_Detection…` damage half (unregistered) | `$E677` |
//! | [`en_shield_gate`] | `bank7_code39` (unregistered) | `$E558` |
//! | [`en_horsehead`] … [`en_dark_link`] | banked boss halves (unregistered) | — |
//!
//! Banked code (banks 1/2/4/5 `$8000-$BFFF`) is listed in
//! [`ENEMY_TRAPS_BANKED`] with `bank = None` and intentionally *not*
//! registered: 16-bit trap keys alias across `$8000-$BFFF` (see `traps.rs`
//! M1 caveat). The boss shims below exercise the banked pure halves on
//! staged regs for unit coverage only. Display emission (`bank7_Display`,
//! `$EF11`) is PPU scope and stays unregistered; see
//! [`ENEMY_TRAPS_UNREGISTERED`].

use crate::bank7_common::{
    adc_val, cmp_val, inner_jsr_frame, pop_word, push_word, sbc_val, set_nz,
};
use crate::cpu::{bus_read, bus_write, FLAG_C, FLAG_N};
use crate::enemy as en;
use crate::enemy_boss as boss;
use crate::game::Game;

// ---------------------------------------------------------------------------
// Trap tables.
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Fixed-bank enemy traps actually registered by [`register_enemy_traps`].
pub const ENEMY_TRAPS: &[TrapEntry] = &[
    ("bank7_enemy_every_frame_routine", Some(7), 0xD6CA),
    ("bank7_Link_Collision_Detection", Some(7), 0xD6C1),
    ("bank7_Enemy_Stops_when_Hit", Some(7), 0xDA02),
    (
        "bank7_Determine_Enemy_Facing_Direction_relative_to_Link",
        Some(7),
        0xDC91,
    ),
    (
        "bank7_Change_Enemy_Facing_Direction_and_X_Velocity",
        Some(7),
        0xE8EB,
    ),
    ("bank7_remove_enemy_or_item", Some(7), 0xDD47),
    ("bank7_KillAllMonsters", Some(7), 0xE18F),
    ("bank7_monster_death_give_exp", Some(7), 0xDDEC),
    ("bank7_monster_death", Some(7), 0xE880),
    ("bank7_Spawn_New_Projectile", Some(7), 0xDBCE),
    ("bank7_spawn_new_bubble_or_rock", Some(7), 0xDBFB),
    ("LE6E8_projectile_disintegrate", Some(7), 0xE6E8),
    (
        "bank7_Sword_Hit_Detection_maybe__probably_part_of_it_at_least",
        Some(7),
        0xE677,
    ),
    ("bank7_code39", Some(7), 0xE558),
    ("bank7_Enemy_Routines1_Bot", Some(7), 0xDA0C),
    ("bank7_Enemy_Routines1_Deeler", Some(7), 0xD6DF),
    ("bank7_Enemy_Routines1_Raising_Bubbles", Some(7), 0xDC15),
    ("bank7_Enemy_Routines1_Octorok", Some(7), 0xD888),
    ("bank7_Enemy_Routines1_Elevator", Some(7), 0xD8C2),
    ("bank7_Enemy_Routines1_Locked_Door", Some(7), 0xD991),
    ("bank7_Simple_Horizontal_Movement", Some(7), 0xDEB8),
    ("bank7_Simple_Vertical_Movement", Some(7), 0xDEC8),
    ("bank7_Gravity", Some(7), 0xDEBE),
];

/// Number of fixed-bank enemy traps actually registered.
///
/// [`ENEMY_TRAPS`] lists 23 fixed-bank entries; the two shared with
/// `player_traps` (`$E677` sword-hit, `$E558` shield-gate — same addresses,
/// player-owned shims) are listed but skipped here so combined runs keep
/// last-writer-wins out of the picture. Fresh-`Game` count is 21.
pub const ENEMY_TRAP_COUNT: usize = 21;

/// Fixed-bank entries shared with `player_traps` (listed, not registered
/// here): the sword-hit gate and shield router are player-owned; the enemy
/// halves (`en_sword_hit` / `en_shield_gate` above) run for unit coverage
/// and A/B diffs against the same addresses.
pub const ENEMY_TRAPS_SHARED: &[TrapEntry] = &[
    (
        "bank7_Sword_Hit_Detection_maybe__probably_part_of_it_at_least",
        Some(7),
        0xE677,
    ),
    ("bank7_code39", Some(7), 0xE558),
];

/// Banked region/boss routines: listed, never registered (aliasing).
pub const ENEMY_TRAPS_BANKED: &[TrapEntry] = &[
    ("bank1_Enemy_Routines1_Megmat", None, 0x987E),
    ("bank1_Enemy_Routines1_Goriya", None, 0x9972),
    ("bank1_Enemy_Routines1_Daira", None, 0x9A15),
    ("bank2_Enemy_Routines1_Tektite", None, 0x9805),
    ("bank2_Enemy_Routines1_Lizalfos_Rock_Tossing", None, 0x9730),
    ("bank4_Enemy_Routines1_Horsehead", None, 0x981D),
    ("bank4_Enemy_Routines_Helmethead__Gooma", None, 0xBAC3),
    ("bank4_Enemy_Routines_Horsehead", None, 0xBB5F),
    ("bank4_Enemy_Routines_Floating_Helmet", None, 0xBCEF),
    ("bank4_Enemy_Routines1_Helmethead__Gooma", None, 0xBD75),
    ("bank5_Enemy_Routines1_Thunderbird", None, 0xA359),
    ("bank5_dark_link_AI_movement_maybe0", None, 0x98EB),
    (
        "bank5_Enemy_Routines1_Dark_Link_Battle_Trigger",
        None,
        0x97C6,
    ),
    (
        "bank4_Enemy_Routines1_Helmethead__Gooma_alias",
        None,
        0xBD75,
    ),
];

/// Fixed-bank routines intentionally unregistered (display/PPU scope).
pub const ENEMY_TRAPS_UNREGISTERED: &[TrapEntry] = &[("bank7_Display", Some(7), 0xEF11)];

// ---------------------------------------------------------------------------
// Addresses the ports touch (fixed bank / RAM / WRAM).
// ---------------------------------------------------------------------------

/// `bank7_Link_Hit_Routine` (`$E2EF`), the frozen-gate tail target.
const LINK_HIT_ROUTINE: u16 = 0xE2EF;
/// `LDE40` (`$DE40`): per-frame enemy tail (collision + display).
const ENEMY_TAIL_DE40: u16 = 0xDE40;
/// `bank7_Enemy_Routines1_Myu` (`$DA47`): Bot's shared tail.
const MYU_ROUTINE: u16 = 0xDA47;
/// `bank7_Enemy_Routines1_Deeler_Code` (`$D750`).
const DEELER_CODE: u16 = 0xD750;
/// `LD773` (`$D773`): Deeler jump-integration tail (blue-deeler drop path).
const DEELER_INTEGRATE: u16 = 0xD773;
/// `LD848` (`$D848`): Octorok hop/gravity/display helper.
const OCTOROK_HELPER: u16 = 0xD848;
/// `LE187` (`$E187`): elevator floor-change exit.
const ELEVATOR_EXIT: u16 = 0xE187;
/// WRAM enemy-routine vector table (`$6D8D`, `code * 2`).
const ENEMY_VECTORS: u16 = 0x6D8D;
/// WRAM enemy attribute tables (`$6DD5` / `$6DF9` / `$6E1D` / `$6E41`).
const ATTR_6DD5: u16 = 0x6DD5;
const ATTR_6DF9: u16 = 0x6DF9;
const ATTR_6E1D: u16 = 0x6E1D;
const ATTR_6E41: u16 = 0x6E41;
/// ROM tables (`bank7_Table_for_Deeler` `$D6DB`/`$D6DD`,
/// `bank7_Table_for_Bot` `$D9FE`, `LDB50` projectile speeds,
/// `bank7_Table_for_Elevator_Y_Velocity` `$D8BF`, exp `$DDC0`/`$DDDC`,
/// drop probabilities `$E870`).
const TBL_DEELER_Y: u16 = 0xD6DB;
const TBL_DEELER_X: u16 = 0xD6DD;
const TBL_BOT_X: u16 = 0xD9FE;
const TBL_PROJ_SPEED: u16 = 0xDB50;
const TBL_ELEVATOR_Y: u16 = 0xD8BF;
const TBL_EXP_LO: u16 = 0xDDC0;
const TBL_EXP_HI: u16 = 0xDDDC;
const TBL_DROP: u16 = 0xE870;

// ---------------------------------------------------------------------------
// Small helpers (register file, addressing, stack, flags, cycles).
// ---------------------------------------------------------------------------

/// `zp,x` effective address (wraps inside the zero page).
fn zpx(base: u8, x: u8) -> u16 {
    u16::from(base.wrapping_add(x))
}

/// Read `zp,x`.
fn rzpx(game: &Game, base: u8, x: u8) -> u8 {
    game.ram[zpx(base, x) as usize]
}

/// Write `zp,x`.
fn wzpx(game: &mut Game, base: u8, x: u8, v: u8) {
    game.ram[zpx(base, x) as usize] = v;
}

/// Read `abs,idx` (RAM/WRAM/ROM through the bus); returns `(value, page_cross)`.
fn rabs(game: &mut Game, base: u16, idx: u8) -> (u8, bool) {
    let ea = base.wrapping_add(u16::from(idx));
    (bus_read(game, ea), (base & 0xFF00) != (ea & 0xFF00))
}

/// Write `abs,idx` (RAM/WRAM through the bus).
fn wabs(game: &mut Game, base: u16, idx: u8, v: u8) {
    bus_write(game, base.wrapping_add(u16::from(idx)), v);
}

/// `LDA`-style load into `A` (sets `N`/`Z`).
fn lda(game: &mut Game, v: u8) {
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
}

/// `LDY`-style load into `Y` (sets `N`/`Z`).
fn ldy(game: &mut Game, v: u8) {
    game.cpu.y = v;
    set_nz(&mut game.cpu.p, v);
}

/// `LDX`-style load into `X` (sets `N`/`Z`).
fn ldx(game: &mut Game, v: u8) {
    game.cpu.x = v;
    set_nz(&mut game.cpu.p, v);
}

/// `ASL A`: `C` = old bit 7, `N`/`Z` from the result.
fn asl_a(game: &mut Game) {
    let a = game.cpu.a;
    set_c(game, a & 0x80 != 0);
    lda(game, a << 1);
}

/// `LSR A`: `C` = old bit 0, `N` = 0, `Z` from the result.
fn lsr_a(game: &mut Game) {
    let a = game.cpu.a;
    set_c(game, a & 0x01 != 0);
    lda(game, a >> 1);
}

/// `ROR mem`: bit 7 ← `C`, `C` ← old bit 0, `N`/`Z` from the result.
fn ror_val(game: &mut Game, v: u8) -> u8 {
    let cin = u8::from(game.cpu.p & FLAG_C != 0);
    set_c(game, v & 0x01 != 0);
    let r = (v >> 1) | (cin << 7);
    set_nz(&mut game.cpu.p, r);
    r
}

/// Set/clear `C`.
fn set_c(game: &mut Game, c: bool) {
    if c {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
}

/// `PHA` (mirrors `cpu.rs::push`).
fn pha(game: &mut Game) {
    game.ram[0x0100 + usize::from(game.cpu.sp)] = game.cpu.a;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// `PLA` (mirrors `cpu.rs::pop`; sets `N`/`Z`).
fn pla(game: &mut Game) {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    let v = game.ram[0x0100 + usize::from(game.cpu.sp)];
    lda(game, v);
}

/// Charge `cycles` to the CPU clock (see module docs, *Cycle charging*).
fn charge(game: &mut Game, cycles: u64) {
    game.cpu.cycles += cycles;
}

/// Hand-off: run `JSR callee` at `site` as ASM. Pushes the continuation
/// (`site + 2`, popped by the callee's `RTS` into `site + 3`) and tail-jumps
/// into `callee` ([`Game::trap_jump`]: its trap fires when it has one, the
/// interpreter runs it otherwise); the interpreter resumes the ASM body at
/// `site + 3` either way.
fn hand_off(game: &mut Game, site: u16, callee: u16) {
    push_word(game, site.wrapping_add(2));
    game.trap_jump(callee);
}

// ---------------------------------------------------------------------------
// Ports: lifecycle core.
// ---------------------------------------------------------------------------

/// `bank7_enemy_every_frame_routine` (bank 7 `$D6CA`).
///
/// `Y = $A1,x * 2` (`ASL` leaves `C` = code bit 7), `$0E/$0F` = WRAM vector
/// `$6D8D,y`/`$6D8E,y`, then `JMP ($0E)` into the enemy's routine
/// ([`Game::trap_jump`]). `A` = vector high byte, `N`/`Z` from it. 27 cycles +
/// page-cross extras on the two `abs,y` loads (codes `>= $39`).
pub fn en_every_frame(game: &mut Game) {
    let x = game.cpu.x;
    let code = rzpx(game, 0xA1, x);
    lda(game, code);
    asl_a(game);
    let y = game.cpu.a;
    ldy(game, y);
    let (lo, c1) = rabs(game, ENEMY_VECTORS, y);
    lda(game, lo);
    game.ram[0x000E] = lo;
    let (hi, c2) = rabs(game, ENEMY_VECTORS + 1, y);
    lda(game, hi);
    game.ram[0x000F] = hi;
    charge(game, 27 + u64::from(c1) + u64::from(c2));
    let target = u16::from(lo) | (u16::from(hi) << 8);
    game.trap_jump(target);
}

/// `bank7_Link_Collision_Detection` (bank 7 `$D6C1`).
///
/// `A = $A8,x & $10`; zero → `RTS` (15 cycles); else tail `JMP
/// bank7_Link_Hit_Routine` (`$E2EF`, 11 cycles + the callee).
pub fn en_link_collision(game: &mut Game) {
    let x = game.cpu.x;
    let v = rzpx(game, 0xA8, x) & 0x10;
    lda(game, v);
    if v == 0 {
        charge(game, 15);
    } else {
        charge(game, 11);
        game.trap_jump(LINK_HIT_ROUTINE);
    }
}

/// Body of `bank7_Enemy_Stops_when_Hit` (bank 7 `$DA02`) for inner use.
///
/// Returns `true` when the enemy is stunned: the caller's `JSR` frame has
/// been popped (`PLA : PLA`, `A` = return high byte) and control tail-jumps
/// to `LDE40`; the caller must return immediately. 14 cycles on the live
/// path (`LDA abs,x` + taken cross-page `BEQ` + `RTS`), 17 on the stun path
/// (+ the `LDE40` tail).
fn stun_body(game: &mut Game) -> bool {
    let x = game.cpu.x;
    let (stun, _) = rabs(game, 0x040E, x);
    lda(game, stun);
    if stun == 0 {
        charge(game, 14);
        return false;
    }
    let ret = pop_word(game);
    // Second PLA lands the high byte in A (N/Z from it).
    lda(game, (ret >> 8) as u8);
    charge(game, 17);
    game.trap_jump(ENEMY_TAIL_DE40);
    true
}

/// `bank7_Enemy_Stops_when_Hit` (bank 7 `$DA02`): trap entry.
pub fn en_stun_gate(game: &mut Game) {
    let _ = stun_body(game);
}

/// Body of `bank7_Determine_Enemy_Facing_Direction_relative_to_Link`
/// (bank 7 `$DC91`), inner-callable.
///
/// `Y = 1`; `LinkX + 8` with the *incoming* carry (`ADC #$08` without
/// `CLC`), page carry into `$0E`, `PHA`/`PLA` round trip, `SBC $4E,x` →
/// `$0F`, `SBC $3C,x` on the page; `BMI` → `INY`. `$60,x = Y`, then `DEY`
/// (`Y` = facing − 1, `N`/`Z` from it; `C`/`V` from the last `SBC`; `A` =
/// page difference). 51 cycles (Link right/at) or 52 (Link left).
fn facing_body(game: &mut Game) {
    let x = game.cpu.x;
    ldy(game, 1);
    let lx = game.ram[0x004D];
    lda(game, lx);
    let a = adc_val(&mut game.cpu.p, game.cpu.a, 0x08);
    game.cpu.a = a;
    pha(game);
    let lp = game.ram[0x003B];
    lda(game, lp);
    let a = adc_val(&mut game.cpu.p, game.cpu.a, 0x00);
    game.cpu.a = a;
    game.ram[0x000E] = a;
    pla(game);
    let ex = rzpx(game, 0x4E, x);
    let a = sbc_val(&mut game.cpu.p, game.cpu.a, ex);
    game.cpu.a = a;
    game.ram[0x000F] = a;
    let hi = game.ram[0x000E];
    lda(game, hi);
    let ep = rzpx(game, 0x3C, x);
    let a = sbc_val(&mut game.cpu.p, game.cpu.a, ep);
    game.cpu.a = a;
    let mut cyc = 51;
    if game.cpu.p & FLAG_N != 0 {
        let y = game.cpu.y.wrapping_add(1);
        ldy(game, y);
        cyc = 52;
    }
    let y = game.cpu.y;
    wzpx(game, 0x60, x, y);
    let y = y.wrapping_sub(1);
    ldy(game, y);
    charge(game, cyc);
}

/// `bank7_Determine_Enemy_Facing…` (bank 7 `$DC91`): trap entry.
pub fn en_facing(game: &mut Game) {
    facing_body(game);
}

/// `bank7_Change_Enemy_Facing_Direction_and_X_Velocity` (bank 7 `$E8EB`).
///
/// `$60,x ^= 3`; `$71,x = -$71,x` via `EOR #$FF : TAY : INY : STY`
/// (`A` = one's complement, `Y` = negated speed, `N`/`Z` from `INY`).
/// 30 cycles.
pub fn en_flip(game: &mut Game) {
    let x = game.cpu.x;
    let (f, v) = en::flip_on_wall(rzpx(game, 0x60, x), rzpx(game, 0x71, x));
    lda(game, f);
    wzpx(game, 0x60, x, f);
    lda(game, !rzpx(game, 0x71, x));
    ldy(game, v);
    wzpx(game, 0x71, x, v);
    charge(game, 30);
}

/// Body of `bank7_remove_enemy_or_item` (bank 7 `$DD47`): `A = 0`,
/// `$B6,x = 0`, `RTS`. 12 cycles.
fn remove_body(game: &mut Game) {
    let x = game.cpu.x;
    lda(game, en::remove_enemy());
    wzpx(game, 0xB6, x, 0);
    charge(game, 12);
}

/// `bank7_remove_enemy_or_item` (bank 7 `$DD47`): trap entry.
pub fn en_remove(game: &mut Game) {
    remove_body(game);
}

/// `LDD3D` (bank 7 `$DD3D`): regen-list bit clear + remove.
///
/// `Y = $BC,x`; negative → remove; else `($D6),y &= $7F`, then remove.
/// Cycles: 4 + 3 (+12) on the negative path; 4 + 2 + 5(+1) + 2 + 6 = 19
/// (+12) on the clear path.
fn ldd3d_body(game: &mut Game) {
    let x = game.cpu.x;
    let anchor = rzpx(game, 0xBC, x);
    ldy(game, anchor);
    if anchor & 0x80 != 0 {
        charge(game, 7);
    } else {
        let ptr = u16::from(game.ram[0x00D6]) | (u16::from(game.ram[0x00D7]) << 8);
        let (v, cross) = rabs(game, ptr, anchor);
        lda(game, v & 0x7F);
        wabs(game, ptr, anchor, v & 0x7F);
        charge(game, 19 + u64::from(cross));
    }
    remove_body(game);
}

/// `LDD34` (bank 7 `$DD34`): regenerate-bit gate ahead of `LDD3D`.
///
/// `Y = $A1,x`; `$6E41,y & $40 == 0` → straight remove (4 + 4 + 2 + 3,
/// then the 12-cycle remove); else fall into `LDD3D` (4 + 4 + 2 + 2, …).
fn ldd34_body(game: &mut Game) {
    let x = game.cpu.x;
    let code = rzpx(game, 0xA1, x);
    ldy(game, code);
    let (attr, cross) = rabs(game, ATTR_6E41, code);
    lda(game, attr & 0x40);
    if attr & 0x40 == 0 {
        charge(game, 13 + u64::from(cross));
        remove_body(game);
    } else {
        charge(game, 12 + u64::from(cross));
        ldd3d_body(game);
    }
}

/// `bank7_KillAllMonsters` (bank 7 `$E18F`).
///
/// Sweeps `X` down to 0: live slots (`$B6,x != 0`) run `LDD3D` then
/// `INC $B6,x` (BUG-compatible: killed slots read back 1). Ends with
/// `LDX $10` (`N`/`Z` from it) + `RTS`. Cycles counted per iteration.
pub fn en_kill_all(game: &mut Game) {
    loop {
        let x = game.cpu.x;
        let e = rzpx(game, 0xB6, x);
        lda(game, e);
        if e == 0 {
            charge(game, 7);
        } else {
            charge(game, 8);
            inner_jsr_frame(game, 0xE193, ldd3d_body);
            let n = rzpx(game, 0xB6, x).wrapping_add(1);
            set_nz(&mut game.cpu.p, n);
            wzpx(game, 0xB6, x, n);
            charge(game, 6);
        }
        let nx = x.wrapping_sub(1);
        ldx(game, nx);
        if nx & 0x80 == 0 {
            charge(game, 5);
        } else {
            charge(game, 4);
            break;
        }
    }
    let cur = game.ram[0x0010];
    ldx(game, cur);
    charge(game, 9);
}

/// `LDE20` (bank 7 `$DE20`): dead enemy → dropped item in place.
///
/// `JSR LDD34`, sound `$EF = 2`, `$AF,x = $048E,x` (item code),
/// `$BC,x = $FF`, `$B6,x = $A1,x = 1`, `LSR` → `$057E,x = $C2,x = 0`
/// (`C` = 1, `Z` = 1). 52 cycles around the inner `LDD34`.
fn lde20_body(game: &mut Game) {
    let x = game.cpu.x;
    inner_jsr_frame(game, 0xDE20, ldd34_body);
    lda(game, 0x02);
    game.ram[0x00EF] = 0x02;
    let (item, _) = rabs(game, 0x048E, x);
    lda(game, item);
    wzpx(game, 0xAF, x, item);
    lda(game, 0xFF);
    wzpx(game, 0xBC, x, 0xFF);
    lda(game, 0x01);
    wzpx(game, 0xB6, x, 0x01);
    wzpx(game, 0xA1, x, 0x01);
    lsr_a(game);
    wabs(game, 0x057E, x, 0);
    wzpx(game, 0xC2, x, 0);
    charge(game, 52);
}

/// `bank7_monster_death_give_exp` (bank 7 `$DDEC`).
///
/// `$414,x` negative (drop-armed by `ROR` in `$E880`) → `LDE20` item drop.
/// Else add `exp_lo/hi[rank]` into `$0756/$0755`; `$6E1D[code] == $FF`
/// (dead boss) → slot becomes a key at `($80, $40)`, `LDE20`, `$AF,x = 8`;
/// otherwise `JMP LDD34` (regen bit + remove).
pub fn en_death_exp(game: &mut Game) {
    let x = game.cpu.x;
    let (rank, _) = rabs(game, 0x0414, x);
    lda(game, rank);
    if rank & 0x80 != 0 {
        charge(game, 8);
        lde20_body(game);
        return;
    }
    ldy(game, rank);
    let (lo, c1) = rabs(game, TBL_EXP_LO, rank);
    lda(game, lo);
    game.cpu.p &= !FLAG_C;
    let sum = adc_val(&mut game.cpu.p, game.cpu.a, game.ram[0x0756]);
    game.cpu.a = sum;
    game.ram[0x0756] = sum;
    let (hi, c2) = rabs(game, TBL_EXP_HI, rank);
    lda(game, hi);
    let sum = adc_val(&mut game.cpu.p, game.cpu.a, game.ram[0x0755]);
    game.cpu.a = sum;
    game.ram[0x0755] = sum;
    let code = rzpx(game, 0xA1, x);
    ldy(game, code);
    let (marker, c3) = rabs(game, ATTR_6E1D, code);
    lda(game, marker);
    cmp_val(&mut game.cpu.p, marker, 0xFF);
    let base = 4 + 2 + 2 + 4 + 2 + 4 + 4 + 4 + 4 + 4 + 4 + 4 + 2;
    let extra = u64::from(c1) + u64::from(c2) + u64::from(c3);
    if marker != 0xFF {
        // BNE taken (3) + JMP LDD34 (3).
        charge(game, base + extra + 6);
        ldd34_body(game);
        return;
    }
    // Boss: BNE not taken (2), then the key conversion.
    charge(game, base + extra + 2);
    let (kx, ky, item) = en::boss_key_drop();
    lda(game, kx);
    wzpx(game, 0x4E, x, kx);
    lsr_a(game);
    wzpx(game, 0x2A, x, ky);
    charge(game, 2 + 4 + 2 + 4 + 6);
    inner_jsr_frame(game, 0xDE15, lde20_body);
    lda(game, item);
    wzpx(game, 0xAF, x, item);
    charge(game, 2 + 4 + 6);
}

/// `bank7_monster_death` (bank 7 `$E880`).
///
/// Latches `$414,x = $6DD5[code] & $0F`; `$6DF9[code] & $C0` picks the
/// drop group (0 = none): `INC $05DE,group`, 6th kill resets the counter,
/// `ROR $414,x` (arms the give-exp drop path), rolls `$051B,x & 7`
/// (+8 for the strong group) into `$048E,x` from the `$E870` table.
/// Always: `X = $10`, `$504,x = $25`, `$43E,x = 0`, `$B6,x = 2`,
/// `$EF = 4`, `SEC`, `RTS` (`A = 4`, `C = 1`).
pub fn en_death(game: &mut Game) {
    let x = game.cpu.x;
    let code = rzpx(game, 0xA1, x);
    ldy(game, code);
    let (p1, c1) = rabs(game, ATTR_6DD5, code);
    let rank = p1 & 0x0F;
    lda(game, rank);
    wabs(game, 0x0414, x, rank);
    let (p2, c2) = rabs(game, ATTR_6DF9, code);
    let group_bits = p2 & 0xC0;
    lda(game, group_bits);
    let mut cyc = 4 + 4 + 2 + 5 + 4 + 2 + u64::from(c1) + u64::from(c2);
    if group_bits == 0 {
        cyc += 3;
    } else {
        cyc += 2;
        let g = group_bits >> 6;
        lda(game, g);
        // LSR x6 (C = last shifted-out bit = bit 5 of the AND result = 0).
        set_c(game, false);
        game.cpu.x = g;
        game.cpu.y = g;
        let (ctr, _) = rabs(game, 0x05DE, g);
        let n = ctr.wrapping_add(1);
        set_nz(&mut game.cpu.p, n);
        wabs(game, 0x05DE, g, n);
        lda(game, n);
        cmp_val(&mut game.cpu.p, n, 0x06);
        cyc += 12 + 2 + 2 + 7 + 4 + 2;
        if n != 0x06 {
            cyc += 3;
        } else {
            cyc += 2;
            lda(game, 0);
            wabs(game, 0x05DE, g, 0);
            let cur = game.ram[0x0010];
            ldx(game, cur);
            let (r, _) = rabs(game, 0x0414, cur);
            let r = ror_val(game, r);
            wabs(game, 0x0414, cur, r);
            let (rng, _) = rabs(game, 0x051B, cur);
            lda(game, rng & 0x07);
            cmp_val(&mut game.cpu.p, game.cpu.y, 0x02);
            cyc += 2 + 5 + 3 + 7 + 4 + 2 + 2;
            if game.cpu.y == 0x02 {
                let a = adc_val(&mut game.cpu.p, game.cpu.a, 0x07);
                game.cpu.a = a;
                cyc += 2 + 2;
            } else {
                cyc += 3;
            }
            let idx = game.cpu.a;
            ldy(game, idx);
            let (drop, c3) = rabs(game, TBL_DROP, idx);
            lda(game, drop);
            wabs(game, 0x048E, cur, drop);
            cyc += 2 + 4 + 5 + u64::from(c3);
        }
    }
    let cur = game.ram[0x0010];
    ldx(game, cur);
    lda(game, en::DEATH_TIMER);
    wabs(game, 0x0504, cur, en::DEATH_TIMER);
    lda(game, 0);
    wabs(game, 0x043E, cur, 0);
    lda(game, 2);
    wzpx(game, 0xB6, cur, 2);
    lda(game, 4);
    game.ram[0x00EF] = 4;
    set_c(game, true);
    cyc += 3 + 2 + 5 + 2 + 5 + 2 + 4 + 2 + 3 + 2 + 6;
    charge(game, cyc);
}

// ---------------------------------------------------------------------------
// Ports: projectiles.
// ---------------------------------------------------------------------------

/// `LDBFD` (bank 7 `$DBFD`): free-slot scan from `Y` down to 0.
///
/// Found (`$87,y == 0`): `$20,y = $66,y = 1`, `LSR` → `$0584,y = 0`
/// (`A = 0`, `Z = 1`), `CLC`, `RTS`; returns `true`. Exhausted: `Y = $FF`
/// (`N` = 1), `A` = `$87,0`, `SEC`, `RTS`; returns `false`. Cycles per
/// probe: 4 + 3 (hit) or 4 + 2 + 2 + 4 (`BPL` back crosses a page).
fn slot_scan_body(game: &mut Game) -> bool {
    let mut cyc = 0u64;
    loop {
        let y = game.cpu.y;
        let (t, _) = rabs(game, 0x0087, y);
        lda(game, t);
        cyc += 4;
        if t == 0 {
            cyc += 3;
            lda(game, 0x01);
            wabs(game, 0x0020, y, 1);
            wabs(game, 0x0066, y, 1);
            lsr_a(game);
            wabs(game, 0x0584, y, 0);
            set_c(game, false);
            charge(game, cyc + 2 + 5 + 5 + 2 + 5 + 2 + 6);
            return true;
        }
        cyc += 2;
        let ny = y.wrapping_sub(1);
        ldy(game, ny);
        cyc += 2;
        if ny & 0x80 == 0 {
            cyc += 4;
        } else {
            cyc += 2;
            break;
        }
    }
    set_c(game, true);
    charge(game, cyc + 2 + 6);
    false
}

/// `bank7_Spawn_New_Projectile` (bank 7 `$DBCE`).
///
/// `Y = 5`, `JSR LDBFD`; no slot → `RTS` with `C` set. Else type `$04`
/// and the enemy X/page/Y/facing copied into the slot, `X = facing`, speed
/// from `LDB50,x`, `X = $10`, `CLC`, `RTS`.
pub fn en_spawn_proj(game: &mut Game) {
    let x = game.cpu.x;
    ldy(game, 0x05);
    charge(game, 2 + 6);
    inner_jsr_frame(game, 0xDBD0, |g| {
        let _ = slot_scan_body(g);
    });
    if game.cpu.p & FLAG_C != 0 {
        charge(game, 3 + 6);
        return;
    }
    let y = game.cpu.y;
    let p = en::spawn_projectile_copy(
        rzpx(game, 0x4E, x),
        rzpx(game, 0x3C, x),
        rzpx(game, 0x2A, x),
        rzpx(game, 0x60, x),
        0,
    );
    lda(game, p.kind);
    wabs(game, 0x0087, y, p.kind);
    lda(game, p.x);
    wabs(game, 0x0054, y, p.x);
    lda(game, p.page);
    wabs(game, 0x0042, y, p.page);
    lda(game, p.y);
    wabs(game, 0x0030, y, p.y);
    lda(game, p.facing);
    wabs(game, 0x0066, y, p.facing);
    game.cpu.x = p.facing;
    let (speed, cross) = rabs(game, TBL_PROJ_SPEED, p.facing);
    lda(game, speed);
    wabs(game, 0x0077, y, speed);
    let cur = game.ram[0x0010];
    ldx(game, cur);
    set_c(game, false);
    charge(
        game,
        2 + 2 + 5 + 4 + 5 + 4 + 5 + 4 + 5 + 4 + 5 + 2 + 4 + 5 + 3 + 2 + 6 + u64::from(cross),
    );
}

/// `bank7_spawn_new_bubble_or_rock` (bank 7 `$DBFB`): `Y = 3` + `LDBFD`.
pub fn en_spawn_bubble(game: &mut Game) {
    ldy(game, 0x03);
    charge(game, 2);
    let _ = slot_scan_body(game);
}

/// `LE6E8` (bank 7 `$E6E8`): deflect projectile `Y` into disintegration.
///
/// `$7D,y = 0`, `$8D,y = $F2` (`A = $F2`, `N` = 1). 20 cycles.
pub fn en_proj_disintegrate(game: &mut Game) {
    let y = game.cpu.y;
    let (xsub, flag) = en::projectile_disintegrate();
    lda(game, xsub);
    wabs(game, 0x007D, y, xsub);
    lda(game, flag);
    wabs(game, 0x008D, y, flag);
    charge(game, 20);
}

// ---------------------------------------------------------------------------
// Ports: sword / shield vs enemy (unregistered here; player-owned addrs).
// ---------------------------------------------------------------------------

/// `bank7_Sword_Hit_Detection…` damage half (bank 7 `$E677`/`$E726`).
///
/// Gate staged in scratch: `$02` = overlap (from `player_traps::pl_overlap`
/// over the sword box), `$03` bit `$10` = untouchable + bit `$20` = fire,
/// `$0B` blade. Hit: `HP -= ATTACK_POWER[atk-1]` (staged power in `$07`),
/// `$040E = $30`, `$ED = $10`, `$A8 |= $20`, kill (`$B6 = 2`) or recoil
/// note in `$02 = 1`. Up-stab hover / down-stab bounce quirks preserved.
/// Unit-coverage shim (not registered; `$E677` is player-owned).
pub fn en_sword_hit(game: &mut Game) {
    let s = usize::from(game.ram[0x0010]) % en::ENEMY_SLOTS;
    let s16 = s as u16;
    let code = game.ram[0x00A1 + s];
    let gate = en::SwordGate {
        retracted: game.ram[0x0480] == 0xF8,
        untouchable: game.ram[0x0003] & 0x10 != 0,
        slot_live: game.ram[0x00B6 + s] == 0x01,
        enemy_code: code,
        overlaps: game.ram[0x0002] != 0,
        blade: game.ram[0x000B],
        fire_immune: game.ram[0x0003] & 0x20 != 0,
        jar_guarded: code == 0x01 && game.ram[0x040E + s] != 0,
    };
    match en::sword_gate(gate) {
        en::SwordOutcome::Miss => {
            game.ram[0x00A8 + s] &= 0xDF;
        }
        en::SwordOutcome::Deflect => {
            game.ram[0x00EC] = 0x02;
            game.ram[0x0002] = 0x02;
        }
        en::SwordOutcome::TouchItem => {
            game.ram[0x0002] = 0x03;
        }
        en::SwordOutcome::Hit => {
            let dmg = en::sword_damage_step(
                game.ram[0x00C2 + s],
                game.ram[0x0007],
                game.ram[0x0080],
                0,
                game.ram[0x0029],
                game.ram[0x002A + s],
            );
            if let Some(v) = dmg.link_vspeed {
                game.ram[0x057D] = v;
            }
            game.ram[0x00C2 + s] = dmg.hp;
            game.ram[usize::from(0x040E + s16)] = dmg.stun;
            game.ram[0x00ED] = dmg.sound;
            game.ram[0x00A8 + s] |= 0x20;
            if dmg.dead {
                game.ram[0x00B6 + s] = 0x02;
            }
            game.ram[0x0002] = 0x01;
        }
    }
}

/// `bank7_code39` (bank 7 `$E558`): shield-vs-body router.
///
/// Staged shield overlap in `$02`, body overlap in `$03`. Records `$02`
/// (1 = shield block with `$EC = 2`, `$0B = 0`; 0 = body path).
/// Unit-coverage shim (not registered; `$E558` is player-owned).
pub fn en_shield_gate(game: &mut Game) {
    let s = usize::from(game.ram[0x0010]) % en::ENEMY_SLOTS;
    let code = game.ram[0x00A1 + s];
    let blocked = en::shield_blocks(game.ram[0x0710] != 0, code, game.ram[0x0002] != 0);
    if blocked {
        game.ram[0x00EC] = 0x02;
        game.ram[0x000B] = 0x00;
        game.ram[0x0002] = 0x01;
    } else {
        game.ram[0x0002] = 0x00;
    }
}

// ---------------------------------------------------------------------------
// Ports: fixed-bank enemy families.
// ---------------------------------------------------------------------------

/// `bank7_Enemy_Routines1_Bot` (bank 7 `$DA0C`).
///
/// `JSR` stun gate (stunned → `LDE40`, see [`stun_body`]); grounded
/// (`$A8,x & 4`) with `$051B,x * 2 == 0` (1:128) → hop: `$057E,x = $E5`,
/// facing toward Link, `$71,x = bank7_Table_for_Bot[Y]`; then `JMP Myu`
/// (`$DA47`, the shared Bit/Bot/Myu tail — runs as ASM via [`Game::trap_jump`]).
pub fn en_walker(game: &mut Game) {
    let x = game.cpu.x;
    charge(game, 6);
    push_word(game, 0xDA0E);
    if stun_body(game) {
        return;
    }
    let _ = pop_word(game);
    let st = rzpx(game, 0xA8, x) & 0x04;
    lda(game, st);
    let mut cyc = 4 + 2;
    if st != 0 {
        cyc += 2;
        let (rng, _) = rabs(game, 0x051B, x);
        lda(game, rng);
        asl_a(game);
        cyc += 4 + 2;
        if game.cpu.a == 0 {
            cyc += 2;
            lda(game, 0xE5);
            wabs(game, 0x057E, x, 0xE5);
            cyc += 2 + 5 + 6;
            inner_jsr_frame(game, 0xDA20, facing_body);
            let y = game.cpu.y;
            let (v, cross) = rabs(game, TBL_BOT_X, y);
            lda(game, v);
            wzpx(game, 0x71, x, v);
            cyc += 4 + 4 + u64::from(cross);
        } else {
            cyc += 3;
        }
    } else {
        cyc += 3;
    }
    charge(game, cyc + 3);
    game.trap_jump(MYU_ROUTINE);
}

/// `bank7_Enemy_Routines1_Deeler` (bank 7 `$D6DF`).
///
/// Stun gate; Link-collision gate (frozen → hand off to
/// `bank7_Link_Hit_Routine` and let the ASM body resume at `$D6E5`);
/// `$AF,x` negative → `JMP Deeler_Code` (`$D750`). Else facing, the
/// hover-vs-drop select on the horizontal distance `$0F + $40 < $80`
/// (red/blue drop roll `$051B,x & $1F | $C9`), the `$05DC,y` descent
/// table walk, blue-deeler (`$0E`) recovery at `Y >= $8E` (`ROR $AF,x`,
/// `$71,x = 0`, `$057E,x = $504,x = $20`, → `LD773`), or the
/// `bank7_Table_for_Deeler` vertical step; all paths end in `LDE40`.
pub fn en_flyer(game: &mut Game) {
    let x = game.cpu.x;
    charge(game, 6);
    push_word(game, 0xD6E1);
    if stun_body(game) {
        return;
    }
    let _ = pop_word(game);
    // JSR bank7_Link_Collision_Detection ($D6E2).
    charge(game, 6);
    let st = rzpx(game, 0xA8, x) & 0x10;
    lda(game, st);
    if st != 0 {
        charge(game, 11);
        hand_off(game, 0xD6E2, LINK_HIT_ROUTINE);
        return;
    }
    charge(game, 15);
    let aux = rzpx(game, 0xAF, x);
    lda(game, aux);
    if aux & 0x80 != 0 {
        charge(game, 4 + 2 + 3);
        game.trap_jump(DEELER_CODE);
        return;
    }
    charge(game, 4 + 3 + 6);
    inner_jsr_frame(game, 0xD6EC, facing_body);
    let aux = rzpx(game, 0xAF, x);
    lda(game, aux);
    charge(game, 4);
    let mut goto_711 = aux != 0;
    charge(game, if goto_711 { 3 } else { 2 });
    if !goto_711 {
        let d = game.ram[0x000F];
        lda(game, d);
        let a = adc_val(&mut game.cpu.p, game.cpu.a, 0x40);
        game.cpu.a = a;
        cmp_val(&mut game.cpu.p, a, 0x80);
        charge(game, 3 + 2 + 2);
        let mut hover = a < 0x80;
        charge(game, if hover { 3 } else { 2 });
        if hover {
            // bank7_Deeler_Red_Blue: drop roll.
            let (rng, _) = rabs(game, 0x051B, x);
            lda(game, (rng & 0x1F) | game.ram[0x00C9]);
            charge(game, 4 + 2 + 3);
            if game.cpu.a != 0 {
                charge(game, 4);
                hover = false;
            } else {
                charge(game, 2);
                let n = aux.wrapping_add(1);
                set_nz(&mut game.cpu.p, n);
                wzpx(game, 0xAF, x, n);
                charge(game, 6);
                goto_711 = true;
            }
        }
        if !hover && !goto_711 {
            // LD6FB: horizontal cruise then LDE40.
            let y = game.cpu.y;
            let (v, cross) = rabs(game, TBL_DEELER_X, y);
            lda(game, v);
            wzpx(game, 0x71, x, v);
            charge(game, 4 + 4 + u64::from(cross) + 6);
            inner_jsr_frame(game, 0xD700, en_simple_h);
            charge(game, 3 + 3);
            game.trap_jump(ENEMY_TAIL_DE40);
            return;
        }
    }
    // LD711 descent-table walk (loops back through LD70F while the
    // stage matches its target row and the stage is not 2).
    loop {
        let aux = rzpx(game, 0xAF, x);
        lda(game, aux);
        lsr_a(game);
        let y = game.cpu.a;
        ldy(game, y);
        let ey = rzpx(game, 0x2A, x);
        lda(game, ey);
        let (row, cross) = rabs(game, 0x05DC, y);
        cmp_val(&mut game.cpu.p, ey, row);
        charge(game, 4 + 2 + 2 + 4 + 4 + u64::from(cross));
        if ey != row {
            charge(game, 3);
            break;
        }
        let aux = rzpx(game, 0xAF, x);
        lda(game, aux);
        cmp_val(&mut game.cpu.p, aux, 0x02);
        charge(game, 2 + 4 + 2);
        if aux != 0x02 {
            let n = aux.wrapping_add(1);
            set_nz(&mut game.cpu.p, n);
            wzpx(game, 0xAF, x, n);
            charge(game, 3 + 6);
            continue;
        }
        lda(game, 0);
        wzpx(game, 0xAF, x, 0);
        charge(game, 2 + 2 + 4 + 3 + 3);
        game.trap_jump(ENEMY_TAIL_DE40);
        return;
    }
    // LD728: blue-deeler recovery check.
    let code = rzpx(game, 0xA1, x);
    lda(game, code);
    cmp_val(&mut game.cpu.p, code, 0x0E);
    charge(game, 4 + 2);
    let mut recover = false;
    if code == 0x0E {
        charge(game, 2);
        let ey = rzpx(game, 0x2A, x);
        lda(game, ey);
        cmp_val(&mut game.cpu.p, ey, 0x8E);
        charge(game, 4 + 2);
        if ey >= 0x8E {
            charge(game, 2);
            recover = true;
        } else {
            charge(game, 3);
        }
    } else {
        charge(game, 3);
    }
    if recover {
        let aux = rzpx(game, 0xAF, x);
        let r = ror_val(game, aux);
        wzpx(game, 0xAF, x, r);
        lda(game, 0);
        wzpx(game, 0x71, x, 0);
        lda(game, 0x20);
        wabs(game, 0x057E, x, 0x20);
        wabs(game, 0x0504, x, 0x20);
        charge(game, 6 + 2 + 4 + 2 + 5 + 5 + 3);
        game.trap_jump(DEELER_INTEGRATE);
        return;
    }
    // LD744: table vertical velocity + simple vertical step, then LDE40.
    let y = game.cpu.y;
    let (v, cross) = rabs(game, TBL_DEELER_Y, y);
    lda(game, v);
    wabs(game, 0x057E, x, v);
    charge(game, 4 + 5 + u64::from(cross) + 6);
    inner_jsr_frame(game, 0xD74A, en_simple_v);
    charge(game, 3);
    game.trap_jump(ENEMY_TAIL_DE40);
}

/// `bank7_Enemy_Routines1_Raising_Bubbles` (bank 7 `$DC15`).
///
/// `INC $AF,x`; every 32nd frame (`& $1F == 0`) claims a bubble slot
/// (`Y = 3` scan): type `$02`, X = `$072C + $051B,x` (with the scan's
/// cleared carry) `& $F0`, page = `$072A + carry`, `Y = $E0`,
/// `$0584,y = $E4`. `RTS` on every path.
pub fn en_generator(game: &mut Game) {
    let x = game.cpu.x;
    let aux = rzpx(game, 0xAF, x).wrapping_add(1);
    set_nz(&mut game.cpu.p, aux);
    wzpx(game, 0xAF, x, aux);
    lda(game, aux & 0x1F);
    charge(game, 6 + 4 + 2);
    if aux & 0x1F != 0 {
        charge(game, 3 + 6);
        return;
    }
    charge(game, 2 + 6);
    inner_jsr_frame(game, 0xDC1D, en_spawn_bubble);
    if game.cpu.p & FLAG_C != 0 {
        charge(game, 3 + 6);
        return;
    }
    let y = game.cpu.y;
    lda(game, 0x02);
    wabs(game, 0x0087, y, 0x02);
    let scroll_lo = game.ram[0x072C];
    lda(game, scroll_lo);
    let (rng, _) = rabs(game, 0x051B, x);
    let a = adc_val(&mut game.cpu.p, game.cpu.a, rng);
    lda(game, a & 0xF0);
    wabs(game, 0x0054, y, a & 0xF0);
    let scroll_hi = game.ram[0x072A];
    lda(game, scroll_hi);
    let a = adc_val(&mut game.cpu.p, game.cpu.a, 0x00);
    game.cpu.a = a;
    wabs(game, 0x0042, y, a);
    lda(game, 0xE0);
    wabs(game, 0x0030, y, 0xE0);
    lda(game, 0xE4);
    wabs(game, 0x0584, y, 0xE4);
    charge(
        game,
        2 + 2 + 5 + 4 + 4 + 2 + 5 + 4 + 2 + 5 + 2 + 5 + 2 + 5 + 6,
    );
}

/// `bank7_Enemy_Routines1_Octorok` (bank 7 `$D888`).
///
/// Stun gate, then `JSR LD848` (hop timer + gravity + display + Link
/// collision) which is unported: hand off so the ASM body resumes at
/// `$D88E` (wall flip, cruise, aim, rock cooldown) after `LD848` returns.
pub fn en_shooter(game: &mut Game) {
    charge(game, 6);
    push_word(game, 0xD88A);
    if stun_body(game) {
        return;
    }
    let _ = pop_word(game);
    charge(game, 6);
    hand_off(game, 0xD88B, OCTOROK_HELPER);
}

/// `bank7_Enemy_Routines1_Elevator` (bank 7 `$D8C2`).
///
/// Link not riding (`$A8,x & $10 == 0`) → `JMP LDE40`. Riding: `$0754 =
/// $10`, `$0479 = $057D = 0`, `$057E,x = bank7_Table_for_Elevator_Y_Velocity
/// [$0743 >> 2]`; with a direction held, a mismatch against Link's
/// above/below collision bits (`$A7 & $0C`) plays `$EF = $20` and steps
/// vertically; then `$29 = $2A,x + 8`; `$2A,x >= $D8` → floor change
/// (`$4D = $70`, `$0735 = 6`, `$0734 = $0B`, `$FD = $072C = 0`, `A = $13`,
/// `JMP LE187`), else `JMP LDE40`.
pub fn en_elevator(game: &mut Game) {
    let x = game.cpu.x;
    let riding = rzpx(game, 0xA8, x) & 0x10;
    lda(game, riding);
    charge(game, 4 + 2);
    if riding == 0 {
        charge(game, 4 + 3);
        game.trap_jump(ENEMY_TAIL_DE40);
        return;
    }
    game.ram[0x0754] = riding;
    lda(game, 0);
    game.ram[0x0479] = 0;
    game.ram[0x057D] = 0;
    let held = game.ram[0x0743];
    lda(game, held);
    lsr_a(game);
    lsr_a(game);
    let y = game.cpu.a;
    ldy(game, y);
    let (vy, cross) = rabs(game, TBL_ELEVATOR_Y, y);
    lda(game, vy);
    wabs(game, 0x057E, x, vy);
    let held = game.ram[0x0743];
    lda(game, held);
    charge(
        game,
        2 + 4 + 2 + 4 + 4 + 4 + 2 + 2 + 2 + 4 + 5 + 4 + u64::from(cross),
    );
    if held != 0 {
        // BEQ LD8F4 not taken; compare the held direction with Link's
        // above/below collision bits.
        charge(game, 2);
        let coll = game.ram[0x00A7] & 0x0C;
        lda(game, coll ^ held);
        charge(game, 3 + 2 + 4);
        if game.cpu.a != 0 {
            charge(game, 2);
            lda(game, 0x20);
            game.ram[0x00EF] = 0x20;
            charge(game, 2 + 3 + 6);
            inner_jsr_frame(game, 0xD8F1, en_simple_v);
        } else {
            // BEQ LD8FB: skips the Link Y re-seat.
            charge(game, 3);
            elevator_floor_check(game);
            return;
        }
    } else {
        charge(game, 3);
    }
    // LD8F4: seat Link 8px below the platform top.
    let ey = rzpx(game, 0x2A, x);
    lda(game, ey);
    game.cpu.p &= !FLAG_C;
    let a = adc_val(&mut game.cpu.p, game.cpu.a, 0x08);
    game.cpu.a = a;
    game.ram[0x0029] = a;
    charge(game, 4 + 2 + 2 + 3);
    elevator_floor_check(game);
}

/// `LD8FB` tail of the elevator: floor change at `$2A,x >= $D8`.
fn elevator_floor_check(game: &mut Game) {
    let x = game.cpu.x;
    let ey = rzpx(game, 0x2A, x);
    lda(game, ey);
    cmp_val(&mut game.cpu.p, ey, 0xD8);
    charge(game, 4 + 2);
    if ey < 0xD8 {
        charge(game, 3 + 3);
        game.trap_jump(ENEMY_TAIL_DE40);
        return;
    }
    lda(game, 0x70);
    game.ram[0x004D] = 0x70;
    lda(game, 0x06);
    game.ram[0x0735] = 0x06;
    lda(game, 0x0B);
    game.ram[0x0734] = 0x0B;
    lda(game, 0x00);
    game.ram[0x00FD] = 0x00;
    game.ram[0x072C] = 0x00;
    lda(game, 0x13);
    charge(game, 2 + 2 + 3 + 2 + 4 + 2 + 4 + 2 + 3 + 4 + 2 + 3);
    game.trap_jump(ELEVATOR_EXIT);
}

/// `bank7_Enemy_Routines1_Locked_Door` (bank 7 `$D991`).
///
/// Opens with `JSR LDE40` (display tail, unported): hand off so the ASM
/// body resumes at `$D994` (open-animation counter, key consumption).
pub fn en_locked_door(game: &mut Game) {
    charge(game, 6);
    hand_off(game, 0xD991, ENEMY_TAIL_DE40);
}

// ---------------------------------------------------------------------------
// Ports: shared movement kernels.
// ---------------------------------------------------------------------------

/// `bank7_XY_Movements_Routine` (bank 7 `$D1CE`) on the current `X`.
///
/// Splits `$70,x` into 4.4 fixed point (`$01` = fraction, `$00` = signed
/// integer, `$02` = sign extension), adds the fraction into `$03D6,x` and
/// the integer + carry into `$4D,x`/`$3B,x`, and returns `A = carry +
/// integer` (`LD247`: `PLA : CLC : ADC $00`, flags from that `ADC`, `Y` =
/// sign). Cycles counted per path (`BCC`/`BPL` taken = 3, untaken = 2
/// plus the `ORA`/`DEY` they skip); 108 on the short/short path.
fn xy_movement_body(game: &mut Game) {
    let x = game.cpu.x;
    let v = rzpx(game, 0x70, x);
    lda(game, v);
    let frac = v << 4;
    lda(game, frac);
    game.ram[0x0001] = frac;
    lda(game, v);
    let mut whole = v >> 4;
    let mut cyc: u64 = 4 + 8 + 3 + 4 + 8 + 2;
    set_c(game, false);
    cmp_val(&mut game.cpu.p, whole, 0x08);
    if whole >= 0x08 {
        whole |= 0xF0;
        cyc += 2 + 2;
    } else {
        cyc += 3;
    }
    lda(game, whole);
    game.ram[0x0000] = whole;
    ldy(game, 0);
    cmp_val(&mut game.cpu.p, whole, 0x00);
    cyc += 3 + 2 + 2;
    if whole & 0x80 != 0 {
        ldy(game, 0xFF);
        cyc += 2 + 2;
    } else {
        cyc += 3;
    }
    let sign = game.cpu.y;
    game.ram[0x0002] = sign;
    let (sub, _) = rabs(game, 0x03D6, x);
    lda(game, sub);
    game.cpu.p &= !FLAG_C;
    let nsub = adc_val(&mut game.cpu.p, sub, frac);
    game.cpu.a = nsub;
    wabs(game, 0x03D6, x, nsub);
    let carry = u8::from(game.cpu.p & FLAG_C != 0);
    // LDA #0 : ROL : PHA : ROR — parks the carry on the stack.
    lda(game, carry);
    pha(game);
    lda(game, 0);
    set_c(game, carry != 0);
    let lo = rzpx(game, 0x4D, x);
    lda(game, lo);
    let nlo = adc_val(&mut game.cpu.p, lo, whole);
    game.cpu.a = nlo;
    wzpx(game, 0x4D, x, nlo);
    let hi = rzpx(game, 0x3B, x);
    lda(game, hi);
    let nhi = adc_val(&mut game.cpu.p, hi, sign);
    game.cpu.a = nhi;
    wzpx(game, 0x3B, x, nhi);
    cyc += 3 + 4 + 2 + 3 + 5 + 2 + 2 + 3 + 2 + 4 + 3 + 4 + 4 + 3 + 4 + 3;
    // LD247: PLA : CLC : ADC $00 : RTS.
    pla(game);
    game.cpu.p &= !FLAG_C;
    let a = adc_val(&mut game.cpu.p, game.cpu.a, whole);
    game.cpu.a = a;
    cyc += 4 + 2 + 3 + 6;
    charge(game, cyc);
}

/// `LD20A` (bank 7 `$D20A`) on the current `X`: vertical twin of
/// [`xy_movement_body`] over `$057D,x` → `$03E6,x` / `$0029,x` / `$19,x`.
fn y_movement_body(game: &mut Game) {
    let x = game.cpu.x;
    let (v, _) = rabs(game, 0x057D, x);
    lda(game, v);
    let frac = v << 4;
    lda(game, frac);
    game.ram[0x0001] = frac;
    lda(game, v);
    let mut whole = v >> 4;
    let mut cyc: u64 = 4 + 8 + 3 + 4 + 8 + 2;
    set_c(game, false);
    cmp_val(&mut game.cpu.p, whole, 0x08);
    if whole >= 0x08 {
        whole |= 0xF0;
        cyc += 2 + 2;
    } else {
        cyc += 3;
    }
    lda(game, whole);
    game.ram[0x0000] = whole;
    ldy(game, 0);
    cmp_val(&mut game.cpu.p, whole, 0x00);
    cyc += 3 + 2 + 2;
    if whole & 0x80 != 0 {
        ldy(game, 0xFF);
        cyc += 2 + 2;
    } else {
        cyc += 3;
    }
    let sign = game.cpu.y;
    game.ram[0x0002] = sign;
    let (sub, _) = rabs(game, 0x03E6, x);
    lda(game, sub);
    game.cpu.p &= !FLAG_C;
    let nsub = adc_val(&mut game.cpu.p, sub, frac);
    game.cpu.a = nsub;
    wabs(game, 0x03E6, x, nsub);
    let carry = u8::from(game.cpu.p & FLAG_C != 0);
    lda(game, carry);
    pha(game);
    lda(game, 0);
    set_c(game, carry != 0);
    let (y0, _) = rabs(game, 0x0029, x);
    lda(game, y0);
    let ny = adc_val(&mut game.cpu.p, y0, whole);
    game.cpu.a = ny;
    wabs(game, 0x0029, x, ny);
    let pg = rzpx(game, 0x19, x);
    lda(game, pg);
    let npg = adc_val(&mut game.cpu.p, pg, sign);
    game.cpu.a = npg;
    wzpx(game, 0x19, x, npg);
    cyc += 3 + 4 + 2 + 3 + 5 + 2 + 2 + 3 + 2 + 4 + 3 + 5 + 4 + 3 + 4;
    pla(game);
    game.cpu.p &= !FLAG_C;
    let a = adc_val(&mut game.cpu.p, game.cpu.a, whole);
    game.cpu.a = a;
    cyc += 4 + 2 + 3 + 6;
    charge(game, cyc);
}

/// `bank7_Simple_Horizontal_Movement` (bank 7 `$DEB8`).
///
/// `INX : JSR $D1CE : DEX : RTS` — integrates `$71,x` over
/// `$3D7,x`/`$4E,x`/`$3C,x` (the enemy rows sit one past Link's).
/// `N`/`Z` from the final `DEX`; `C`/`V` from the kernel's last `ADC`.
pub fn en_simple_h(game: &mut Game) {
    let x = game.cpu.x;
    ldx(game, x.wrapping_add(1));
    charge(game, 2 + 6);
    inner_jsr_frame(game, 0xDEB9, xy_movement_body);
    ldx(game, x);
    charge(game, 2 + 6);
}

/// `bank7_Simple_Vertical_Movement` (bank 7 `$DEC8`).
///
/// `INX : JSR LD20A : DEX : RTS` — integrates `$057E,x` over
/// `$3E7,x`/`$2A,x`/`$1A,x`.
pub fn en_simple_v(game: &mut Game) {
    let x = game.cpu.x;
    ldx(game, x.wrapping_add(1));
    charge(game, 2 + 6);
    inner_jsr_frame(game, 0xDEC9, y_movement_body);
    ldx(game, x);
    charge(game, 2 + 6);
}

/// `bank7_Gravity` (bank 7 `$DEBE`).
///
/// `JSR bank7_Simple_Vertical_Movement`, then `INC $057E,x` twice
/// (`N`/`Z` from the second increment).
pub fn en_gravity(game: &mut Game) {
    let x = game.cpu.x;
    charge(game, 6);
    inner_jsr_frame(game, 0xDEBE, en_simple_v);
    let (v, _) = rabs(game, 0x057E, x);
    let v = v.wrapping_add(2);
    set_nz(&mut game.cpu.p, v);
    wabs(game, 0x057E, x, v);
    charge(game, 7 + 7 + 6);
}

// ---------------------------------------------------------------------------
// Shims: bosses (banked halves on staged regs; unit coverage only).
// ---------------------------------------------------------------------------

fn slot(game: &Game) -> usize {
    usize::from(game.ram[0x0010]) % en::ENEMY_SLOTS
}

/// `bank4_Enemy_Routines_Horsehead` (bank 4 `$BB5F`, banked — shim runs the
/// pure half on staged regs for unit coverage; NOT registered).
pub fn en_horsehead(game: &mut Game) {
    let s = slot(game);
    let out = boss::horsehead(boss::HorseheadIn {
        aux: game.ram[0x00AF + s],
        dist: game.ram[0x000F],
        frame: game.ram[0x0012],
        rng: game.ram[0x051B + s % 0x20],
        facing: game.ram[0x0060 + s],
        code: game.ram[0x00A1 + s],
        anim: game.ram[0x0081 + s],
    });
    game.ram[0x0071 + s] = out.speed;
    game.ram[0x00AF + s] = out.aux;
    game.ram[0x0002] = u8::from(out.swing);
}

/// `bank4_Enemy_Routines_Helmethead__Gooma` (`$BAC3`, banked — same note).
pub fn en_helmethead(game: &mut Game) {
    let s = slot(game);
    let out = boss::helmethead(boss::HelmetheadIn {
        aux: game.ram[0x00AF + s],
        frame: game.ram[0x0012],
        rng: game.ram[0x051B + s % 0x20],
        strafing: game.ram[0x00C8] & 0x06 != 0,
    });
    game.ram[0x00AF + s] = out.aux;
    game.ram[0x0002] = u8::from(out.fire);
}

/// Gooma half of `$BAC3` (banked — same note).
pub fn en_gooma(game: &mut Game) {
    let s = slot(game);
    let out = boss::gooma(boss::GoomaIn {
        aux: game.ram[0x00AF + s],
        frame: game.ram[0x0012],
        rng: game.ram[0x051B + s % 0x20],
        grounded: game.ram[0x00A8 + s] & 0x04 != 0,
    });
    game.ram[0x00AF + s] = out.aux;
    game.ram[0x057E + s] = out.yspeed;
    game.ram[0x0002] = u8::from(out.swing);
}

/// `bank4_Enemy_Routines_Floating_Helmet` (`$BCEF`, banked — same note).
pub fn en_floating_helmet(game: &mut Game) {
    let s = slot(game);
    let out = boss::floating_helmet(boss::FloatingHelmetIn {
        aux: game.ram[0x00AF + s],
        y: game.ram[0x002A + s],
        yspeed: game.ram[0x057E + s],
        timer: game.ram[0x0504 + s],
        armor: game.ram[0x0444 + s],
    });
    game.ram[0x00AF + s] = out.aux;
    game.ram[0x057E + s] = out.yspeed;
    game.ram[0x0504 + s] = out.timer;
    game.ram[0x0002] = u8::from(out.slam);
}

/// `bank5_Enemy_Routines1_Thunderbird` (`$A359`, banked — same note).
pub fn en_thunderbird(game: &mut Game) {
    let s = slot(game);
    let out = boss::thunderbird(boss::ThunderbirdIn {
        awake: game.ram[0x00D9] != 0,
        aux: game.ram[0x00AF + s],
        frame: game.ram[0x0012],
        rng: game.ram[0x051B + s % 0x20],
        thunder_struck: game.ram[0x0003] != 0,
    });
    game.ram[0x00D9] = u8::from(out.awake);
    game.ram[0x00AF + s] = out.aux;
    game.ram[0x0002] = u8::from(out.fire);
}

/// `bank5_dark_link_AI_movement_maybe0` (`$98EB`, banked — same note).
pub fn en_dark_link(game: &mut Game) {
    let s = slot(game);
    let out = boss::dark_link(boss::DarkLinkIn {
        link_speed: game.ram[0x0070],
        link_anim: game.ram[0x0080],
        link_y: game.ram[0x0029],
        self_y: game.ram[0x002A + s],
        frame: game.ram[0x0012],
        rng: game.ram[0x051B + s % 0x20],
        atk: game.ram[0x0777],
    });
    game.ram[0x0071 + s] = out.speed;
    game.ram[0x0002] = u8::from(out.crouch);
    game.ram[0x0003] = u8::from(out.stab);
}

// ---------------------------------------------------------------------------
// Registration.
// ---------------------------------------------------------------------------

/// Register every fixed-bank enemy trap on `game`.
///
/// All ports charge their (variable-path) cycles in-port, so the static
/// [`crate::traps::TrapInfo::cycles`] is 0 for each. Banked (`None`)
/// entries are skipped (aliasing caveat); the two [`ENEMY_TRAPS_SHARED`]
/// entries are skipped (player-owned). Idempotent.
pub fn register_enemy_traps(game: &mut Game) {
    let mut reg = |name: &'static str, addr: u16, f: fn(&mut Game)| {
        game.trap_register_cycles(name, Some(7), addr, f, 0);
    };
    reg("bank7_enemy_every_frame_routine", 0xD6CA, en_every_frame);
    reg("bank7_Link_Collision_Detection", 0xD6C1, en_link_collision);
    reg("bank7_Enemy_Stops_when_Hit", 0xDA02, en_stun_gate);
    reg(
        "bank7_Determine_Enemy_Facing_Direction_relative_to_Link",
        0xDC91,
        en_facing,
    );
    reg(
        "bank7_Change_Enemy_Facing_Direction_and_X_Velocity",
        0xE8EB,
        en_flip,
    );
    reg("bank7_remove_enemy_or_item", 0xDD47, en_remove);
    reg("bank7_KillAllMonsters", 0xE18F, en_kill_all);
    reg("bank7_monster_death_give_exp", 0xDDEC, en_death_exp);
    reg("bank7_monster_death", 0xE880, en_death);
    reg("bank7_Spawn_New_Projectile", 0xDBCE, en_spawn_proj);
    reg("bank7_spawn_new_bubble_or_rock", 0xDBFB, en_spawn_bubble);
    reg(
        "LE6E8_projectile_disintegrate",
        0xE6E8,
        en_proj_disintegrate,
    );
    // Shared with player_traps ($E677 sword-hit, $E558 shield-gate):
    // listed in ENEMY_TRAPS/ENEMY_TRAPS_SHARED, owned by player_traps —
    // intentionally not registered here (see ENEMY_TRAP_COUNT docs).
    reg("bank7_Enemy_Routines1_Bot", 0xDA0C, en_walker);
    reg("bank7_Enemy_Routines1_Deeler", 0xD6DF, en_flyer);
    reg(
        "bank7_Enemy_Routines1_Raising_Bubbles",
        0xDC15,
        en_generator,
    );
    reg("bank7_Enemy_Routines1_Octorok", 0xD888, en_shooter);
    reg("bank7_Enemy_Routines1_Elevator", 0xD8C2, en_elevator);
    reg("bank7_Enemy_Routines1_Locked_Door", 0xD991, en_locked_door);
    reg("bank7_Simple_Horizontal_Movement", 0xDEB8, en_simple_h);
    reg("bank7_Simple_Vertical_Movement", 0xDEC8, en_simple_v);
    reg("bank7_Gravity", 0xDEBE, en_gravity);
    // Keep the banked-only boss shims referenced so unit coverage stays
    // wired even though they are never registered (aliasing caveat).
    let _ = (
        en_horsehead as fn(&mut Game),
        en_helmethead as fn(&mut Game),
        en_gooma as fn(&mut Game),
        en_floating_helmet as fn(&mut Game),
        en_thunderbird as fn(&mut Game),
        en_dark_link as fn(&mut Game),
        en_sword_hit as fn(&mut Game),
        en_shield_gate as fn(&mut Game),
    );
}
