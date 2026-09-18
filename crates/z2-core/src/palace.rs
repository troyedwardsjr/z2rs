//! Palace engine: index/map sets, keys, item rooms, crystals, crumble,
//! Great Palace, Thunderbird/Dark Link triggers, ending.
//!
//! Pure logic over explicit params and slices (no `Game` dep); trap entry
//! points are listed in [`crate::palace_traps::PALACE_TRAPS`]. Boss behavior
//! kernels live in [`crate::enemy_boss`] (read-only reuse: Thunderbird/Dark
//! Link ticks delegate there); item classification (`bank7_get_item`,
//! `$E771`) lives in [`crate::player_magic`] (read-only reuse — this module
//! only maps palaces to item codes and applies flag writes); the sideview
//! item spawner (`LC9A5`, bank 7 `$C9A5`) lives in `sideview_spawn` /
//! `sideview_traps` (read-only reuse); overworld tile words live in
//! [`crate::overworld_map`] (read-only reuse for the stone write-back).
//! Nothing is copied here.
//!
//! Conventions: `ROM_*` constants are ROM offsets only — bytes are loaded
//! at runtime by the caller (see `*_at` loaders taking `&[u8]`). All fns are
//! total (no panics); wrapping arithmetic preserves 6502 wrap quirks.
//!
//! | section | label | addr | status |
//! |---|---|---|
//! | [`palace_map_set`] | `bank4_Area_Pointers_Palaces_Type_A` / `bank4_Area_Pointers_Palaces_Type_B_` | bank 4 `$8523` / `$A000` | ported |
//! | [`entrance_shared`] | `bank4_Area_Data_for_Palaces_Type_A_Entrance` | bank 4 `$82E5` | ported |
//! | [`boss_room_shared`] | `bank4_Area_Data_for_Palaces_Type_A_Boss_Room_and_Crystal_Statue` | bank 4 `$831B` | ported |
//! | [`lava_pit_height`] | `bank4_Objects_Construction_Routines_Lava_Pit_2_high_bottom_of_screen` / `bank5_Objects_Construction_Routines_Object_Lava_Pit__3_high_bottom_of_screen` | bank 4 `$820E` / bank 5 `$821F` | ported |
//! | [`locked_door_consume`] | `bank7_Enemy_Routines1_Locked_Door` (+ `bank7_link_door_collision_maybe`) | bank 7 `$D991` | ported |
//! | [`key_pickup`] | `LE7B5` (`INC $0793`) | bank 7 `$E7B5` | ported |
//! | [`grant_palace_item`] | `bank7_get_item` (`Inventory` arm) | bank 7 `$E78F` | ported |
//! | [`palace_item`] | palace↔item map (candle..cross) | bank 4/5 area data | ported |
//! | [`item_code_at`] | `LC9A5` (`LDA ($D4),y` → `$AF,x`) | bank 7 `$C9E5` | ported |
//! | [`crystal_place_gate`] / [`crystal_slot_index`] | `bank4_Related_to_placing_crystal_onto_statue` | bank 4 `$9AEB` | ported |
//! | [`crystal_flight_step`] | `bank4_Crystal_Flying_Up` | bank 4 `$9B2B` | ported |
//! | [`crystal_refill_pending`] | `L9B47` | bank 4 `$9B47` | ported |
//! | [`crystal_done`] | `L9B56` | bank 4 `$9B56` | ported |
//! | [`stone_tile`] | `Overworld_Tile_Mappings_and_Overworld_Palette_Codes` + `bank7_Turn_Palaces_into_Stone_Bank_1` | bank 0 `$87A3` / bank 7 `$E01B` | ported |
//! | [`barrier_gate`] | `bank5_Enemy_Routines1_Electric_Barrier` | bank 5 `$A238` | ported |
//! | [`thunder_door_gate`] | `bank5_Enemy_Routines1_Thunderbird` (door half) | bank 5 `$A359` | ported |
//! | [`thunder_wake`] | `bank5_Enemy_Routines1_Thunderbird` (wake half, `$A36B`) | bank 5 `$A36B` | ported |
//! | [`thunder_init_gate`] | `bank5_Enemy_Init_Routines_Thunderbird` | bank 5 `$A33B` | ported |
//! | [`dark_phase`] | `bank5_Enemy_Routines1_Dark_Link_Battle_Trigger` dispatch | bank 5 `$97C6` | ported |
//! | [`darklink_setup_gate`] | `L97DE` | bank 5 `$97DE` | ported |
//! | [`darklink_spawn_gate`] | `L983E` (`$074B == $81` → `$0725 = $0D`) | bank 5 `$983E` | ported |
//! | [`triforce_emit`] | `LB39E` (`A = $D2` tile, `$01` attr) | bank 5 `$B3A7` | ported |
//! | [`wake_zelda`] / [`roll_credits`] | `STA/INC $076C` (`LA6D9`/`$9244`/`$A6EC`/`$A7BD`/`$B2FF`) | bank 5 `$A6EC` et al. | ported |
//!
//! # Preserved quirks
//!
//! * Crystal slot math keeps the 6502 carry: `SEC; ADC $056C` stores
//!   `region_adj + palace_code + 1`, and `STA $078C,y` writes that index
//!   back (nonzero = placed) overlapping the `$0785-$078C` item flags.
//! * `INC $0793` / `DEC $0794` wrap (`$FF→$00` / `$00→$FF`); the crystal
//!   `DEC` is guarded by `BEQ` so the wrap is unreachable (kept anyway).
//! * Thunderbird is dormant (and fireball-immune) until Thunder lands
//!   (`$6E3F` sign gate); Dark Link corner-crouch dice stay in the
//!   [`crate::enemy_boss`] kernel — this module only wires trigger context.
//!
//! # Gaps
//!
//! * Exact `$56C` encodings for the Great Palace and the `$479F`/`$879F`
//!   stone-pointer semantics need a ROM image: pointer bytes are
//!   caller-supplied (`*_at` loaders), never embedded.
//! * OAM/PPU-macro emission (`$0725` selectors, `$0301` packets) is
//!   display-only; shims record the selector byte for the PPU layer.
//! * `$0720`/`$0723`-class scratch roles are modeled where routines touch
//!   them (`$0725` macro select, `$0728` freeze, `$072A` page); untouched
//!   scratch bytes are out of scope.

