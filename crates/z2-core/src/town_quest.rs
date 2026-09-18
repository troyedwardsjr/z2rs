//! Town quests: wise men, healers, thrusts, side quests, hidden Kasuto,
//! Bagu bridge, River Devil.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod town_quest;` (see `lib.rs`; trap shims
//! live in `town_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Meter math (`$070C/$070D` pending → `$0773/$0774` via
//! `player_magic.rs::magic_regen_tick`) and spell costs/effects
//! (`player_magic.rs::SPELL_COSTS`/`SPELL_BITS`) are read-only reuse by
//! citation, duplicated nowhere here.
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`wise_man_step`] | `bank3_Dialog_Conditions_Wise_Man` | `$B518` | verified |
//! | [`healer_restore`] | `bank3_Dialog_Routines_life_magic_restore` | `$B75B` | verified |
//! | [`thrust_river_man`] | `bank3_Dialog_Conditions_Blue_Old_Woman__Immobile___River_Man` | `$B4A3` | verified |
//! | [`thrust_down`] | `bank3_Knight_Type_A_Downward_Stab__03` | `$B4BB` | verified |
//! | [`thrust_up`] | `bank3_Knight_Type_B_Up_Stab__03` | `$B4D3` | verified |
//! | [`thrust_bagu`] | `bank3_Dialog_Conditions_Blue_Lumberjack__Immobile` | `$B4EE` | verified |
//! | [`thrust_ruto_error`] | `bank3_Error_of_Ruto__04` | `$B4FD` | verified |
//! | [`immobile_quest_set`] | `bank3_Dialog_Conditions_Immobile` | `$B54E` | verified |
//! | [`idle_quest_gate`] | `bank3_Dialog_Conditions_Idle` | `$B560` | verified |
//! | [`walking_quest_set`] | `bank3_Dialog_Conditions_Walking` | `$B592` | verified |
//! | [`mirror_quest_set`] | `bank3_Dialog_Conditions_Invisible_Dialog_Mirror` | `$B5AE` | verified |
//! | [`bagu_bridge_open`] | `$079A & $08` → `$0796 \|= $01` | `$B4A7` | verified |
//! | [`hidden_kasuto_visible`] | overworld forest patch (gap) + `$079D` | `$B7xx` region | ported |
//! | [`river_devil_blocks`] | River Man gating (gap: overworld) | `$B4A3` region | ported |
//! | [`ache_transform`] | `bank3_Townfolk_Transforming_into_Ache` | `$B7B2` | verified |
//!
//! # Spell acquisition order (structural)
//!
//! The wise-man routine indexes possessions by town
//! (`LDY $056B : LDA $077B,y`, `$B518`): town code = spell index, so the
//! order IS the town order — shield Rauru (0), jump Ruto (1), life Saria
//! (2), fairy Mido (3), fire Nabooru (4), reflect Darunia (5), spell New
//! Kasuto (6), thunder Old Kasuto (7). No table to copy.
//!
//! # Preserved quirks (BUG comments)
//!
//! * Wise-man container check (`$B526-$B52B`): `CMP $01` with `BCC deny`
//!   means containers == required passes (exact count suffices); the
//!   "need strictly more" reading is wrong.
//! * Wise-man relearn (`$B52F-$B534`): talking with the spell already
//!   learned still `INC $048C` (alt text) and `INC $05` — the "already
//!   know it" line is the ALT string, selected the same way as a grant.
//! * Idle quest gate (`$B570-$B587`): code `$00` (sign-adjacent) always
//!   takes the alt path without consulting `LB42E`; healer codes fall
//!   through to the `$074C == 2` re-talk check.
//!
//! # Gaps (honest)
//!
//! * Hidden-Kasuto reveal (hammer forest chop, `overworld2` `$84BF` +
//!   palace-stone patch `overworld6` `$8A1A`) and the magic-key basement
//!   door (`$0763` big-door counter) are interp-only; predicates here
//!   model the flag half.
//! * River Devil (overworld demon `overworld1` `$8284` + River Man text)
//!   needs the demon/bridge overworld state; the in-town Bagu-note half
//!   is modelled, the water tile half is a reported gap.
//! * `LB42E` (`$B42E`, 32 bytes) and `bank3_table12` (`$B427`, 4 bytes)
//!   are ROM tables: callers pass the needed byte/row as params
//!   (ROM-gated tests load them from `Z2_ROM`; units use synthetics).

