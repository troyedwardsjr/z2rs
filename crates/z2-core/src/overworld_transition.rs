//! Area transitions, sideview return, enemy SRAM copy, map patches.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod overworld_transition;`.
//!
//! # Key-area tables (4-byte entries, strided)
//!
//! Each region has 63 key areas (towns, palaces, caves, bridges, docks…)
//! stored as four 63-byte strides — Y, X, map, world — 252 bytes total
//! (ROM file offsets below; `z2-assets` `AREAS_*` sections, same blobs):
//!
//! | region | Y stride | X stride | map stride | world stride | file |
//! |---|---|---|---|---|
//! | West | `$861F` | `$865E` | `$869D` | `$86DC` | `$462F` |
//! | Death Mountain | `$A000`+ | `$A03E`+ | `$A07D`+ | `$A0BC`+ | `$610C` |
//! | East | `$861F`¹ | … | … | … | `$862F` |
//! | Maze Island | `$A000`+ | … | … | … | `$A10C` |
//!
//! ¹ Same low addresses in the bank-2 image. `bank7_code18` (bank 7
//! `$CDF7-$CE13`) loads the active region's 252 area bytes plus room
//! connectivity into WRAM `$6A00-$6C57` (`$6A00`+256, `$6B00`+256,
//! `$6C00`+88), preserving the strided layout: entry `i` reads
//! `$6A00[i]` (Y, `& $7F`), `$6A3F[i]` (X, `& $3F`), `$6A7E[i]` (map),
//! `$6ABD[i]` (world).
//!
//! # Transition check (`Check_if_Link_stepped_on_a_Key_Area`, `$857D`)
//!
//! Scans `X = $3D..0` for `Y == $73 && X == $74` (masked). On match:
//! dock slot `$29` (West) without raft (`$0787 == 0`) outside East refuses;
//! the slot is stored to `$0748`; dock tiles matching
//! `LOCATION_OF_THE_RAFT_SPOT` (`$8528`: X `[$3D,$07]`, `$852A`: Y
//! `[$4D,$34]`) start a raft ride (`$07A9` direction, mode `$17`), else
//! the sideview entry (`L85D5`, bank 0 `$85D5`: `$0729 = 0`, `$0726++`,
//! `$0736++`, `$0768 = 6`, `$074C = 0`).
//!
//! # Sideview return (`bank7_go_outside`, bank 7 `$CCB3-$CCF9`)
//!
//! Clears `$0709`/`$075B`, restores `$73`/`$74` from the `$6A00` area bytes
//! of slot `$0748` (`Y & $7F`, `X & $3F`; the `$3D`/`$51` hole edge case
//! maps to `$51`), then nudges one tile away from the entered side based
//! on facing (`$0562` vs sideview `$5F`) and the area "external" bit
//! (`$6A00 & $80`).
//!
//! # Enemy-data SRAM copy (`bank7_code18`, bank 7 `$CD9A-$CDBC`)
//!
//! Four 256-byte chunks from the sideview bank's `$88A0/$89A0/$8AA0/$8BA0`
//! enemy tables to SRAM `$7000/$7100/$7200/$7300` (1024 bytes total;
//! `z2-assets` `SB1_ENEMIES`/`SB2_ENEMIES` cover the `$88A0` source).
//!
//! # Map-modification writes
//!
//! * Hammer (`L850C`, bank 0 `$850C` → `bank7_forest_chop_with_hammer`,
//!   bank 7 `$DF79`): the faced tile (`$84AD` offsets) is transformed via
//!   `HAMMER_TRANSFORM` (`bank7_Table_for_OW_tiles…`, `$DF5E` → `$DF62`):
//!   rock `$0E`→desert `$04`, spider `$0F`→desert, desert `$04`→palace
//!   `$02`, forest `$06`→town `$00`. Palace/town reveals additionally
//!   patch the `$6A00` area bytes (`$E6/$D1` rows, `$DFC8-$DFCE`).
//! * Stone palaces (`bank7_Turn_Palaces_into_Stone_Bank_1`, `$E01B` →
//!   `L879B`, bank 0 `$879B`): completed palaces are stamped stone using
//!   the SRAM-offset pointer table at `$479F` (West/DM) / `$879F`
//!   (East/Maze), 4 words (`z2-assets` `PATCH_WEST`/`PATCH_EAST`).
//! * Hidden palace spot: region 2 + (`$73 == $64`, `$74 == $2D`) with the
//!   flute triggers the chop path (`bank1_Check_for_Hidden_Palace_spot`,
//!   bank 1 `$8368`).

