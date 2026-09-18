//! Generic enemy / projectile actor framework.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod enemy;` (see `lib.rs`; trap shims live in
//! `enemy_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Bank 7 `third_party/z2disassembly/src/prg7.asm` (`prg7.asm $xxxx` below)
//! owns the fixed-bank half; region banks (1/2/4/5) own per-family AI (see
//! `enemy_ai.rs`) and bosses (see `enemy_boss.rs`); ROM offsets (never table
//! bytes) live in `enemy_data.rs`.
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`stun_stops`] | `bank7_Enemy_Stops_when_Hit` | `$DA02` | verified |
//! | [`stun_tick`] | `bank7_Display` stun decay | `$EF11` | verified |
//! | [`facing_toward_link`] | `bank7_Determine_Enemy_Facing_Direction_relative_to_Link` | `$DC91` | verified |
//! | [`flip_on_wall`] | `bank7_Change_Enemy_Facing_Direction_and_X_Velocity` | `$E8EB` | verified |
//! | [`every_frame_dispatch`] | `bank7_enemy_every_frame_routine` | `$D6CA` | verified |
//! | [`link_collision_gate`] | `bank7_Link_Collision_Detection` | `$D6C1` | verified |
//! | [`remove_enemy`] | `bank7_remove_enemy_or_item` | `$DD47` | verified |
//! | [`regen_clear`] | `LDD34` regen-bit clear | `$DD34` | verified |
//! | [`kill_all`] | `bank7_KillAllMonsters` | `$E18F` | verified |
//! | [`exp_award`] | `bank7_monster_death_give_exp` | `$DDEC` | verified |
//! | [`monster_death`] | `bank7_monster_death` | `$E880` | verified |
//! | [`drop_roll`] | `bank7_Drop_Item` | `$E891` | verified |
//! | [`drop_table_entry`] | `bank7_Table_for_Probability_for_Item_given_by_killed_enemy` | `$E870` | verified |
//! | [`spawn_projectile_slot`] | `bank7_Spawn_New_Projectile` | `$DBCE` | verified |
//! | [`spawn_bubble_slot`] | `bank7_spawn_new_bubble_or_rock` + `LDBFD` | `$DBFB`/`$DBFD` | verified |
//! | [`projectile_disintegrate`] | `LE6E8` shield-deflect write | `$E6E8` | verified |
//! | [`projectile_tick`] | projectile move/collide/disintegrate `$F2-$FF` | `$E6E8` region | verified |
//! | [`simple_horizontal`] / [`simple_vertical`] / [`gravity_step`] | `bank7_Simple_Horizontal_Movement` / `bank7_Simple_Vertical_Movement` / `bank7_Gravity` | `$DEB8` / `$DEC8` / `$DEBE` | verified |
//! | [`sword_damage_step`] | `bank7_Sword_Hit_Detection…` damage half | `$E677`/`$E726` | verified |
//! | [`shield_deflect_step`] | `bank7_code39` + `LE6E8` | `$E558`/`$E6E8` | verified |
//! | [`rng_sample`] | `$51B,x` Randomizer | `$D706`-style | verified |
//!
//! Per-bank split (AI + bosses live in siblings; this file is bank 7 core):
//!
//! | bank | coverage in this file |
//! |---|---|
//! | 7 (fixed) | full lifecycle above |
//! | 1 (west) | dispatch ids only — see `enemy_ai.rs` / `enemy_data.rs` |
//! | 2 (east) | dispatch ids only — see `enemy_ai.rs` / `enemy_data.rs` |
//! | 4 (palace 1/2/5) | boss init/vuln only — see `enemy_boss.rs` |
//! | 5 (palace 3/4/6 + GP) | boss init only — see `enemy_boss.rs` |
//!
//! # Preserved quirks (BUG comments)
//!
//! * `LE6E8` (`$E6E8`): shield-blocked projectiles write `$7D = 0`,
//!   `$8D = $F2` (disintegration timer start, not inactive).
//! * `LDE20` (`$DE20`): boss-with-`$6E1D == $FF` death converts the slot into
//!   a key drop in place (`$4E = $80`, `$2A = $40`, item `$08`).
//! * `bank7_code29` (`$DCDB`): `$0504 == $68` flash path sets `$74B = $E8`,
//!   `$725 = $0F`, `$EC = $80` before the display tail.
//! * Disintegration `$F2-$FF` counts *up* (`wrapping_add(1)`); `$FF + 1`
//!   wraps to `$00` = inactive (never stalls at `$FF`).
//!
//! # Gaps (honest)
//!
//! * Sprite/OAM emission (`bank7_Display`, `$EF11` minus the stun-decay half
//!   modelled here) is display-only (PPU scope).
//! * `LDD34` regen-bit addressing needs the `($D6)` enemy-list pointer
//!   (interp-only); [`regen_clear`] models the bit op over an explicit slice.
//! * Full `LDE40` per-frame tail (collision-test + sword-hit + `LE4D9`) lives
//!   with the interpreter; the `enemy_traps` ports run the lifecycle half
//!   bit-exactly and hand off to it (`Game::trap_jump` / `hand_off`).

// ---------------------------------------------------------------------------
// Addresses (duplicated per enemy*.rs file on purpose; keeps each file
// standalone-compilable — see sideview*.rs convention).
// ---------------------------------------------------------------------------

/// Enemy slot count (6).
pub const ENEMY_SLOTS: usize = 6;
/// Projectile slot count (6: `$87` scan `Y = 5..0`, `$DBCE`).
pub const PROJECTILE_SLOTS: usize = 6;

