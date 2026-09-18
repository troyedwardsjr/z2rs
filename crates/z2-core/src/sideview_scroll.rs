//! Sideview scrolling, freeze, entry, exits, elevators.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod sideview_scroll;`.
//!
//! # Scroll registers (`$072A-$072D`)
//!
//! The 4-byte scroll window is two (high, low) pairs (`prg7.asm`):
//!
//! * `$072A` high + `$072C` low = left-edge scroll (Link-anchored;
//!   `LCC50` bank 7 `$CC50` zeroes `$072C`, copies `$075C` → `$072A`/`$3B`).
//! * `$072B` high + `$072D` low = right-edge window used by the despawn
//!   test (`LDE6C`, bank 7 `$DE6C`: `$072C-$60`/`$072A` vs
//!   `$072D+$60`/`$072B`).
//!
//! Page-by-page horizontal scrolling: `$3B` (Link page) and `$072A` advance
//! together (`LE16F`, bank 7 `$E16F`: `INC $3B` when moving right past the
//! edge; symmetric `DEC` path moving left). `$072C` is the pixel offset
//! within the page; `$FD` mirrors its screen base (`LCC50` zeroes both).
//!
//! # Freeze (`$0728`, `_728_FreezeScrolling`)
//!
//! Set on boss lock (`$E7A2-$E7A9`: `LDA $0728 : STA $0728` — actually
//! `LDA #$01 : STA $0728` on the boss path; read at `LE157` bank 7 `$E157`).
//! Cleared on sideview init (`Side_View_Initialization_when_entering_a_Key_Area`,
//! bank 0 `$8D00`: `STA $0728`). While nonzero, side exits are suppressed:
//! `LE157` (`$E157`) tests `$0728`; when set it only allows the
//! boss-door nudge (`AND #$09` path, `$E15C`) and otherwise `RTS` without
//! paging (`LE16E`, `$E16E`). Reported gap: the exact boss list behind
//! `$E7A2` needs per-boss RAM confirmation; the freeze bit itself is exact.
//!
//! # Side entry (`$0701`, 0 left / 1 right)
//!
//! Derived by `bank7_Get_Area_Code__Enter_Code_and_Direction` (bank 7
//! `$CC97`): `area = map & $3F` (→ `$0561`), `enter = (map << 2) & 3`
//! (→ `$075C`), `dir = enter & 1` (→ `$0701`). Pass-through areas
//! (`$CBD0` path) flip `dir` from overworld facing (`$0562` vs `$5F`);
//! door/elevator/side exits write `$0701` directly (`$CBFA`/`$CCAB`/
//! `$CBE8`/`$CC52`/`$CFC4`/`$D01B`).
//!
//! `LCC50` (bank 7 `$CC50`) anchors Link on entry: `$4D = $70`,
//! `$0734 = $0B`, `$0735 = $06`, `$FD = $072C = 0`, `$3B = $072A = $075C`,
//! `$0732 = $075C - 1`, `$0733 = $075C + 1`, game mode `$12`/`$0C`
//! (`$CC7F-$CC85`).
//!
//! # Exits + overworld return (`$0709`/`$0748`/`$0706`)
//!
//! * Side exit (`bank7_take_side_exit`, bank 7 `$CF4C`): `Y = area*4 + page`,
//!   connectivity `$6AFC,y`; wall (`$FC`) → overworld-return path (bump
//!   `$0748`, mute, `INC $0709` when leaving world 0), else new area/page
//!   (`LSR : LSR : STA $0561`, `$CFB4`) + `$075C`/`$0701` update. Elevator
//!   rides skip the `$075C` rewrite when `$0704 != 0` (`$CFBB`).
//! * Door exit (`bank7_take_door_exit`, bank 7 `$CFFC`): same index into the
//!   door table (`L8817,y`, `$D007`), `area = (b & $FC) >> 2`,
//!   `page = b & 3` (`$D00A-$D016`), then `LC690` snapshot + `LCFEC` mode.
//! * Elevator exit (`bank7_take_elevator_exit`, bank 7 `$C644`): room bytes
//!   at `$6AFC,y` with `+1` for Up (`$0743 == 8`, `$C64E-$C657`);
//!   `area = byte >> 2`, `page = byte & 3` → `$3B`/`$072A` (`$C65D-$C667`),
//!   `$0733 = page + 1`, `$0732 = page - 1` (with `$FF → 3` wrap, `$C679`),
//!   `$075C = 4` (middle), `INC $0705` (entered-via-elevator).
//! * Outside (`bank7_go_outside`, bank 7 `$CCB3`): clears `$0709`/`$075B`,
//!   restores `$73`/`$74` from `$6A00` area bytes (masks `$7F`/`$3F`, hole
//!   edge `$00`/`$3D` → `$51`, `$CCBE-$CCCC`), then nudges one tile away
//!   from the entered side (palace/external bits, `$CCDC-$CD16`). Pure
//!   helpers here mirror the overworld-transition port's `return_tiles` /
//!   `return_nudge` without importing it (standalone rule).
//!
//! # Elevators (`$0704` start, `$0743` direction)
//!
//! `bank7_Enemy_Routines1_Elevator` (bank 7 `$D8C2`): when Link touches the
//! cabin (`$A8 & $10`), `$0754 = 1` (in-elevator), Link Y tracks cabin
//! (`$29 = $2A,x + 8`, `$D8F4`), vertical velocity from
//! `bank7_Table_for_Elevator_Y_Velocity` (bank 7 `$D8BF`: `00/18/E8`)
//! indexed by `$0743 >> 2` (`LSR : LSR : TAY`, `$D8D3-$D8D8`). Riding into
//! `$D8` (floor) recentres Link (`$4D = $70`, `$0735 = 6`, `$0734 = $0B`,
//! `$FD = $072C = 0`) and takes exit `$13` via `LE187` (`$D901-$D918`).

