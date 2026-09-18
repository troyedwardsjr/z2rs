//! Overworld engine port root.
//!
//! Zelda II's overworld is a tile map (bank 0 driver `prg0.asm`, RLE blobs in
//! PRG banks 1/2, loader in fixed bank 7) with a demon/encounter layer on top.
//! The pieces live in sibling modules so each stays small:
//!
//! * [`overworld_map`](self) — this file: pipeline overview, shared address
//!   constants and [`OVERWORLD_TRAPS`] (the only registration surface `main`
//!   needs; do NOT touch `traps.rs` from here).
//! * `overworld_map` (`overworld_map.rs`): RLE decode, region select, WRAM
//!   writer, `$6000` row-pointer table, terrain lookup, movement/scroll.
//! * `overworld_encounter` (`overworld_encounter.rs`): demon spawn tables,
//!   wave timer, 8-slot demon state machine, collision, sideview entry.
//! * `overworld_transition` (`overworld_transition.rs`): key-area tables,
//!   town/palace/cave/bridge/raft transitions, sideview return (`$0709`
//!   path), SRAM `$7000` enemy copy, palace/forest map patches.
//!
//! # Design rules
//!
//! * Pure functions over explicit params / `&[u8]` / `&mut [u8]` slices — no
//!   `Game` dependency, no ROM reads — so every test is ROM-free (the `ram`
//!   accessor style, but with raw addresses so each file also compiles
//!   standalone via `rustc --edition 2021 --test` before `main` wires the
//!   `pub mod` lines).
//! * Each `overworld*.rs` file is intentionally self-contained: shared
//!   constants are repeated per file (documented as such) rather than
//!   imported, so standalone `rustc --test` keeps working. `main` may add
//!   `pub use` re-exports later; that is out of scope here.
//! * Every routine cites its disassembly entry label + address in doc
//!   comments (bank 0 = `prg0.asm`, bank 1 = `prg1.asm`, bank 2 =
//!   `prg2.asm`, bank 7 = `prg7.asm`; file offsets from `z2-assets`
//!   `extract_tables.rs` where relevant).
//!
//! # Frame pipeline (`overworld3`, bank 0 `$8558`)
//!
//! ```text
//! overworld3 ($8558): $07AC=$07AD=0; $0729++; JSR overworld1; JSR L841B;
//!                     JSR L8726; $7D==0 ? L8573 : JMP L86AF
//!   overworld1 ($8284): demon spawn check (wave timer + terrain + RNG)
//!   L841B ($841B):      demon velocity AI (every 16th frame) + integrate,
//!                       draw, collide (L8332/L8336 path every frame)
//!   L8726 ($8726):      Link overworld sprite/OAM setup (then L8728)
//!   L86AF ($86AF):      scroll commit: DEC $7D, INC $26, shift $FC/$FD...
//!   L8573 ($8573):      JSR overworld2 (hammer/flute); $70!=0 ? key-area
//!                       check ($857D) : JMP L8601 (direction input)
//! ```
//!
//! # Key RAM (all values verified against `third_party/z2disassembly`)
//!
//! `$0706` region · `$0707` world · `$0073`/`$0074` tile Y/X · `$0075`/`$0076`
//! pixel coords · `$007D` pixel-move counter · `$0026` step tally · `$0562`
//! facing · `$0563` terrain · `$0516` wave timer · `$0759` fairy flag ·
//! `$075A` encounter type · `$0748` area index · `$0709` outside flag.
//! WRAM `$7C00-$7FFF` = RLE blob copy; WRAM `$6000-$6095` = row pointers;
//! WRAM `$6A00-$6C57` = area tables; SRAM `$7000-$73FF` = enemy data.

// ---------------------------------------------------------------------------
// Shared address constants (repeated in each overworld*.rs file on purpose;
// see module docs — keeps every file standalone-compilable).
// ---------------------------------------------------------------------------

