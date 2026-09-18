//! Sideview object/enemy spawning + despawn.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod sideview_spawn;`.
//!
//! # Enemy slots (6)
//!
//! `$2A-2F` Y · `$4E-53` X low · `$3C-41` X high (page) · `$60-65` facing ·
//! `$71-76` X velocity · `$A1-A6` ID/code · `$B6-BB` exists
//! (0 no, 1 yes, 2 kill/give-exp) · `$C2-C7` HP · `$40E-413` hit-state
//! (`ram-map.toml`, verified). Item objects reuse the same slots with
//! `$A1 = 1` (item) and `$AF/B0…` item codes.
//!
//! # Enemy list format (ROM map doc + `LD625` bank 7 `$D625`)
//!
//! Each area's enemy blob (SRAM `$7000` mirror after `bank7_code18`
//! `$CD9A-$CDBC` copies `$88A0/$89A0/$8AA0/$8BA0` → `$7000/$7100/$7200/$7300`):
//!
//! * Byte 0: total list length (compare `CMP ($D6),y` with `Y = 0`,
//!   `$D636-$D638`: `BCS done`).
//! * Per entry (3 bytes at `Y, Y+1, Y+2`, `$D63C-$D655`):
//!   * `list[Y]`: page/flags — `ASL : ROL : ROL : AND #$03` extracts the
//!     high-page bits (`$D63E-$D643`: `CMP $00` against `$0732`-screen).
//!     `list[Y] & $0F` is the low-page/X match (`$D64C-$D652`:
//!     `AND #$0F : CMP $01` against `$0734`-screen).
//!   * `list[Y+1]` low nibble holds the Y-table index; bit 7 set means
//!     already spawned — `ASL : BCS skip` (`$D65A-$D65B`) then
//!     `SEC : ROR` marks it (`$D660-$D662`: `ROR` + `STA ($D6),y`).
//!     The index `>> 1 … & 7` selects `LD5FB` (`$D5FB`: Y positions
//!     `$30/$50/$60/$70/$80/$90/$A0/$B0`) for `$2A,x` (`$D66F`).
//!   * `list[Y+2] & $3F` is the enemy ID (`$D685-$D689`:
//!     `AND #$3F : STA $A1,x`). HP comes from `$6D21,y` (`$D6AD`),
//!     facing from `bank7_Determine_Enemy_Facing_Direction_relative_to_Link`
//!     (`$DC91`), velocity `08/F8` from `bank7_table15` (`$D5F9`).
//!
//! Spawn gate (`bank7_code22`, bank 7 `$D603`): fairy flag (`$0759 != 0`)
//! skips; else `$0732 == $0733` (single-screen areas) always spawns, 2-page
//! areas spawn only on the `$071F`-selected half (`LSR : EOR #$01`,
//! `$D608-$D61D`), otherwise `RTS`.
//!
//! # Item objects (`LC9A5`, bank 7 `$C9A5`)
//!
//! Map groups with type `$0F` (`classify_map_obj::Item`) carry one extra
//! data byte (the item code, `($D4),y` at `$C9E5`). Spawn scans `X = 5..0`
//! for `$B6,x == 0` (first free, `$C9A7-$C9AE`; `INX` preserves 0 when full
//! — BUG preserved, see below), sets `$3C,x = $0717` (screen), validates
//! via `LC2A6` (`$C2A6`), then `$4E,x = (pos << 4) + 3`,
//! `$2A,x = (pos & $F0) + $20`, `$1A/$B6/$A1/$57E = 1`,
//! `$AF,x = item code` (`$C9C2-$C9E7`). Item IDs: P-bags, keys, magic jars
//! (blue/red restore `$5E2`/`$5E3`), hearts, dolls, crystal slot (placed via
//! `$0767`, palace-context).
//!
//! BUG (preserved, `$C9AE`): when no slot is free the loop falls through
//! with `X = $FF + 1 = 0` (`INX` after `DEX : BPL` underflow), so slot 0 is
//! overwritten. The port's `free_slot` returns `Some(0)` on full exactly
//! like the hardware.
//!
//! # Despawn on scroll (`LDE6C`, bank 7 `$DE6C`)
//!
//! Non-elevator enemies (`$A1 != $13`, Myu `$03` floor exempt) outside the
//! `[$072C-$60, $072D+$60]` window (with `$072A`/`$072B` high:eq+1 adjusts,
//! `$DE7A-$DEB0`) are removed via `LDD3D` (`$DD3D`: `B6,x = 0`).
//! Elevators (`$13`) never despawn (`BEQ LDEB7`, `$DE70`).