use crate::enemy_boss;
use crate::overworld_map as omap;

// ---------------------------------------------------------------------------
// RAM / ROM addresses.
// ---------------------------------------------------------------------------

/// Palace code (`Palace Code`, `$056C`).
pub const ADDR_PALACE_CODE: u16 = 0x056C;
/// Scene layout index (`$0561`).
pub const ADDR_SCENE: u16 = 0x0561;
/// World/area type (`$0707`): 3 = palaces 1/2/5, 4 = palaces 3/4/6,
/// 5 = Great Palace + ending.
pub const ADDR_WORLD: u16 = 0x0707;
/// Overworld/region index (`$0706`).
pub const ADDR_REGION: u16 = 0x0706;
/// Keys carried (`$0793`).
pub const ADDR_KEYS: u16 = 0x0793;
/// Crystals left (`$0794`).
pub const ADDR_CRYSTALS_LEFT: u16 = 0x0794;
/// Item flags base (`$0785-$078C`: candle..magic key).
pub const ADDR_ITEMS: u16 = 0x0785;
/// Crystal-placed flags overlap base (`$078C,y` store target).
pub const ADDR_CRYSTALS_PLACED: u16 = 0x078C;
/// Scroll freeze (`$0728`: 1 = freeze, no left/right exit).
pub const ADDR_FREEZE: u16 = 0x0728;
/// Map page (`$072A`).
pub const ADDR_PAGE: u16 = 0x072A;
/// PPU macro selector (`$0725`).
pub const ADDR_PPU_MACRO: u16 = 0x0725;
/// Game state / special routine (`$076C`).
pub const ADDR_GAME_STATE: u16 = 0x076C;
/// Crystal flight-done timer (`$0767`, set when the crystal seats).
pub const ADDR_CRYSTAL_TIMER: u16 = 0x0767;
/// Magic/life refill staging (`$070C`/`$070D`, `$FF` = pending).
pub const ADDR_MAGIC_ADD: u16 = 0x070C;
/// Life refill staging (see [`ADDR_MAGIC_ADD`]).
pub const ADDR_LIFE_ADD: u16 = 0x070D;
/// Boss-beaten key latch (`$07FB`: nonzero = key already taken music path).
pub const ADDR_BEATEN: u16 = 0x07FB;
/// Thunder-spell modifier (`$D9` scratch; `$6E3F` sign = Thunder struck).
pub const ADDR_THUNDER_MOD: u16 = 0x00D9;
/// Dark-Link phase dispatcher (`$63` index into `$97CE` table).
pub const ADDR_DARK_PHASE: u16 = 0x0063;

// ---------------------------------------------------------------------------
// ROM-offset descriptors (never bytes).
// ---------------------------------------------------------------------------