// ---------------------------------------------------------------------------
// Addresses (duplicated per overworld*.rs file; see overworld.rs).
// ---------------------------------------------------------------------------

/// Area index (`$0748`).
pub const ADDR_AREA_INDEX: u16 = 0x0748;
/// Encounter type (`$075A`).
pub const ADDR_ENCOUNTER_TYPE: u16 = 0x075A;
/// Fairy flag (`$0759`).
pub const ADDR_FAIRY_FLAG: u16 = 0x0759;
/// Outside flag (`$0709`).
pub const ADDR_OUTSIDE: u16 = 0x0709;
/// Game mode (`$0736`).
pub const ADDR_GAME_MODE: u16 = 0x0736;
/// Raft owned (`$0787`).
pub const ADDR_RAFT: u16 = 0x0787;
/// Hammer owned (`$078B`).
pub const ADDR_HAMMER: u16 = 0x078B;
/// Raft direction (`$07A9`): 1 right, 2 left.
pub const ADDR_RAFT_DIR: u16 = 0x07A9;
/// SRAM enemy-data base (`$7000`, 1024 bytes to `$73FF`).
pub const SRAM_ENEMY_BASE: usize = 0x7000;
/// SRAM enemy-data length (4 × 256).
pub const SRAM_ENEMY_LEN: usize = 1024;
/// WRAM area-table base (`$6A00`).
pub const WRAM_AREA_BASE: usize = 0x6A00;

// ---------------------------------------------------------------------------
// Area-table catalog.
// ---------------------------------------------------------------------------

/// Key-area entries per region (scan covers `X = $3D..0`, bank 0 `$8580`).
pub const AREA_COUNT: usize = 63;
/// Scan start index (`LDX #$3D`, bank 0 `$8580`).
pub const AREA_SCAN_TOP: u8 = 0x3D;
/// Dock slot that needs the raft in West Hyrule (`CPX #$29`, `$8599`).
pub const DOCK_SLOT: u8 = 0x29;
/// Y mask for area byte 0 (`AND #$7F`, `$8585`; bit 7 = "external").
pub const AREA_Y_MASK: u8 = 0x7F;
/// X mask for area byte 1 (`AND #$3F`, `$858E`).
pub const AREA_X_MASK: u8 = 0x3F;
/// "External" flag bit in area byte 0 (`ASL : BCC`, `$CCEC`).
pub const AREA_EXTERNAL_BIT: u8 = 0x80;

/// ROM file offsets + bank addresses of the four area-table groups
/// (252 bytes each: Y[63] X[63] map[63] world[63]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaGroup {
    /// West Hyrule: bank 1 `$861F`, file `$462F`.
    West,
    /// Death Mountain: bank 1 `$A000` region, file `$610C`.
    DeathMountain,
    /// East Hyrule: bank 2 `$861F`, file `$862F`.
    East,
    /// Maze Island: bank 2 `$A000` region, file `$A10C`.
    MazeIsland,
}

/// (file offset, bank address) of an area group.
pub const fn area_group_loc(group: AreaGroup) -> (u32, u16) {
    match group {
        AreaGroup::West => (0x462F, 0x861F),
        AreaGroup::DeathMountain => (0x610C, 0xA000),
        AreaGroup::East => (0x862F, 0x861F),
        AreaGroup::MazeIsland => (0xA10C, 0xA000),
    }
}