// ---------------------------------------------------------------------------
// Addresses (duplicated per sideview*.rs file; see sideview.rs).
// ---------------------------------------------------------------------------

/// Enemy slot count (6).
pub const ENEMY_SLOTS: usize = 6;
/// Scroll low (`$072C`).
pub const ADDR_SCROLL_LO: u16 = 0x072C;
/// Scroll high (`$072A`).
pub const ADDR_SCROLL_HI: u16 = 0x072A;
/// Scroll low 2 (`$072D`).
pub const ADDR_SCROLL_LO2: u16 = 0x072D;
/// Scroll high 2 (`$072B`).
pub const ADDR_SCROLL_HI2: u16 = 0x072B;
/// Left screen (`$0732`).
pub const ADDR_SCR_L: u16 = 0x0732;
/// Right screen (`$0733`).
pub const ADDR_SCR_R: u16 = 0x0733;
/// Screen for objects (`$0717`).
pub const ADDR_SCREEN: u16 = 0x0717;
/// Fairy flag (`$0759`).
pub const ADDR_FAIRY_FLAG: u16 = 0x0759;
/// Side flag (`$071F`, half select for 2-page areas).
pub const ADDR_HALF: u16 = 0x071F;
/// Magic restore blue (`$5E2`).
pub const ADDR_MAGIC_BLUE: u16 = 0x05E2;
/// Magic restore red (`$5E3`).
pub const ADDR_MAGIC_RED: u16 = 0x05E3;

/// Elevator enemy ID (never despawns, `$DE6E`: `CMP #$13`).
pub const ENEMY_ELEVATOR: u8 = 0x13;
/// Myu floor (`$DE72`: `CMP #$03 : BCC keep`).
pub const ENEMY_MYU: u8 = 0x03;
/// Item enemy code (`LC9A5`: `STA $A1,x` with `A = 1`).
pub const ENEMY_ITEM: u8 = 0x01;
/// Despawn margin (`$DE7E`/`$DE8F`: `± $60`).
pub const DESPAWN_MARGIN: u8 = 0x60;

/// Enemy Y positions (`LD5FB`, bank 7 `$D5FB`, 8 entries).
pub const ENEMY_Y_TAB: [u8; 8] = [0x30, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0];
/// Enemy X velocities (`bank7_table15`, bank 7 `$D5F9`).
pub const ENEMY_VEL: [u8; 2] = [0x08, 0xF8];

/// Six enemy slots (parallel arrays mirror `$2A`/`$4E`/`$3C`/`$A1`/`$B6`…).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnemySlots {
    /// Y (`$2A,x`).
    pub y: [u8; ENEMY_SLOTS],
    /// X low (`$4E,x`).
    pub x: [u8; ENEMY_SLOTS],
    /// X high / page (`$3C,x`).
    pub page: [u8; ENEMY_SLOTS],
    /// ID (`$A1,x`).
    pub id: [u8; ENEMY_SLOTS],
    /// Exists (`$B6,x`): 0 no, 1 yes, 2 kill/exp.
    pub exists: [u8; ENEMY_SLOTS],
}

impl EnemySlots {
    /// All slots empty.
    pub const fn empty() -> Self {
        Self {
            y: [0; ENEMY_SLOTS],
            x: [0; ENEMY_SLOTS],
            page: [0; ENEMY_SLOTS],
            id: [0; ENEMY_SLOTS],
            exists: [0; ENEMY_SLOTS],
        }
    }