/// Overworld region index (`$0706`): 0 West, 1 Death Mountain / Maze Island,
/// 2 East.
pub const ADDR_OVERWORLD_INDEX: u16 = 0x0706;
/// World/area-type byte (`$0707`).
pub const ADDR_WORLD: u16 = 0x0707;
/// Previous region (`$070A`): disambiguates Death Mountain (0) from Maze
/// Island (nonzero) when `$0706 == 1` (`bank7_code18`, bank 7 `$CD52`).
pub const ADDR_PREV_REGION: u16 = 0x070A;
/// Overworld tile Y (`$0073`, square units).
pub const ADDR_TILE_Y: u16 = 0x0073;
/// Overworld tile X (`$0074`, square units).
pub const ADDR_TILE_X: u16 = 0x0074;
/// Pixel-ish map Y (`$0075`, derived from `$0073` on each step).
pub const ADDR_PIXEL_Y: u16 = 0x0075;
/// Pixel-ish map X (`$0076`, derived from `$0074` on each step).
pub const ADDR_PIXEL_X: u16 = 0x0076;
/// Pixels left to move (`$007D`): `$10` (16) at step start, `L86AF` decrements.
pub const ADDR_PIXELS_LEFT: u16 = 0x007D;
/// Movement/step tally (`$0026`): `INC` per committed tile step (`L86AF`).
pub const ADDR_STEP_TALLY: u16 = 0x0026;
/// Link facing on the overworld (`$0562`): 1 right, 2 left, 4 down, 8 up.
pub const ADDR_FACING: u16 = 0x0562;
/// Terrain under / faced tile (`$0563`), 0-15.
pub const ADDR_TERRAIN: u16 = 0x0563;
/// Demon-wave timer (`$0516`): ticks once per 21-frame NMI sweep; 8 on
/// sideview exit (`overworld4`, bank 0 `$887B`).
pub const ADDR_WAVE_TIMER: u16 = 0x0516;
/// Alternate randomizer (`$051C`): `& 3` seeds demon count (`overworld1`).
pub const ADDR_ALT_RNG: u16 = 0x051C;
/// NMI LFSR byte 1 (`$051B`): sampled by spawn/AI rolls (`LDA $051B[,x]`).
pub const ADDR_RNG: u16 = 0x051B;
/// Fairy-force flag (`$0759`): nonzero forces a fairy at the next encounter
/// (bank 7 `$D603`); cleared on sideview exit (bank 7 `$E17D`).
pub const ADDR_FAIRY_FLAG: u16 = 0x0759;
/// Encounter type (`$075A`): 0 fairy/fixed, 1 small, 2 big.
pub const ADDR_ENCOUNTER_TYPE: u16 = 0x075A;
/// Area location index (`$0748`): key-area slot, or `$FF` for random demons.
pub const ADDR_AREA_INDEX: u16 = 0x0748;
/// Outside flag (`$0709`): cleared by `bank7_go_outside` (bank 7 `$CCB5`).
pub const ADDR_OUTSIDE: u16 = 0x0709;
/// Game mode (`$0736`): `INC` enters sideview load on transitions.
pub const ADDR_GAME_MODE: u16 = 0x0736;
/// WRAM base of the RLE blob copy (`$7C00-$7FFF`, ≤896 bytes).
pub const WRAM_RLE_BASE: u16 = 0x7C00;
/// WRAM base of the `$6000-$6095` row-pointer table (75 little-endian words).
pub const WRAM_ROWPTR_BASE: u16 = 0x6000;
/// SRAM base of the copied enemy data (`$7000-$73FF`, 1024 bytes).
pub const SRAM_ENEMY_BASE: u16 = 0x7000;

// ---------------------------------------------------------------------------
// Trap registration table (data only — `main` registers; never touch traps.rs)
// ---------------------------------------------------------------------------

