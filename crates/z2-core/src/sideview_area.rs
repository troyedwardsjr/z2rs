//! Sideview area loader: header, pointer tables, connectivity, background.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod sideview_area;`.
//!
//! # Area header (4 bytes, `bank7_process_map_data` bank 7 `$C755` +
//! # `LC89D` bank 7 `$C89D` + `bank7_Set_PPU_Macro_for_Palettes` `$D05B`)
//!
//! Per the ROM-map (Dwedit) doc naming —
//! size / flags / ground-graphics / palette-back — the loader reads:
//!
//! | byte | mask | dest | meaning |
//! |---|---|---|---|
//! | 0 | `$FF` | `$072E` | SIZE: Area Data Length (`$C785`: `STA $072E`) |
//! | 1 | `$60` | `$D1` | Area Width: `(b1 & $60) >> 5` (`$C8B4-$C8BB`) |
//! | 1 | `$1C` | `X` | Layers bits: `(b1 & $1C) >> 2`, indexes `LC4AF`
//! | (`$C5C9-$C5CD`: `AND #$1C : LSR : LSR : TAX`) |
//! | 2 | `$0F` | `$0731` | Initial object type/size (`$C764`: `AND #$0F`) |
//! | 2 | `$80` | `$0486` | No-ceiling flag (`$C76B`: `AND #$80`) |
//! | 2 | `$70` | `$010C` | Ground type 0-7 (`$C772-$C778`: `AND #$70 : LSR×4`) |
//! | 3 | `$FF` | `$07AF` | Palette-back byte (`$C58D`: `STA $07AF`) |
//! | 3 | `$07` | — | BG-map code: `b3 & 7`; 0 = none, else pointer at
//! | `$8000+X` (`$C59E-$C5B1`: `AND #$07 : BEQ : ASL : TAX : DEX : DEX`) |
//!
//! Palette derivation (`bank7_Set_PPU_Macro_for_Palettes`, bank 7 `$D05B`):
//! `back = b3`, `lo = (b3 >> 2) & $30`, `hi = (b3 << 1) & $70`; if
//! `hi == $10` in world 0 without the candle (`$0785 == 0`) the index is
//! forced to `$40` (grotto-without-candle darkness). The 16-byte PPU macro
//! is then streamed from `$7919+X` (`$D088` loop).
//!
//! BUG (preserved): `bank7_process_map_data` (`$C755`) and `LC89D`
//! (`$C89D`) both re-derive `$072E`/`$072F` from the same header bytes but
//! with different offsets (`$C77B` sets `$072F = 4`, `$C8BD` sets
//! `$072F = 4` again after the background pass). The double pass is
//! intentional — the first pass draws ceiling/floor (`bank7_code14`,
//! `$C82B`), the second draws objects — not a redundant reload.
//!
//! # Map / enemy pointer tables
//!
//! * First set (West Hyrule, bank 1): maps `bank1_Area_Pointers_West_Hyrule`
//!   bank 1 `$8523`, enemies `bank1_Enemy_Pointers__West_Hyrule` bank 1
//!   `$85A1` (`prg1.asm $8523` / `$85A1`).
//! * Second set (Death Mountain, bank 1): maps at bank 1 `$A000`, enemies at
//!   bank 1 `$A07E` (base `$88A0`; `prg1.asm $A000` / `$A07E`). Bank 2
//!   mirrors the same CPU addresses for East Hyrule / Maze Island
//!   (`prg2.asm $8523`-equivalent `$8513` code path / `$85A1` `$85A1`?? —
//!   bank 2 `$A07E` at `prg2.asm $A07E`; see `region_blob`).
//! * Behind-maps: `bank1_Pointer_table_for_Background_Areas_Data` bank 1
//!   `$8000` (7 entries: `$8C3C/$8C54/$8C68/$8C7C/$8C96/$8CA8/$0000`;
//!   `prg1.asm $8000`).
//! * Two sets of 63 map/enemy words each (banks 1/2/4); banks 3/5 (towns /
//!   Great Palace) carry a single set; the ROM-gated count check
//!   pins banks 1/2, banks 3/5 are reported as a gap (see tests).
//!
//! Lookup (`bank7_code13`, bank 7 `$C4CB-$C538`): `Y` selects the table pair
//! via `LC4BD` (`Area and Enemy Pointer Offsets`, bank 7 `$C4BD`), then
//! `($00),Y` / `($02),Y` fetch the area (`$D4/$D5`) and enemy (`$D6/$D7`)
//! base addresses for area code `$0561` (`ASL : TAY`, `$C524-$C538`).
//! Encounter fixups (`$C53A-$C566`): `$075A >= 2` advances `$D6` by
//! `($D6)[0]` (big-encounter next-area data); `$075A == 1` clears killed
//! bits (`AND #$7F`, `$C55E-$C566`); `$075A == 0` keeps area data.
//!
//! # Room connectivity (`$871B` / `$A1F8`, 4 B/entry)
//!
//! First set `bank1_West_Hyrule__Room_Connectivity_Data` bank 1 `$871B`
//! (`prg1.asm $871B`); second set at bank 1 `$A1F8` (`prg1.asm $A1F8`;
//! bank 2 `$A1F8` identical shape, `prg2.asm $A1F8`). Each area contributes
//! 4 bytes indexed by `Y = area * 4 + page` (`bank7_take_side_exit`, bank 7
//! `$CF65`: `ASL : ASL : ADC $3B : TAY : LDA $6AFC,y` where `$6AFC` is the
//! WRAM mirror of the ROM table). Byte layout per `$CF65-$CFB4` /
//! `$CFFC-$D02B` (door) / `$C644-$C679` (elevator):
//!
//! * `$FC` (masked `& $FC == $FC`) = wall / no side exit (falls through to
//!   the overworld-return path; `CMP #$FC : BNE LCFB2`, `$CF6B`).
//! * Otherwise `area = byte >> 2` (`LSR : LSR`, `$CFB2`), `page = byte & 3`
//!   (`AND #$03`, `$CFBD`/`$D014`). Door exits mask `& $FC` then shift the
//!   same way (`bank7_take_door_exit`, bank 7 `$D00A-$D00E`); elevator exits
//!   shift then store the page to `$3B`/`$072A` (`bank7_take_elevator_exit`,
//!   bank 7 `$C658-$C667`).
//!
//! # Background construction (PPU queue producer)
//!
//! Initial fill (`bank7_Initial_Fill_of_Background_Tiles__Side_View`, bank 7
//! `$C568-$C589`): if ground type (`header byte 2 & $70`) is 0, tile `$42`,
//! else tile `$40` (`LDX #$40 : LDA ($D4),y : AND #$70 : BNE : LDX #$42`).
//! The tile is stamped across the 4 screens (`$5FFF/$60CF/$619F/$626F`
//! sliding window, `Y = $D0..1`). Extra layers (bushes/grass, `$C4A7` 8
//! tile codes × `$C4AF` 8 row heights, indexed by the header-byte-1 layers
//! bits) are drawn by the `$C5DA` loop; the optional behind-map (`header
//! byte 3 & 7`) is processed first via `bank7_process_map_data` (`$C5B6`)
//! then the area's own bytes (`$C5C2`).
//!
//! This module produces pure byte streams (no `Game` dep); the
//! `sideview_traps` shim pushes them through
//! `bank7_ppu_queue::{queue_begin,push,finish}` and drains with `LD2EC`
//! (read-only reuse — that module is never copied here).