/// Stride length of each Y/X/map/world lane (63 entries).
pub const AREA_STRIDE: usize = 63;

/// Read entry `i` from strided lanes (`Y & $7F`, `X & $3F`, map, world).
/// Mirrors the `$6A00`/`$6A3F`/`$6A7E`/`$6ABD` indexed reads.
pub fn area_entry(
    y_lane: &[u8],
    x_lane: &[u8],
    map_lane: &[u8],
    world_lane: &[u8],
    i: usize,
) -> (u8, u8, u8, u8) {
    (
        y_lane[i] & AREA_Y_MASK,
        x_lane[i] & AREA_X_MASK,
        map_lane[i],
        world_lane[i],
    )
}

/// Scan for the key area under (`tile_y`, `tile_x`) top-down from
/// `AREA_SCAN_TOP` (`$857D-$8597`). Returns the slot index.
pub fn find_key_area(
    y_lane: &[u8],
    x_lane: &[u8],
    map_lane: &[u8],
    world_lane: &[u8],
    tile_y: u8,
    tile_x: u8,
) -> Option<usize> {
    let mut x = AREA_SCAN_TOP as usize;
    loop {
        let (y, ax, _, _) = area_entry(y_lane, x_lane, map_lane, world_lane, x);
        if y == tile_y && ax == tile_x {
            return Some(x);
        }
        if x == 0 {
            return None;
        }
        x -= 1;
    }
}

// ---------------------------------------------------------------------------
// Transitions.
// ---------------------------------------------------------------------------

/// Dock X positions (`LOCATION_OF_THE_RAFT_SPOT`, bank 0 `$8528`).
pub const RAFT_SPOT_X: [u8; 2] = [0x3D, 0x07];
/// Dock Y positions (`L852A`, bank 0 `$852A`).
pub const RAFT_SPOT_Y: [u8; 2] = [0x4D, 0x34];
/// Raft-ride game mode (`LDA #$17 : STA $0736`, bank 0 `$85CB`).
pub const RAFT_MODE: u8 = 0x17;
/// Sideview-entry transition effect (`STA $0768`, bank 0 `$85F8`).
pub const ENTER_EFFECT: u8 = 0x06;
/// Hidden-palace flute spot: region 2, Y `$64`, X `$2D` (bank 1 `$8371`).
pub const HIDDEN_PALACE_SPOT: (u8, u8, u8) = (2, 0x64, 0x2D);

/// Outcome of the key-area check for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// No area under Link (or dock refused): stay on the overworld.
    None,
    /// Fixed sideview encounter: slot → `$0748`, then `L85D5` entry.
    Sideview {
        /// Key-area slot (→ `$0748`).
        slot: u8,
    },
    /// Raft ride: dock index 0/1 (→ `$07A9 = index + 1`), mode `$17`.
    Raft {
        /// Dock index (0 west, 1 east).
        dock: u8,
    },
}

/// Pure key-area transition (`$857D-$85D2`).
///
/// * `slot` — `Some(i)` from [`find_key_area`].
/// * `region` — `$0706`; `has_raft` — `$0787 != 0`.
/// * Dock tiles — whether (`tile_y`, `tile_x`) equal dock `d`.
pub fn key_area_transition(
    slot: Option<usize>,
    region: u8,
    has_raft: bool,
    tile_y: u8,
    tile_x: u8,
) -> Transition {
    let Some(i) = slot else {
        return Transition::None;
    };
    // Dock `$29` needs the raft outside East (`$8599-$85A7`).
    if i as u8 == DOCK_SLOT && region != 2 && !has_raft {
        return Transition::None;
    }
    for (d, (&rx, &ry)) in RAFT_SPOT_X.iter().zip(RAFT_SPOT_Y.iter()).enumerate() {
        if tile_x == rx && tile_y == ry {
            return Transition::Raft { dock: d as u8 };
        }
    }
    Transition::Sideview { slot: i as u8 }
}

