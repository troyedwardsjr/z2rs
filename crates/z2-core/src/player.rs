//! Player engine: movement, jumping, sword/thrusts, damage, fairy form.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod player;` (see `lib.rs`; trap shims live in
//! `player_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Spells/items/leveling/death live in the sibling [`player_magic`](self)
//! module file (`player_magic.rs`); hitbox builders here feed the sword and
//! shield paths there and in the traps.
//!
//! # Sources
//!
//! Bank 7 `third_party/z2disassembly/src/prg7.asm` (`prg7.asm $xxxx` below)
//! and bank 0 `src/prg0.asm`; RAM labels from `ram-map.toml` / `variables.asm`.
//!
//! | fn | label | addr |
//! |---|---|---|
//! | [`move_x_subpixel`] / [`move_y_subpixel`] | `bank7_XY_Movements_Routine` | `$D1CE` |
//! | [`apply_gravity`] | `bank7_applyGravityMotion` | `$D19B` |
//! | [`walk_step`] | sideview main walk (`LE16F`-region / `$E16F` ff.) | `$E16F` (region; GAP below) |
//! | [`jump_init`] | jump kick (`$0479` set, `$D8CD`/`$D9F8` region) | `$D8CD` |
//! | [`duck_state`] | `LE0E6` duck/chimney gate | `$E0E6` |
//! | [`sword_box`] | `bank7_code44` | `$E9A2` |
//! | [`shield_box`] | `bank7_code43` | `$E975` |
//! | [`body_box`] | `bank7_code45` | `$E9D8` |
//! | [`boxes_overlap`] | `bank7_idem__maybe` | `$E9F9` |
//! | [`damage_to_link`] | `bank7_Link_Hit_Routine` + `bank7_Table_for_Enemy_Damage` | `$E2EF` / `$E2AF` |
//! | [`apply_damage`] | `bank7_Link_Hit_Routine` HP sub (`$E331-$E349`) | `$E331` |
//! | [`recoil_select`] | `bank7_Set_Links_Recoil` + recoil table | `$E399` / `$E36A` |
//! | [`shield_blocks`] | `bank7_code39` | `$E558` |
//! | [`sword_hit_step`] | `bank7_Sword_Hit_Detection…` | `$E677` |
//! | [`fairy_step`] | `bank7_code47` + `bank7_Table_for_Fairy_floating_movement` | `$EBB8` / `$EBB0` |
//! | [`PlayerState::step`] | headless deterministic model (fuzz harness) | — |
//!
//! # Preserved quirks (BUG comments)
//!
//! * Down-thrust jackhammer: landing a down stab (`$80 == $09`, `$0B == 0`)
//!   writes `$057D = $FE` (`$E666`, and `$E720` on armored foes), so holding
//!   B while overlapping bounces Link back up every frame.
//! * Crouch-thrust: a down stab is only special when `$80 == $09`; the
//!   display routine subtracts 4 from Y when ducked-midair (`$EC0A-$EC15`).
//! * Up-stab hover: landing an up stab with `$0B == 0` while above the enemy
//!   writes `$057D = $00` (`$E705`), killing vertical motion for a frame.
//! * Shield-recoil path (`$E654`): a blocked hit with `$80 == $09` also
//!   bounces (`$057D = $FE`) before tail-calling `bank7_code37`.
//!
//! # Gaps (honest)
//!
//! * Exact walk accel/friction increments and jump-impulse bytes: the Listing
//!   walk driver inside the `LE16F`-region sideview main was not isolated to
//!   single bytes yet; [`walk_step`] / [`jump_init`] take the
//!   rates as explicit params with defaults marked `INFERRED` (ROM-gated).
//!   Subpixel integration, gravity accumulation and terminal-velocity clamp
//!   are byte-exact to `$D1CE` / `$D19B`.
//! * Sprite/OAM emission (`bank7_Links_Display_Routine`, `$EBF0`) is display
//!   only and intentionally not modelled (needs PPU/OAM; PPU scope).

// ---------------------------------------------------------------------------
// Addresses (duplicated per player*.rs file on purpose; keeps each file
// standalone-compilable — see sideview*.rs convention).
// ---------------------------------------------------------------------------

/// Link Y (`$29`).
pub const ADDR_LINK_Y: u16 = 0x0029;
/// Link X low (`$4D`).
pub const ADDR_LINK_X: u16 = 0x004D;
/// Link X high / page (`$3B`).
pub const ADDR_LINK_PAGE: u16 = 0x003B;
/// Link X velocity (`$70`, signed).
pub const ADDR_X_SPEED: u16 = 0x0070;
/// Link Y velocity (`$57D`, signed, negative = up).
pub const ADDR_Y_SPEED: u16 = 0x057D;
/// Mid-air flag (`$479`: 0 grounded, 1 falling, 2 rising).
pub const ADDR_MIDAIR: u16 = 0x0479;
/// Ducking flag (`$495`).
pub const ADDR_DUCK: u16 = 0x0495;
/// X subpixel accumulator (`$3D6`).
pub const ADDR_X_SUB: u16 = 0x03D6;
/// Gravity counter (`$3E6`, accumulates `$00` each tick).
pub const ADDR_GRAV_CTR: u16 = 0x03E6;
/// Down/Up thrust techs (`$796`: `$10` down, `$04` up).
pub const ADDR_THRUST: u16 = 0x0796;
/// Stun/invincibility tick (`$500`, reloaded `$14` by NMI).
pub const ADDR_INV_STUN: u16 = 0x0500;
/// Immunity timer (`$518`: 0 vulnerable).
pub const ADDR_INVULN: u16 = 0x0518;
/// Injured-state timer (`$50C`, set `$20` on hit).
pub const ADDR_INJURED: u16 = 0x050C;
/// Facing/dir pressed (`$9F`: 1 right, 2 left).
pub const ADDR_FACING: u16 = 0x009F;
/// Facing direction (`$5F`, display mirror of `$9F`).
pub const ADDR_FACING_DISP: u16 = 0x005F;
/// Animation frame (`$80`: 0-3 walk, 4 wind-up, 5 stab, 6 duck, 7 duck-stab,
/// 8 up-stab, 9 down-stab, 11 backflip-ish; forces attacks).
pub const ADDR_ANIM: u16 = 0x0080;
/// Shield position (`$17`; 0 = ducked).
pub const ADDR_SHIELD_POS: u16 = 0x0017;
/// Fairy state (`$13`: 0 Link, 8 fairy).
pub const ADDR_FAIRY: u16 = 0x0013;
/// Collision bits (`$A7`, `0000ABLR`).
pub const ADDR_COLL: u16 = 0x00A7;
/// Slash frame code (`$400`, cleared on hit).
pub const ADDR_SLASH: u16 = 0x0400;
/// Kill-Link flag (`$494`: nonzero → die path).
pub const ADDR_KILL: u16 = 0x0494;
/// Sword-work register `$0B` (0 = live blade).
pub const ADDR_BLADE: u16 = 0x000B;
/// Sword X anchor (`$47E`, compared vs screen X `$CC`).
pub const ADDR_SWORD_X: u16 = 0x047E;
/// Sword Y anchor (`$480`; `$F8` = retracted/inactive).
pub const ADDR_SWORD_Y: u16 = 0x0480;
/// Screen X (`$CC`).
pub const ADDR_SCREEN_X: u16 = 0x00CC;
/// Hurt sound (`$E9`, set 1 on hit).
pub const ADDR_HURT_SND: u16 = 0x00E9;
/// Shield-spell tint (`$70F`: nonzero halves damage).
pub const ADDR_SHIELD_FX: u16 = 0x070F;
/// Reflect-spell flag (`$710`).
pub const ADDR_REFLECT: u16 = 0x0710;
/// Current life (`$774`).
pub const ADDR_HP: u16 = 0x0774;
/// Life level 1-8 (`$779`).
pub const ADDR_LIFE_LVL: u16 = 0x0779;
/// Pause-pane dirty bits (`$74F`, `OR #$40` on hit).
pub const ADDR_PANE: u16 = 0x074F;
/// Exp-loss pending (`$5E8`: 10, or 20 vs Moa code 6).
pub const ADDR_EXP_LOSS: u16 = 0x05E8;
/// Frame counter (`$12`).
pub const ADDR_FRAME: u16 = 0x0012;