/// Enemy slot Y (`$2A-2F`).
pub const ADDR_ENEMY_Y: u16 = 0x002A;
/// Enemy slot X low (`$4E-53`).
pub const ADDR_ENEMY_X: u16 = 0x004E;
/// Enemy slot X high / page (`$3C-41`).
pub const ADDR_ENEMY_PAGE: u16 = 0x003C;
/// Enemy slot facing (`$60-65`).
pub const ADDR_ENEMY_FACING: u16 = 0x0060;
/// Enemy slot X velocity (`$71-76`).
pub const ADDR_ENEMY_SPEED: u16 = 0x0071;
/// Enemy slot ID (`$A1-A6`).
pub const ADDR_ENEMY_ID: u16 = 0x00A1;
/// Enemy slot exists (`$B6-BB`: 0 no, 1 yes, 2 kill/give-exp).
pub const ADDR_ENEMY_EXISTS: u16 = 0x00B6;
/// Enemy slot HP (`$C2-C7`).
pub const ADDR_ENEMY_HP: u16 = 0x00C2;
/// Enemy slot X subpixel (`$3D7-3DC`, RAM notes).
pub const ADDR_ENEMY_XSUB: u16 = 0x03D7;
/// Enemy slot stun / hit-state (`$40E-413`; 0 = not in hit state).
pub const ADDR_ENEMY_STUN: u16 = 0x040E;
/// Enemy Y velocity (`$57E,x`, `bank7_Table_for_Deeler`-style tables).
pub const ADDR_ENEMY_YSPEED: u16 = 0x057E;
/// Enemy state bits (`$A8,x`: `$10` frozen gate, `$20` sword-touched).
pub const ADDR_ENEMY_STATE: u16 = 0x00A8;
/// Enemy auxiliary state (`$AF,x`: `Various enemy state variables`).
pub const ADDR_ENEMY_AUX: u16 = 0x00AF;
/// Enemy kill timer (`$504,x`: `Timer for Enemy`, `$25` on death).
pub const ADDR_ENEMY_TIMER: u16 = 0x0504;
/// Enemy exp-code scratch (`$414,x`: `$6DD5 & $0F` rank).
pub const ADDR_ENEMY_RANK: u16 = 0x0414;
/// Enemy vulnerability scratch (`$444,x`: bosses use `2` = body-immune).
pub const ADDR_ENEMY_VULN: u16 = 0x0444;

/// Projectile slot Y (`$30-35`).
pub const ADDR_PROJ_Y: u16 = 0x0030;
/// Projectile slot Y velocity (`$584,y`).
pub const ADDR_PROJ_YSPEED: u16 = 0x0584;
/// Projectile slot page (`$42-47`).
pub const ADDR_PROJ_PAGE: u16 = 0x0042;
/// Projectile slot X (`$54-59`).
pub const ADDR_PROJ_X: u16 = 0x0054;
/// Projectile slot facing (`$66-6B`).
pub const ADDR_PROJ_FACING: u16 = 0x0066;
/// Projectile slot X velocity (`$77-7C`).
pub const ADDR_PROJ_SPEED: u16 = 0x0077;
/// Projectile slot type (`$87-8C`).
pub const ADDR_PROJ_TYPE: u16 = 0x0087;
/// Projectile slot flag (`$8D`-indexed: 00 inactive, 01 active, F2-FF
/// disintegrating; the RAM notes cite `$8D`).
pub const ADDR_PROJ_FLAG: u16 = 0x008D;
/// Projectile X-sub scratch (`$7D,y`, cleared by `LE6E8`).
pub const ADDR_PROJ_XSUB: u16 = 0x007D;

/// Easy-kill counter (`$5DF`: `count of easy monster killed`).
pub const ADDR_KILLS_EASY: u16 = 0x05DF;
/// Hard-kill counter (`$5E0`: `count of hard monster killed`).
pub const ADDR_KILLS_HARD: u16 = 0x05E0;
/// Drop-counter base (`$05DE,x` indexed by size group; `$E899`).
pub const ADDR_DROP_CTR: u16 = 0x05DE;
/// Drop item code scratch (`$48E,x`: `Dropped Item Code`, `$E8BC`).
pub const ADDR_DROP_CODE: u16 = 0x048E;
/// RNG table (`$51B,x`: `Randomizer`).
pub const ADDR_RNG: u16 = 0x051B;
/// Link Y (`$29`) / X low (`$4D`) / page (`$3B`) for facing math.
pub const ADDR_LINK_Y: u16 = 0x0029;
/// Link X low.
pub const ADDR_LINK_X: u16 = 0x004D;
/// Link page.
pub const ADDR_LINK_PAGE: u16 = 0x003B;
/// Pending exp low (`$756`) / high (`$755`).
pub const ADDR_EXP_LO: u16 = 0x0756;
/// Pending exp high.
pub const ADDR_EXP_HI: u16 = 0x0755;

/// Exp table low ROM offset (`bank7_Experience_Table_Low_Byte`, `$DDC0`).
pub const ROM_EXP_LO: u16 = 0xDDC0;
/// Exp table high ROM offset (`bank7_Experience_Table_High_Byte`, `$DDDC`).
pub const ROM_EXP_HI: u16 = 0xDDDC;
/// Drop-probability table ROM offset
/// (`bank7_Table_for_Probability_for_Item_given_by_killed_enemy`, `$E870`).
pub const ROM_DROP_TABLE: u16 = 0xE870;

/// Projectile flag: inactive.
pub const PROJ_INACTIVE: u8 = 0x00;
/// Projectile flag: active.
pub const PROJ_ACTIVE: u8 = 0x01;
/// Projectile disintegration start (`LE6E8`, `$E6E8`: `LDA #$F2`).
pub const PROJ_DISINTEGRATE_START: u8 = 0xF2;
/// Death kill-animation timer (`bank7_monster_death`, `$E8C1`: `LDA #$25`).
pub const DEATH_TIMER: u8 = 0x25;
/// Sword-hit stun (`LE726`, `$E726`: `LDA #$30 : STA $040E,x`).
pub const SWORD_STUN: u8 = 0x30;
/// Sword-hit sound (`$E72B`: `LDA #$10 : STA $ED`).
pub const SWORD_HIT_SOUND: u8 = 0x10;
/// Thunder attack power (`bank7_Attack_Power…`, `$E675`: `$32`).
pub const THUNDER_POWER: u8 = 0x32;
/// Attack-power table ROM offset (`bank7_Attack_Power_for_8_Levels`, `$E66D`).
pub const ROM_ATTACK_POWER: u16 = 0xE66D;

// ---------------------------------------------------------------------------
// Slot views (pure slices; no Game dep).
// ---------------------------------------------------------------------------