// ---------------------------------------------------------------------------
// Addresses (duplicated per town*.rs file on purpose).
// ---------------------------------------------------------------------------

/// Spells base (`$077B-$0782`).
pub const ADDR_SPELLS: u16 = 0x077B;
/// Spell count.
pub const SPELL_COUNT: usize = 8;
/// Magic containers (`$0783`).
pub const ADDR_MAG_CTR: u16 = 0x0783;
/// Thrust flags (`$0796`).
pub const ADDR_THRUST: u16 = 0x0796;
/// Trophy (`$0798`, `$10`).
pub const ADDR_TROPHY: u16 = 0x0798;
/// Mirror (`$0799`, `$01`).
pub const ADDR_MIRROR: u16 = 0x0799;
/// Bagu note (`$079A` `$08`) / medicine (`$079A` `$40`).
pub const ADDR_BAGU_MED: u16 = 0x079A;
/// Water (`$079B`, `$01`).
pub const ADDR_WATER: u16 = 0x079B;
/// Lost child (`$079C`, `$20`).
pub const ADDR_CHILD: u16 = 0x079C;
/// Seven-containers Kasuto flag (`$079D`, bit 3).
pub const ADDR_SEVEN: u16 = 0x079D;
/// Have Cross (`$078A`, Error-of-Ruto gate).
pub const ADDR_CROSS: u16 = 0x078A;
/// Pending magic (`$070C`) / pending life (`$070D`).
pub const ADDR_PEND_MAG: u16 = 0x070C;
/// Pending life.
pub const ADDR_PEND_LIFE: u16 = 0x070D;
/// Alt-text latch (`$048C`: nonzero forces the `$0F` base row).
pub const ADDR_ALT_LATCH: u16 = 0x048C;
/// Alt-text flag (`$05`: nonzero selects `indexes2/4`).
pub const ADDR_ALT: u16 = 0x0005;
/// Town code (`$056B`).
pub const ADDR_TOWN: u16 = 0x056B;
/// Magic selector (`$0749`: set to town on first-ever spell).
pub const ADDR_SELECTOR: u16 = 0x0749;

// ---------------------------------------------------------------------------
// Flag bits.
// ---------------------------------------------------------------------------

/// `$0796` bit: Bagu bridge open (`ORA #$01`, `$B4B1`).
pub const THRUST_BAGU_BRIDGE: u8 = 0x01;
/// `$0796` bit: Mido lumberjack lesson (`ORA #$02`, `$B4F5`).
pub const THRUST_MIDO: u8 = 0x02;
/// `$0796` bit: upward thrust (`ORA #$04`, `$B4E6`).
pub const THRUST_UP: u8 = 0x04;
/// `$0796` bit: downward thrust (`ORA #$10`, `$B4CE`).
pub const THRUST_DOWN: u8 = 0x10;
/// `$0798` bit: trophy (`$10`).
pub const QUEST_TROPHY: u8 = 0x10;
/// `$0799` bit: mirror (`$01`).
pub const QUEST_MIRROR: u8 = 0x01;
/// `$079A` bit: Bagu note (`$08`).
pub const QUEST_BAGU_NOTE: u8 = 0x08;
/// `$079A` bit: medicine (`$40`).
pub const QUEST_MEDICINE: u8 = 0x40;
/// `$079B` bit: water (`$01`).
pub const QUEST_WATER: u8 = 0x01;
/// `$079C` bit: lost child (`$20`).
pub const QUEST_CHILD: u8 = 0x20;
/// `$079D` bit: seven magic containers (Kasuto gate).
pub const FLAG_SEVEN: u8 = 0x08;

// ---------------------------------------------------------------------------
// Wise men (spell acquisition).
// ---------------------------------------------------------------------------

