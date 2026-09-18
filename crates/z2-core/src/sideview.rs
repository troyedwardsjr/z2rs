//! Sideview engine port root.
//!
//! Zelda II's sideview is the paged room engine (banks 1/2/4/5 area blobs,
//! bank 7 fixed loader `prg7.asm`, bank-0 init `prg0.asm`): a 4-byte area
//! header, per-page map bytes, a parallel enemy list, and 4 screens of level
//! RAM (`$6000/$60D0/$61A0/$6270`) with scrolling, collision, doors,
//! elevators, exits and 6-slot enemy/item spawning on top.
//!
//! Pieces live in sibling modules so each stays small:
//!
//! * [`sideview_area`](self) — this file: pipeline overview, shared address
//!   constants and the [`TrapEntry`] alias (data only; registration lives in
//!   `sideview_traps` so `traps.rs` stays untouched).
//! * `sideview_area` (`sideview_area.rs`): 4-byte header decode, map/enemy
//!   pointer tables (`$8523`/`$85A1`, `$A000`/`$A07E`, behind-maps `$8000`),
//!   room connectivity (`$871B`/`$A1F8`), background construction.
//! * `sideview_scroll` (`sideview_scroll.rs`): page-by-page horizontal
//!   scrolling (`$72A-$72D`, `$75C` start page), freeze `$728`, side entry
//!   `$701`, exits + overworld return (`$709`/`$748`/`$706`), elevators
//!   (`$704`/`$743`).
//! * `sideview_collision` (`sideview_collision.rs`): tile attributes and
//!   Link collision (`$A7` bits, lava/water, jump-through, sword-breakable).
//! * `sideview_spawn` (`sideview_spawn.rs`): enemy/item spawning into the six
//!   slots (`$2A-2F`/`$4E-53`/`$3C-41`/`$60-65`/`$71-76`/`$A1-A6`/`$B6-BB`/
//!   `$C2-C7`/`$40E-413`), despawn on scroll, P-bags/keys/jars/hearts.
//! * `sideview_traps` (`sideview_traps.rs`): `Game`-shim trap bodies +
//!   [`SIDEVIEW_TRAPS`](crate::sideview_traps::SIDEVIEW_TRAPS) and
//!   [`register_sideview_traps`](crate::sideview_traps::register_sideview_traps)
//!   plus the overworld shims
//!   [`register_overworld_traps`](crate::sideview_traps::register_overworld_traps).
//!
//! # Design rules
//!
//! * Pure functions over explicit params / `&[u8]` / `&mut [u8]` slices — no
//!   `Game` dependency, no ROM reads — so every test is ROM-free (the `ram`
//!   accessor style, but with raw addresses so each file also compiles
//!   standalone via `rustc --edition 2021 --test` before `main` wires the
//!   `pub mod` lines). Only `sideview_traps.rs` takes `&mut Game` (the
//!   bank-7 shim style, where bus/RAM-mirror access is needed).
//! * Each `sideview*.rs` pure file is intentionally self-contained: shared
//!   constants are repeated per file (documented as such) rather than
//!   imported, so standalone `rustc --test` keeps working.
//! * Every routine cites its disassembly entry label + address in doc
//!   comments (bank 1 = `prg1.asm`, bank 2 = `prg2.asm`, bank 7 =
//!   `prg7.asm`, bank 0 = `prg0.asm`).
//! * Bugs are preserved with `BUG:` comments rather than fixed.
//!
//! # Frame pipeline (sideview load → per-frame)
//!
//! ```text
//! bank7_code17 ($CB35): key-area → $561/$75C/$701 (or random-battle path)
//! bank7_code13 ($C4CB): area pointers → $D4/$D6 → enemy-list fixups ($75A)
//! bank7_process_map_data ($C755) + LC89D ($C89D): header + map bytes → screens
//! bank7_code33 ($E030): ground-find Link Y ($29=$AF down)
//! per-frame: scroll ($E157 freeze) → spawn (LD625 $D625) → enemies → collision ($D6C1)
//!   exits: side ($CF4C) / door ($CFFC) / elevator ($C644) / outside ($CCB3)
//! ```
//!
//! # Key RAM (all values verified against `third_party/z2disassembly`)
//!
//! `$0701` side entry (0 left, 1 right) · `$0704` elevator start
//! (0 bottom, 1 top) · `$0706` region · `$0709` outside flag · `$0728`
//! freeze (1 = boss lock, no side exits) · `$072A-$072D` scroll · `$072E`
//! area length · `$072F` read offset · `$0730` object placement · `$0731`
//! object type/size · `$075C` start page (0-3, 4 = middle) · `$0743`
//! elevator direction (8 up, 4 down) · `$0748` area index · `$075A`
//! encounter type · enemy slots `$2A`/`$4E`/`$3C`/`$60`/`$71`/`$A1`/`$B6`/
//! `$C2`/`$40E` (6 slots) · screens `$6000`/`$60D0`/`$61A0`/`$6270`.