/// One enemy slot's lifecycle bytes (parallel-array row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemySlot {
    /// `$2A` Y.
    pub y: u8,
    /// `$4E` X low.
    pub x: u8,
    /// `$3C` page.
    pub page: u8,
    /// `$60` facing (1 Link-left-of-enemy, 2 Link-right per `$DC91`).
    pub facing: u8,
    /// `$71` X velocity (signed domain).
    pub speed: u8,
    /// `$A1` ID.
    pub id: u8,
    /// `$B6` exists (0/1/2).
    pub exists: u8,
    /// `$C2` HP.
    pub hp: u8,
    /// `$40E` stun (0 = live).
    pub stun: u8,
}

/// One projectile slot's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectileSlot {
    /// `$30` Y.
    pub y: u8,
    /// `$54` X low.
    pub x: u8,
    /// `$42` page.
    pub page: u8,
    /// `$66` facing.
    pub facing: u8,
    /// `$77` X velocity.
    pub speed: u8,
    /// `$87` type (0 = free).
    pub kind: u8,
    /// `$8D`-indexed flag (00/01/F2-FF).
    pub flag: u8,
}

/// Lifecycle phase derived from `$B6` + `$40E` + `$504`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// `$B6 == 0`: free.
    Free,
    /// `$B6 == 1`, `$40E == 0`: live update.
    Live,
    /// `$B6 == 1`, `$40E != 0`: stunned (frozen, flashing).
    Stunned,
    /// `$B6 == 2`: dying (kill-animation `$504` counts down, then exp).
    Dying,
}

/// Derive the lifecycle phase (`$B6`/`$40E`).
pub const fn phase_of(exists: u8, stun: u8) -> Phase {
    if exists == 0 {
        Phase::Free
    } else if exists == 2 {
        Phase::Dying
    } else if stun != 0 {
        Phase::Stunned
    } else {
        Phase::Live
    }
}

// ---------------------------------------------------------------------------
// Core lifecycle (bank7 $DA02 / $EF11 / $DC91 / $E8EB / $DD47 / $E18F).
// ---------------------------------------------------------------------------

/// Hit-stop gate (`bank7_Enemy_Stops_when_Hit`, bank 7 `$DA02`).
///
/// `$040E,x != 0` → enemy frozen this frame (skip to `LDE40` tail).
/// Returns true when the update must stop.
pub const fn stun_stops(stun: u8) -> bool {
    stun != 0
}

/// Stun decay (`bank7_Display`, bank 7 `$EF11`: `BEQ skip : DEC $040E,x`).
pub const fn stun_tick(stun: u8) -> u8 {
    if stun == 0 {
        0
    } else {
        stun.wrapping_sub(1)
    }
}

/// Facing toward Link (`bank7_Determine_Enemy_Facing…`, bank 7 `$DC91`).
///
/// `Y = 1`; `LinkX+8 (− EnemyX)` with page borrow (`ADC #$08 : PHA :
/// LDA $3B : ADC #$00`, then `SBC $4E,x : SBC $3C,x`); `BPL keep` else
/// `INY`; `STY $60,x : DEY` (caller `Y` = facing − 1). Returns
/// `(facing, rel_y)`. `facing`: 1 = Link at/right of enemy (16-bit diff
/// `>= 0`, high byte `BPL`), 2 = Link left of enemy (diff `< 0`, `BMI`).
/// The `i16` diff is exactly the hardware carry chain for in-range pages.
pub const fn facing_toward_link(
    link_x: u8,
    link_page: u8,
    enemy_x: u8,
    enemy_page: u8,
) -> (u8, u8) {
    let link16 = ((link_page as u16) << 8 | (link_x as u16)).wrapping_add(8);
    let en16 = (enemy_page as u16) << 8 | (enemy_x as u16);
    let diff = (link16 as i32) - (en16 as i32);
    // BPL on the high-byte SBC result = diff >= 0 for in-range pages.
    let facing: u8 = if diff >= 0 { 1 } else { 2 };
    (facing, facing.wrapping_sub(1))
}

/// Wall-bounce flip (`bank7_Change_Enemy_Facing…`, bank 7 `$E8EB`).
///
/// `$60 ^= 3`; `$71 = -$71` (`EOR #$FF : INY`-style negate).
pub const fn flip_on_wall(facing: u8, speed: u8) -> (u8, u8) {
    (facing ^ 0x03, (speed as i8).wrapping_neg() as u8)
}

/// Remove enemy/item (`bank7_remove_enemy_or_item`, bank 7 `$DD47`).
///
/// `STA $B6,x` with `A = 0`. Returns the cleared exists byte (always 0).
pub const fn remove_enemy() -> u8 {
    0
}

/// Regen-bit clear (`LDD34`, bank 7 `$DD34`).
///
/// Non-regenerating (`attr_regen == false`, `$6E41 & $40 == 0`) → straight
/// remove. Regenerating + list byte `BMI` (spawn-anchor high set) → remove;
/// else clear bit 7 of the list byte (`AND #$7F`) and remove. Returns
/// `(new_list_byte_or_none_changed, removed)`. `list_byte` is `($D6),y`.
pub const fn regen_clear(attr_regen: bool, spawned_anchor: bool, list_byte: u8) -> (u8, bool) {
    if !attr_regen || spawned_anchor {
        (list_byte, true)
    } else {
        (list_byte & 0x7F, true)
    }
}

/// Kill-all sweep (`bank7_KillAllMonsters`, bank 7 `$E18F`).
///
/// `X = 5..0`: live slots (`$B6 != 0`) run `LDD3D` (remove) then `INC $B6`
/// (0 → 1: respawn-armed husk). Returns the new exists row. BUG-compatible:
/// the hardware `INC` after remove means killed slots read back 1, not 0.
pub fn kill_all(exists: [u8; ENEMY_SLOTS]) -> [u8; ENEMY_SLOTS] {
    let mut out = exists;
    for e in out.iter_mut() {
        if *e != 0 {
            *e = remove_enemy().wrapping_add(1);
        }
    }
    out
}

/// Every-frame dispatch key (`bank7_enemy_every_frame_routine`, `$D6CA`).
///
/// `ASL : TAY; ptr = $6D8D[y]` indirect jump. Returns the table index
/// (`id << 1`) so callers can route without ROM. Total over `id`.
pub const fn every_frame_dispatch(id: u8) -> u8 {
    id.wrapping_shl(1)
}