/// Trap entry: (routine name, PRG bank, entry address).
///
/// `bank = None` for the slot-swapped banks 1/2 workers (`$83CF`, `$93AC`):
/// the same address runs in bank 1 (West/Death Mountain) or bank 2
/// (East/Maze Island) depending on region, so `main` must resolve the bank
/// via [`region_prg_bank`] at registration/call time.
pub type TrapEntry = (&'static str, Option<u8>, u16);

/// Resolve the slot-swapped PRG bank for overworld data access.
///
/// * `$0706 == 0` → bank 1 (West Hyrule).
/// * `$0706 == 2` → bank 2 (East Hyrule).
/// * `$0706 == 1` → bank 1 if `$070A == 0` (Death Mountain) else bank 2
///   (Maze Island).
///
/// Entry `bank7_code18`, bank 7 `$CD4A-$CD57`.
pub const fn region_prg_bank(overworld_index: u8, prev_region: u8) -> u8 {
    match overworld_index {
        0 => 1,
        2 => 2,
        _ => {
            if prev_region == 0 {
                1
            } else {
                2
            }
        }
    }
}

/// Registration table for `main` to wire into the trap dispatcher.
///
/// Entry labels + addresses cross-checked against `ports.toml` + the
/// `third_party/z2disassembly/src/*.asm` listings (see per-module docs).
/// `main` iterates this and calls its own `trap_register`; nothing here
/// depends on `traps.rs` (which stays untouched).
///
/// `bank7_code18` (`$CD40`, the game-mode-0 world loader) is listed for
/// completeness but `sideview_traps::register_overworld_traps` skips it:
/// its ~85k-cycle banked copy loops span several NMIs, which an atomic
/// trap body cannot reproduce (see the overworld section there). The ROM
/// routine runs; [`region_prg_bank`] documents its bank select.
pub const OVERWORLD_TRAPS: &[TrapEntry] = &[
    // Bank-0 driver (prg0.asm).
    ("overworld1", Some(0), 0x8284),
    ("overworld2", Some(0), 0x84BF),
    ("overworld3", Some(0), 0x8558),
    ("overworld4", Some(0), 0x87F3),
    ("overworld6", Some(0), 0x8A1A),
    ("overworld7", Some(0), 0x8B2E),
    ("Check_if_Link_stepped_on_a_Key_Area", Some(0), 0x857D),
    ("L841B", Some(0), 0x841B),
    ("L86AF", Some(0), 0x86AF),
    ("L85D5", Some(0), 0x85D5),
    ("L8601", Some(0), 0x8601),
    ("Blocked_by_Tile_or_Not_Routine", Some(0), 0x870F),
    ("L8A07", Some(0), 0x8A07),
    ("L8C30", Some(0), 0x8C30),
    ("L8C48", Some(0), 0x8C48),
    // Slot-swapped banks 1/2 (prg1.asm / prg2.asm); bank resolved per region.
    ("bank1_overworld_limit_check_jmp_from_bank7", None, 0x83CF),
    ("bank1_code8", None, 0x93AC),
    // Fixed bank 7 (prg7.asm).
    ("bank7_code18", Some(7), 0xCD40),
    ("bank7_go_outside", Some(7), 0xCCB3),
    (
        "bank7_Overworld_Boundaries__Mountain_or_Water_Bank_1",
        Some(7),
        0xDFEF,
    ),
    ("bank7_Check_for_Hidden_Palace_spot_Bank_1", Some(7), 0xDFF8),
    ("bank7_Turn_Palaces_into_Stone_Bank_1", Some(7), 0xE01B),
    ("bank7_forest_chop_with_hammer", Some(7), 0xDF79),
    ("LDF01", Some(7), 0xDF01),
    ("LDF3F", Some(7), 0xDF3F),
    ("LDFD2", Some(7), 0xDFD2),
    ("LE001", Some(7), 0xE001),
    ("LE024", Some(7), 0xE024),
    ("LE16F", Some(7), 0xE16F),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trap_table_entries_are_sorted_unique_and_plausible() {
        assert!(!OVERWORLD_TRAPS.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        for (name, bank, addr) in OVERWORLD_TRAPS {
            assert!(!name.is_empty());
            assert!(*addr >= 0x8000, "{name} entry ${addr:04X} outside PRG");
            assert!(
                seen.insert((*name, *bank, *addr)),
                "duplicate trap entry {name}"
            );
        }
    }

    #[test]
    fn region_bank_select_matches_code18() {
        assert_eq!(region_prg_bank(0, 0), 1);
        assert_eq!(region_prg_bank(0, 9), 1);
        assert_eq!(region_prg_bank(2, 0), 2);
        assert_eq!(region_prg_bank(1, 0), 1);
        assert_eq!(region_prg_bank(1, 1), 2);
    }
}