// ---------------------------------------------------------------------------
// Buttons (standard NES order A B Select Start Up Down Left Right = 0..7).
// ---------------------------------------------------------------------------

/// Button mask: A.
pub const BTN_A: u8 = 0x01;
/// Button mask: B (sword).
pub const BTN_B: u8 = 0x02;
/// Button mask: Select (spell cast path gates on this in bank 0).
pub const BTN_SELECT: u8 = 0x04;
/// Button mask: Start.
pub const BTN_START: u8 = 0x08;
/// Button mask: Up.
pub const BTN_UP: u8 = 0x10;
/// Button mask: Down.
pub const BTN_DOWN: u8 = 0x20;
/// Button mask: Left.
pub const BTN_LEFT: u8 = 0x40;
/// Button mask: Right.
pub const BTN_RIGHT: u8 = 0x80;

// ---------------------------------------------------------------------------
// Tables (copied from the disassembly; see doc table).
// ---------------------------------------------------------------------------

/// Thrust-tech bits (`$796`): down / up.
pub const THRUST_DOWN: u8 = 0x10;
/// Thrust-tech bits (`$796`): up.
pub const THRUST_UP: u8 = 0x04;

/// Anim ids (`$80`).
pub const ANIM_STAB: u8 = 0x05;
/// Anim ids (`$80`): ducked.
pub const ANIM_DUCK: u8 = 0x06;
/// Anim ids (`$80`): up stab.
pub const ANIM_UP_STAB: u8 = 0x08;
/// Anim ids (`$80`): down stab.
pub const ANIM_DOWN_STAB: u8 = 0x09;
/// Sword-retracted sentinel (`$480 == $F8` skips hit detection, `$E67A`).
pub const SWORD_RETRACTED: u8 = 0xF8;

/// Enemy damage table (`bank7_Table_for_Enemy_Damage`, bank 7 `$E2AF`,
/// 7 damage codes × 8 life levels). Indexed `[dmg_code][life_level - 1]`.
pub const ENEMY_DAMAGE: [[u8; 8]; 7] = [
    [0x10, 0x0C, 0x0C, 0x0C, 0x08, 0x04, 0x04, 0x04],
    [0x20, 0x1C, 0x14, 0x10, 0x0C, 0x0C, 0x08, 0x08],
    [0x30, 0x28, 0x24, 0x20, 0x18, 0x14, 0x10, 0x0C],
    [0x60, 0x48, 0x38, 0x30, 0x28, 0x20, 0x1C, 0x18],
    [0x90, 0x78, 0x60, 0x48, 0x38, 0x30, 0x28, 0x20],
    [0xE0, 0xA0, 0x80, 0x70, 0x60, 0x50, 0x40, 0x30],
    [0xE0, 0xC0, 0xA0, 0x90, 0x80, 0x70, 0x60, 0x50],
];

/// Attack power per attack level (`bank7_Attack_Power_for_8_Levels`, bank 7
/// `$E66D`: `02 03 04 06 09 0C 12 18`; thunder `$32` lives in `player_magic`).
pub const ATTACK_POWER: [u8; 8] = [0x02, 0x03, 0x04, 0x06, 0x09, 0x0C, 0x12, 0x18];

/// Recoil table (`bank7_Table_for_various_recoil_variables`, bank 7 `$E36A`):
/// flying-blade L/R, shield-hit L/R, link-hit L/R, then `$0C/$04/$FC/$0D/$F3`
/// tail used by `bank7_Set_Links_Recoil` (`$E399`) via `LE36C,y`.
pub const RECOIL_TABLE: [u8; 7] = [0x0C, 0xF4, 0x0C, 0x04, 0xFC, 0x0D, 0xF3];

/// Knockback X on strong-boss hit (`bank7_Table_for_X_Velocity_when_Link_is_hit_`,
/// bank 7 `$E556`): indexed by facing-relative Y (`$18` / `$E8`).
pub const KNOCKBACK_X: [u8; 2] = [0x18, 0xE8];

/// Knock-up Y on hit (`$E363`: `LDA #$FE : STA $057D`).
pub const HIT_VSPEED: u8 = 0xFE;
/// Injured timer set on hit (`$E34C`: `LDA #$20 : STA $050C`).
pub const HIT_STUN: u8 = 0x20;
/// Immunity timer set on hit (`$E353`: `LDA #$04 : STA $0518`).
pub const HIT_IMMUNITY: u8 = 0x04;
/// Down-stab bounce (`$E666` / `$E720`: `LDA #$FE : STA $057D`).
pub const DOWNSTAB_BOUNCE: u8 = 0xFE;
/// Up-stab hover (`$E705`: `STA $057D` with `A = $00`).
pub const UPSTAB_HOVER: u8 = 0x00;

/// Fairy float table (`bank7_Table_for_Fairy_floating_movement`, bank 7
/// `$EBB0`: `01 02 03 04 03 02 01 00`), indexed by `($12 & $38) >> 3`.
pub const FAIRY_FLOAT: [u8; 8] = [0x01, 0x02, 0x03, 0x04, 0x03, 0x02, 0x01, 0x00];