/// Sideview-entry register effects (`L85D5`, bank 0 `$85D5-$8600`):
/// returns (`mode_delta`, `enter_effect`, `clear_dialog`).
/// Mode `$0736` is incremented (0 → overworld→sideview load chain),
/// `$0768 = 6`, `$074C = 0`, `$0729 = 0`, `$0726++`.
pub const fn sideview_entry_effects() -> (u8, u8, u8) {
    (1, ENTER_EFFECT, 0)
}

// ---------------------------------------------------------------------------
// Sideview return (`bank7_go_outside`, bank 7 `$CCB3`).
// ---------------------------------------------------------------------------

/// Hole-edge special: Y byte `$00` + X byte `$3D` maps back to Y `$51`
/// (`LDA #$51`, bank 7 `$CCCA`).
pub const HOLE_EDGE_X: u8 = 0x3D;
/// Y value used by the hole-edge special above.
pub const HOLE_EDGE_Y: u8 = 0x51;

/// Restore overworld tiles from area slot bytes (`$CCB3-$CCD5`).
/// `area_y`/`area_x` are the raw `$6A00` bytes (masks applied here).
pub const fn return_tiles(area_y: u8, area_x: u8) -> (u8, u8) {
    if area_y == 0x00 && (area_x & AREA_X_MASK) == HOLE_EDGE_X {
        return (HOLE_EDGE_Y, HOLE_EDGE_X);
    }
    (area_y & AREA_Y_MASK, area_x & AREA_X_MASK)
}

/// Post-return nudge away from the entered side (`$CCDC-$CD16`).
///
/// Palace-bit areas (`world & $40 != 0`, `$6ABD`) with horizontal facing
/// (`$0562 < 4`) step `+X`; "external"-bit areas pick the nudge from
/// sideview facing `$5F` (even → `−2X` after the `+X`, i.e. net `−X`);
/// vertical facings step `+Y` then back off `−2Y` unless the nudge target
/// matches. Returns the adjusted `(tile_y, tile_x)`.
pub const fn return_nudge(
    tile_y: u8,
    tile_x: u8,
    palace_bit: bool,
    facing_ow: u8,
    facing_side: u8,
    external_bit: bool,
) -> (u8, u8) {
    if palace_bit && facing_ow < 4 {
        let nx = tile_x.wrapping_add(1);
        let pick = if external_bit { facing_ow } else { facing_side };
        if pick.wrapping_sub(1) == 0 {
            return (tile_y, nx);
        }
        return (tile_y, nx.wrapping_sub(2));
    }
    if palace_bit {
        let ny = tile_y.wrapping_add(1);
        let pick = if external_bit {
            facing_ow >> 2
        } else {
            facing_side
        };
        if pick.wrapping_sub(1) == 0 {
            return (ny, tile_x);
        }
        return (ny.wrapping_sub(2), tile_x);
    }
    (tile_y, tile_x)
}

// ---------------------------------------------------------------------------
// Enemy-data SRAM copy + map patches.
// ---------------------------------------------------------------------------

/// Enemy-table chunk sources in the sideview bank
/// (`L88A0/L89A0/L8AA0/L8BA0`, bank 1 `$88A0…`; bank 2 identical offsets).
pub const ENEMY_SRC_OFFSETS: [u16; 4] = [0x88A0, 0x89A0, 0x8AA0, 0x8BA0];
/// SRAM destinations (`$7000/$7100/$7200/$7300`, bank 7 `$CD9D…`).
pub const ENEMY_DST_OFFSETS: [u16; 4] = [0x7000, 0x7100, 0x7200, 0x7300];
/// Chunk length (256 bytes each; `INY : BNE` loops).
pub const ENEMY_CHUNK_LEN: usize = 256;