/// One ROM-resident table: `(PRG bank, CPU address, length)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomOff {
    /// PRG bank.
    pub bank: u8,
    /// CPU address of the first byte.
    pub addr: u16,
    /// Length in bytes (`0` = variable / caller-bounded).
    pub len: u16,
}

/// Shared palace entrance (`bank4_Area_Data_for_Palaces_Type_A_Entrance`,
/// bank 4 `$82E5`): every Type-A slot and every Type-B `$A000` slot points
/// here — a palace-specific quirk (entrances are identical).
pub const ROM_ENTRANCE_A: RomOff = RomOff {
    bank: 4,
    addr: 0x82E5,
    len: 0,
};
/// Shared boss/crystal room
/// (`bank4_Area_Data_for_Palaces_Type_A_Boss_Room_and_Crystal_Statue`,
/// bank 4 `$831B`).
pub const ROM_BOSS_ROOM_A: RomOff = RomOff {
    bank: 4,
    addr: 0x831B,
    len: 0,
};
/// Type-A palace bodies (`bank4_Area_Data_for_Palaces_Type_A`, `$861F`).
pub const ROM_TYPE_A: RomOff = RomOff {
    bank: 4,
    addr: 0x861F,
    len: 0,
};
/// Type-B palace bodies (`bank4_Area_Data_Palaces_Type_B0/B1/B2/B3`,
/// `$A0FC`/`$A2F4`/`$A440`/`$A640`).
pub const ROM_TYPE_B: [RomOff; 4] = [
    RomOff {
        bank: 4,
        addr: 0xA0FC,
        len: 0,
    },
    RomOff {
        bank: 4,
        addr: 0xA2F4,
        len: 0,
    },
    RomOff {
        bank: 4,
        addr: 0xA440,
        len: 0,
    },
    RomOff {
        bank: 4,
        addr: 0xA640,
        len: 0,
    },
];
/// Great Palace map chunks (`bank5_Area_Data_Great_Palace0..3`,
/// `$834E`/`$861F`/`$8817`/`$89D8`).
pub const ROM_GREAT_PALACE: [RomOff; 4] = [
    RomOff {
        bank: 5,
        addr: 0x834E,
        len: 0,
    },
    RomOff {
        bank: 5,
        addr: 0x861F,
        len: 0,
    },
    RomOff {
        bank: 5,
        addr: 0x8817,
        len: 0,
    },
    RomOff {
        bank: 5,
        addr: 0x89D8,
        len: 0,
    },
];
/// Crystal statue tiles (`bank4_Table_for_Crystal_Statue_Tile_Mappings`,
/// bank 4 `$827E`, 4 rows).
pub const ROM_STATUE_TILES: RomOff = RomOff {
    bank: 4,
    addr: 0x827E,
    len: 16,
};
/// Thunderbird wing table (`bank5_table_Thunderbird`, bank 5 `$A34F`, 2
/// bytes) + wave table (`LA351`, `$A351`, 8 bytes).
pub const ROM_THUNDERBIRD_TAB: RomOff = RomOff {
    bank: 5,
    addr: 0xA34F,
    len: 10,
};
/// Dark-Link phase table (`bank5_pointer_table_dark_link`, bank 5 `$97CE`,
/// 8 entries).
pub const ROM_DARKLINK_TAB: RomOff = RomOff {
    bank: 5,
    addr: 0x97CE,
    len: 16,
};
/// Stone write-back pointers (`$479F`/`$879F` classes): caller-supplied
/// bytes, never embedded (gap: exact pointer semantics need a ROM image).
pub const ROM_STONE_PTR_LO: RomOff = RomOff {
    bank: 0,
    addr: 0x479F,
    len: 0,
};
/// Stone write-back mirror pointer (see [`ROM_STONE_PTR_LO`]).
pub const ROM_STONE_PTR_HI: RomOff = RomOff {
    bank: 0,
    addr: 0x879F,
    len: 0,
};

// ---------------------------------------------------------------------------
// Palace index + bank-4 map sets.
// ---------------------------------------------------------------------------

/// Palace number 1-6 (Great Palace is separate; `$56C` holds the code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PalaceId {
    /// Parapa (candle).
    P1,
    /// Midoro (glove).
    P2,
    /// Island (raft).
    P3,
    /// Maze (boots).
    P4,
    /// Ocean (flute).
    P5,
    /// Hidden (cross).
    P6,
}

/// Great Palace marker (bank 5, world 5; `$56C` encoding differs — gap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GreatPalace;