/// Sword-box X nudge table (`bank7_table26`, bank 7 `$E99C`: `F8 02`).
pub const SWORD_DX: [u8; 2] = [0xF8, 0x02];
/// Sword-box Y add/height (`LE99E`/`LE9A0`, bank 7 `$E99E`/`$E9A0`):
/// normal `(07, 03/03)`, stab `(00, 03/03)`.
pub const SWORD_DY: [u8; 2] = [0x07, 0x00];
/// Sword-box heights continued.
pub const SWORD_H: [u8; 2] = [0x03, 0x03];
/// Sword-box width (`$E9B4`: `LDA #$0E`).
pub const SWORD_W: u8 = 0x0E;

/// Body-box X nudge (`bank7_table27`, bank 7 `$E9D4`: `0E FF`).
pub const BODY_DX: [u8; 2] = [0x0E, 0xFF];
/// Body-box Y add (`LE9D6`, bank 7 `$E9D6`: `11 02`); height `$0C`, width `$05`.
pub const BODY_DY: [u8; 2] = [0x11, 0x02];
/// Body-box width (`$E9E6`).
pub const BODY_W: u8 = 0x05;
/// Body-box height (`$E9F4`).
pub const BODY_H: u8 = 0x0C;

/// Shield-box base (`bank7_code43`, `$E975`: `$CC + $09`, width `$0D`).
pub const SHIELD_DX: u8 = 0x09;
/// Shield-box width.
pub const SHIELD_W: u8 = 0x0D;

// ---------------------------------------------------------------------------
// Subpixel integration (bank7_XY_Movements_Routine, bank 7 $D1CE).
// ---------------------------------------------------------------------------

/// Split a signed 8-bit velocity into the 4.4-fixed-point halves the routine
/// uses: low nibble shifted up (`ASL ×4`, added to the subpixel byte) and
/// sign-extended high nibble (added to the position with the subpixel carry).
///
/// `$D1D0-$D1E2`: `ASL×4 → $01`; `LSR×4` + sign-extend (`ORA #$F0` when
/// `>= $08`) → `$00`; `Y = $FF` when negative → `$02`.
pub const fn vel_split(vel: u8) -> (u8, u8, u8) {
    let lo = vel.wrapping_shl(4);
    let mut hi = vel.wrapping_shr(4);
    if hi >= 0x08 {
        hi |= 0xF0;
    }
    let hi_hi = if (hi as i8) < 0 { 0xFF } else { 0x00 };
    (lo, hi, hi_hi)
}

/// Horizontal subpixel step (`$D1CE-$D207` path).
///
/// Adds the low half to `$3D6`, folds the carry (`ROL`/`PHA`/`ROR` dance)
/// into `$4D`, then the sign-extended high half (+ carry) into `$4D`/`$3B`.
/// Returns `(x_lo, x_hi, sub)`.
pub fn move_x_subpixel(x_lo: u8, x_hi: u8, sub: u8, hspeed: u8) -> (u8, u8, u8) {
    let (lo, hi, hi_hi) = vel_split(hspeed);
    let (sub2, carry) = sub.overflowing_add(lo);
    // `LDA #$00 : ROL` picks up the bit-7 carry out of the ADC; since `lo`
    // is a multiple of 16 the carry out of bit 7 equals the overflowing_add
    // carry, and `ROR` folds exactly that bit back into the `$00` addend.
    let carry_in: u8 = u8::from(carry);
    let (mid, c1) = x_lo.overflowing_add(hi);
    // The second carry (from the ROR-folded bit) rides along the same add.
    let (x_lo2, c2) = mid.overflowing_add(carry_in);
    let c = u8::from(c1 || c2);
    let (mut x_hi2, _) = x_hi.overflowing_add(hi_hi);
    x_hi2 = x_hi2.wrapping_add(c);
    (x_lo2, x_hi2, sub2)
}

/// Vertical subpixel step (`LD20A`, bank 7 `$D20A-$D24B` path).
///
/// Same split applied to `$057D`, accumulated into `$3E6` (gravity counter
/// doubles as the Y subpixel byte), position `$29`/`$19` (Y + fall-high).
/// Returns `(y_lo, y_hi, grav_ctr)`.
pub fn move_y_subpixel(y_lo: u8, y_hi: u8, grav_ctr: u8, vspeed: u8) -> (u8, u8, u8) {
    let (ny, nh, nc) = move_x_subpixel(y_lo, y_hi, grav_ctr, vspeed);
    (ny, nh, nc)
}

// ---------------------------------------------------------------------------
// Gravity (bank7_applyGravityMotion, bank 7 $D19B).
// ---------------------------------------------------------------------------

/// Gravity tick (`$D19B-$D1CD`).
///
/// `Y += vspeed` (16-bit `$29`/`$19`); `$3E6 += grav_add` (`ADC $00`);
/// `vspeed += carry`, clamped at `max_fall` (`CMP $02 : BNE` over the
/// re-store; on equality the counter is cleared). Returns
/// `(y_lo, y_hi, grav_ctr, vspeed)`.
pub fn apply_gravity(
    y_lo: u8,
    y_hi: u8,
    vspeed: u8,
    grav_ctr: u8,
    grav_add: u8,
    max_fall: u8,
) -> (u8, u8, u8, u8) {
    // Sign-extend vspeed into $07.
    let y_ext: u8 = if (vspeed as i8) < 0 { 0xFF } else { 0x00 };
    let (y1, c1) = y_lo.overflowing_add(vspeed);
    let mut y_hi1 = y_hi.wrapping_add(y_ext);
    if c1 {
        // ADC $07 folds the low carry into the high byte.
        y_hi1 = y_hi1.wrapping_add(1);
    }
    let (ctr1, c2) = grav_ctr.overflowing_add(grav_add);
    // `LDA vspeed : ADC #$00` — adds only the counter carry.
    let vs1 = vspeed.wrapping_add(u8::from(c2));
    if vs1 == max_fall {
        // Terminal clamp: re-store + clear counter (`$D1C5-$D1CA`).
        (y1, y_hi1, 0x00, vs1)
    } else {
        (y1, y_hi1, ctr1, vs1)
    }
}

// ---------------------------------------------------------------------------
// Walk / jump / duck (sideview main; INFERRED rates — see module gaps).
// ---------------------------------------------------------------------------

/// Walk tuning (INFERRED — ROM-gated; see module docs).
///
/// The integration underneath (`move_x_subpixel`) is byte-exact; only these
/// per-frame deltas await a ROM-gated pass against the sideview main driver.
pub const WALK_ACCEL: u8 = 0x02;
/// Walk tuning: friction per frame with no direction held (INFERRED).
pub const WALK_FRICTION: u8 = 0x02;
/// Walk tuning: max run speed magnitude (INFERRED; `$18` matches the
/// knockback-table magnitude at `$E556`).
pub const WALK_MAX: u8 = 0x18;
/// Walk tuning: mid-air control accel (INFERRED; weaker than grounded).
pub const AIR_ACCEL: u8 = 0x01;