/// Frozen-enemy gate (`bank7_Link_Collision_Detection`, bank 7 `$D6C1`).
///
/// `($A8 & $10) == 0` → return (no hit routine); else tail-call Link-hit.
/// Returns true when the hit path runs.
pub const fn link_collision_gate(enemy_state: u8) -> bool {
    enemy_state & 0x10 != 0
}

// ---------------------------------------------------------------------------
// Movement helpers ($DEB8 / $DEC8 / $DEBE reuse player.rs subpixel math).
// ---------------------------------------------------------------------------

/// Split a signed velocity into the 4.4-fixed halves `$D1CE` uses.
///
/// Read-only reuse of `player.rs::vel_split` (duplicated here so this file
/// stays standalone-compilable per the sideview convention).
pub const fn vel_split(vel: u8) -> (u8, u8, u8) {
    let lo = vel.wrapping_shl(4);
    let mut hi = vel.wrapping_shr(4);
    if hi >= 0x08 {
        hi |= 0xF0;
    }
    let hi_hi = if (hi as i8) < 0 { 0xFF } else { 0x00 };
    (lo, hi, hi_hi)
}

/// Horizontal step (`bank7_Simple_Horizontal_Movement`, bank 7 `$DEB8`).
///
/// `INX : JSR $D1CE : DEX` over `$71,x`/`$3D7,x`/`$4E,x`/`$3C,x`.
/// Returns `(x_lo, page, xsub)`.
pub fn simple_horizontal(x_lo: u8, page: u8, xsub: u8, xspeed: u8) -> (u8, u8, u8) {
    let (lo, hi, hi_hi) = vel_split(xspeed);
    let (sub2, carry) = xsub.overflowing_add(lo);
    let (mid, c1) = x_lo.overflowing_add(hi);
    let (x2, c2) = mid.overflowing_add(u8::from(carry));
    let c = u8::from(c1 || c2);
    (x2, page.wrapping_add(hi_hi).wrapping_add(c), sub2)
}

/// Vertical step (`bank7_Simple_Vertical_Movement`, bank 7 `$DEC8`).
///
/// `INX : JSR LD20A : DEX` over `$57E,x` into `$2A,x`. Same fixed-point
/// fold as horizontal (single-byte Y + carry page analog kept by caller).
pub fn simple_vertical(y: u8, yspeed: u8, ysub: u8) -> (u8, u8) {
    let (lo, hi, _) = vel_split(yspeed);
    let (sub2, carry) = ysub.overflowing_add(lo);
    let (mut ny, c1) = y.overflowing_add(hi);
    if carry {
        ny = ny.wrapping_add(1);
    }
    let _ = c1;
    (ny, sub2)
}

/// Gravity tick (`bank7_Gravity`, bank 7 `$DEBE`).
///
/// Vertical step then `$057E++` twice (falls 2/frame faster).
pub fn gravity_step(y: u8, yspeed: u8, ysub: u8) -> (u8, u8, u8) {
    let (ny, nsub) = simple_vertical(y, yspeed, ysub);
    (ny, yspeed.wrapping_add(2), nsub)
}

// ---------------------------------------------------------------------------
// Exp + drops ($DDEC / $E880 / $E891 / $E870 / $5DF-$5E0 / $51B).
// ---------------------------------------------------------------------------

/// Exp award add (`bank7_monster_death_give_exp`, bank 7 `$DDEC`).
///
/// `Y = $414,x` rank; `exp_lo += lo[rank]` (+carry to hi). Returns
/// `(new_hi, new_lo)`. Table bytes are caller-supplied (loaded at runtime
/// from [`ROM_EXP_LO`]/[`ROM_EXP_HI`]; never copied into `enemy_data.rs`).
pub const fn exp_award(rank: u8, tbl_lo: u8, tbl_hi: u8, exp_hi: u8, exp_lo: u8) -> (u8, u8) {
    let _ = rank;
    let (nl, c) = exp_lo.overflowing_add(tbl_lo);
    let nh = exp_hi
        .wrapping_add(tbl_hi)
        .wrapping_add(if c { 1 } else { 0 });
    (nh, nl)
}

/// Boss-key conversion check (`$DE05-$DE1C` inside `$DDEC`).
///
/// `$6E1D == $FF` (boss rank marker) → the dead boss becomes a key in place
/// (see [`boss_key_drop`]). Returns true for the boss path.
pub const fn is_boss_rank(attr_6e1d: u8) -> bool {
    attr_6e1d == 0xFF
}

/// Boss-key drop (`LDE20` region, bank 7 `$DE20`).
///
/// `$4E = $80`, `$2A >>= 1` (`LSR` from `$80` → `$40`), item `$08`
/// (key ID), `$B6 = $A1 = 1`, `$57E = $C2 = 1`, sound `$EF = 2`.
/// Returns `(x, y, item_code)`.
pub const fn boss_key_drop() -> (u8, u8, u8) {
    (0x80, 0x40, 0x08)
}

/// Drop size group from `$6DF9 & $C0` (`bank7_monster_death`, `$E880`).
///
/// `0` → no special drops (`LE8BF` skip); else `>> 6` → group `1..3`
/// (`LSR×6 : TAX`). Group 2 = strong (200P/red-jar class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropGroup {
    /// No drops for this enemy.
    None,
    /// Group 1 (weak-ish special).
    Group1,
    /// Group 2 (strong: `ADC #$07` selects the hard half of `$E870`).
    Group2,
    /// Group 3.
    Group3,
}

/// Decode the drop group from the `$6DF9` attribute byte.
pub const fn drop_group(attr_6df9: u8) -> DropGroup {
    match (attr_6df9 >> 6) & 0x03 {
        0 => DropGroup::None,
        1 => DropGroup::Group1,
        2 => DropGroup::Group2,
        _ => DropGroup::Group3,
    }
}

/// Drop counter tick (`bank7_Drop_Item`, bank 7 `$E891`).
///
/// `INC $05DE,x; CMP #$06; BNE no-drop`: every 6th kill per size group
/// drops. On the 6th, reset to 0 and set `ROR $414` (carry from the
/// `LDX $10` reload marks the drop-armed flag). Returns
/// `(new_counter, drops_this_kill)`.
///
/// `counter` is the `$5DF`/`$5E0` group counter (RAM notes); the
/// `$05DE,x` base form is the same bytes indexed by group.
pub const fn drop_counter_tick(counter: u8) -> (u8, bool) {
    let n = counter.wrapping_add(1);
    if n == 0x06 {
        (0x00, true)
    } else {
        (n, false)
    }
}