/// Map set: palaces 1/2/5 use set A (world 3), 3/4/6 use set B (world 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapSet {
    /// Palaces 1/2/5 (`bank4_Area_Pointers_Palaces_Type_A`, `$8523`).
    SetA,
    /// Palaces 3/4/6 (`bank4_Area_Pointers_Palaces_Type_B_`, `$A000`).
    SetB,
}

/// Rooms per map set (two sets of 63).
pub const ROOMS_PER_SET: usize = 63;

/// Decode a palace number (1-6) to [`PalaceId`).
pub const fn palace_of_number(n: u8) -> Option<PalaceId> {
    match n {
        1 => Some(PalaceId::P1),
        2 => Some(PalaceId::P2),
        3 => Some(PalaceId::P3),
        4 => Some(PalaceId::P4),
        5 => Some(PalaceId::P5),
        6 => Some(PalaceId::P6),
        _ => None,
    }
}

/// Palace number back (1-6).
pub const fn palace_number(p: PalaceId) -> u8 {
    match p {
        PalaceId::P1 => 1,
        PalaceId::P2 => 2,
        PalaceId::P3 => 3,
        PalaceId::P4 => 4,
        PalaceId::P5 => 5,
        PalaceId::P6 => 6,
    }
}

/// Map-set select: 1/2/5 → A, 3/4/6 → B
/// (`bank4_Area_Pointers_Palaces_Type_A` vs `..._Type_B_`).
pub const fn palace_map_set(p: PalaceId) -> MapSet {
    match p {
        PalaceId::P1 | PalaceId::P2 | PalaceId::P5 => MapSet::SetA,
        PalaceId::P3 | PalaceId::P4 | PalaceId::P6 => MapSet::SetB,
    }
}

/// World byte (`$0707`) for a map set: A → 3, B → 4.
pub const fn world_of_set(set: MapSet) -> u8 {
    match set {
        MapSet::SetA => 3,
        MapSet::SetB => 4,
    }
}

/// World byte for the Great Palace + ending (`$0707 == 5`).
pub const WORLD_GREAT_PALACE: u8 = 5;

/// Entrances are shared: every Type-A slot and every Type-B `$A000` slot
/// points at [`ROM_ENTRANCE_A`] (bank 4 `$82E5`).
pub const fn entrance_shared() -> bool {
    true
}

/// Boss/crystal rooms are shared via [`ROM_BOSS_ROOM_A`] (bank 4 `$831B`).
pub const fn boss_room_shared() -> bool {
    true
}

/// Lava-pit object height quirk: bank 4 builds 2-high pits (`$820E`),
/// bank 5 builds 3-high pits (`$821F`). Returns `None` for other banks.
pub const fn lava_pit_height(prg_bank: u8) -> Option<u8> {
    match prg_bank {
        4 => Some(2),
        5 => Some(3),
        _ => None,
    }
}

/// Type-B body selector: which of the four `B0..B3` blobs a set-B palace
/// uses. Order follows the `$A000`-table slots (gap: exact slot ↔ palace
/// wiring needs a ROM image; caller passes the slot).
pub const fn type_b_body(slot: u8) -> RomOff {
    ROM_TYPE_B[(slot as usize) % ROM_TYPE_B.len()]
}

/// Great Palace chunk selector (4 chunks, `$834E`/`$861F`/`$8817`/`$89D8`).
pub const fn great_palace_chunk(slot: u8) -> RomOff {
    ROM_GREAT_PALACE[(slot as usize) % ROM_GREAT_PALACE.len()]
}

// ---------------------------------------------------------------------------
// Keys + locked doors ($0793 / $078C magic key).
// ---------------------------------------------------------------------------

/// Maximum displayable keys (`Keys (00-09)` comment at `$D9E4`).
pub const MAX_KEYS: u8 = 9;

/// Key pickup (`LE7B5`: `INC $0793`). Wraps `$FF→$00` like the 6502.
pub const fn key_pickup(keys: u8) -> u8 {
    keys.wrapping_add(1)
}

/// Locked-door consumption (`bank7_Enemy_Routines1_Locked_Door`, `$D991`):
/// the magic key (`$078C != 0`) bypasses without decrementing; otherwise a
/// carried key is consumed. Returns the new key count, or `None` when the
/// door stays shut (no key and no magic key).
pub const fn locked_door_consume(keys: u8, magic_key: bool) -> Option<u8> {
    if magic_key {
        Some(keys)
    } else if keys == 0 {
        None
    } else {
        Some(keys - 1)
    }
}