// ---------------------------------------------------------------------------
// Shared address constants (repeated in each sideview*.rs file on purpose;
// see module docs — keeps every file standalone-compilable).
// ---------------------------------------------------------------------------

/// Side entry (`$0701`): 0 = Link enters from the left, 1 = from the right
/// (`bank7_Get_Area_Code__Enter_Code_and_Direction`, bank 7 `$CC97`).
pub const ADDR_SIDE_ENTRY: u16 = 0x0701;
/// Elevator start (`$0704`): 0 = start bottom, 1 = start top
/// (`bank7_Related_to_Link_falling`, bank 7 `$C6EF`).
pub const ADDR_ELEV_START: u16 = 0x0704;
/// Overworld region (`$0706`): 0 West, 1 DM/Maze, 2 East.
pub const ADDR_OVERWORLD_INDEX: u16 = 0x0706;
/// Outside flag (`$0709`): cleared/set by `bank7_go_outside` (bank 7 `$CCB3`).
pub const ADDR_OUTSIDE: u16 = 0x0709;
/// Scroll freeze (`$0728`): 1 = freeze screen, prevent side exits
/// (`_728_FreezeScrolling`, bank 7 `$E157` read, `$E7A9` write, bank 0
/// `$8D00` clear on sideview init).
pub const ADDR_FREEZE: u16 = 0x0728;
/// Scroll high (`$072A`): page/offset high (`bank7_code13`, bank 7 `$C4E6`).
pub const ADDR_SCROLL_HI: u16 = 0x072A;
/// Scroll byte 1 (`$072B`): second high byte (right-edge window, `$DE82`).
pub const ADDR_SCROLL_HI2: u16 = 0x072B;
/// Scroll low (`$072C`): offset low (`LCC50`, bank 7 `$CC67`).
pub const ADDR_SCROLL_LO: u16 = 0x072C;
/// Scroll byte 3 (`$072D`): second low byte (right-edge window, `$DE8B`).
pub const ADDR_SCROLL_LO2: u16 = 0x072D;
/// Start page (`$075C`): pages into the scene, 0-3 (4 = middle)
/// (`bank7_Get_Area_Code…`, bank 7 `$CCA6`).
pub const ADDR_START_PAGE: u16 = 0x075C;
/// Elevator direction (`$0743`): 8 = up, 4 = down
/// (`bank7_Enemy_Routines1_Elevator`, bank 7 `$D8D3`).
pub const ADDR_ELEV_DIR: u16 = 0x0743;
/// Area location index (`$0748`).
pub const ADDR_AREA_INDEX: u16 = 0x0748;
/// Encounter type (`$075A`): 0 fairy/fixed, 1 small, 2 big.
pub const ADDR_ENCOUNTER_TYPE: u16 = 0x075A;
/// Game mode (`$0736`).
pub const ADDR_GAME_MODE: u16 = 0x0736;

/// Trap entry: (routine name, PRG bank, entry address).
///
/// `bank = None` for slot-swapped sideview banks (1/2/4/5 `$8000-$BFFF`):
/// the same CPU address runs under different banks, so `main` must resolve
/// the bank at registration/call time. Fixed-bank (`Some(7)`) entries are
/// safe to register today (see `traps.rs` aliasing caveat).
pub type TrapEntry = (&'static str, Option<u8>, u16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_addrs_match_ram_map_listing() {
        assert_eq!(ADDR_SIDE_ENTRY, 0x0701);
        assert_eq!(ADDR_ELEV_START, 0x0704);
        assert_eq!(ADDR_FREEZE, 0x0728);
        assert_eq!(ADDR_SCROLL_HI, 0x072A);
        assert_eq!(ADDR_SCROLL_LO, 0x072C);
        assert_eq!(ADDR_START_PAGE, 0x075C);
        assert_eq!(ADDR_ELEV_DIR, 0x0743);
        assert_eq!(ADDR_AREA_INDEX, 0x0748);
    }
}