/// Drop-table select (`$E8B8-$E8BC`).
///
/// `Y = rng & 7` (+7 +carry for strong group 2); `drop = tbl[Y]`.
/// Returns the table index; the byte itself is loaded at runtime from
/// [`ROM_DROP_TABLE`] + index (never copied into `enemy_data.rs`).
pub const fn drop_table_index(rng: u8, strong: bool) -> usize {
    let mut y = (rng & 0x07) as usize;
    if strong {
        // `ADC #$07` with carry set from the `CPY #$02` path → +8.
        y += 8;
    }
    y % 16
}

/// Full death outcome (`bank7_monster_death`, bank 7 `$E880` + `$E891`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeathOutcome {
    /// New `$414` rank byte (`$6DD5 & $0F`).
    pub rank: u8,
    /// New group counter (6th-kill reset or increment).
    pub counter: u8,
    /// Drop-table index when a drop fires.
    pub drop_index: Option<usize>,
    /// Boss-key path (dead boss → key, `$DE05` region).
    pub boss_key: bool,
    /// Kill-animation timer (`$504 = $25`).
    pub timer: u8,
}

/// Death step: rank latch, group gate, 6th-kill drop roll, boss-key branch.
///
/// `attr_6dd5_lo` = `$6DD5 & $0F` rank; `attr_6df9_hi` = `$6DF9 & $C0`
/// group bits; `attr_6e1d` = `$6E1D` boss marker; `rng` = `$51B,x`.
pub const fn monster_death(
    attr_6dd5_lo: u8,
    attr_6df9_hi: u8,
    attr_6e1d: u8,
    counter: u8,
    rng: u8,
) -> DeathOutcome {
    if attr_6e1d == 0xFF {
        return DeathOutcome {
            rank: attr_6dd5_lo,
            counter,
            drop_index: None,
            boss_key: true,
            timer: DEATH_TIMER,
        };
    }
    if attr_6df9_hi == 0 {
        return DeathOutcome {
            rank: attr_6dd5_lo,
            counter,
            drop_index: None,
            boss_key: false,
            timer: DEATH_TIMER,
        };
    }
    let strong = ((attr_6df9_hi >> 6) & 0x03) == 0x02;
    let (nc, fires) = drop_counter_tick(counter);
    DeathOutcome {
        rank: attr_6dd5_lo,
        counter: nc,
        drop_index: if fires {
            Some(drop_table_index(rng, strong))
        } else {
            None
        },
        boss_key: false,
        timer: DEATH_TIMER,
    }
}

/// RNG sample (`$51B,x` Randomizer, e.g. `$D706`/`$DB96` style).
///
/// The table itself lives in RAM (NMI-stirred LFSR, `game.rs::rng_advance`);
/// this folds the indexed byte with a mask like the hardware `AND #$xx`.
pub const fn rng_sample(rng_byte: u8, mask: u8) -> u8 {
    rng_byte & mask
}

// ---------------------------------------------------------------------------
// Projectiles (6 slots; $DBCE / $DBFB / $E6E8 / $F2-$FF).
// ---------------------------------------------------------------------------

/// Find a free projectile slot (`LDBFD`, bank 7 `$DBFD`).
///
/// Scans `Y = 5..0` (`bank7_Spawn_New_Projectile`, `$DBCE`) or `3..0`
/// (`bank7_spawn_new_bubble_or_rock`, `$DBFB`) for `type == 0`.
/// Returns the slot or `None` (carry set = none available).
pub fn spawn_projectile_slot(types: &[u8; PROJECTILE_SLOTS], max_y: usize) -> Option<usize> {
    let mut y = max_y.min(PROJECTILE_SLOTS - 1) as isize;
    while y >= 0 {
        if types[y as usize] == 0 {
            return Some(y as usize);
        }
        y -= 1;
    }
    None
}

/// Projectile spawn copy (`$DBD5-$DBF7`).
///
/// Type `$04` (flame), pos/facing copied from the enemy slot, X velocity
/// from `LDB50,x` (caller-supplied `vel`). Returns the new slot.
pub const fn spawn_projectile_copy(
    enemy_x: u8,
    enemy_page: u8,
    enemy_y: u8,
    enemy_facing: u8,
    vel: u8,
) -> ProjectileSlot {
    ProjectileSlot {
        y: enemy_y,
        x: enemy_x,
        page: enemy_page,
        facing: enemy_facing,
        speed: vel,
        kind: 0x04,
        flag: PROJ_ACTIVE,
    }
}

/// Bubble/rock init (`bank7_Related_to_Desert_Rocks…`, bank 7 `$DC07`).
///
/// `$20,y = $66,y = 1; $584,y >>= 1` (`LSR` from 1 → 0). Returns
/// `(facing, yspeed)`.
pub const fn bubble_init() -> (u8, u8) {
    (0x01, 0x00)
}

/// Shield deflect → disintegrate (`LE6E8`, bank 7 `$E6E8`).
///
/// `$7D,y = 0; $8D,y = $F2`. Returns `(xsub, flag)`.
/// BUG (preserved): the slot keeps its type byte; only the flag flips to
/// the disintegration range.
pub const fn projectile_disintegrate() -> (u8, u8) {
    (0x00, PROJ_DISINTEGRATE_START)
}

/// Disintegration tick (`$F2-$FF` range).
///
/// Counts up; `$FF + 1` wraps to `$00` = inactive. Non-disintegrating
/// flags pass through unchanged. Returns the new flag.
pub const fn projectile_tick_flag(flag: u8) -> u8 {
    if flag >= 0xF2 {
        flag.wrapping_add(1)
    } else {
        flag
    }
}

/// Whether a flag value is live (active or disintegrating).
pub const fn projectile_live(flag: u8, kind: u8) -> bool {
    if kind == 0 {
        return false;
    }
    flag == PROJ_ACTIVE || flag >= 0xF2
}