// ---------------------------------------------------------------------------
// Addresses (duplicated per sideview*.rs file; see sideview.rs).
// ---------------------------------------------------------------------------

/// Side entry (`$0701`): 0 left, 1 right.
pub const ADDR_SIDE_ENTRY: u16 = 0x0701;
/// Elevator start (`$0704`): 0 bottom, 1 top.
pub const ADDR_ELEV_START: u16 = 0x0704;
/// Entered-via code (`$0705`): `INC` on elevator/fall entries.
pub const ADDR_ENTER_CODE: u16 = 0x0705;
/// Region (`$0706`).
pub const ADDR_OVERWORLD_INDEX: u16 = 0x0706;
/// Outside flag (`$0709`).
pub const ADDR_OUTSIDE: u16 = 0x0709;
/// In-elevator (`$0754`).
pub const ADDR_IN_ELEV: u16 = 0x0754;
/// Elevator map position (`$0757`, from object construction `$82BD`).
pub const ADDR_ELEV_POS: u16 = 0x0757;
/// Locked-door map position (`$0758`, from `$82C4`).
pub const ADDR_DOOR_POS: u16 = 0x0758;
/// Freeze (`$0728`).
pub const ADDR_FREEZE: u16 = 0x0728;
/// Scroll high (`$072A`).
pub const ADDR_SCROLL_HI: u16 = 0x072A;
/// Scroll high 2 (`$072B`).
pub const ADDR_SCROLL_HI2: u16 = 0x072B;
/// Scroll low (`$072C`).
pub const ADDR_SCROLL_LO: u16 = 0x072C;
/// Scroll low 2 (`$072D`).
pub const ADDR_SCROLL_LO2: u16 = 0x072D;
/// Start page (`$075C`, 0-3, 4 = middle).
pub const ADDR_START_PAGE: u16 = 0x075C;
/// Elevator direction (`$0743`): 8 up, 4 down.
pub const ADDR_ELEV_DIR: u16 = 0x0743;
/// Link page (`$3B`).
pub const ADDR_LINK_PAGE: u16 = 0x003B;
/// Link X low (`$4D`).
pub const ADDR_LINK_X: u16 = 0x004D;
/// Area index (`$0748`).
pub const ADDR_AREA_INDEX: u16 = 0x0748;
/// Game mode (`$0736`).
pub const ADDR_GAME_MODE: u16 = 0x0736;

/// Elevator Y-velocity table (`bank7_Table_for_Elevator_Y_Velocity`,
/// bank 7 `$D8BF`): index `$0743 >> 2` (0 = idle, 1 = down `$18`,
/// 2 = up `$E8` as signed velocity).
pub const ELEV_VEL: [u8; 3] = [0x00, 0x18, 0xE8];
/// Elevator direction: up bit (`$0743 == 8`).
pub const ELEV_UP: u8 = 0x08;
/// Elevator direction: down bit (`$0743 == 4`).
pub const ELEV_DOWN: u8 = 0x04;
/// Elevator floor Y (`$D8FD`: `CMP #$D8`).
pub const ELEV_FLOOR_Y: u8 = 0xD8;
/// Middle start page for elevator exits (`$C67C`: `LDX #$04`).
pub const ELEV_MIDDLE_PAGE: u8 = 0x04;
/// Side-exit game modes (`$CC7F`: `$12` outdoor, `$0C` indoor via carry).
pub const EXIT_MODE_OUTDOOR: u8 = 0x12;
/// Side-exit indoor mode.
pub const EXIT_MODE_INDOOR: u8 = 0x0C;