/// Facing after a directional press (`$9F`: 1 right, 2 left).
pub const FACING_RIGHT: u8 = 0x01;
/// Facing after a directional press (`$9F`: 1 right, 2 left).
pub const FACING_LEFT: u8 = 0x02;

/// One walk frame: accelerate toward the held direction, apply friction when
/// neither (or both) held, clamp to `±max`. `hspeed` is signed (`i8`
/// domain, stored as `u8`). Grounded uses `accel`, mid-air uses `air_accel`.
/// Returns the new `$70`.
pub fn walk_step(
    hspeed: u8,
    held: u8,
    midair: bool,
    accel: u8,
    air_accel: u8,
    friction: u8,
    max: u8,
) -> u8 {
    let v = hspeed as i8;
    let accel_i = i16::from(if midair { air_accel } else { accel });
    let max_i = i16::from(max as i8);
    let left = held & BTN_LEFT != 0;
    let right = held & BTN_RIGHT != 0;
    let nv = match (left, right) {
        (true, false) => (i16::from(v) - accel_i).max(-max_i),
        (false, true) => (i16::from(v) + accel_i).min(max_i),
        _ => {
            // Friction toward 0.
            let f = i16::from(friction);
            if v > 0 {
                (i16::from(v) - f).max(0)
            } else if v < 0 {
                (i16::from(v) + f).min(0)
            } else {
                0
            }
        }
    };
    nv as i8 as u8
}

/// Default-rate walk frame (uses [`WALK_ACCEL`] / [`AIR_ACCEL`] /
/// [`WALK_FRICTION`] / [`WALK_MAX`]).
pub fn walk_step_default(hspeed: u8, held: u8, midair: bool) -> u8 {
    walk_step(
        hspeed,
        held,
        midair,
        WALK_ACCEL,
        AIR_ACCEL,
        WALK_FRICTION,
        WALK_MAX,
    )
}

/// Facing update from held directions (last-press wins toward held side;
/// neutral keeps facing). Mirrors `$9F` maintenance in the sideview driver.
pub const fn facing_step(facing: u8, held: u8) -> u8 {
    if held & BTN_LEFT != 0 && held & BTN_RIGHT == 0 {
        FACING_LEFT
    } else if held & BTN_RIGHT != 0 && held & BTN_LEFT == 0 {
        FACING_RIGHT
    } else {
        facing
    }
}

/// Jump impulse (INFERRED — ROM-gated; `$D0` jump-spell flag strengthens it).
pub const JUMP_VELOCITY: u8 = 0xC4;
/// Jump impulse with the Jump spell active (INFERRED — higher jump).
pub const JUMP_VELOCITY_SPELL: u8 = 0xAC;

/// Jump kick: grounded + A-pressed starts the rise (`$0479 = 2`, vspeed =
/// impulse). Returns `(midair, vspeed)`; airborne input leaves both alone.
pub const fn jump_init(midair: u8, a_pressed: bool, jump_spell: bool) -> (u8, u8) {
    if midair == 0 && a_pressed {
        let v = if jump_spell {
            JUMP_VELOCITY_SPELL
        } else {
            JUMP_VELOCITY
        };
        (0x02, v)
    } else {
        (midair, 0x00)
    }
}

/// Duck state (`LE0E6`, bank 7 `$E0E6`): Down-held + grounded ducks
/// (`$495 = 1`, `$17 = 0` shield-low); chimney sink (`INC $070E`) and
/// door-open (`INC $075B`, `$16 → LE187`) are caller-side effects reported
/// via the returned flags. Returns `(ducking, chimney_sink, door_open)`.
pub const fn duck_state(down_held: bool, midair: u8, shield_pos: u8) -> (bool, bool, bool) {
    if down_held && midair == 0 {
        // `$E0DE-$E0E5`: shield already low → chimney sink path.
        if shield_pos == 0 {
            (true, true, false)
        } else {
            (true, false, false)
        }
    } else {
        (false, false, false)
    }
}

// ---------------------------------------------------------------------------
// Hitboxes (bank7_code43/44/45 + bank7_idem__maybe).
// ---------------------------------------------------------------------------

/// Axis-aligned box in screen space `(x, y, w, h)`.
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

/// Shield box (`bank7_code43`, bank 7 `$E975`).
///
/// `$00 = $CC + $09`; `$02 = $0D`; `$01 = $29 + LE971[$17]`;
/// `$03 = LE973[$17]`; the `$00 + $02` carry path complements `$02`
/// (`EOR #$FF`) on overflow. `dy`/`h` fold the `LE971`/`LE973` tables
/// (caller-supplied: indexed by shield pos `$17`).
pub const fn shield_box(screen_x: u8, link_y: u8, dy: u8, h: u8) -> HitBox {
    let (x, c) = screen_x.overflowing_add(SHIELD_DX);
    let _ = c;
    let (y, _) = link_y.overflowing_add(dy);
    HitBox {
        x,
        y,
        w: SHIELD_W,
        h,
    }
}

/// Sword box (`bank7_code44`, bank 7 `$E9A2`).
///
/// `Y` selects the side by `$47E` vs `$CC` (`BCS` → 0 else 1);
/// `$00 = $47E + table26[Y]`; `$02 = $0E`; stab anims (`$80` 8/9) use the
/// second `LE99E`/`LE9A0` row (`$01 = $480 + dy`, `$03 = h`).
/// `up_down_stab` selects row 1; `side_right` selects `SWORD_DX[1]`.
pub const fn sword_box(sword_x: u8, sword_y: u8, side_right: bool, up_down_stab: bool) -> HitBox {
    let dx = if side_right { SWORD_DX[1] } else { SWORD_DX[0] };
    let row = if up_down_stab { 1 } else { 0 };
    let (x, _) = sword_x.overflowing_add(dx);
    let (y, _) = sword_y.overflowing_add(SWORD_DY[row]);
    HitBox {
        x,
        y,
        w: SWORD_W,
        h: SWORD_H[row],
    }
}

/// Body box (`bank7_code45`, bank 7 `$E9D8`).
///
/// `Y = $9F - 1` (facing); `$00 = $CC + $08 + table27[Y]`; `$02 = $05`;
/// `$01 = $29 + LE9D6[$17]`; `$03 = $0C`.
pub const fn body_box(screen_x: u8, link_y: u8, facing_right: bool, dy: u8) -> HitBox {
    let dx = if facing_right { BODY_DX[0] } else { BODY_DX[1] };
    let (base, _) = screen_x.overflowing_add(0x08);
    let (x, _) = base.overflowing_add(dx);
    let (y, _) = link_y.overflowing_add(dy);
    HitBox {
        x,
        y,
        w: BODY_W,
        h: BODY_H,
    }
}