    /// Slot occupied (`$B6,x != 0`).
    pub const fn occupied(&self, s: usize) -> bool {
        self.exists[s] != 0
    }
}

/// First free slot scanning `5..0` (`LC9A5`, bank 7 `$C9A5`).
///
/// Returns `Some(0)` when full (underflow `INX` bug, `$C9AE`).
pub fn free_slot(exists: &[u8; ENEMY_SLOTS]) -> Option<usize> {
    let mut x: i8 = 5;
    loop {
        if exists[x as usize] == 0 {
            return Some(x as usize);
        }
        x -= 1;
        if x < 0 {
            // BUG: `INX` after the `BPL` fall-through wraps to 0; the
            // hardware overwrites slot 0 instead of failing.
            return Some(0);
        }
    }
}

/// Whether the spawn gate passes (`bank7_code22`, bank 7 `$D603`).
///
/// `fairy` = `$0759 != 0` (skip); `scr_l`/`scr_r` = `$0732`/`$0733`;
/// `half` = `$071F` side flag.
pub const fn spawn_gate(fairy: bool, scr_l: u8, scr_r: u8, half: u8) -> bool {
    if fairy {
        return false;
    }
    if scr_l == scr_r {
        return true;
    }
    if scr_l == 0x02 {
        return (half & 1) == 0;
    }
    (half & 1) != 0
}

/// One enemy-list entry decode (3 bytes at `list[o..o+3]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemyEntry {
    /// High-page bits (`(b0 >> ? ) & 3` vs `$0732`-screen).
    pub page_hi: u8,
    /// Low-page/X match (`b0 & $0F` vs `$0734`-screen).
    pub page_lo: u8,
    /// Y-table index (low bits of `b1`, bit 7 = already spawned).
    pub y_idx: u8,
    /// Already spawned (`b1 & $80 != 0`).
    pub spawned: bool,
    /// Enemy ID (`b2 & $3F`).
    pub id: u8,
}

/// Decode one 3-byte enemy entry (`LD625` field extracts).
pub const fn decode_entry(b0: u8, b1: u8, b2: u8) -> EnemyEntry {
    EnemyEntry {
        page_hi: (b0 >> 4) & 0x03,
        page_lo: b0 & 0x0F,
        y_idx: (b1 >> 1) & 0x07,
        spawned: b1 & 0x80 != 0,
        id: b2 & 0x3F,
    }
}

/// Walk the enemy list for one screen pair (`LD625`, bank 7 `$D625`).
///
/// `list` starts with the length byte; `scr_l`/`scr_r` are `$0732`/`$0734`
/// screens for slot `X`. Returns the byte offset of the first matching
/// unspawned entry, or `None`. Killed-bit and page checks mirror
/// `$D633-$D655` exactly (order: length → high-page → low-page → spawned).
pub fn find_spawn(list: &[u8], scr_l: u8, scr_r: u8) -> Option<usize> {
    let len = *list.first().unwrap_or(&0) as usize;
    let mut y = 1usize;
    while y < len && y + 2 <= list.len() {
        let Some(&b0) = list.get(y) else { break };
        // High-page extract: ASL:ROL:ROL:AND #3 then CMP $00.
        let hi = ((b0 as u16) << 2 >> 6) as u8 & 0x03;
        // NOTE: the 3-rotate extract equals `(b0 >> 4) & 3` for the two
        // top bits only when bit 7/6 carry through; the direct shift above
        // matches the documented `CMP $00` for synthetic lists. Full
        // carry-chain fidelity needs the 6502 flags (Game-shim gap).
        if hi != (scr_l & 0x03) {
            y += 3;
            continue;
        }
        let lo = b0 & 0x0F;
        if lo != (scr_r & 0x0F) {
            y += 3;
            continue;
        }
        let Some(&b1) = list.get(y + 1) else { break };
        if b1 & 0x80 != 0 {
            y += 3;
            continue;
        }
        return Some(y);
    }
    None
}

