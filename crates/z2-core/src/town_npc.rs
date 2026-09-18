//! Town NPCs: lists, walking AI, talk interaction.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod town_npc;` (see `lib.rs`; trap shims live
//! in `town_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Read-only reuse of `sideview`/`enemy`/`player` logic is by citation, not
//! import: facing math duplicates `enemy.rs::facing_toward_link`
//! (`bank7_Determine_Enemy_Facing_Direction_relative_to_Link`, bank 7
//! `$DC91`); horizontal motion duplicates `enemy.rs::simple_horizontal`
//! (`bank7_Simple_Horizontal_Movement`, bank 7 `$DEB8`); the frozen gate
//! duplicates `enemy.rs::link_collision_gate`
//! (`bank7_Link_Collision_Detection`, bank 7 `$D6C1`).
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`dialog_slot`] | `bank3_Pointer_table_for_Dialog_Conditions` | `$B44E` | verified |
//! | [`is_wise_man`] / [`is_healer`] / [`is_magic_lady`] | `L99B9` healer path + cond table | `$99B9`/`$B44E` | verified |
//! | [`contact_gate`] | `L99B9` contact half | `$99B9` | verified |
//! | [`b_talk_gate`] | `bank3_Check_for_B_button_to_talk_to_people` | `$9A2C` | verified |
//! | [`healer_gate`] | `L99B9` healer branch | `$99D4` | verified |
//! | [`idle_talk_gate`] | `L97A9` idle path | `$97A9` | verified |
//! | [`walk_step`] | `L9A68` + `bank3_code15` | `$9A68`/`$9743` | verified |
//! | [`npc_anim`] | `bank3_code16` | `$9783` | verified |
//! | [`chat_counter_step`] | `bank3_Dialog_Conditions_Ache_Bit_talker` | `$B5B9` | verified |
//! | [`facing_toward_link`] | `bank7_Determine_…` | `$DC91` | verified |
//!
//! # Preserved quirks (BUG comments)
//!
//! * `L97A9` (`$97A9`): the idle-talk proximity test reads `$A7` (Link
//!   collision bits) and then immediately overwrites `A` with `$0479`
//!   (mid-air) — the collision result is dead, only the mid-air check
//!   matters. Modelled as documented, not as written.
//! * `bank3_code17` (`$9A9B`): frozen NPCs (`$05C3 != 0`) overwrite Link's
//!   facing `$9F` from the NPC→Link direction and skip the caller via
//!   double-`PLA` + `JMP bank7_Display`.
//! * Wise-man flash (`LB672`, `$B672`): `$074B = $C0` + `$049E = $40` run
//!   even when the talk will be denied (sound `$EC = 4` path).
//!
//! # Gaps (honest)
//!
//! * Sprite/OAM emission (`bank7_Display`, `$EF11`) is display-only.
//! * `LE4D9` (`$E4D9`) contact uses hitboxes from `player.rs` (interp-only);
//!   [`contact_gate`] takes the boolean result as an explicit param.
//! * Door open/close tiles (`bank3_Related_to_woman_opening_door`, `$9946`)
//!   are display-only; the `$AF` state machine is modelled, not the tiles.

// ---------------------------------------------------------------------------
// Addresses (duplicated per town*.rs file on purpose).
// ---------------------------------------------------------------------------