/// Overlap test (`bank7_idem__maybe`, bank 7 `$E9F9`).
///
/// Per axis: `($04 - $00) + $06` vs `$02 + $06` (`SEC/SBC/CLC/ADC/CMP/BCC`
/// chain over `X = 1, 0`); carry clear on either axis = no overlap. Both
/// boxes are `(x, y, w, h)` with the second box's extent folded like
/// `$04/$06` (pos/size). Returns true on overlap (carry set, `RTS` with
/// `X = $10` restored).
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

// ---------------------------------------------------------------------------
// Damage (bank7_Link_Hit_Routine, bank 7 $E2EF).
// ---------------------------------------------------------------------------

/// Look up raw damage (`$E31D-$E32F`).
///
/// `code = dmg_nibble << 3 + life_level` (`ASL×3 : ADC $0779`);
/// `dmg = table[code]`; halved (`LSR`) when the Shield spell tint
/// (`$070F`) is set. `dmg_code` is the low nibble of `$6DF9,y`
/// (`AND #$0F`); `life_level` is 1-8 (clamped).
pub fn damage_to_link(dmg_code: u8, life_level: u8, shield_active: bool) -> u8 {
    let lvl = life_level.clamp(1, 8) - 1;
    let code = (usize::from(dmg_code & 0x07) * 8 + usize::from(lvl)) % 56;
    let row = code / 8;
    let col = code % 8;
    let mut dmg = ENEMY_DAMAGE[row][col];
    if shield_active {
        dmg >>= 1;
    }
    dmg
}

/// Exp-loss on contact (`$E2F1-$E305`): Moa-flagged (`$6DD5 & $10`) enemies
/// drain pending exp (`$05E8 = 10`, or 20 for enemy code 6). Returns the
/// `$05E8` value (0 = no drain). `invuln` nonzero still records the drain
/// path (`LE308` runs before the immunity early-out).
pub const fn exp_loss_on_hit(enemy_steals: bool, enemy_code: u8) -> u8 {
    if enemy_steals {
        if enemy_code == 0x06 {
            0x14
        } else {
            0x0A
        }
    } else {
        0x00
    }
}

/// HP subtraction (`$E331-$E349`): `hp -= dmg` (`SEC : SBC`);
/// borrow → clamp 0 + `INC $0494` (kill Link). Returns `(hp, killed)`.
pub const fn apply_damage(hp: u8, dmg: u8) -> (u8, bool) {
    let (nhp, borrow) = hp.overflowing_sub(dmg);
    if borrow {
        (0x00, true)
    } else {
        (nhp, false)
    }
}

/// Full hit tick: immunity (`$0518 != 0`) skips everything but the exp-loss
/// bookkeeping; else hurt sound, recoil select, damage, HP, timers
/// (`$050C = $20`, `$0518 = $04`, `$0400 = 0`, `$A7 &= $FB`,
/// `$057D = $FE`, carry set). Returns an [`HitOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitOutcome {
    /// New HP.
    pub hp: u8,
    /// Kill flag raised (`INC $0494`).
    pub killed: bool,
    /// Exp-loss written to `$05E8`.
    pub exp_loss: u8,
    /// Whether the damage half ran (false = immune, `LE368` `CLC` path).
    pub damaged: bool,
}

/// Hit-tick input bundle (explicit params; no RAM dependency).
pub fn link_hit_tick(
    hp: u8,
    invuln: u8,
    dmg_code: u8,
    life_level: u8,
    shield_active: bool,
    enemy_steals: bool,
    enemy_code: u8,
) -> HitOutcome {
    let exp_loss = exp_loss_on_hit(enemy_steals, enemy_code);
    if invuln != 0 {
        // `LE368`: CLC, no damage half.
        return HitOutcome {
            hp,
            killed: false,
            exp_loss,
            damaged: false,
        };
    }
    let dmg = damage_to_link(dmg_code, life_level, shield_active);
    let (nhp, killed) = apply_damage(hp, dmg);
    HitOutcome {
        hp: nhp,
        killed,
        exp_loss,
        damaged: true,
    }
}

// ---------------------------------------------------------------------------
// Recoil (bank7_Set_Links_Recoil, bank 7 $E399 + bank7_code37 $E371).
// ---------------------------------------------------------------------------

/// Recoil select (`$E399-$E3B8`).
///
/// Re-faces the slot toward Link, bumps `Y` (`INY`), compares vs collision
/// low bits (`CPY $0D`); equal → no recoil. Else shield-hit (`blade != 0`)
/// skips the extra `INY×2`, and `RECOIL_TABLE[Y]` (the `LE36C` window
/// `0C 04 FC 0D F3`) becomes `$70`. Returns `Some(hspeed)` or `None` when
/// recoil is suppressed. `rel_dir` is the post-`Determine…` facing index
/// (0/1), `coll_low` the `$A7 & $03` bits, `blade` the `$00` register.
pub const fn recoil_select(rel_dir: u8, coll_low: u8, blade: u8) -> Option<u8> {
    let y = rel_dir.wrapping_add(1);
    if y == coll_low {
        return None;
    }
    let y2 = if blade == 0 { y.wrapping_add(2) } else { y };
    // `LE36C,y` window offset: table base `$E36A` + 2.
    let idx = y2.wrapping_sub(2) as usize % RECOIL_TABLE.len();
    Some(RECOIL_TABLE[idx])
}

/// Shield-block gate (`bank7_code39`, bank 7 `$E558`).
///
/// Reflect active → shield path always; enemies `>= $17` (Orange Daira+)
/// pierce to the body-hit path; else the shield/box tests decide.
/// Returns true when the hit is a *shield block* (code37 path), false when
/// it falls through to the body path (`LE579`).
pub const fn shield_blocks(reflect_active: bool, enemy_code: u8, shield_hit: bool) -> bool {
    if !reflect_active && enemy_code >= 0x17 {
        return false;
    }
    shield_hit
}

// ---------------------------------------------------------------------------
// Sword hit step (bank7_Sword_Hit_Detection…, bank 7 $E677).
// ---------------------------------------------------------------------------