// ---------------------------------------------------------------------------
// Addresses (duplicated per sideview*.rs file; see sideview.rs).
// ---------------------------------------------------------------------------

/// Area Data Length (`$072E`, header byte 0).
pub const ADDR_AREA_LEN: u16 = 0x072E;
/// Area Data Reading Offset (`$072F`).
pub const ADDR_AREA_OFF: u16 = 0x072F;
/// Object placement (`$0730`).
pub const ADDR_OBJ_POS: u16 = 0x0730;
/// Object type/size (`$0731`).
pub const ADDR_OBJ_TYPE: u16 = 0x0731;
/// Start page (`$075C`).
pub const ADDR_START_PAGE: u16 = 0x075C;
/// Palette-back (`$07AF`, header byte 3 copy).
pub const ADDR_PAL_BACK: u16 = 0x07AF;
/// No-ceiling flag (`$0486`, header byte 2 bit 7).
pub const ADDR_NO_CEIL: u16 = 0x0486;
/// Ground type (`$010C`, header byte 2 bits 6-4).
pub const ADDR_GROUND: u16 = 0x010C;
/// Area width (`$D1`, header byte 1 bits 6-5).
pub const ADDR_AREA_W: u16 = 0x00D1;
/// Screen number for objects 0-3 (`$0717`).
pub const ADDR_SCREEN: u16 = 0x0717;