/// Door-blocked predicate (inverse of [`locked_door_consume`]).
pub const fn door_blocked(keys: u8, magic_key: bool) -> bool {
    locked_door_consume(keys, magic_key).is_none()
}

// ---------------------------------------------------------------------------
// Item rooms ($0785-$078C, $0793 keys, containers).
// ---------------------------------------------------------------------------

/// Palace item codes (`bank7_get_item`, `$E771`: `Y = $AF & $7F`).
pub const ITEM_CANDLE: u8 = 0;
/// Handy glove.
pub const ITEM_GLOVE: u8 = 1;
/// Raft.
pub const ITEM_RAFT: u8 = 2;
/// Boots.
pub const ITEM_BOOTS: u8 = 3;
/// Flute.
pub const ITEM_FLUTE: u8 = 4;
/// Cross.
pub const ITEM_CROSS: u8 = 5;
/// Hammer (overworld-found; same flag row).
pub const ITEM_HAMMER: u8 = 6;
/// Magic key (bypasses [`locked_door_consume`]).
pub const ITEM_MAGIC_KEY: u8 = 7;
/// Dropped key (goes to `$0793`, not the flag row).
pub const ITEM_KEY: u8 = 8;
/// Magic container.
pub const ITEM_MAGIC_CONTAINER: u8 = 0x0E;
/// Heart container.
pub const ITEM_HEART_CONTAINER: u8 = 0x0F;

/// Which palace item each palace holds (bytes live in ROM area data;
/// loaded at runtime via [`item_code_at`]).
pub const fn palace_item(p: PalaceId) -> u8 {
    match p {
        PalaceId::P1 => ITEM_CANDLE,
        PalaceId::P2 => ITEM_GLOVE,
        PalaceId::P3 => ITEM_RAFT,
        PalaceId::P4 => ITEM_BOOTS,
        PalaceId::P5 => ITEM_FLUTE,
        PalaceId::P6 => ITEM_CROSS,
    }
}

/// Grant a flag-row item over the 8-byte `$0785-$078C` slice
/// (`LDA $0785,y : ORA #$01 : STA $0785,y` at `$E78F-$E794`).
///
/// Classification is owned by [`crate::player_magic::item_pickup`]
/// (read-only reuse); callers pass `code` through only on its `Inventory`
/// arm (`code < 8`). Returns `false` when `code` is not a flag item or the
/// slice is short.
pub fn grant_palace_item(items: &mut [u8], code: u8) -> bool {
    let y = code & 0x7F;
    if y >= 8 {
        return false;
    }
    let i = y as usize;
    if i < items.len() {
        items[i] |= 0x01;
        true
    } else {
        false
    }
}

/// Read the item code for an item-room object from staged area bytes
/// (`LC9A5` tail: `LDA ($D4),y` at `$C9E5` → `$AF,x`). `None` when
/// `offset` is out of range (ROM bytes need `Z2_ROM`; tests stage them).
pub fn item_code_at(area_bytes: &[u8], offset: usize) -> Option<u8> {
    area_bytes.get(offset).copied()
}

// ---------------------------------------------------------------------------
// Crystals ($0794 / $078C,y / $0767 / $070C-$070D).
// ---------------------------------------------------------------------------

/// Crystal spawn anchor (`bank4_Enemy_Init_Routines_Crystal_Slot_and_Crystal`,
/// `$9A73`: `$2A = $62`, `$4E = $7C`).
pub const CRYSTAL_Y: u8 = 0x62;
/// Crystal spawn X (see [`CRYSTAL_Y`]).
pub const CRYSTAL_X: u8 = 0x7C;
/// Seated-crystal Y target (`bank4_Crystal_Flying_Up`, `$9B2B`: `CMP #$62`).
pub const CRYSTAL_SEAT_Y: u8 = 0x62;
/// Placed-crystal perch after Link sets it (`$9B0B`: `$2A = $A0`).
pub const CRYSTAL_PLACE_Y: u8 = 0xA0;
/// Refill sentinel (`L9B47`: `$070C = $070D = $FF` = pending).
pub const REFILL_PENDING: u8 = 0xFF;

/// Crystal-placement gate (`bank4_Related_to_placing_crystal_onto_statue`,
/// `$9AEB`): decor active (`c9 != 0`) blocks; no crystals left blocks;
/// needs enemy touch (`$A8 & $10`) and Link grounded (`$A7 & $04`).
pub const fn crystal_place_gate(
    decor_c9: u8,
    crystals_left: u8,
    enemy_touch: bool,
    link_grounded: bool,
) -> bool {
    if decor_c9 != 0 {
        return false;
    }
    if crystals_left == 0 {
        return false;
    }
    enemy_touch && link_grounded
}