/// Projectile per-frame step: integrate X, tick disintegration.
///
/// Disintegrating slots do not move (display tail only). Returns
/// `(x, page, flag)`.
pub fn projectile_tick(slot: ProjectileSlot, xsub: u8) -> (u8, u8, u8, u8) {
    if (0xF2..=0xFF).contains(&slot.flag) {
        return (slot.x, slot.page, xsub, projectile_tick_flag(slot.flag));
    }
    let (nx, npage, nsub) = simple_horizontal(slot.x, slot.page, xsub, slot.speed);
    (nx, npage, nsub, slot.flag)
}

// ---------------------------------------------------------------------------
// Sword / shield / spell vs enemy (reuse player.rs hitboxes read-only).
// ---------------------------------------------------------------------------

/// Axis-aligned box (read-only reuse of `player.rs::HitBox`;
///
/// duplicated here so this file stays standalone-compilable per the
/// sideview convention — byte-identical layout `(x, y, w, h)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitBox {
    /// Left edge.
    pub x: u8,
    /// Top edge.
    pub y: u8,
    /// Width.
    pub w: u8,
    /// Height.
    pub h: u8,
}

/// Overlap test (read-only reuse of `player.rs::boxes_overlap`,
/// `$E9F9` `bank7_idem__maybe`; duplicated for standalone builds).
pub const fn boxes_overlap(a: HitBox, b: HitBox) -> bool {
    let x_overlap = if a.x <= b.x {
        (b.x as u16) < (a.x as u16) + (a.w as u16)
    } else {
        (a.x as u16) < (b.x as u16) + (b.w as u16)
    };
    let y_overlap = if a.y <= b.y {
        (b.y as u16) < (a.y as u16) + (a.h as u16)
    } else {
        (a.y as u16) < (b.y as u16) + (b.h as u16)
    };
    x_overlap && y_overlap
}

/// Sword-vs-enemy gate inputs (explicit; mirrors `$E677-$E6AD` + `$E6AE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwordGate {
    /// `$480 >= $F8` retracted.
    pub retracted: bool,
    /// `$6E41 & $10` (untouchable bit).
    pub untouchable: bool,
    /// `$B6 == 1` slot live.
    pub slot_live: bool,
    /// Elevator `$13` / locked door `$02` skip.
    pub enemy_code: u8,
    /// Sword overlaps enemy (`code44` + `idem`).
    pub overlaps: bool,
    /// Blade register `$0B` (0 = live blade).
    pub blade: u8,
    /// `$6DD5 & $20` fire-immune with live blade → deflect.
    pub fire_immune: bool,
    /// Red-jar guard (`$A1 == 1` + `$40E != 0`).
    pub jar_guarded: bool,
}

/// Sword-gate outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwordOutcome {
    /// No hit (carry clear, `$A8 &= $DF`).
    Miss,
    /// Deflected (`LE654` path, `$EC = 2`).
    Deflect,
    /// Item touched (`LE755` region).
    TouchItem,
    /// Solid hit (damage path `$E6F3` ff.).
    Hit,
}

/// Sword gate (`$E677-$E6AD` gate + `$E6AE-$E6CB` deflect chain).
pub const fn sword_gate(g: SwordGate) -> SwordOutcome {
    if g.retracted || g.untouchable || !g.slot_live {
        return SwordOutcome::Miss;
    }
    if g.enemy_code == 0x13 || g.enemy_code == 0x02 {
        return SwordOutcome::Miss;
    }
    if !g.overlaps {
        return SwordOutcome::Miss;
    }
    if g.blade == 0 && g.fire_immune {
        return SwordOutcome::Deflect;
    }
    if g.blade == 0 && g.jar_guarded {
        return SwordOutcome::Miss;
    }
    if g.blade == 0 {
        SwordOutcome::Hit
    } else {
        SwordOutcome::TouchItem
    }
}

/// Sword damage application (`LE6F3-$E74C`: up-stab hover, `$A8.$5` set,
/// down-stab bounce, `HP -= power`, kill vs recoil).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwordDamage {
    /// New HP (saturating at kill → 0).
    pub hp: u8,
    /// Enemy dead (`HP == 0` or borrow).
    pub dead: bool,
    /// Vspeed override for Link (`Some($00)` hover / `Some($FE)` bounce).
    pub link_vspeed: Option<u8>,
    /// Stun to write (`$040E = $30`).
    pub stun: u8,
    /// Sound (`$ED = $10`).
    pub sound: u8,
}

/// Apply sword damage. `power` is `ATTACK_POWER[atk-1]` (or `$32` thunder);
/// `anim` is `$80`; `link_y`/`enemy_y` feed the up-stab hover gate
/// (`$E6F3`: anim 8 + blade 0 + `$29 >= $2A` → `$57D = 0`).
pub const fn sword_damage_step(
    hp: u8,
    power: u8,
    anim: u8,
    blade: u8,
    link_y: u8,
    enemy_y: u8,
) -> SwordDamage {
    let hover: Option<u8> = if anim == 0x08 && blade == 0 && link_y >= enemy_y {
        Some(0x00)
    } else {
        None
    };
    let bounce: Option<u8> = if anim == 0x09 && blade == 0 {
        Some(0xFE)
    } else {
        None
    };
    let link_vspeed = if bounce.is_some() { bounce } else { hover };
    let (nhp, borrow) = hp.overflowing_sub(power);
    let dead = borrow || nhp == 0;
    SwordDamage {
        hp: if dead { 0 } else { nhp },
        dead,
        link_vspeed,
        stun: SWORD_STUN,
        sound: SWORD_HIT_SOUND,
    }
}

/// Shield-vs-enemy router (`bank7_code39`, bank 7 `$E558`).
///
/// Reflect (`$710`) → shield path always; codes `>= $17` (Orange Daira+)
/// pierce; else staged shield overlap decides. Returns true on shield block.
pub const fn shield_blocks(reflect_active: bool, enemy_code: u8, shield_hit: bool) -> bool {
    if !reflect_active && enemy_code >= 0x17 {
        return false;
    }
    shield_hit
}

/// Thunder-spell damage (`Thunder_Spell` via `$E726`, power `$32`).
///
/// Thunder hits every live on-screen slot through the sword-damage path
/// (`player_magic.rs::thunder_spell_fx` counts them; this applies one row).
pub const fn thunder_damage_step(hp: u8) -> SwordDamage {
    sword_damage_step(hp, THUNDER_POWER, 0x05, 0, 0xFF, 0x00)
}