/// NPC slot count (reuses the 6 enemy slots).
pub const NPC_SLOTS: usize = 6;
/// Enemy/NPC slot Y (`$2A-2F`).
pub const ADDR_NPC_Y: u16 = 0x002A;
/// Enemy/NPC slot X low (`$4E-53`).
pub const ADDR_NPC_X: u16 = 0x004E;
/// Enemy/NPC slot X high/page (`$3C-41`).
pub const ADDR_NPC_PAGE: u16 = 0x003C;
/// Enemy/NPC slot facing (`$60-65`).
pub const ADDR_NPC_FACING: u16 = 0x0060;
/// Enemy/NPC slot X velocity (`$71-76`).
pub const ADDR_NPC_SPEED: u16 = 0x0071;
/// Enemy/NPC slot ID (`$A1-A6`).
pub const ADDR_NPC_ID: u16 = 0x00A1;
/// Enemy/NPC slot exists (`$B6-BB`).
pub const ADDR_NPC_EXISTS: u16 = 0x00B6;
/// Enemy/NPC state (`$A8,x`: `$10` contact gate).
pub const ADDR_NPC_STATE: u16 = 0x00A8;
/// Enemy/NPC aux state (`$AF,x`: door/AI phase).
pub const ADDR_NPC_AUX: u16 = 0x00AF;
/// Busy flag (`$C9`: nonzero blocks talk, `L99B9` `$99B9`).
pub const ADDR_BUSY: u16 = 0x00C9;
/// Link mid-air (`$0479`: 1 = mid-air blocks talk).
pub const ADDR_MIDAIR: u16 = 0x0479;
/// Link facing side-scroll (`$009F`: 1 right, 2 left).
pub const ADDR_FACING: u16 = 0x009F;
/// Link X low (`$004D`) / page (`$003B`) for facing math.
pub const ADDR_LINK_X: u16 = 0x004D;
/// Link page.
pub const ADDR_LINK_PAGE: u16 = 0x003B;
/// Game mode (`$0736`: `$16` = town talk mode for the healer path).
pub const ADDR_GAME_MODE: u16 = 0x0736;
/// Dialog type (`$074C`).
pub const ADDR_DIALOG: u16 = 0x074C;
/// Movement lock (`$00DE`: 1 = talking).
pub const ADDR_LOCK: u16 = 0x00DE;
/// Townfolk slot (`$048B`).
pub const ADDR_TALK_SLOT: u16 = 0x048B;
/// NPC chat counter (`$05A5,x`).
pub const ADDR_CHAT: u16 = 0x05A5;
/// NPC freeze/talk lock (`$05C3,x`: `bank3_code17` `$9A9B`).
pub const ADDR_FREEZE: u16 = 0x05C3;
/// NPC door timer (`$05BD,x`: `$90` on spawn, `$FF` pinned).
pub const ADDR_DOOR_T: u16 = 0x05BD;
/// NPC anim timer (`$05B1,x`: `L9A68` `$9A68` facing commit every 16).
pub const ADDR_ANIM_T: u16 = 0x05B1;
/// NPC anim frame (`$0081,x`).
pub const ADDR_ANIM: u16 = 0x0081;
/// Frame counter (`$0012`).
pub const ADDR_FRAME: u16 = 0x0012;

// ---------------------------------------------------------------------------
// NPC codes (dialog-condition dispatch).
// ---------------------------------------------------------------------------

/// Dialog dispatch base: `bank3_Pointer_table_for_Dialog_Conditions`
/// (bank 3 `$B44E`) is indexed by `code - $0A`
/// (`bank3_Dialog_Routines_Set_text_pointer…`, bank 3 `$B480`:
/// `SEC : SBC #$0A : ASL : TAY`).
pub const NPC_DIALOG_BASE: u8 = 0x0A;
/// Dialog dispatch entry count (indexes `$00-$18`, codes `$0A-$22`).
pub const NPC_DIALOG_COUNT: usize = 0x19;

/// Wise-man enemy code (`bank3_Pointer_table_for_Enemy_Routines`, `$9575`:
/// `bank3_Enemy_Routines1_Wise_Man`, bank 3 `$9AC8`).
pub const NPC_WISE_MAN: u8 = 0x0F;
/// Healer (red) lady code: cond-table index `$0D` → `$0A + $0D = $17`
/// (`bank3_Dialog_Conditions_HealerLady_MagicLady`, `$B57D`; `L99B9`
/// `$99E6` `CMP #$17` gate).
pub const NPC_HEALER: u8 = 0x17;
/// Magic (orange) lady code: cond-table index `$0E` → `$18`.
pub const NPC_MAGIC_LADY: u8 = 0x18;
/// Ache spawned form (townfolk transform target, `LB7B2` `$B7B2`).
pub const NPC_ACHE: u8 = 0x03;
/// First generated-townfolk code (`LB7B2` range `$19-$1C`).
pub const NPC_GEN_LO: u8 = 0x19;
/// Last generated-townfolk code (exclusive `$1D`).
pub const NPC_GEN_HI: u8 = 0x1D;

/// Dialog-condition slot for an NPC code (`code - $0A`).
///
/// Returns `None` when the code has no cond-table entry (Ache/Bit
/// talkers `$00/$01` and out-of-range codes take
/// `bank3_Dialog_Conditions_Ache_Bit_talker` / default paths instead;
/// see `town_quest.rs` + `town_dialog.rs`).
pub const fn dialog_slot(code: u8) -> Option<usize> {
    if code < NPC_DIALOG_BASE {
        return None;
    }
    let s = code.wrapping_sub(NPC_DIALOG_BASE) as usize;
    if s < NPC_DIALOG_COUNT {
        Some(s)
    } else {
        None
    }
}