/// Crystal slot index (`$9B1A-$9B26`): `region == 0` passes through, else
/// `+2`; then `SEC; ADC $056C` adds the palace code plus the carry.
/// `STA $078C,y` stores that same index (nonzero = placed).
pub const fn crystal_slot_index(region: u8, palace_code: u8) -> u8 {
    let base = if region == 0 {
        region
    } else {
        region.wrapping_add(2)
    };
    base.wrapping_add(palace_code).wrapping_add(1)
}

/// Absolute flag address for a slot (`$078C + y`).
pub const fn crystal_flag_addr(slot: u8) -> u16 {
    ADDR_CRYSTALS_PLACED.wrapping_add(slot as u16)
}

/// `DEC $0794` (wrapping; the `BEQ` guard makes `$00→$FF` unreachable).
pub const fn crystals_left_after(crystals_left: u8) -> u8 {
    crystals_left.wrapping_sub(1)
}

/// Crystal flight step (`bank4_Crystal_Flying_Up`, `$9B2B`): rises one px;
/// seating exactly on `$62` latches `$0767` and the fanfare. Returns
/// `(new_y, seated)`.
pub const fn crystal_flight_step(y: u8) -> (u8, bool) {
    let next = y.wrapping_sub(1);
    (next, next == CRYSTAL_SEAT_Y)
}

/// Refill kick (`L9B47`, `$9B47`): only when `$07FB == 0` (boss key not yet
/// taken) does the game stage `$FF` into `$070C`/`$070D` and advance `$AF`.
/// Returns the sentinel to store, or `None` to hold.
pub const fn crystal_refill_pending(beaten_fb: u8) -> Option<u8> {
    if beaten_fb == 0 {
        Some(REFILL_PENDING)
    } else {
        None
    }
}

/// Completion gate (`L9B56`, `$9B56`): done when `$070C | $070D == 0`.
pub const fn crystal_done(magic_add: u8, life_add: u8) -> bool {
    magic_add | life_add == 0
}

// ---------------------------------------------------------------------------
// Palace crumble + overworld stone write-back.
// ---------------------------------------------------------------------------

/// Stone write-back trigger: a seated crystal with its flag stored
/// (`bank7_Turn_Palaces_into_Stone_Bank_1`, `$E01B` → `L879B`, `$879B`).
pub const fn stone_patch_gate(crystal_seated: bool, flag_stored: bool) -> bool {
    crystal_seated && flag_stored
}

/// Map one overworld tile to its post-crumble form, reusing
/// [`omap::TILE_MAPPINGS`] read-only (palace row `$60-$63` → rock row
/// `$56-$59`; everything else is identity). Cites bank 0 `$87A3`.
pub fn stone_tile(tile: u8) -> u8 {
    let palace = omap::TILE_MAPPINGS[2];
    let rock = omap::TILE_MAPPINGS[14];
    for i in 0..4 {
        if palace[i] == tile {
            return rock[i];
        }
    }
    tile
}

/// True when `tile` is a palace tile awaiting the stone stamp.
pub fn is_palace_tile(tile: u8) -> bool {
    stone_tile(tile) != tile
}

// ---------------------------------------------------------------------------
// Great Palace: barrier ($A238) + Thunderbird ($A359/$9EBF/$A33B).
// ---------------------------------------------------------------------------

/// Electric-barrier trigger X (`$A245`: `CMP #$C0`).
pub const BARRIER_TRIGGER_X: u8 = 0xC0;
/// Thunderbird door timer (`$A366`: `$0504 = $90`).
pub const THUNDER_DOOR_TIMER: u8 = 0x90;
/// Thunderbird wake HP split (`$A3CE`: `CMP #$C0` display-only above).
pub const THUNDER_HP_SPLIT: u8 = 0xC0;
/// Thunderbird enrage HP (`$A402`: `CPY #$60` doubles the fire rate below).
pub const THUNDER_ENRAGE_HP: u8 = 0x60;

/// Barrier gate (`bank5_Enemy_Routines1_Electric_Barrier`, `$A238`):
/// an opening barrier (`aux != 0`) runs the dissolve path; otherwise all
/// six crystals must be placed (`crystals_left == 0`), decor must be idle,
/// Link must be on page 0 at/after `$C0` — then the passage opens
/// (`ROL $DE` locks input, `$AF = 3`, `$80 = 3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierGate {
    /// Keep sparkling (hold).
    Hold,
    /// Passage opens (lock input, advance `$AF`).
    Open,
    /// Already opening (dissolve anim).
    Dissolving,
}