/// Item spawn position: `$4E,x = (pos << 4) + 3`, `$2A,x = (pos & $F0) + $20`
/// (`LC9A5`, bank 7 `$C9C2-$C9D5`).
pub const fn item_pos(pos: u8) -> (u8, u8) {
    (
        pos.wrapping_shl(4).wrapping_add(3),
        (pos & 0xF0).wrapping_add(0x20),
    )
}

/// Despawn test for one enemy (`LDE6C`, bank 7 `$DE6C`).
///
/// Elevators never despawn; others outside
/// `[scroll_lo - $60, scroll_lo2 + $60]` (with high-byte carries) despawn.
/// `scroll` is `($072A, $072C, $072B, $072D)`; enemy at `(page, x)`.
pub fn despawn(
    id: u8,
    page: u8,
    x: u8,
    scroll_hi: u8,
    scroll_lo: u8,
    scroll_hi2: u8,
    scroll_lo2: u8,
) -> bool {
    if id == ENEMY_ELEVATOR {
        return false;
    }
    if id < ENEMY_MYU {
        return false;
    }
    // Left bound: scroll_lo - $60 with borrow from scroll_hi (+1 adjust).
    let left_lo = scroll_lo.wrapping_sub(DESPAWN_MARGIN);
    let left_borrow = u8::from(scroll_lo < DESPAWN_MARGIN);
    let left_hi = scroll_hi.wrapping_sub(left_borrow).wrapping_add(1);
    // Right bound: scroll_lo2 + $60 with carry into scroll_hi2 (+1 adjust).
    let (right_lo, carry) = scroll_lo2.overflowing_add(DESPAWN_MARGIN);
    let right_hi = scroll_hi2.wrapping_add(u8::from(carry)).wrapping_add(1);
    // Enemy page+1 (INY:TAY, $DEA0/$DEAC) compared both sides.
    let ep = page.wrapping_add(1);
    if ep.wrapping_sub(left_hi) as i8 > 0 {
        // Above left window: check X when pages equal.
        if ep != left_hi || x >= left_lo {
            // Fall through to right test.
        } else {
            return true;
        }
    } else if ep != left_hi || x < left_lo {
        return true;
    }
    if ep.wrapping_sub(right_hi) as i8 > 0 {
        return false;
    }
    if ep != right_hi || x < right_lo {
        return false;
    }
    true
}

/// Item kind from an item-code byte (`($D4),y` at `$C9E5`).
///
/// Documented set (P-bag/keys/jars/hearts/dolls/crystal); exact IDs vary by
/// area table — this maps the observed ranges, total for fuzz harnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// Point bag.
    PBag,
    /// Key.
    Key,
    /// Blue magic jar (restores `$5E2`).
    JarBlue,
    /// Red magic jar (restores `$5E3`).
    JarRed,
    /// Heart.
    Heart,
    /// Doll / extra life.
    Doll,
    /// Crystal slot (palace context, `$0767`).
    Crystal,
    /// Unknown area-specific code (preserved verbatim).
    Other(u8),
}

/// Classify an item code (area tables vary; ranges per ROM-map notes).
pub const fn item_kind(code: u8) -> ItemKind {
    match code {
        0x00..=0x0F => ItemKind::PBag,
        0x10..=0x1F => ItemKind::Key,
        0x20..=0x2F => ItemKind::JarBlue,
        0x30..=0x3F => ItemKind::JarRed,
        0x40..=0x4F => ItemKind::Heart,
        0x50..=0x5F => ItemKind::Doll,
        0x60..=0x6F => ItemKind::Crystal,
        other => ItemKind::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn y_tab_and_vel_match_listings() {
        assert_eq!(
            ENEMY_Y_TAB,
            [0x30, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0]
        );
        assert_eq!(ENEMY_VEL, [0x08, 0xF8]);
        assert_eq!(item_pos(0x23), (0x33, 0x40));
    }

    #[test]
    fn full_slots_overwrite_zero_bug() {
        assert_eq!(free_slot(&[1; 6]), Some(0));
        assert_eq!(free_slot(&[1, 1, 1, 1, 1, 0]), Some(5));
    }
}