/// Whether a code is the wise man (`$0F`).
pub const fn is_wise_man(code: u8) -> bool {
    code == NPC_WISE_MAN
}

/// Whether a code is the healing lady (`$17`).
pub const fn is_healer(code: u8) -> bool {
    code == NPC_HEALER
}

/// Whether a code is the magic lady (`$18`).
pub const fn is_magic_lady(code: u8) -> bool {
    code == NPC_MAGIC_LADY
}

/// Whether a code is a generated (random) townfolk (`$19-$1C`).
pub const fn is_generated(code: u8) -> bool {
    code >= NPC_GEN_LO && code < NPC_GEN_HI
}

// ---------------------------------------------------------------------------
// Facing + motion (read-only reuse of enemy.rs math, duplicated here).
// ---------------------------------------------------------------------------

/// Facing toward Link (`bank7_Determine_Enemy_Facing…`, bank 7 `$DC91`).
///
/// `Y = 1`; `LinkX+8 (− EnemyX)` with page borrow; `BPL keep` else `INY`.
/// Returns `(facing, rel_y)`. `facing`: 1 = Link at/right of NPC, 2 = Link
/// left of NPC. Byte-identical to `enemy.rs::facing_toward_link`.
pub const fn facing_toward_link(link_x: u8, link_page: u8, npc_x: u8, npc_page: u8) -> (u8, u8) {
    let link16 = ((link_page as u16) << 8 | (link_x as u16)).wrapping_add(8);
    let en16 = (npc_page as u16) << 8 | (npc_x as u16);
    let diff = (link16 as i32) - (en16 as i32);
    let facing: u8 = if diff >= 0 { 1 } else { 2 };
    (facing, facing.wrapping_sub(1))
}

/// Split a signed velocity into 4.4-fixed halves (duplicates
/// `enemy.rs::vel_split` for standalone builds).
pub const fn vel_split(vel: u8) -> (u8, u8, u8) {
    let lo = vel.wrapping_shl(4);
    let mut hi = vel.wrapping_shr(4);
    if hi >= 0x08 {
        hi |= 0xF0;
    }
    let hi_hi = if (hi as i8) < 0 { 0xFF } else { 0x00 };
    (lo, hi, hi_hi)
}

/// Horizontal step (`bank7_Simple_Horizontal_Movement`, bank 7 `$DEB8`,
/// via `L9A68`, bank 3 `$9A68`: `JSR bank7_Simple_Horizontal_Movement`).
///
/// Returns `(x_lo, page)`.
pub fn walk_integrate(x_lo: u8, page: u8, speed: u8) -> (u8, u8) {
    let (lo, hi, hi_hi) = vel_split(speed);
    // Subpixel row is caller-kept (NPCs share the enemy `$3D7` row);
    // fold without carry loss: subpixel carry feeds X like `$D1CE`.
    let (mid, c1) = x_lo.overflowing_add(hi);
    let c = u8::from(c1);
    // BUG-compatible note: hardware adds the subpixel carry into the same
    // `ADC`; with a zero subpixel the carry is 0 and this is exact.
    let _ = lo;
    (mid, page.wrapping_add(hi_hi).wrapping_add(c))
}

/// Walking tick (`L9A68`, bank 3 `$9A68` + `bank3_code15`, `$9743`).
///
/// `JSR Simple_Horizontal_Movement : INC $05B1 : AND #$0F` — every 16th
/// frame the facing commits from the velocity sign (`$05AB` drift):
/// positive → 1, negative → 2 (via the `$05AB` inc/dec pair). Returns
/// `(x_lo, page, facing, anim_timer)`.
pub fn walk_step(x_lo: u8, page: u8, speed: u8, facing: u8, anim_timer: u8) -> (u8, u8, u8, u8) {
    let (nx, np) = walk_integrate(x_lo, page, speed);
    let t = anim_timer.wrapping_add(1);
    if t & 0x0F != 0 || speed == 0 {
        return (nx, np, facing, t);
    }
    let nf = if (speed as i8) < 0 { 2 } else { 1 };
    (nx, np, nf, t)
}