// ---------------------------------------------------------------------------
// Scroll window.
// ---------------------------------------------------------------------------

/// 4-byte scroll window (`$072A-$072D`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    /// `$072A` left high (page).
    pub hi: u8,
    /// `$072B` right high.
    pub hi2: u8,
    /// `$072C` left low (pixel).
    pub lo: u8,
    /// `$072D` right low.
    pub lo2: u8,
}

impl Scroll {
    /// Left-edge 16-bit scroll (`hi:lo`).
    pub const fn left(self) -> u16 {
        ((self.hi as u16) << 8) | self.lo as u16
    }

    /// Right-edge 16-bit scroll (`hi2:lo2`).
    pub const fn right(self) -> u16 {
        ((self.hi2 as u16) << 8) | self.lo2 as u16
    }
}

/// Anchor scroll + Link page on side entry (`LCC50`, bank 7 `$CC50-$CC7F`).
///
/// `$4D = $70`, `$0734 = $0B`, `$0735 = $06`, `$FD = $072C = 0`,
/// `$3B = $072A = start_page`, `$0732 = start_page - 1`,
/// `$0733 = start_page + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryAnchor {
    /// Link X low (`$4D`): always `$70`.
    pub link_x: u8,
    /// `$0734`: always `$0B`.
    pub r34: u8,
    /// `$0735`: always `$06`.
    pub r35: u8,
    /// Link page (`$3B`) = start page.
    pub page: u8,
    /// `$072A` = start page.
    pub scroll_hi: u8,
    /// `$0732` = start page − 1 (wrapping).
    pub r32: u8,
    /// `$0733` = start page + 1 (wrapping).
    pub r33: u8,
}

/// Compute the `LCC50` entry anchor for `start_page` (`$075C`).
pub const fn entry_anchor(start_page: u8) -> EntryAnchor {
    EntryAnchor {
        link_x: 0x70,
        r34: 0x0B,
        r35: 0x06,
        page: start_page,
        scroll_hi: start_page,
        r32: start_page.wrapping_sub(1),
        r33: start_page.wrapping_add(1),
    }
}

/// Whether side paging is frozen (`LE157`, bank 7 `$E157`: `LDY $0728`).
pub const fn scroll_frozen(freeze: u8) -> bool {
    freeze != 0
}

/// Page step on crossing a side boundary (`LE16F`, bank 7 `$E16F`).
///
/// `moving_right` mirrors the `AND #$06 / AND #$04` decode (`$E16F-$E177`):
/// right crossing does `INC $3B`, left crossing holds (caller decrements on
/// the mirrored path). Returns the new page, wrapping.
pub const fn page_step(page: u8, moving_right: bool) -> u8 {
    if moving_right {
        page.wrapping_add(1)
    } else {
        page.wrapping_sub(1)
    }
}

// ---------------------------------------------------------------------------
// Side entry decode.
// ---------------------------------------------------------------------------

/// Decode `bank7_Get_Area_Code__Enter_Code_and_Direction` (bank 7 `$CC97`).
///
/// `map` is area byte 2 (`$6A7E,y`): `area = map & $3F` (→ `$0561`),
/// `enter = (map >> 6) & 3` (→ `$075C`; `ASL : ROL : ROL : AND #$03`
/// extracts bits 7-6), `dir = enter & 1` (→ `$0701`).
pub const fn decode_entry(map: u8) -> (u8, u8, u8) {
    let area = map & 0x3F;
    let enter = (map >> 6) & 0x03;
    (area, enter, enter & 0x01)
}

// ---------------------------------------------------------------------------
// Exits.
// ---------------------------------------------------------------------------

/// Side-exit outcome (`bank7_take_side_exit`, bank 7 `$CF4C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideExitStep {
    /// Wall: stay sideways, bump `$0748` toward the overworld path.
    Wall {
        /// New `$0748` (`old + (conn & 3)`).
        area_index: u8,
    },
    /// Room change: new area + page/dir.
    Room {
        /// New area (`conn >> 2`).
        area: u8,
        /// New start page (`conn & 3`).
        page: u8,
        /// New side entry (`page & 1`), unless elevator ride holds it.
        dir: u8,
    },
}