/// Pure SRAM copy (`LOOP_load_enemy_data_to_ram7000_7CFF`): copies the four
/// 256-byte `src_chunks` (in [`ENEMY_SRC_OFFSETS`] order) into `sram`
/// starting at offset 0 (caller maps `$7000`). Returns bytes written.
pub fn copy_enemy_data(sram: &mut [u8], src_chunks: [&[u8]; 4]) -> usize {
    let mut n = 0usize;
    for (dst_base, src) in ENEMY_DST_OFFSETS.iter().zip(src_chunks.iter()) {
        let dst_off = (dst_base - ENEMY_DST_OFFSETS[0]) as usize;
        let take = ENEMY_CHUNK_LEN.min(src.len());
        if dst_off + take > sram.len() {
            break;
        }
        sram[dst_off..dst_off + take].copy_from_slice(&src[..take]);
        n += take;
    }
    n
}

/// Hammer transform pairs (`bank7_Table_for_OW_tiles…`, bank 7 `$DF5E` →
/// `bank7_Transform_into_this`, `$DF62`): faced tile becomes the mapped
/// terrain, else unchanged (`LDF7C` scan + `LDFD1` return).
pub const HAMMER_TRANSFORM: [(u8, u8); 4] = [
    (0x0E, 0x04), // rock → desert
    (0x0F, 0x04), // spider → desert
    (0x04, 0x02), // desert → palace (reveals palace 6 / hidden Kasuto area)
    (0x06, 0x00), // forest → town (reveals hidden town)
];

/// Apply the hammer transform to a faced tile (`bank7_forest_chop…`,
/// bank 7 `$DF79-$DFC4`). Returns `(new_terrain, revealed)` where
/// `revealed` is true for palace/town reveals (which also patch `$6A00`).
pub const fn hammer_transform(tile: u8) -> (u8, bool) {
    let mut i = 0usize;
    while i < HAMMER_TRANSFORM.len() {
        if HAMMER_TRANSFORM[i].0 == tile {
            return (
                HAMMER_TRANSFORM[i].1,
                HAMMER_TRANSFORM[i].0 == 0x04 || HAMMER_TRANSFORM[i].0 == 0x06,
            );
        }
        i += 1;
    }
    (tile, false)
}

/// Palace-completion SRAM pointer tables (`$479F` West/DM → `PATCH_WEST`,
/// `$879F` East/Maze → `PATCH_EAST`; 4 words each, `z2-assets` len 8).
pub const PATCH_TABLE_LEN_WORDS: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strided_entry_masks_match_listing() {
        let y = [0x80 | 0x34];
        let x = [0xC0 | 0x17];
        let (ry, rx, _, _) = area_entry(&y, &x, &[9], &[1], 0);
        assert_eq!((ry, rx), (0x34, 0x17));
    }

    #[test]
    fn dock_gate_needs_raft_outside_east() {
        assert_eq!(
            key_area_transition(Some(0x29), 0, false, 0x00, 0x00),
            Transition::None
        );
        assert_eq!(
            key_area_transition(Some(0x29), 0, true, 0x00, 0x00),
            Transition::Sideview { slot: 0x29 }
        );
        assert_eq!(
            key_area_transition(Some(0x29), 2, false, 0x00, 0x00),
            Transition::Sideview { slot: 0x29 }
        );
        assert_eq!(
            key_area_transition(Some(3), 0, false, RAFT_SPOT_Y[0], RAFT_SPOT_X[0]),
            Transition::Raft { dock: 0 }
        );
        assert_eq!(key_area_transition(None, 0, true, 0, 0), Transition::None);
    }

    #[test]
    fn hammer_table_covers_four_transforms() {
        assert_eq!(hammer_transform(0x0E), (0x04, false));
        assert_eq!(hammer_transform(0x04), (0x02, true));
        assert_eq!(hammer_transform(0x06), (0x00, true));
        assert_eq!(hammer_transform(0x05), (0x05, false));
    }
}