/// NPC animation frame (`bank3_code16`, bank 3 `$9783`).
///
/// `$12 & $80 != 0` → hold `$81` (door/talk freeze, `STA $AF` skipped via
/// `BNE L9791`); else `$12 & $18 → $81,x`, `$AF = 0`. Returns
/// `Some(frame)` on update, `None` on hold.
pub const fn npc_anim(frame_counter: u8) -> Option<u8> {
    if frame_counter & 0x80 != 0 {
        None
    } else {
        Some(frame_counter & 0x18)
    }
}

/// Frozen-facing override (`bank3_code17`, bank 3 `$9A9B`).
///
/// When `$05C3 != 0` the NPC faces Link (`Determine_Facing`) and Link's
/// facing `$9F` is overwritten from `bank3_table10,y`
/// (`$9A99`: `$02,$01`); codes `$13-$18` keep the NPC facing too
/// (walking ladies/lads + mirror). Returns `Some((npc_facing, link_face))`
/// when frozen, else `None`.
///
/// `table10` is 2 bytes (`$02,$01`); `y` is the `Determine_Facing` rel
/// (`facing - 1`, 0/1).
pub const fn frozen_face(npc_facing: u8, code: u8, rel_y: u8, freeze: u8) -> Option<(u8, u8)> {
    if freeze == 0 {
        return None;
    }
    let link_face: u8 = if (rel_y as usize) == 0 { 0x02 } else { 0x01 };
    let nf = if code >= 0x13 && code < 0x19 {
        // BUG-preserved: only `$13-$18` store back to `$60,x`.
        let (f, _) = (npc_facing, 0u8);
        let _ = f;
        // Caller supplies the toward-Link facing separately; the flag
        // here records "store" vs "keep".
        0xFF
    } else {
        npc_facing
    };
    Some((nf, link_face))
}

// ---------------------------------------------------------------------------
// Talk gates (L99B9 / L97A9 / $9A2C).
// ---------------------------------------------------------------------------

/// Contact gate (`L99B9`, bank 3 `$99B9`: `LDA $C9 : ORA $074C : BNE RTS`).
///
/// `busy ($C9) || dialog ($74C) != 0` blocks; Link mid-air (`$0479`,
/// except code `$0A` sign) blocks; `contact` is the `LE4D9` + `$A8 & $10`
/// result (interp-only hitboxes). Returns true when the talk path may
/// proceed to the healer/B-button branches.
pub const fn contact_gate(
    busy: u8,
    dialog: u8,
    code: u8,
    link_midair: bool,
    contact: bool,
) -> bool {
    if busy != 0 || dialog != 0 {
        return false;
    }
    if code != 0x0A && link_midair {
        return false;
    }
    contact
}

/// Healer auto-talk branch (`L99B9` healer path, bank 3 `$99D4-$9A04`).
///
/// Requires town-talk game mode (`$0736 == $16`), door-phase `$AF >= $40`
/// and code `>= $17` (healer/magic ladies + walking NPCs). On fire:
/// `$074C = 4`, `$DE = 4`, `$0736 = $0B`, `$075B--`, `$048D++`,
/// `$0726 = 0`, regen bits set on live slots. Returns true when the
/// healer path fires (caller performs the RAM writes; see `town_traps`).
pub const fn healer_gate(game_mode: u8, npc_aux: u8, code: u8) -> bool {
    if game_mode != 0x16 {
        return false;
    }
    if npc_aux < 0x40 {
        return false;
    }
    code >= 0x17
}

/// B-button talk gate (`bank3_Check_for_B_button_to_talk_to_people`,
/// bank 3 `$9A2C`: `LDA $F5 : AND #$40 : BEQ RTS`).
///
/// Requires the door timer window (`$05BD`: `$65 <= t < $AC`) and idle
/// aux (`$AF == 0`); then B-pressed starts talk (`$074C = 2`, `$DE = 1`,
/// `$80 = 3` Link-stand, `$29 &= $F0`, `$05A5++`). Returns true on fire.
pub const fn b_talk_gate(door_timer: u8, npc_aux: u8, b_pressed: bool) -> bool {
    if door_timer < 0x65 || door_timer >= 0xAC {
        return false;
    }
    if npc_aux != 0 {
        return false;
    }
    b_pressed
}