/// Barrier gate inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarrierIn {
    /// `$AF` (nonzero = already opening).
    pub aux: u8,
    /// `$0794` crystals left.
    pub crystals_left: u8,
    /// `$C9` decor state (nonzero = busy).
    pub decor: u8,
    /// `$3B` page.
    pub page: u8,
    /// `$4D` Link X.
    pub link_x: u8,
}

/// Barrier gate (`$A234-$A25F`).
pub const fn barrier_gate(inp: BarrierIn) -> BarrierGate {
    if inp.aux != 0 {
        return BarrierGate::Dissolving;
    }
    if inp.crystals_left != 0 || inp.decor != 0 {
        return BarrierGate::Hold;
    }
    if inp.page != 0 || inp.link_x < BARRIER_TRIGGER_X {
        return BarrierGate::Hold;
    }
    BarrierGate::Open
}

/// Thunderbird door/trigger gate (`bank5_Enemy_Routines1_Thunderbird`,
/// `$A359`): frozen (`$0728 != 0`) means combat (kernel owns it); on page 0
/// the room holds; otherwise the doors slam (`INC $0728`,
/// `$0504 = $90`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThunderDoor {
    /// Combat phase (delegate to [`enemy_boss::thunderbird`]).
    Combat,
    /// Not yet entered (hold on page 0).
    Hold,
    /// Doors slam shut (freeze + timer).
    Slam,
}

/// Thunder door gate inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThunderDoorIn {
    /// `$0728` freeze.
    pub frozen: u8,
    /// `$072A` page.
    pub page: u8,
}

/// Thunder door gate (`$A359-$A363`).
pub const fn thunder_door_gate(inp: ThunderDoorIn) -> ThunderDoor {
    if inp.frozen != 0 {
        ThunderDoor::Combat
    } else if inp.page == 0 {
        ThunderDoor::Hold
    } else {
        ThunderDoor::Slam
    }
}

/// Thunderbird init gate (`bank5_Enemy_Init_Routines_Thunderbird`, `$A33B`):
/// the shared `LC2A6` screen check — `0` despawns (`$B6 = 0`), else the
/// bird arms (`$057E = $08`, i.e. [`enemy_boss::boss_init`] keep path).
/// Returns `true` when the slot is kept.
pub const fn thunder_init_gate(screen_check: u8) -> bool {
    enemy_boss::boss_init(screen_check).0
}

/// Thunder wake flag (`$A36B`: `$6E3F` sign; `BPL` = still dormant).
/// Delegates to the kernel's [`enemy_boss::thunderbird_awake`] (Thunder
/// striking sets it; fireballs stay dead until then).
pub const fn thunder_wake(awake: bool, thunder_struck: bool) -> bool {
    enemy_boss::thunderbird_awake(awake, thunder_struck)
}

/// Thunderbird combat tick: palace-context wrapper over the
/// [`enemy_boss::thunderbird`] kernel (fire rate `$1F`, `$0F` enraged —
/// kernel-owned; corner timings preserved there).
pub const fn thunder_tick(inp: enemy_boss::ThunderbirdIn) -> enemy_boss::ThunderbirdOut {
    enemy_boss::thunderbird(inp)
}

// ---------------------------------------------------------------------------
// Dark Link trigger room ($9796/$97C6/$98EB) + Triforce + ending ($76C).
// ---------------------------------------------------------------------------

/// Dark-Link phase dispatcher (`bank5_Enemy_Routines1_Dark_Link_Battle_Trigger`,
/// `$97C6` via `$63` into the `$97CE` table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DarkPhase {
    /// `L97DE`: setup — freeze when the page scrolls in, ground Link.
    Setup,
    /// `L980D`: Triforce display + fadeout.
    Triforce,
    /// `L983E`: flash + palette swap (spawns on `$074B == $81`).
    Flash,
    /// `L987B`: spawn Dark Link proper.
    Spawn,
    /// `L9B3A`/`L9B53`/`L9B91`/`L9BA8`: shared duel tail (kernel owns AI).
    Duel,
}

/// Dispatch `$63` to a phase (table has 8 entries; 4-7 share the duel tail).
pub const fn dark_phase(state63: u8) -> DarkPhase {
    match state63 {
        0 => DarkPhase::Setup,
        1 => DarkPhase::Triforce,
        2 => DarkPhase::Flash,
        3 => DarkPhase::Spawn,
        _ => DarkPhase::Duel,
    }
}