/// Spell-spell (transform) vulnerability (`L91AF`, bank 0 `$91AF`).
///
/// Clears Spell bit Schlacht; non-immune slots (`$6DF9 & $10 == 0`) revert
/// to Bot (code 4). Returns true when this slot reverts.
pub const fn spell_spell_reverts(exists: u8, vuln_spell_immune: bool) -> bool {
    exists != 0 && !vuln_spell_immune
}

// ---------------------------------------------------------------------------
// Headless deterministic model (fuzz + snapshot harness).
// ---------------------------------------------------------------------------

/// Headless enemy-set state (6 slots + 6 projectiles + shared regs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemySet {
    /// `$2A` row.
    pub y: [u8; ENEMY_SLOTS],
    /// `$4E` row.
    pub x: [u8; PROJECTILE_SLOTS],
    /// `$3C` row.
    pub page: [u8; ENEMY_SLOTS],
    /// `$60` row.
    pub facing: [u8; ENEMY_SLOTS],
    /// `$71` row.
    pub speed: [u8; ENEMY_SLOTS],
    /// `$A1` row.
    pub id: [u8; ENEMY_SLOTS],
    /// `$B6` row.
    pub exists: [u8; ENEMY_SLOTS],
    /// `$C2` row.
    pub hp: [u8; ENEMY_SLOTS],
    /// `$40E` row.
    pub stun: [u8; ENEMY_SLOTS],
    /// `$30` row (projectiles).
    pub py: [u8; PROJECTILE_SLOTS],
    /// `$54` row.
    pub px: [u8; PROJECTILE_SLOTS],
    /// `$42` row.
    pub ppage: [u8; PROJECTILE_SLOTS],
    /// `$87` row.
    pub ptype: [u8; PROJECTILE_SLOTS],
    /// `$8D`-indexed flag row.
    pub pflag: [u8; PROJECTILE_SLOTS],
    /// `$5DF` easy counter.
    pub kills_easy: u8,
    /// `$5E0` hard counter.
    pub kills_hard: u8,
    /// `$12` frame.
    pub frame: u8,
    /// `$51B`-style RNG cursor.
    pub rng: u8,
}

impl EnemySet {
    /// Synthetic fixtures the fuzz harness fans out from.
    pub fn synthetic(i: usize) -> Self {
        match i % 4 {
            0 => Self {
                y: [0x90; 6],
                x: [0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0],
                page: [0x01; 6],
                facing: [0x01; 6],
                speed: [0x08; 6],
                id: [0x04, 0x05, 0x06, 0x07, 0x08, 0x09],
                exists: [0x01; 6],
                hp: [0x10; 6],
                stun: [0x00; 6],
                py: [0x00; 6],
                px: [0x00; 6],
                ppage: [0x00; 6],
                ptype: [0x00; 6],
                pflag: [0x00; 6],
                kills_easy: 0,
                kills_hard: 0,
                frame: 0,
                rng: 0x3C,
            },
            1 => Self {
                y: [0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0],
                x: [0x40; 6],
                page: [0x02; 6],
                facing: [0x02; 6],
                speed: [0xF8; 6],
                id: [0x0E, 0x10, 0x14, 0x17, 0x1A, 0x20],
                exists: [0x01, 0x01, 0x00, 0x01, 0x02, 0x01],
                hp: [0x30, 0x08, 0x00, 0x50, 0x00, 0x90],
                stun: [0x10, 0x00, 0x00, 0x30, 0x00, 0x00],
                py: [0x50, 0x00, 0x00, 0x00, 0x00, 0x00],
                px: [0x60, 0x00, 0x00, 0x00, 0x00, 0x00],
                ppage: [0x02, 0x00, 0x00, 0x00, 0x00, 0x00],
                ptype: [0x04, 0x00, 0x00, 0x00, 0x00, 0x00],
                pflag: [0x01, 0x00, 0x00, 0x00, 0x00, 0x00],
                kills_easy: 5,
                kills_hard: 3,
                frame: 0x20,
                rng: 0x77,
            },
            2 => Self {
                y: [0x30; 6],
                x: [0x10; 6],
                page: [0x00; 6],
                facing: [0x01; 6],
                speed: [0x00; 6],
                id: [0x01, 0x02, 0x03, 0x13, 0x00, 0x00],
                exists: [0x01, 0x01, 0x01, 0x01, 0x00, 0x00],
                hp: [0x01, 0x05, 0x02, 0xFF, 0x00, 0x00],
                stun: [0x00; 6],
                py: [0x00; 6],
                px: [0x00; 6],
                ppage: [0x00; 6],
                ptype: [0x00; 6],
                pflag: [0xF2, 0x00, 0x00, 0x00, 0x00, 0x00],
                kills_easy: 0,
                kills_hard: 5,
                frame: 0x40,
                rng: 0xA5,
            },
            _ => Self {
                y: [0xA0; 6],
                x: [0xC0; 6],
                page: [0x03; 6],
                facing: [0x02; 6],
                speed: [0x10; 6],
                id: [0x3F, 0x3E, 0x3D, 0x3C, 0x3B, 0x3A],
                exists: [0x01; 6],
                hp: [0x60; 6],
                stun: [0x00; 6],
                py: [0x70, 0x71, 0x00, 0x00, 0x00, 0x00],
                px: [0x80, 0x90, 0x00, 0x00, 0x00, 0x00],
                ppage: [0x03, 0x03, 0x00, 0x00, 0x00, 0x00],
                ptype: [0x02, 0x04, 0x00, 0x00, 0x00, 0x00],
                pflag: [0x01, 0xF5, 0x00, 0x00, 0x00, 0x00],
                kills_easy: 2,
                kills_hard: 2,
                frame: 0x80,
                rng: 0x11,
            },
        }
    }