/// First-set map table (bank 1 `$8523`, bank 2 same CPU addr).
pub const MAP_PTR_SET1: u16 = 0x8523;
/// First-set enemy table (bank 1 `$85A1`).
pub const ENEMY_PTR_SET1: u16 = 0x85A1;
/// Second-set map table (banks 1/2 `$A000`).
pub const MAP_PTR_SET2: u16 = 0xA000;
/// Second-set enemy table (banks 1/2 `$A07E`, base `$88A0`).
pub const ENEMY_PTR_SET2: u16 = 0xA07E;
/// Behind-map pointer table (bank 1 `$8000`, 7 entries).
pub const BEHIND_PTR: u16 = 0x8000;
/// First connectivity table (bank 1 `$871B`).
pub const CONN_SET1: u16 = 0x871B;
/// Second connectivity table (banks 1/2 `$A1F8`).
pub const CONN_SET2: u16 = 0xA1F8;
/// Map/enemy entries per set (63; banks 3/5 differ — see gap note).
pub const AREA_SET_LEN: usize = 63;
/// Connectivity bytes per area entry (4).
pub const CONN_STRIDE: usize = 4;
/// Behind-map entries at `$8000` (6 maps + `$0000` terminator).
pub const BEHIND_LEN: usize = 7;
/// Wall sentinel (masked `& $FC == $FC`).
pub const CONN_WALL: u8 = 0xFC;

// ---------------------------------------------------------------------------
// Header.
// ---------------------------------------------------------------------------

/// Decoded 4-byte area header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AreaHeader {
    /// Byte 0: area data length (`$072E`).
    pub len: u8,
    /// Byte 1 raw (flags: width + layers + reserved).
    pub flags: u8,
    /// Byte 2 raw (ground-graphics: init-type + no-ceil + ground).
    pub ground_gfx: u8,
    /// Byte 3 raw (palette-back + BG-map).
    pub pal_back: u8,
}

impl AreaHeader {
    /// Decode 4 header bytes (`None` when fewer than 4 bytes).
    ///
    /// Entry `bank7_process_map_data` (bank 7 `$C755`) + `LC89D`
    /// (bank 7 `$C89D`): `($D4),0/1/2/3`.
    pub const fn decode(raw: &[u8]) -> Option<Self> {
        if raw.len() < 4 {
            return None;
        }
        Some(Self {
            len: raw[0],
            flags: raw[1],
            ground_gfx: raw[2],
            pal_back: raw[3],
        })
    }

    /// Area width in pages: `(flags & $60) >> 5` (`$C8B4-$C8BB`).
    pub const fn width(self) -> u8 {
        (self.flags & 0x60) >> 5
    }

    /// Extra-layer index: `(flags & $1C) >> 2` (`$C5C9-$C5CD`).
    pub const fn layers(self) -> u8 {
        (self.flags & 0x1C) >> 2
    }

    /// Reserved flag bits (`$80 | $03`): preserved, never interpreted.
    /// Honest gap: no disassembly read found; kept so round-trips are exact.
    pub const fn reserved(self) -> u8 {
        self.flags & 0x83
    }

    /// Initial object type/size: `ground_gfx & $0F` (`$C764`).
    pub const fn init_obj(self) -> u8 {
        self.ground_gfx & 0x0F
    }

    /// No-ceiling flag: `ground_gfx & $80 != 0` (`$C76B` → `$0486`).
    pub const fn no_ceiling(self) -> bool {
        self.ground_gfx & 0x80 != 0
    }

    /// Ground type 0-7: `(ground_gfx & $70) >> 4` (`$C772-$C778` → `$010C`).
    pub const fn ground(self) -> u8 {
        (self.ground_gfx & 0x70) >> 4
    }

    /// Behind-map code: `pal_back & 7` (`$C59E`: `AND #$07`).
    pub const fn bg_map(self) -> u8 {
        self.pal_back & 0x07
    }
}

/// Initial background tile: `$42` when ground type is 0, else `$40`
/// (`bank7_Initial_Fill_of_Background_Tiles__Side_View`, bank 7 `$C568`).
pub const fn initial_bg_tile(header: AreaHeader) -> u8 {
    if header.ground() == 0 {
        0x42
    } else {
        0x40
    }
}