/// Setup gate (`L97DE`, `$97DE`): advances `$AF`, then holds on page 0;
/// otherwise freezes scrolling, zeroes Link speed, and — when Link is
/// grounded (`$A7 & $04`) — plants him at `$29 = $A0`, anim 3, facing in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DarkSetup {
    /// Page 0: hold (room not entered).
    Hold,
    /// Freeze + wait (airborne or pre-plant).
    Freeze,
    /// Freeze + plant Link for the Triforce reveal.
    Plant,
}

/// Setup gate inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DarkSetupIn {
    /// `$072A` page.
    pub page: u8,
    /// `$A7 & $04 != 0` (Link grounded).
    pub grounded: bool,
}

/// Setup gate (`$97E3-$980A`).
pub const fn darklink_setup_gate(inp: DarkSetupIn) -> DarkSetup {
    if inp.page == 0 {
        DarkSetup::Hold
    } else if inp.grounded {
        DarkSetup::Plant
    } else {
        DarkSetup::Freeze
    }
}

/// Flash→spawn gate (`L983E`, `$983E`): the palette swap fires when
/// `$074B == $81`, selecting PPU macro `$0D`.
pub const FLASH_SPAWN_COUNTER: u8 = 0x81;
/// PPU macro for the flash swap (see [`FLASH_SPAWN_COUNTER`]).
pub const FLASH_SPAWN_MACRO: u8 = 0x0D;

/// Flash gate: `true` when the spawn swap should fire.
pub const fn darklink_spawn_gate(flash_counter: u8) -> bool {
    flash_counter == FLASH_SPAWN_COUNTER
}

/// Dark-Link init snapshot (`bank5_Enemy_Init_Routines_Dark_Link_Battle_Trigger`,
/// `$9796`): monster `$A1 = $23`, HP `$C2 = $08`, position copied from the
/// trigger slot, `$0753 = 2`, sprite `$1A = 1`, slots latched. Stat mirroring
/// (attack/life levels) is kernel-owned; this is the trigger context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DarkInit {
    /// Monster id (`$A1`).
    pub mon_id: u8,
    /// Monster HP (`$C2`).
    pub mon_hp: u8,
    /// `$0753` latch.
    pub latch_753: u8,
    /// Sprite latch (`$1A`).
    pub sprite: u8,
}

/// Init constants (`$9796-$97B2`).
pub const fn darklink_init() -> DarkInit {
    DarkInit {
        mon_id: 0x23,
        mon_hp: 0x08,
        latch_753: 0x02,
        sprite: 0x01,
    }
}

/// Dark-Link duel tick: palace-context wrapper over the
/// [`enemy_boss::dark_link`] kernel (mirror-drift, crouch dice including
/// the corner-crouch quirk, whiff punish — all kernel-owned).
pub const fn darklink_tick(inp: enemy_boss::DarkLinkIn) -> enemy_boss::DarkLinkOut {
    enemy_boss::dark_link(inp)
}

/// Triforce tile code (`LB39E`, `$B3A7`: `A = $D2`).
pub const TRIFORCE_TILE: u8 = 0xD2;
/// Triforce attribute code (`$B3AC`: `A = $01`).
pub const TRIFORCE_ATTR: u8 = 0x01;

/// Triforce OAM emit (`LB39E`): tile + attribute words pushed per cleared
/// game slot. Returns the `(tile, attr)` pair to store.
pub const fn triforce_emit() -> (u8, u8) {
    (TRIFORCE_TILE, TRIFORCE_ATTR)
}

/// `$076C` game states (Data Crystal + listing `76C begin a special
/// routine`).
pub const STATE_INGAME: u8 = 1;
/// Wake-Zelda path (`STA $076C` at `$A6EC`).
pub const STATE_WAKE_ZELDA: u8 = 3;
/// Roll-credits path (`INC $076C` at `$9244`/`$A7BD`/`$B2FF`).
pub const STATE_CREDITS: u8 = 4;

/// Wake-Zelda trigger: Triforce claimed in the Dark-Link room drives
/// `$076C = 3` (`STA $076C`, `$A6EC`).
pub const fn wake_zelda() -> u8 {
    STATE_WAKE_ZELDA
}

/// Credits step (`INC $076C`): wraps like the 6502.
pub const fn roll_credits(state: u8) -> u8 {
    state.wrapping_add(1)
}

/// Full ending chain check: `3 → 4` is wake-then-credits.
pub const fn ending_chain(state: u8) -> bool {
    state == STATE_WAKE_ZELDA || state == STATE_CREDITS
}