/// Sword-gate inputs (explicit; no RAM dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwordGate {
    /// `$480 == $F8` (retracted).
    pub retracted: bool,
    /// Target immune bit (`$6E41 & $10`).
    pub target_immune: bool,
    /// Slot live (`$B6 == 1`).
    pub slot_live: bool,
    /// Elevator (`$A1 == $13`) / locked door (`$02`) skip damage.
    pub enemy_code: u8,
    /// Sword overlaps enemy (`code44` + `idem`).
    pub overlaps: bool,
    /// Blade register `$0B` (0 = live blade, else shield-touch path).
    pub blade: u8,
    /// Fire-immune bit (`$6DD5 & $20` with `$0B == 0` → deflect `$E654`).
    pub fire_immune: bool,
    /// Red-jar guard (`$A1 == 1` + `$40E != 0` → no re-hit).
    pub jar_guarded: bool,
}

/// Sword-gate outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwordOutcome {
    /// No hit (`LE6A6`/`LE6AC`: `CLC`, `$A8 &= $DF` clear path).
    Miss,
    /// Deflected with shield sound (`LE654` path, `$EC = 2`).
    Deflect,
    /// Item touched (red-jar/item branch, `LE755` region).
    TouchItem,
    /// Solid hit: damage `ATTACK_POWER[level]` applies (carry set).
    Hit,
}

/// Sword gate (`$E677-$E6AD`): retracted / immune / dead-slot short-circuit
/// to miss; elevator + locked-door skip to miss; a failed overlap clears
/// `$A8.$5` to miss; else the `$0B`/fire/jar chain decides deflect / touch /
/// hit. BUG-compatible: the `$0480 >= $F8` skip and the `$A8.$5` set live
/// with the caller (reported via the outcome).
pub const fn sword_gate(g: SwordGate) -> SwordOutcome {
    if g.retracted || g.target_immune || !g.slot_live {
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

/// Up-stab special (`$E6F3-$E705`): anim `$08` + live blade + Link at/below
/// the enemy's Y (`$29 >= $2A,x`, `BCC` miss otherwise) freezes vertical
/// motion (`$057D = $00`, hover quirk). Returns the vspeed override
/// (`Some(0)`) or `None`.
pub const fn upstab_hover(anim: u8, blade: u8, link_y: u8, enemy_y: u8) -> Option<u8> {
    if anim == ANIM_UP_STAB && blade == 0 && link_y >= enemy_y {
        Some(UPSTAB_HOVER)
    } else {
        None
    }
}

/// Down-stab bounce (`$E726-$E720` + `$E654` shield path): anim `$09` +
/// live blade bounces (`$057D = $FE`, jackhammer quirk). Returns the vspeed
/// override (`Some($FE)`) or `None`.
pub const fn downstab_bounce(anim: u8, blade: u8) -> Option<u8> {
    if anim == ANIM_DOWN_STAB && blade == 0 {
        Some(DOWNSTAB_BOUNCE)
    } else {
        None
    }
}

/// Thrust availability from `$796` tech bits + airborne state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrustAvail {
    /// Up thrust usable (tech + airborne + Up held).
    pub up: bool,
    /// Down thrust usable (tech + airborne + Down held).
    pub down: bool,
}

/// Thrust gate: `$796 & $04` (up) / `$796 & $10` (down), airborne only
/// (`$0479 != 0`); B-press with the direction starts the stab anim.
pub const fn thrust_avail(thrust_flags: u8, midair: u8, held: u8) -> ThrustAvail {
    let air = midair != 0;
    ThrustAvail {
        up: air && thrust_flags & THRUST_UP != 0 && held & BTN_UP != 0,
        down: air && thrust_flags & THRUST_DOWN != 0 && held & BTN_DOWN != 0,
    }
}

// ---------------------------------------------------------------------------
// Fairy form ($13 = 8).
// ---------------------------------------------------------------------------

/// Fairy flag value (fairy form).
pub const FAIRY_ON: u8 = 0x08;
/// Fairy flag value (Link form).
pub const FAIRY_OFF: u8 = 0x00;
/// Fairy revert-block value (`$076F = $FF` prevents `$13: 8 → 0`).
pub const FAIRY_LOCK: u8 = 0xFF;
/// Fairy minimum cast height (`bank0_Fairy_Spell`, bank 0 `$91A4`:
/// `CMP #$20 : BCC RTS` — below `$20` the cast fizzles).
pub const FAIRY_MIN_Y: u8 = 0x20;

/// Fairy cast gate (bank 0 `$91A4-$91AE`): Link form + `Y >= $20` transforms
/// (`$13 = 8`); otherwise no change. Returns the new `$13`.
pub const fn fairy_cast(fairy: u8, link_y: u8) -> u8 {
    if fairy == FAIRY_OFF && link_y >= FAIRY_MIN_Y {
        FAIRY_ON
    } else {
        fairy
    }
}

/// Fairy revert: `$076F = $FF` blocks the `8 → 0` change (listing note on
/// `$13`); else writing 0 reverts. Returns the new `$13`.
pub const fn fairy_revert(fairy: u8, magic_state: u8) -> u8 {
    if fairy == FAIRY_ON && magic_state != FAIRY_LOCK {
        FAIRY_OFF
    } else {
        fairy
    }
}

/// Fairy flight frame (`bank7_code47`, bank 7 `$EBB8`).
///
/// Free flight: `Y += float[(frame & $38) >> 3]` bob added to the sprite
/// anchor (`$29 + 8 + float`), `X = $CC + $0C`; injured flicker
/// (`$050C != 0`: `LSR : AND #3`) picks the sprite slot. Collision uses the
/// short spans (`Y = 7 + fairy`, then `5 + fairy`; caller feeds
/// `sideview_collision::probe_span` with the fairy row base). Hitbox is the
/// small fairy box (half height of the body box). Returns `(anchor_y,
/// anchor_x)`.
pub const fn fairy_step(link_y: u8, screen_x: u8, frame: u8) -> (u8, u8) {
    let idx = ((frame & 0x38) >> 3) as usize;
    let bob = FAIRY_FLOAT[idx % FAIRY_FLOAT.len()];
    let (base, _) = link_y.overflowing_add(0x08);
    let (anchor_y, _) = base.overflowing_add(bob);
    let (anchor_x, _) = screen_x.overflowing_add(0x0C);
    (anchor_y, anchor_x)
}

/// Fairy flicker slot (`$EBE3-$EBEB`): injured (`$050C != 0`) maps
/// `($050C >> 1) & 3`, else slot 1.
pub const fn fairy_flicker(injured: u8) -> u8 {
    if injured == 0 {
        0x01
    } else {
        (injured >> 1) & 0x03
    }
}

// ---------------------------------------------------------------------------
// Deterministic headless model (fuzz + snapshot harness).
// ---------------------------------------------------------------------------

/// Headless player state (subset of RAM the fuzz harness owns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerState {
    /// `$4D` X low.
    pub x_lo: u8,
    /// `$3B` X high.
    pub x_hi: u8,
    /// `$29` Y.
    pub y: u8,
    /// `$19` Y high / fall byte.
    pub y_hi: u8,
    /// `$3D6` X subpixel.
    pub sub: u8,
    /// `$70` X speed (signed domain).
    pub hspeed: u8,
    /// `$57D` Y speed (signed domain).
    pub vspeed: u8,
    /// `$3E6` gravity counter.
    pub grav_ctr: u8,
    /// `$479` mid-air.
    pub midair: u8,
    /// `$495` ducking.
    pub duck: u8,
    /// `$80` anim.
    pub anim: u8,
    /// `$9F` facing.
    pub facing: u8,
    /// `$796` thrust techs.
    pub thrust: u8,
    /// `$774` HP.
    pub hp: u8,
    /// `$500` stun/invincibility.
    pub inv_stun: u8,
    /// `$518` immunity.
    pub invuln: u8,
    /// `$13` fairy.
    pub fairy: u8,
    /// `$12` frame.
    pub frame: u8,
    /// `$A7` collision bits (fed by the caller: 0 open, bit2 grounded).
    pub coll: u8,
}