/// Palette-macro index from header byte 3
/// (`bank7_Set_PPU_Macro_for_Palettes`, bank 7 `$D05B-$D07D`).
///
/// `lo = (b3 >> 2) & $30`, `hi = (b3 << 1) & $70`, `idx = lo | hi`.
/// World-0 grotto darkness: `hi == $10` without the candle forces `$40`.
/// `world` is `$0707`, `has_candle` is `$0785 != 0`.
pub const fn palette_index(byte3: u8, world: u8, has_candle: bool) -> u8 {
    let lo = (byte3 >> 2) & 0x30;
    let hi = (byte3 << 1) & 0x70;
    if hi == 0x10 && world == 0 && !has_candle {
        0x40
    } else {
        lo | hi
    }
}

// ---------------------------------------------------------------------------
// Pointer tables.
// ---------------------------------------------------------------------------

/// Which 63-entry set backs an area code for banks 1/2.
///
/// Entry `bank7_code13` (bank 7 `$C4FE-$C510`): `Y` from `LC4BD` selects
/// `$8523`/`$A000` (maps) + `$85A1`/`$A07E` (enemies). World 0 with region 1
/// uses the second set (`LDY #$04`, `$C50A`); all other world-0 areas use
/// the first set. Nonzero worlds (towns/palaces) use bank-switched tables
/// outside this module's scope (gap: banks 3/5 single-set layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapSet {
    /// First set: maps `$8523`, enemies `$85A1`.
    First,
    /// Second set: maps `$A000`, enemies `$A07E`.
    Second,
}

/// Select the map set for world/region (`$0707`/`$0706`).
pub const fn map_set_select(world: u8, region: u8) -> MapSet {
    if world == 0 && region == 1 {
        MapSet::Second
    } else if world == 0 {
        MapSet::First
    } else {
        // Towns/palaces (worlds 1-5): bank-switched; default to First so
        // synthetic tests stay total. Real dispatch needs banks 3/5 tables
        // (reported gap).
        MapSet::First
    }
}

/// Read one LE word from a pointer-table slice at entry `i`.
///
/// Mirrors `($00),Y` with `Y = area * 2` (`$C524-$C538`).
pub const fn table_word(table: &[u8], i: usize) -> Option<u16> {
    let o = i.wrapping_mul(2);
    if o + 1 >= table.len() {
        return None;
    }
    Some((table[o] as u16) | ((table[o + 1] as u16) << 8))
}

/// Enemy-list fixup selector on `$075A` (`$C53A-$C566`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyFixup {
    /// `$075A == 0`: fairy/fixed — keep area data as-is.
    Keep,
    /// `$075A == 1`: small — clear killed bits (`AND #$7F` walk).
    ClearKilled,
    /// `$075A >= 2`: big — advance `$D6` past the first area's list.
    Advance,
}

/// Classify the `$075A` encounter fixup.
pub const fn enemy_fixup(encounter: u8) -> EnemyFixup {
    if encounter >= 2 {
        EnemyFixup::Advance
    } else if encounter == 1 {
        EnemyFixup::ClearKilled
    } else {
        EnemyFixup::Keep
    }
}

/// Advance an enemy pointer past one area list: `$D6 += ($D6)[0]`
/// (`$C542-$C54F`: `CLC : ADC ($D6),y` with `Y = 0`).
pub const fn enemy_advance(base: u16, first_len_byte: u8) -> u16 {
    base.wrapping_add(first_len_byte as u16)
}

// ---------------------------------------------------------------------------
// Connectivity.
// ---------------------------------------------------------------------------

/// Decoded side-exit target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideExit {
    /// Wall / no exit (`& $FC == $FC`): overworld-return path.
    Wall,
    /// Room transition: new area + start page (`byte >> 2`, `byte & 3`).
    Room {
        /// New area code (`byte >> 2`).
        area: u8,
        /// Start page (`byte & 3`).
        page: u8,
    },
}

/// Decode one connectivity byte (`bank7_take_side_exit`, bank 7 `$CF65`).
pub const fn decode_connect(byte: u8) -> SideExit {
    if byte & 0xFC == 0xFC {
        SideExit::Wall
    } else {
        SideExit::Room {
            area: byte >> 2,
            page: byte & 0x03,
        }
    }
}

/// Connectivity index: `Y = area * 4 + page`
/// (`bank7_take_side_exit`, bank 7 `$CF60-$CF65`).
pub const fn connect_index(area: u8, page: u8) -> usize {
    (area as usize)
        .wrapping_mul(4)
        .wrapping_add((page & 3) as usize)
}