/// Pure side-exit step for connectivity byte `conn`.
///
/// `elevator_ride` = `$0704 != 0` (holds `$075C`/`$0701`, `$CFBB`).
/// `old_area_index` = `$0748` (wall path only).
pub const fn side_exit_step(conn: u8, elevator_ride: bool, old_area_index: u8) -> SideExitStep {
    if conn & 0xFC == 0xFC {
        SideExitStep::Wall {
            area_index: old_area_index.wrapping_add(conn & 0x03),
        }
    } else {
        let area = conn >> 2;
        let page = conn & 0x03;
        // BUG (preserved): when `elevator_ride` the ASM still computes the
        // page/dir but skips the `$075C` store (`BNE LCFB2` at `$CFBB`);
        // the port reports the computed values and lets the shim skip the
        // write, so A/B stays bit-identical.
        let _ = elevator_ride;
        SideExitStep::Room {
            area,
            page,
            dir: page & 0x01,
        }
    }
}

/// Door-exit decode (`bank7_take_door_exit`, bank 7 `$D007-$D016`).
/// Same shift as side exits, over the door table byte.
pub const fn door_exit_step(conn: u8) -> (u8, u8, u8) {
    (conn >> 2, conn & 0x03, conn & 0x01)
}

/// Elevator-exit decode (`bank7_take_elevator_exit`, bank 7 `$C644`).
///
/// `room` = `$6AFC,y` byte, `going_up` = `$0743 == 8`.
/// Returns `(area, page, link_page)` where `link_page = page`; the caller
/// also sets `$0733 = page + 1`, `$0732 = page - 1` (with `$FF → 3` wrap at
/// `$C679`), `$075C = 4`, `$3B = $072A = page`.
pub const fn elevator_exit_step(room: u8, going_up: bool) -> (u8, u8) {
    // `going_up` selects `y + 1` before the read (`ADC #$00` vs `#$01`,
    // `$C64C-$C657`); the byte itself decodes identically. Kept as a param
    // so callers mirror the ASM's branch.
    let _ = going_up;
    (room >> 2, room & 0x03)
}

/// Elevator `$0732` wrap: `page - 1`, with `$FF → 3`
/// (`$C670-$C679`: `SEC : SBC #$02 : CMP #$FF : BNE : LDA #$03`).
pub const fn elev_r32(page: u8) -> u8 {
    let v = (page.wrapping_add(1)).wrapping_sub(2);
    if v == 0xFF {
        0x03
    } else {
        v
    }
}

// ---------------------------------------------------------------------------
// Elevator ride.
// ---------------------------------------------------------------------------

/// Elevator velocity index: `$0743 >> 2` (`$D8D3-$D8D8`: `LSR : LSR : TAY`).
pub const fn elev_vel_index(dir: u8) -> usize {
    ((dir >> 2) as usize) & 0x03
}

/// Elevator velocity byte from `$0743` (`$D8BF` table, clamped).
pub const fn elev_velocity(dir: u8) -> u8 {
    let i = elev_vel_index(dir);
    ELEV_VEL[if i > 2 { 2 } else { i }]
}

/// Elevator floor test: cabin `Y >= $D8` exits the shaft
/// (`$D8FB-$D8FF`: `CMP #$D8 : BCC ride`).
pub const fn elev_at_floor(cabin_y: u8) -> bool {
    cabin_y >= ELEV_FLOOR_Y
}

/// Link Y while riding: `$29 = cabin_y + 8`
/// (`$D8F4-$D8F9`: `CLC : ADC #$08 : STA $29`).
pub const fn elev_link_y(cabin_y: u8) -> u8 {
    cabin_y.wrapping_add(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_anchor_matches_lcc50() {
        let a = entry_anchor(2);
        assert_eq!((a.link_x, a.r34, a.r35), (0x70, 0x0B, 0x06));
        assert_eq!((a.page, a.scroll_hi), (2, 2));
        assert_eq!((a.r32, a.r33), (1, 3));
        assert_eq!(entry_anchor(0).r32, 0xFF);
    }

    #[test]
    fn decode_entry_splits_map_byte() {
        // map = 0b10_101010 → area 0x2A, enter (0x2A<<2)&3 = 2, dir 0.
        assert_eq!(decode_entry(0xAA), (0x2A, 0x02, 0x00));
        assert_eq!(decode_entry(0x3F), (0x3F, 0x00, 0x00));
    }
}