/// Collision-bit: below (grounded).
pub const COLL_BELOW: u8 = 0x04;

impl PlayerState {
    /// Five synthetic snapshots the fuzz harness fans out from.
    pub fn synthetic(i: usize) -> Self {
        match i % 5 {
            0 => Self {
                x_lo: 0x80,
                x_hi: 0x01,
                y: 0x90,
                y_hi: 0x01,
                sub: 0x00,
                hspeed: 0x00,
                vspeed: 0x00,
                grav_ctr: 0x00,
                midair: 0x00,
                duck: 0x00,
                anim: 0x00,
                facing: FACING_RIGHT,
                thrust: THRUST_UP | THRUST_DOWN,
                hp: 0x80,
                inv_stun: 0x00,
                invuln: 0x00,
                fairy: FAIRY_OFF,
                frame: 0x00,
                coll: COLL_BELOW,
            },
            1 => Self {
                x_lo: 0x20,
                x_hi: 0x02,
                y: 0x40,
                y_hi: 0x01,
                sub: 0x80,
                hspeed: 0x10,
                vspeed: 0xE0,
                grav_ctr: 0x40,
                midair: 0x02,
                duck: 0x00,
                anim: ANIM_DOWN_STAB,
                facing: FACING_LEFT,
                thrust: THRUST_DOWN,
                hp: 0x60,
                inv_stun: 0x00,
                invuln: 0x00,
                fairy: FAIRY_OFF,
                frame: 0x10,
                coll: 0x00,
            },
            2 => Self {
                x_lo: 0xF0,
                x_hi: 0x00,
                y: 0xA0,
                y_hi: 0x01,
                sub: 0xF0,
                hspeed: 0xF0,
                vspeed: 0x00,
                grav_ctr: 0x00,
                midair: 0x00,
                duck: 0x01,
                anim: ANIM_DUCK,
                facing: FACING_RIGHT,
                thrust: 0x00,
                hp: 0xFF,
                inv_stun: 0x05,
                invuln: 0x04,
                fairy: FAIRY_OFF,
                frame: 0x20,
                coll: COLL_BELOW,
            },
            3 => Self {
                x_lo: 0x70,
                x_hi: 0x01,
                y: 0x60,
                y_hi: 0x01,
                sub: 0x00,
                hspeed: 0x00,
                vspeed: 0x00,
                grav_ctr: 0x00,
                midair: 0x01,
                duck: 0x00,
                anim: ANIM_UP_STAB,
                facing: FACING_RIGHT,
                thrust: THRUST_UP,
                hp: 0x40,
                inv_stun: 0x00,
                invuln: 0x00,
                fairy: FAIRY_ON,
                frame: 0x30,
                coll: 0x00,
            },
            _ => Self {
                x_lo: 0x00,
                x_hi: 0x03,
                y: 0xB0,
                y_hi: 0x01,
                sub: 0x40,
                hspeed: 0xE8,
                vspeed: 0x10,
                grav_ctr: 0x80,
                midair: 0x01,
                duck: 0x00,
                anim: 0x00,
                facing: FACING_LEFT,
                thrust: THRUST_UP | THRUST_DOWN,
                hp: 0x10,
                inv_stun: 0x00,
                invuln: 0x02,
                fairy: FAIRY_OFF,
                frame: 0x40,
                coll: 0x00,
            },
        }
    }

    /// Deterministic per-frame update (headless; collision fed via `coll`).
    ///
    /// Order mirrors the frame pipeline: input → walk → jump → gravity →
    /// integrate → duck/anim → timers → frame++. No RNG, no ROM, no float.
    pub fn step(&mut self, held: u8, pressed: u8) {
        if self.fairy == FAIRY_ON {
            // Fairy: free flight, no gravity.
            if held & BTN_LEFT != 0 {
                let t = move_x_subpixel(self.x_lo, self.x_hi, self.sub, 0xF0);
                self.x_lo = t.0;
                self.x_hi = t.1;
                self.sub = t.2;
                self.facing = FACING_LEFT;
            } else if held & BTN_RIGHT != 0 {
                let t = move_x_subpixel(self.x_lo, self.x_hi, self.sub, 0x10);
                self.x_lo = t.0;
                self.x_hi = t.1;
                self.sub = t.2;
                self.facing = FACING_RIGHT;
            }
            if held & BTN_UP != 0 {
                self.y = self.y.wrapping_sub(1);
            } else if held & BTN_DOWN != 0 {
                self.y = self.y.wrapping_add(1);
            }
            self.frame = self.frame.wrapping_add(1);
            return;
        }
        self.facing = facing_step(self.facing, held);
        let grounded = self.midair == 0;
        self.hspeed = walk_step_default(self.hspeed, held, !grounded);
        // Jump kick.
        if grounded && pressed & BTN_A != 0 {
            self.midair = 0x02;
            self.vspeed = JUMP_VELOCITY;
            self.grav_ctr = 0x00;
        }
        // Gravity + integrate.
        if self.midair != 0 {
            let (y, yh, ctr, vs) =
                apply_gravity(self.y, self.y_hi, self.vspeed, self.grav_ctr, 0x08, 0x40);
            self.y = y;
            self.y_hi = yh;
            self.grav_ctr = ctr;
            self.vspeed = vs;
            let (iy, iyh, ictr) = move_y_subpixel(self.y, self.y_hi, self.grav_ctr, self.vspeed);
            self.y = iy;
            self.y_hi = iyh;
            self.grav_ctr = ictr;
            // Landing: caller reports grounded via coll bit.
            if self.coll & COLL_BELOW != 0 && (self.vspeed as i8) >= 0 {
                self.midair = 0x00;
                self.vspeed = 0x00;
            } else if self.coll & COLL_BELOW == 0 {
                self.midair = 0x01;
            }
        }
        let (nx, nxh, nsub) = move_x_subpixel(self.x_lo, self.x_hi, self.sub, self.hspeed);
        self.x_lo = nx;
        self.x_hi = nxh;
        self.sub = nsub;
        // Duck / anim.
        let (duck, _, _) = duck_state(held & BTN_DOWN != 0, self.midair, 1);
        self.duck = u8::from(duck);
        if pressed & BTN_B != 0 {
            let t = thrust_avail(self.thrust, self.midair, held);
            if t.up {
                self.anim = ANIM_UP_STAB;
            } else if t.down {
                self.anim = ANIM_DOWN_STAB;
            } else if duck {
                self.anim = 0x07;
            } else {
                self.anim = ANIM_STAB;
            }
        }
        // Down-stab jackhammer: bounce while falling onto the (assumed) foe.
        if self.anim == ANIM_DOWN_STAB && self.midair != 0 && held & BTN_B != 0 {
            self.vspeed = DOWNSTAB_BOUNCE;
        }
        // Timers tick down.
        self.inv_stun = self.inv_stun.saturating_sub(1);
        self.invuln = self.invuln.saturating_sub(1);
        self.frame = self.frame.wrapping_add(1);
    }