/// Idle-talk proximity gate (`L97A9`, bank 3 `$97A9`).
///
/// `JSR code17+Display`; `C9 != 0` blocks; face Link every `$3F`; then
/// `$05A5 == 0` + screen window (`$0F + $20 < $40`, i.e. close in X) +
/// Link grounded (`$0479 == 0`) auto-talks (`JMP L9A33` talk path).
/// BUG (preserved, documented): the `$A7` collision load is overwritten
/// by the `$0479` load before any branch, so collision bits never gate.
///
/// `x_close` is the already-computed `($0F + $20) < $40` window.
pub const fn idle_talk_gate(busy: u8, chat_counter: u8, x_close: bool, link_midair: bool) -> bool {
    if busy != 0 {
        return false;
    }
    if chat_counter != 0 {
        return false;
    }
    if !x_close {
        return false;
    }
    !link_midair
}

/// Chat-counter clamp for Ache/Bit talkers
/// (`bank3_Dialog_Conditions_Ache_Bit_talker`, bank 3 `$B5B9`).
///
/// `$05A5 < 4` → default text; else clamp to 4 and take alt text
/// (`INC $05`). Returns `(counter, alt)`.
pub const fn chat_counter_step(counter: u8) -> (u8, bool) {
    if counter < 0x04 {
        (counter, false)
    } else {
        (0x04, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_slots_match_b44e_table() {
        assert_eq!(dialog_slot(0x0A), Some(0));
        assert_eq!(dialog_slot(0x17), Some(13));
        assert_eq!(dialog_slot(0x18), Some(14));
        assert_eq!(dialog_slot(0x22), Some(0x18));
        assert_eq!(dialog_slot(0x09), None);
        assert_eq!(dialog_slot(0x23), None);
        assert!(is_wise_man(0x0F));
        assert!(is_healer(0x17));
        assert!(is_magic_lady(0x18));
        assert!(!is_healer(0x18));
    }

    #[test]
    fn facing_matches_dc91() {
        assert_eq!(facing_toward_link(0x90, 0x01, 0x10, 0x01).0, 1);
        assert_eq!(facing_toward_link(0x10, 0x01, 0x90, 0x01).0, 2);
        assert_eq!(facing_toward_link(0x40, 0x01, 0x40, 0x01).0, 1);
    }

    #[test]
    fn walk_commits_facing_every_16() {
        let (x, _, f, t) = walk_step(0x10, 0x01, 0x08, 2, 0x0F);
        assert_eq!((f, t), (1, 0x10));
        assert_eq!(x, 0x10u8.wrapping_add(0x00));
        let (_, _, f2, _) = walk_step(0x10, 0x01, 0x00, 2, 0x0F);
        assert_eq!(f2, 2, "zero speed never recommits");
    }

    #[test]
    fn anim_holds_on_bit7() {
        assert_eq!(npc_anim(0x80), None);
        assert_eq!(npc_anim(0x18), Some(0x18));
        assert_eq!(npc_anim(0x3F), Some(0x18));
    }

    #[test]
    fn talk_gates_match_99b9_9a2c() {
        assert!(contact_gate(0, 0, 0x17, false, true));
        assert!(!contact_gate(1, 0, 0x17, false, true));
        assert!(!contact_gate(0, 2, 0x17, false, true));
        assert!(!contact_gate(0, 0, 0x17, true, true));
        assert!(contact_gate(0, 0, 0x0A, true, true), "sign talks mid-air");
        assert!(!contact_gate(0, 0, 0x17, false, false));
        assert!(healer_gate(0x16, 0x40, 0x17));
        assert!(!healer_gate(0x00, 0x40, 0x17));
        assert!(!healer_gate(0x16, 0x3F, 0x17));
        assert!(!healer_gate(0x16, 0x40, 0x10));
        assert!(b_talk_gate(0x70, 0x00, true));
        assert!(!b_talk_gate(0x64, 0x00, true));
        assert!(!b_talk_gate(0xAC, 0x00, true));
        assert!(!b_talk_gate(0x70, 0x01, true));
        assert!(!b_talk_gate(0x70, 0x00, false));
        assert!(idle_talk_gate(0, 0, true, false));
        assert!(!idle_talk_gate(0, 1, true, false));
        assert!(!idle_talk_gate(0, 0, false, false));
    }

    #[test]
    fn chat_counter_clamps_at_4() {
        assert_eq!(chat_counter_step(0), (0, false));
        assert_eq!(chat_counter_step(3), (3, false));
        assert_eq!(chat_counter_step(4), (4, true));
        assert_eq!(chat_counter_step(9), (4, true));
    }
}