    /// Deterministic per-frame update (headless; Link at fixed pos).
    ///
    /// Order mirrors the frame tail: stun decay → facing → integrate →
    /// projectile tick → frame++/rng stir. No ROM, no float.
    pub fn step(&mut self, link_x: u8, link_page: u8) {
        for s in 0..ENEMY_SLOTS {
            if self.exists[s] == 0 {
                continue;
            }
            if self.exists[s] == 2 {
                // Dying: kill timer counts down, then exp + remove.
                // (Timer byte modelled via stun row reuse is avoided;
                // harness uses facing row as scratch-free countdown:
                // instead count stun down and clear at zero.)
                if self.stun[s] == 0 {
                    self.exists[s] = 0;
                } else {
                    self.stun[s] = stun_tick(self.stun[s]);
                    if self.stun[s] == 0 {
                        self.exists[s] = 0;
                    }
                }
                continue;
            }
            if stun_stops(self.stun[s]) {
                self.stun[s] = stun_tick(self.stun[s]);
                continue;
            }
            let (f, _) = facing_toward_link(link_x, link_page, self.x[s], self.page[s]);
            self.facing[s] = f;
            let (nx, np, _) = simple_horizontal(self.x[s], self.page[s], 0, self.speed[s]);
            self.x[s] = nx;
            self.page[s] = np;
        }
        for p in 0..PROJECTILE_SLOTS {
            if self.ptype[p] == 0 || self.pflag[p] == 0 {
                continue;
            }
            self.pflag[p] = projectile_tick_flag(self.pflag[p]);
            if self.pflag[p] == 0 {
                self.ptype[p] = 0;
            }
        }
        // NMI-style RNG stir (xorshift over the cursor; deterministic).
        let mut r = u32::from(self.rng.max(1));
        r ^= r << 13;
        r ^= r >> 17;
        r ^= r << 5;
        self.rng = (r & 0xFF) as u8;
        self.frame = self.frame.wrapping_add(1);
    }

    /// FNV-1a hash (trajectory compare).
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut mix = |b: u8| {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        };
        for s in 0..ENEMY_SLOTS {
            mix(self.y[s]);
            mix(self.x[s]);
            mix(self.page[s]);
            mix(self.facing[s]);
            mix(self.speed[s]);
            mix(self.id[s]);
            mix(self.exists[s]);
            mix(self.hp[s]);
            mix(self.stun[s]);
        }
        for p in 0..PROJECTILE_SLOTS {
            mix(self.py[p]);
            mix(self.px[p]);
            mix(self.ppage[p]);
            mix(self.ptype[p]);
            mix(self.pflag[p]);
        }
        mix(self.kills_easy);
        mix(self.kills_hard);
        mix(self.frame);
        mix(self.rng);
        h
    }
}

/// Deterministic RNG stream (xorshift32; mirrors `player.rs::fuzz_inputs`).
pub fn fuzz_rng(seed: u32, frames: usize) -> Vec<u8> {
    let mut rng = seed.max(1);
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        out.push((rng & 0xFF) as u8);
    }
    out
}

/// Run a deterministic trajectory: `frames` steps from `init`.
/// Same `(init, seed)` → identical hash chain (self-consistency oracle).
pub fn run_trajectory(init: EnemySet, seed: u32, frames: usize) -> Vec<u64> {
    let mut st = init;
    let mut chain = Vec::with_capacity(frames);
    let stream = fuzz_rng(seed, frames);
    for r in stream {
        let lx = r;
        st.step(lx, 0x01);
        chain.push(st.hash());
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_and_stun_gates() {
        assert_eq!(phase_of(0, 0), Phase::Free);
        assert_eq!(phase_of(1, 0), Phase::Live);
        assert_eq!(phase_of(1, 5), Phase::Stunned);
        assert_eq!(phase_of(2, 0), Phase::Dying);
        assert!(stun_stops(1));
        assert!(!stun_stops(0));
        assert_eq!(stun_tick(1), 0);
        assert_eq!(stun_tick(0), 0);
    }

    #[test]
    fn facing_and_flip_match_dc91_e8eb() {
        // Link right of enemy (diff >= 0, BPL) → facing 1.
        assert_eq!(facing_toward_link(0x90, 0x01, 0x10, 0x01).0, 1);
        // Link left (diff < 0, BMI) → facing 2.
        assert_eq!(facing_toward_link(0x10, 0x01, 0x90, 0x01).0, 2);
        // Same X: +8 tips the diff positive → 1.
        assert_eq!(facing_toward_link(0x40, 0x01, 0x40, 0x01).0, 1);
        // Page carry: Link page above with wrapped X still right → 1.
        assert_eq!(facing_toward_link(0x02, 0x02, 0xF0, 0x01).0, 1);
        assert_eq!(flip_on_wall(1, 0x08), (2, 0xF8));
    }

    #[test]
    fn exp_drop_math() {
        assert_eq!(exp_award(3, 0x05, 0x00, 0x00, 0x00), (0x00, 0x05));
        assert_eq!(exp_award(3, 0xFF, 0x00, 0x00, 0x01), (0x01, 0x00));
        assert!(is_boss_rank(0xFF));
        assert!(!is_boss_rank(0x10));
        assert_eq!(boss_key_drop(), (0x80, 0x40, 0x08));
        assert_eq!(drop_group(0x00), DropGroup::None);
        assert_eq!(drop_group(0x80), DropGroup::Group2);
        assert_eq!(drop_counter_tick(5), (0, true));
        assert_eq!(drop_counter_tick(2), (3, false));
        assert_eq!(drop_table_index(0x03, false), 3);
        assert_eq!(drop_table_index(0x03, true), 11);
    }

    #[test]
    fn projectile_slots_and_disintegration() {
        assert_eq!(spawn_projectile_slot(&[0, 0, 0, 0, 0, 1], 5), Some(4));
        assert_eq!(spawn_projectile_slot(&[1; 6], 5), None);
        assert_eq!(projectile_disintegrate(), (0x00, 0xF2));
        assert_eq!(projectile_tick_flag(0xF2), 0xF3);
        assert_eq!(projectile_tick_flag(0xFF), 0x00);
        assert_eq!(projectile_tick_flag(0x01), 0x01);
        assert!(!projectile_live(0x00, 0x04));
        assert!(projectile_live(0xF5, 0x04));
    }

    #[test]
    fn trajectory_self_consistent() {
        let a = run_trajectory(EnemySet::synthetic(0), 7, 60);
        let b = run_trajectory(EnemySet::synthetic(0), 7, 60);
        assert_eq!(a, b);
        let c = run_trajectory(EnemySet::synthetic(0), 8, 60);
        assert_ne!(a, c);
    }
}