    /// FNV-1a hash of the state (trajectory compare for the fuzz harness).
    pub fn hash(&self) -> u64 {
        let bytes = [
            self.x_lo,
            self.x_hi,
            self.y,
            self.y_hi,
            self.sub,
            self.hspeed,
            self.vspeed,
            self.grav_ctr,
            self.midair,
            self.duck,
            self.anim,
            self.facing,
            self.thrust,
            self.hp,
            self.inv_stun,
            self.invuln,
            self.fairy,
            self.frame,
            self.coll,
        ];
        let mut h: u64 = 0xcbf29ce484222325;
        for b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
}

/// Deterministic input stream: xorshift32 over `seed`, one `(held, pressed)`
/// pair per frame (edge-triggered `pressed` derived from held transitions).
pub fn fuzz_inputs(seed: u32, frames: usize) -> Vec<(u8, u8)> {
    let mut rng = seed.max(1);
    let mut out = Vec::with_capacity(frames);
    let mut prev_held: u8 = 0;
    for _ in 0..frames {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let held = (rng & 0xFF) as u8;
        // Mask to plausible buttons (A/B + directions + down).
        let held = held & (BTN_A | BTN_B | BTN_UP | BTN_DOWN | BTN_LEFT | BTN_RIGHT);
        let pressed = held & !prev_held;
        prev_held = held;
        out.push((held, pressed));
    }
    out
}

/// Run a deterministic trajectory: `frames` steps from `init` on `seed`.
/// Same `(init, seed)` → identical hash chain (self-consistency oracle).
pub fn run_trajectory(init: PlayerState, seed: u32, frames: usize) -> Vec<u64> {
    let mut st = init;
    let mut chain = Vec::with_capacity(frames);
    for (held, pressed) in fuzz_inputs(seed, frames) {
        // Alternate grounded/airborne collision like a flat floor every
        // ~30 frames so landings exercise the grounded path.
        st.coll = if st.y >= 0x90 && (st.vspeed as i8) >= 0 {
            COLL_BELOW
        } else {
            0
        };
        st.step(held, pressed);
        chain.push(st.hash());
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vel_split_matches_d1ce() {
        // $18 → lo $80, hi $01.
        assert_eq!(vel_split(0x18), (0x80, 0x01, 0x00));
        // $E8 (-24) → lo $80, hi $FE sign-extended, page $FF.
        assert_eq!(vel_split(0xE8), (0x80, 0xFE, 0xFF));
        assert_eq!(vel_split(0x00), (0x00, 0x00, 0x00));
    }

    #[test]
    fn subpixel_accumulates_like_hardware() {
        // 16 frames at $18: sub $80×16 wraps, x advances 24px total.
        let (mut lo, mut hi, mut sub) = (0u8, 0u8, 0u8);
        for _ in 0..16 {
            let r = move_x_subpixel(lo, hi, sub, 0x18);
            lo = r.0;
            hi = r.1;
            sub = r.2;
        }
        assert_eq!((lo, hi, sub), (0x18, 0x00, 0x00));
    }

    #[test]
    fn gravity_clamps_at_max_fall() {
        // Counter overflow carries into vspeed; hitting max clears counter.
        let (_, _, ctr, vs) = apply_gravity(0x80, 0x01, 0x3F, 0xFF, 0x08, 0x40);
        assert_eq!((ctr, vs), (0x00, 0x40));
        // No overflow: counter accumulates, vspeed holds.
        let (_, _, ctr2, vs2) = apply_gravity(0x80, 0x01, 0x10, 0x00, 0x08, 0x40);
        assert_eq!((ctr2, vs2), (0x08, 0x10));
    }

    #[test]
    fn damage_table_spot_checks() {
        assert_eq!(ENEMY_DAMAGE[0][0], 0x10);
        assert_eq!(damage_to_link(0, 1, false), 0x10);
        assert_eq!(damage_to_link(0, 1, true), 0x08);
        assert_eq!(damage_to_link(3, 8, false), 0x18);
    }

    #[test]
    fn hit_tick_immunity_skips_damage() {
        let o = link_hit_tick(0x80, 0x04, 0, 1, false, true, 0x06);
        assert!(!o.damaged);
        assert_eq!(o.exp_loss, 0x14);
        assert_eq!(o.hp, 0x80);
    }

    #[test]
    fn thrust_gating() {
        let t = thrust_avail(THRUST_UP | THRUST_DOWN, 1, BTN_UP);
        assert!(t.up && !t.down);
        let g = thrust_avail(THRUST_UP | THRUST_DOWN, 0, BTN_DOWN);
        assert!(!g.up && !g.down);
    }

    #[test]
    fn fairy_cast_floor() {
        assert_eq!(fairy_cast(0, 0x10), 0);
        assert_eq!(fairy_cast(0, 0x20), 8);
        assert_eq!(fairy_revert(8, 0), 0);
        assert_eq!(fairy_revert(8, 0xFF), 8);
    }

    #[test]
    fn trajectory_self_consistent() {
        let a = run_trajectory(PlayerState::synthetic(0), 7, 60);
        let b = run_trajectory(PlayerState::synthetic(0), 7, 60);
        assert_eq!(a, b);
        let c = run_trajectory(PlayerState::synthetic(0), 8, 60);
        assert_ne!(a, c);
    }
}