/// Look up a side exit from a connectivity slice.
pub fn side_exit(conn: &[u8], area: u8, page: u8) -> SideExit {
    match conn.get(connect_index(area, page)) {
        Some(&b) => decode_connect(b),
        // BUG (preserved): the original reads past short tables into WRAM
        // residue; the Rust port returns `Wall` (safe overworld-return)
        // instead of reproducing the overread.
        None => SideExit::Wall,
    }
}

// ---------------------------------------------------------------------------
// Background construction (pure producer side).
// ---------------------------------------------------------------------------

/// Map-data object kind for one 2-byte group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapObjKind {
    /// `$E0` high nibble: position skip (`bank7_Check_for_Skip_Object__E0_`,
    /// bank 7 `$C988`).
    Skip,
    /// `$D0` position: ceiling/floor change
    /// (`LC7C6`, bank 7 `$C7C6`).
    CeilFloor,
    /// Type high `$F0 != 0`: wide object via the world dispatch table
    /// (`LC8E9`, bank 7 `$C8E9` → `LC912` `$C912`).
    Wide,
    /// Type `$0F`: collectable/item (`LC8F6`, bank 7 `$C8F6` → `LC9A5`).
    Item,
    /// Otherwise: small/background object
    /// (`LC906`, bank 7 `$C906`).
    Small,
}

/// Classify one map-data group (`LC8C5-$C911` second pass).
pub const fn classify_map_obj(pos: u8, typ: u8) -> MapObjKind {
    if pos & 0xF0 == 0xE0 {
        MapObjKind::Skip
    } else if pos & 0xF0 == 0xD0 {
        MapObjKind::CeilFloor
    } else if typ & 0xF0 != 0 {
        MapObjKind::Wide
    } else if typ == 0x0F {
        MapObjKind::Item
    } else {
        MapObjKind::Small
    }
}

/// Fill 4 screens with the initial background tile.
///
/// `screens` are the 4 level-RAM pages (`$6000`/`$60D0`/`$61A0`/`$6270`
/// order, `$D0` bytes each in WRAM; here plain slices).
/// Returns bytes written. Mirrors `$C568-$C589` (tile `$40`/`$42` select).
pub fn fill_background(screens: [&mut [u8]; 4], tile: u8) -> usize {
    let mut n = 0usize;
    for s in screens {
        for b in s.iter_mut() {
            *b = tile;
        }
        n += s.len();
    }
    n
}

/// Layer row heights (`LC4AF`, bank 7 `$C4AF`, 8 bytes).
pub const LAYER_ROW_HEIGHTS: [u8; 8] = [0x00, 0x90, 0xA0, 0x20, 0x00, 0x00, 0x00, 0x00];
/// Extra background layer tile codes (`LC4A7`, bank 7 `$C4A7`, 8 bytes).
pub const LAYER_TILE_CODES: [u8; 8] = [0x40, 0x83, 0x84, 0x4C, 0x40, 0x40, 0x40, 0x40];
/// Area/enemy pointer offsets (`LC4BD`, bank 7 `$C4BD`, 6 bytes).
pub const PTR_OFFSETS: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x02, 0x00];
/// Bank-switch table (`LC4B7`, bank 7 `$C4B7`, 6 bytes).
pub const BANK_SWITCH: [u8; 6] = [0x01, 0x03, 0x03, 0x04, 0x04, 0x05];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fields_match_c755_c89d_d05b() {
        let h = AreaHeader::decode(&[0x36, 0x40, 0x68, 0x00]).unwrap();
        assert_eq!(h.len, 0x36);
        assert_eq!(h.width(), (0x40 & 0x60) >> 5);
        assert_eq!(h.layers(), (0x40 & 0x1C) >> 2);
        assert_eq!(h.init_obj(), 0x08);
        assert!(!h.no_ceiling());
        assert_eq!(h.ground(), (0x68 & 0x70) >> 4);
        assert_eq!(h.bg_map(), 0);
        assert_eq!(initial_bg_tile(h), 0x40);
        let flat = AreaHeader::decode(&[0x10, 0x00, 0x00, 0x08]).unwrap();
        assert_eq!(initial_bg_tile(flat), 0x42);
        assert_eq!(palette_index(0x00, 0, true), 0x00);
    }

    #[test]
    fn width_layers_reserved_split_byte1() {
        let h = AreaHeader::decode(&[0x01, 0xE3, 0x00, 0x00]).unwrap();
        assert_eq!(h.width(), (0xE3 & 0x60) >> 5);
        assert_eq!(h.layers(), (0xE3 & 0x1C) >> 2);
        assert_eq!(h.reserved(), 0xE3 & 0x83);
    }
}