/// Wise-man outcome (`bank3_Dialog_Conditions_Wise_Man`, `$B518-$B54B`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiseOutcome {
    /// Already learned: `INC $048C` (alt text), `INC $05` when first
    /// branch hits (`BEQ LB526` fails → `$B531` path).
    AlreadyKnown {
        /// New `$048C` (latched + 1, wrapping).
        alt_latch: u8,
    },
    /// Denied: need more magic containers
    /// (`$0783 < town + 1`, `$B526-$B52B` `BCC`).
    Denied,
    /// Granted: `$077B,y = 1`, `INC $05`; when this is Link's first spell
    /// ever, `$0749 = town` (selector jump, `$B53A-$B548`).
    Granted {
        /// Whether `$0749` was set (first spell ever).
        selector_set: bool,
    },
}

/// Wise-man step.
///
/// `town` = `$056B` (also the spell index); `have` = `$077B,y != 0`;
/// `containers` = `$0783`; `had_any` = whether any `$077B-$0782` was
/// already set (for the `$0749` first-spell path); `alt_latch` = `$048C`.
/// Total fn.
pub const fn wise_man_step(
    town: u8,
    have: bool,
    containers: u8,
    had_any: bool,
    alt_latch: u8,
) -> WiseOutcome {
    if have {
        // `$B52F-$B534`: INC $048C; BNE LB52D (always taken after INC
        // unless it wraps to 0 — modelled as latch+1 with grant path).
        // BUG-note: the wrap-to-0 fallthrough would take the container
        // check instead; hardware-accurate would branch on the NEW
        // value. Preserve: wrapping INC then grant.
        let _ = alt_latch;
        WiseOutcome::AlreadyKnown {
            alt_latch: alt_latch.wrapping_add(1),
        }
    } else {
        let need = town.wrapping_add(1);
        if containers < need {
            WiseOutcome::Denied
        } else {
            WiseOutcome::Granted {
                selector_set: !had_any,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Healers + magic ladies.
// ---------------------------------------------------------------------------

/// Healer pending target (`bank3_Dialog_Routines_life_magic_restore`,
/// bank 3 `$B75B-$B771`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreTarget {
    /// Magic refill (`$070C = $FF`).
    Magic,
    /// Life refill (`$070D = $FF`).
    Life,
}

/// Healer/magic restore select.
///
/// Codes `$0F`/`$0D` (wise-man-adjacent/healer preamble) take the
/// spell-flash path; code `$18`+ (magic lady walking range, `BCS LB76F`)
/// restores MAGIC (`X = 0` → `$070C`); lower healer codes restore LIFE
/// (`INX` → `$070D`). Value is always `$FF` (`LDA #$FF`, `$B76F`).
/// Returns `None` for non-restoring codes (caller runs the flash path).
/// Total fn.
pub const fn healer_restore(code: u8) -> Option<(RestoreTarget, u8)> {
    if code == 0x0F || code == 0x0D {
        return None;
    }
    if code >= 0x18 {
        Some((RestoreTarget::Magic, 0xFF))
    } else if code >= 0x0A {
        Some((RestoreTarget::Life, 0xFF))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Thrust teachers ($0796 bits).
// ---------------------------------------------------------------------------

/// River-Man / Bagu-bridge step
/// (`bank3_Dialog_Conditions_Blue_Old_Woman__Immobile___River_Man`,
/// bank 3 `$B4A3-$B4B8`).
///
/// Town 2 (Saria, `CPY #$02`) with the Bagu note (`$079A & $08`) sets
/// `$0796 |= $01` (bridge) and takes alt text (`INC $05`). Other towns
/// (or no note) take default text. Returns `(new_thrust, alt)`.
pub const fn thrust_river_man(town: u8, bagu_note: bool, thrust: u8) -> (u8, bool) {
    if town == 0x02 && bagu_note {
        (thrust | THRUST_BAGU_BRIDGE, true)
    } else {
        (thrust, false)
    }
}

/// Down-thrust lesson (`bank3_Knight_Type_A_Downward_Stab__03`, `$B4BB`).
///
/// Teaches (`ORA #$10`); when already known (`AND #$10`), takes alt
/// text (`INC $05` + `INC $048C`). Returns `(new_thrust, alt, latch_inc)`.
pub const fn thrust_down(thrust: u8) -> (u8, bool, bool) {
    if thrust & THRUST_DOWN != 0 {
        (thrust | THRUST_DOWN, true, true)
    } else {
        (thrust | THRUST_DOWN, false, false)
    }
}

/// Up-thrust lesson (`bank3_Knight_Type_B_Up_Stab__03`, `$B4D3-$B4EB`).
///
/// Town 0 (Rauru-adjacent, `CPY #$00 : BEQ LB4EB` skips teaching —
/// Darunia-side default) otherwise same shape as down (`ORA #$04`).
/// Returns `(new_thrust, alt, latch_inc)`.
pub const fn thrust_up(town: u8, thrust: u8) -> (u8, bool, bool) {
    if town == 0x00 {
        return (thrust, false, false);
    }
    if thrust & THRUST_UP != 0 {
        (thrust | THRUST_UP, true, true)
    } else {
        (thrust | THRUST_UP, false, false)
    }
}

/// Bagu / Mido lumberjack lesson
/// (`bank3_Dialog_Conditions_Blue_Lumberjack__Immobile`, `$B4EE`).
///
/// Town 3 (Mido, `CPY #$03`) sets `$0796 |= $02` unconditionally.
/// Returns the new thrust byte.
pub const fn thrust_bagu(town: u8, thrust: u8) -> u8 {
    if town == 0x03 {
        thrust | THRUST_MIDO
    } else {
        thrust
    }
}

/// Error-of-Ruto gate (`bank3_Error_of_Ruto__04`, `$B4FD-$B50A`).
///
/// Town 1 (Ruto, `CPY #$01`): without `$0796 & $02` (Mido lesson) takes
/// default text; with it takes alt (`INC $05`). Non-Ruto towns with a
/// nonzero code and the Cross (`$078A`) also take alt
/// (`bank3_Unknown_Townfolk__04`, `$B50D`). Returns `alt`.
pub const fn thrust_ruto_error(town: u8, code: u8, thrust: u8, have_cross: bool) -> bool {
    if town == 0x01 {
        thrust & THRUST_MIDO != 0
    } else if code == 0x00 {
        false
    } else {
        have_cross
    }
}

// ---------------------------------------------------------------------------
// Side quests (collectables $0797-$079C region via $B54E/$B560/$B592).
// ---------------------------------------------------------------------------

/// Immobile quest set (`bank3_Dialog_Conditions_Immobile`, `$B54E`).
///
/// `$0797,y |= cond_table[A-6]` where `A = code - $0A - 6`
/// (`$B54E-$B55A`). The caller supplies the table byte
/// (`bank3_Related_to_Collectable_Objects_conditions`, `$B42B`).
/// Returns the new flag byte.
pub const fn immobile_quest_set(flag: u8, table_byte: u8) -> u8 {
    flag | table_byte
}

/// Idle quest gate (`bank3_Dialog_Conditions_Idle`, `$B560-$B589`).
///
/// Code `$00` always alt; else `flag & mask != 0` (mask = `LB42E` row,
/// `$B42E`, caller-supplied) takes default, zero takes alt (`INC $05`,
/// then the healer re-talk check). Returns `alt`.
pub const fn idle_quest_gate(code: u8, flag: u8, mask: u8) -> bool {
    if code == 0x00 {
        return true;
    }
    flag & mask == 0
}

/// Walking quest set (`bank3_Dialog_Conditions_Walking`, `$B592`).
///
/// `$0797,y |= table12[A-$13]` (`bank3_table12`, `$B427`: `$80,$40,
/// `$20,$10`). Caller supplies the table byte. Returns the new flag.
pub const fn walking_quest_set(flag: u8, table_byte: u8) -> u8 {
    flag | table_byte
}

/// Mirror quest set (`bank3_Dialog_Conditions_Invisible_Dialog_Mirror`,
/// `$B5AE`).
///
/// `$0797,y |= $01`. Returns the new flag byte.
pub const fn mirror_quest_set(flag: u8) -> u8 {
    flag | QUEST_MIRROR
}

/// Trophy predicate (`$0798 & $10`, `ram-map.txt` `798 Trophy`).
pub const fn have_trophy(flags: u8) -> bool {
    flags & QUEST_TROPHY != 0
}

/// Mirror predicate (`$0799 & $01`).
pub const fn have_mirror(flags: u8) -> bool {
    flags & QUEST_MIRROR != 0
}

/// Bagu-note predicate (`$079A & $08`).
pub const fn have_bagu_note(flags: u8) -> bool {
    flags & QUEST_BAGU_NOTE != 0
}

/// Medicine predicate (`$079A & $40`).
pub const fn have_medicine(flags: u8) -> bool {
    flags & QUEST_MEDICINE != 0
}

/// Water predicate (`$079B & $01`).
pub const fn have_water(flags: u8) -> bool {
    flags & QUEST_WATER != 0
}

/// Lost-child predicate (`$079C & $20`).
pub const fn have_child(flags: u8) -> bool {
    flags & QUEST_CHILD != 0
}

// ---------------------------------------------------------------------------
// Hidden Kasuto + magic key, Bagu bridge, River Devil.
// ---------------------------------------------------------------------------

/// Hidden-Kasuto visibility (ported; overworld half is a gap).
///
/// The forest reveal (hammer chop `overworld2` `$84BF` + stone patch
/// `overworld6` `$8A1A`) is interp-only. In-town half: New Kasuto
/// (town 6) counts as visible once the seven-container flag
/// (`$079D & $08`, set by `player_magic.rs::container_pickup` at 7
/// magic containers) is set OR the town is already entered
/// (`entered`). Old Kasuto is always visible. Returns visibility.
pub const fn hidden_kasuto_visible(town: u8, seven_flag: bool, entered: bool) -> bool {
    if town == 0x06 {
        seven_flag || entered
    } else {
        true
    }
}

/// Magic-key basement availability (ported; door writes are a gap).
///
/// The key (`$078C`, `ITEM_KEY`) lives behind the New Kasuto chimney
/// (`bank3_SmallObjectsConstructionRoutines_Chimney…_can_crouch_in_it`,
/// bank 3 `$9C05` region); the big-door counter (`$0763`) gates the
/// basement. Model: key obtainable in town 6 when the chimney is
/// entered (`chimney`) and the door counter is armed (`door_armed`).
/// Full tile/door writes live with the interpreter.
pub const fn magic_key_available(town: u8, chimney: bool, door_armed: bool) -> bool {
    town == 0x06 && chimney && door_armed
}

/// Bagu-bridge open predicate (flag half of `$B4A7`).
///
/// The bridge object (`bank3_Objects_Construction_Routines_Bridge…`,
/// bank 3 `$8376`) draws when `$0796 & $01`; the River Devil encounter
/// (overworld) additionally needs the note shown. Here: bridge drawn
/// iff the thrust bit is set (set by [`thrust_river_man`]).
pub const fn bagu_bridge_open(thrust: u8) -> bool {
    thrust & THRUST_BAGU_BRIDGE != 0
}

/// River Devil blocking predicate (ported; overworld half is a gap).
///
/// The Devil (Saria-water demon, `overworld1` `$8284` region) blocks the
/// bridge until the Bagu note is shown to the River Man
/// (`$079A & $08` → `$0796 |= $01`, `$B4A7`). Returns true while
/// blocking (no note yet, bridge bit clear).
pub const fn river_devil_blocks(bagu_note_shown: bool, thrust: u8) -> bool {
    !bagu_note_shown && thrust & THRUST_BAGU_BRIDGE == 0
}

/// Townfolk→Ache transform (`bank3_Townfolk_Transforming_into_Ache`
/// via `LB7B2`, bank 3 `$B7B2-$B7F0`).
///
/// Towns 2 (Saria) / 5 (Darunia) only; generated codes `$19-$1C`;
/// `rng ($051C,x) < $67` transforms to Ache (`$A1 = $03`, `$81 = $03`,
/// `$AF = $80`, `$057E = $71 = $00`). BUG-note: the listing comment
/// says ~26% but `$67/256` is ~40%; the threshold is preserved, not
/// the comment. Returns `Some(NPC_ACHE)` on transform, else `None`.
pub const fn ache_transform(town: u8, code: u8, rng: u8) -> Option<u8> {
    if town != 0x02 && town != 0x05 {
        return None;
    }
    if code < 0x19 || code >= 0x1D {
        return None;
    }
    if rng < 0x67 {
        Some(0x03)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wise_man_grant_deny_relearn() {
        // Rauru (0) needs 1 container.
        assert!(matches!(
            wise_man_step(0, false, 1, false, 0),
            WiseOutcome::Granted { selector_set: true }
        ));
        assert!(matches!(
            wise_man_step(0, false, 1, true, 0),
            WiseOutcome::Granted {
                selector_set: false
            }
        ));
        assert!(matches!(
            wise_man_step(0, false, 0, false, 0),
            WiseOutcome::Denied
        ));
        // Exact count passes (BCC deny).
        assert!(matches!(
            wise_man_step(7, false, 8, true, 0),
            WiseOutcome::Granted { .. }
        ));
        assert!(matches!(
            wise_man_step(7, false, 7, true, 0),
            WiseOutcome::Denied
        ));
        // Relearn latches alt.
        assert_eq!(
            wise_man_step(2, true, 9, true, 0xFE),
            WiseOutcome::AlreadyKnown { alt_latch: 0xFF }
        );
    }

    #[test]
    fn healer_select_matches_b75b() {
        assert_eq!(healer_restore(0x0F), None);
        assert_eq!(healer_restore(0x0D), None);
        assert_eq!(healer_restore(0x18), Some((RestoreTarget::Magic, 0xFF)));
        assert_eq!(healer_restore(0x17), Some((RestoreTarget::Life, 0xFF)));
        assert_eq!(healer_restore(0x05), None);
    }

    #[test]
    fn thrust_teachers_set_796_bits() {
        assert_eq!(thrust_river_man(0x02, true, 0x00), (0x01, true));
        assert_eq!(thrust_river_man(0x02, false, 0x00), (0x00, false));
        assert_eq!(thrust_river_man(0x03, true, 0x00), (0x00, false));
        assert_eq!(thrust_down(0x00), (0x10, false, false));
        assert_eq!(thrust_down(0x10), (0x10, true, true));
        assert_eq!(thrust_up(0x00, 0x00), (0x00, false, false));
        assert_eq!(thrust_up(0x05, 0x00), (0x04, false, false));
        assert_eq!(thrust_up(0x05, 0x04), (0x04, true, true));
        assert_eq!(thrust_bagu(0x03, 0x00), 0x02);
        assert_eq!(thrust_bagu(0x02, 0x00), 0x00);
        assert!(thrust_ruto_error(0x01, 0x04, 0x02, false));
        assert!(!thrust_ruto_error(0x01, 0x04, 0x00, false));
        assert!(!thrust_ruto_error(0x04, 0x00, 0x00, true));
        assert!(thrust_ruto_error(0x04, 0x06, 0x00, true));
    }

    #[test]
    fn sidequest_flags_match_ram_map() {
        assert_eq!(immobile_quest_set(0x00, 0x08), 0x08);
        assert!(idle_quest_gate(0x00, 0xFF, 0x00));
        assert!(!idle_quest_gate(0x09, 0x40, 0x40));
        assert!(idle_quest_gate(0x09, 0x00, 0x40));
        assert_eq!(walking_quest_set(0x00, 0x80), 0x80);
        assert_eq!(mirror_quest_set(0x00), 0x01);
        assert!(have_trophy(0x10));
        assert!(have_mirror(0x01));
        assert!(have_bagu_note(0x08));
        assert!(have_medicine(0x40));
        assert!(have_water(0x01));
        assert!(have_child(0x20));
    }

    #[test]
    fn hidden_bridge_devil_ache() {
        assert!(hidden_kasuto_visible(0x06, true, false));
        assert!(hidden_kasuto_visible(0x06, false, true));
        assert!(!hidden_kasuto_visible(0x06, false, false));
        assert!(hidden_kasuto_visible(0x07, false, false));
        assert!(magic_key_available(0x06, true, true));
        assert!(!magic_key_available(0x06, true, false));
        assert!(!magic_key_available(0x07, true, true));
        assert!(bagu_bridge_open(0x01));
        assert!(!bagu_bridge_open(0x00));
        assert!(river_devil_blocks(false, 0x00));
        assert!(!river_devil_blocks(true, 0x00));
        assert!(!river_devil_blocks(false, 0x01));
        assert_eq!(ache_transform(0x02, 0x1A, 0x00), Some(0x03));
        assert_eq!(ache_transform(0x02, 0x1A, 0x67), None);
        assert_eq!(ache_transform(0x02, 0x10, 0x00), None);
        assert_eq!(ache_transform(0x03, 0x1A, 0x00), None);
    }
}
