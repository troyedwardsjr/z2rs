//! Overworld locations: the per-continent location tables and what each
//! vanilla slot is.
//!
//! # Table layout
//!
//! Each continent has 63 slots stored as four parallel byte arrays at
//! [`Cont::table_addr`] (byte `k` of slot `i` at `base + i + 63 * k`):
//!
//! * byte 0: bits 0-6 raw row (internal row + 30), bit 7 "external" (an
//!   entrance to a town, palace or another continent). `0` = not on the map.
//! * byte 1: bits 0-5 column, bits 6-7 entrance index.
//! * byte 2: bits 0-5 sideview area, bits 6-7 entry page.
//! * byte 3: bits 0-4 world, bit 5 enter from the right, bit 6 passthrough
//!   (Link comes out on the far side), bit 7 fall into a hole.
//!
//! Death Mountain and Maze Island share one table layout; the game keeps a
//! copy in each bank, and each continent only uses its own slots.

use super::map::{Cont, RAW_ROW_BASE};
use super::terrain::Terrain;
use crate::rom::Rom;
use crate::RandoError;

/// Slots per continent table.
pub const SLOTS: usize = 63;

/// One location table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loc {
    /// Raw row (`0` = not on the map).
    pub raw_y: u8,
    /// Column.
    pub x: u8,
    /// Entrance to another world (town, palace, connector).
    pub external: bool,
    /// Entrance index (bits 6-7 of byte 1).
    pub entrance: u8,
    /// Sideview area.
    pub map: u8,
    /// Entry page.
    pub page: u8,
    /// World number.
    pub world: u8,
    /// Enter from the right edge.
    pub right: bool,
    /// Link comes out on the far side.
    pub pass: bool,
    /// Falls into a hole.
    pub hole: bool,
}

impl Loc {
    /// Decode the four table bytes.
    #[must_use]
    pub fn from_bytes(b: [u8; 4]) -> Loc {
        Loc {
            raw_y: b[0] & 0x7F,
            x: b[1] & 0x3F,
            external: b[0] & 0x80 != 0,
            entrance: b[1] >> 6,
            map: b[2] & 0x3F,
            page: b[2] >> 6,
            world: b[3] & 0x1F,
            right: b[3] & 0x20 != 0,
            pass: b[3] & 0x40 != 0,
            hole: b[3] & 0x80 != 0,
        }
    }

    /// Encode the four table bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 4] {
        [
            (self.raw_y & 0x7F) | if self.external { 0x80 } else { 0 },
            (self.x & 0x3F) | (self.entrance << 6),
            (self.map & 0x3F) | (self.page << 6),
            (self.world & 0x1F)
                | if self.right { 0x20 } else { 0 }
                | if self.pass { 0x40 } else { 0 }
                | if self.hole { 0x80 } else { 0 },
        ]
    }

    /// Internal (row, col), when the raw row is on the 75-row map.
    #[must_use]
    pub fn pos(self) -> Option<(usize, usize)> {
        if self.raw_y >= RAW_ROW_BASE && self.raw_y < RAW_ROW_BASE + 75 {
            Some((usize::from(self.raw_y - RAW_ROW_BASE), usize::from(self.x)))
        } else {
            None
        }
    }

    /// Move to internal (row, col).
    pub fn set_pos(&mut self, r: usize, c: usize) {
        self.raw_y = RAW_ROW_BASE + r as u8;
        self.x = c as u8;
    }
}

/// Read one continent's whole table.
pub fn read_table(rom: &Rom, cont: Cont) -> Result<Vec<Loc>, RandoError> {
    let base = rom.cpu_offset(cont.bank(), cont.table_addr())?;
    let mut v = Vec::with_capacity(SLOTS);
    for i in 0..SLOTS {
        let mut b = [0u8; 4];
        for (k, bk) in b.iter_mut().enumerate() {
            *bk = rom.read(base + i + SLOTS * k)?;
        }
        v.push(Loc::from_bytes(b));
    }
    Ok(v)
}

/// Write one slot.
pub fn write_slot(rom: &mut Rom, cont: Cont, slot: usize, b: [u8; 4]) -> Result<(), RandoError> {
    let base = rom.cpu_offset(cont.bank(), cont.table_addr())?;
    for (k, bk) in b.iter().enumerate() {
        rom.write(base + slot + SLOTS * k, &[*bk])?;
    }
    Ok(())
}

/// What a vanilla slot is, for placement and logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The North Palace (the start, never moved).
    Start,
    /// A palace entrance (palace number 1-7 in vanilla).
    Palace(u8),
    /// A town entrance.
    Town,
    /// A cave or tile that holds an item.
    Item,
    /// One end of a cave (or town) that leads to another spot.
    Link,
    /// A plain cave with nothing to take (none in vanilla).
    Cave,
    /// A minor encounter tile (jars, fairies, bags).
    Minor,
    /// A forced encounter on a road, bridge, desert or lava path.
    Encounter,
    /// A continent connector (bridge, raft, cave 1, cave 2).
    Connector,
    /// Something else (Bagu's house, the King's tomb).
    Special,
}

/// Static facts about one vanilla slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotInfo {
    /// Continent.
    pub cont: Cont,
    /// Slot index.
    pub slot: u8,
    /// Spoiler name.
    pub name: &'static str,
    /// Kind.
    pub class: Class,
    /// Linked group (slots that share one sideview and lead to each
    /// other); members list the group's slots.
    pub group: &'static [u8],
}

macro_rules! slots {
    ($cont:expr; $( $slot:literal $name:literal $class:expr $(, [$($g:literal),*])? ;)*) => {
        &[ $( SlotInfo { cont: $cont, slot: $slot, name: $name, class: $class, group: &[$($($g),*)?] } ),* ]
    };
}

use Class::*;

/// West Hyrule slots in use.
pub const WEST: &[SlotInfo] = slots![Cont::West;
    0 "North Palace" Start;
    1 "Trophy cave" Item;
    2 "Forest by the start" Minor;
    3 "Magic container cave" Item;
    4 "Forest by Saria" Minor;
    5 "Grass tile" Item;
    6 "Bagu's woods 1" Minor;
    7 "Mountain pass road" Encounter;
    8 "Swamp 1" Minor;
    9 "Graveyard tile" Minor;
    10 "Parapa cave north" Link, [10, 11];
    11 "Parapa cave south" Link, [10, 11];
    12 "Jump cave north" Link, [12, 13];
    13 "Jump cave south" Link, [12, 13];
    14 "P-bag cave" Item;
    15 "Medicine cave" Item;
    16 "Heart container cave" Item;
    17 "Fairy cave hole" Link, [17, 18];
    18 "Fairy cave exit" Link, [17, 18];
    19 "Bridge north of Saria" Encounter;
    20 "Bridge east of Saria" Encounter;
    21 "Long bridge west" Encounter;
    22 "Long bridge east" Encounter;
    23 "Forest by the jump cave" Minor;
    24 "Swamp 2" Minor;
    25 "Forest east of Saria" Minor;
    26 "Bagu's woods 2" Minor;
    27 "Bagu's woods 3" Minor;
    28 "Bagu's woods 4" Minor;
    29 "Bagu's woods 5" Minor;
    30 "Road tile" Minor;
    32 "Desert tile" Minor;
    40 "West bridge" Connector;
    41 "West raft" Connector;
    42 "Death Mountain entrance" Connector;
    43 "Death Mountain exit" Connector;
    44 "King's tomb" Special;
    45 "Rauru" Town;
    47 "Ruto" Town;
    48 "Saria (south)" Town, [48, 49];
    49 "Saria (north)" Town, [48, 49];
    50 "Bagu's house" Special;
    51 "Mido" Town;
    52 "Palace 1" Palace(1);
    53 "Palace 2" Palace(2);
    54 "Palace 3" Palace(3);
];

/// Death Mountain slots in use.
pub const DM: &[SlotInfo] = slots![Cont::DeathMountain;
    0 "Cave 1 A" Link, [0, 1];
    1 "Cave 1 B" Link, [0, 1];
    2 "Cave 2 A" Link, [2, 3];
    3 "Cave 2 B" Link, [2, 3];
    4 "Cave 3 A" Link, [4, 5];
    5 "Cave 3 B" Link, [4, 5];
    6 "Cave 4 A" Link, [6, 7];
    7 "Cave 4 B" Link, [6, 7];
    8 "Cave 5 A" Link, [8, 9];
    9 "Cave 5 B" Link, [8, 9];
    10 "Cave 6 A" Link, [10, 11];
    11 "Cave 6 B" Link, [10, 11];
    12 "Cave 7 A" Link, [12, 13];
    13 "Cave 7 B" Link, [12, 13];
    14 "Cave 8 A" Link, [14, 15];
    15 "Cave 8 B" Link, [14, 15];
    16 "Cave 9 A" Link, [16, 17];
    17 "Cave 9 B" Link, [16, 17];
    18 "Cave 10 A" Link, [18, 19];
    19 "Cave 10 B" Link, [18, 19];
    20 "Cave 11 A" Link, [20, 21];
    21 "Cave 11 B" Link, [20, 21];
    22 "Cave 12 A" Link, [22, 23];
    23 "Cave 12 B" Link, [22, 23];
    24 "Cave 13 A" Link, [24, 25];
    25 "Cave 13 B" Link, [24, 25];
    26 "Cave 14 A" Link, [26, 27];
    27 "Cave 14 B" Link, [26, 27];
    28 "Hammer cave" Item;
    29 "Four-way cave 1 A" Link, [29, 30, 31, 32];
    30 "Four-way cave 1 B" Link, [29, 30, 31, 32];
    31 "Four-way cave 1 C" Link, [29, 30, 31, 32];
    32 "Four-way cave 1 D" Link, [29, 30, 31, 32];
    33 "Four-way cave 2 A" Link, [33, 34, 35, 36];
    34 "Four-way cave 2 B" Link, [33, 34, 35, 36];
    35 "Four-way cave 2 C" Link, [33, 34, 35, 36];
    36 "Four-way cave 2 D" Link, [33, 34, 35, 36];
    40 "Death Mountain bridge" Connector;
    41 "Death Mountain raft" Connector;
    42 "Death Mountain west exit" Connector;
    43 "Death Mountain east exit" Connector;
    56 "Spectacle rock" Item;
];

/// East Hyrule slots in use.
pub const EAST: &[SlotInfo] = slots![Cont::East;
    0 "Forest by Nabooru" Minor;
    1 "Forest by palace 6" Minor;
    2 "Trap road 1" Encounter;
    3 "Trap road 2" Encounter;
    4 "Trap road 3" Encounter;
    5 "Trap road to the valley" Encounter;
    6 "Bridge to palace 6" Encounter;
    7 "Bridge to Kasuto" Encounter;
    8 "Desert trap 1" Encounter;
    9 "Desert trap 2" Encounter;
    10 "Water tile" Item;
    11 "Nabooru cave south" Link, [11, 12];
    12 "Nabooru cave north" Link, [11, 12];
    13 "P-bag cave 1" Item;
    14 "P-bag cave 2" Item;
    15 "Kasuto cave west" Link, [15, 16];
    16 "Kasuto cave east" Link, [15, 16];
    17 "Valley cave 2 start" Link, [17, 18];
    18 "Valley cave 2 end" Link, [17, 18];
    19 "Valley cave 1 end" Link, [19, 20];
    20 "Valley cave 1 start" Link, [19, 20];
    21 "Swamp tile" Minor;
    23 "Desert tile 1" Minor;
    24 "Desert tile 2" Minor;
    25 "Desert tile 3" Minor;
    26 "Desert tile" Item;
    27 "Forest tile" Minor;
    28 "Lava tile 1" Minor;
    29 "Lava tile 2" Minor;
    30 "Lava trap 1" Encounter;
    31 "Lava trap 2" Encounter;
    32 "Lava trap 3" Encounter;
    40 "East bridge" Connector;
    41 "East raft" Connector;
    42 "East cave 1" Connector;
    43 "East cave 2" Connector;
    45 "Nabooru" Town;
    47 "Darunia" Town;
    49 "New Kasuto" Town;
    51 "Old Kasuto" Town;
    52 "Palace 5" Palace(5);
    53 "Palace 6" Palace(6);
    54 "Great Palace" Palace(7);
];

/// Maze Island slots in use.
pub const MAZE: &[SlotInfo] = slots![Cont::Maze;
    37 "Maze trap 1" Encounter;
    38 "Maze trap 2" Encounter;
    39 "Magic container drop" Item;
    40 "Maze Island bridge" Connector;
    41 "Maze Island raft" Connector;
    42 "Maze Island cave 1" Connector;
    43 "Maze Island cave 2" Connector;
    52 "Palace 4" Palace(4);
    55 "Child drop" Item;
    57 "Maze trap 3" Encounter;
    58 "Maze trap 4" Encounter;
    59 "Maze trap 5" Encounter;
    60 "Maze trap 6" Encounter;
    61 "Maze trap 7" Encounter;
];

/// The slot list for a continent.
#[must_use]
pub fn slots(cont: Cont) -> &'static [SlotInfo] {
    match cont {
        Cont::West => WEST,
        Cont::DeathMountain => DM,
        Cont::East => EAST,
        Cont::Maze => MAZE,
    }
}

/// Facts about (cont, slot), if the slot is in use.
#[must_use]
pub fn info(cont: Cont, slot: usize) -> Option<&'static SlotInfo> {
    slots(cont).iter().find(|s| usize::from(s.slot) == slot)
}

/// Connector slots.
pub const BRIDGE: usize = 40;
/// Raft slot.
pub const RAFT: usize = 41;
/// Cave connector 1.
pub const CAVE1: usize = 42;
/// Cave connector 2.
pub const CAVE2: usize = 43;

/// Spoiler-facing id: continent * 64 + slot (the community numbering).
#[must_use]
pub fn loc_id(cont: Cont, slot: usize) -> u8 {
    (cont.index() * 0x40 + slot) as u8
}

/// The terrain a location shows on the map in vanilla (its icon). Hidden
/// locations show what the reveal turns their tile into.
#[must_use]
pub fn icon_terrain(cont: Cont, slot: usize, vanilla_tile: Terrain) -> Terrain {
    match (cont, slot) {
        (Cont::East, 53) => Terrain::Palace,
        (Cont::East, 49) => Terrain::Town,
        _ => vanilla_tile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip() {
        for b in [
            [0x34, 0x17, 0x80, 0x20],
            [0xCD, 0x3D, 0x29, 0x02],
            [0x2E, 0x77, 0xC7, 0x00],
            [0x60, 0x32, 0x12, 0x80],
            [0x52, 0x10, 0x01, 0x40],
        ] {
            assert_eq!(Loc::from_bytes(b).to_bytes(), b);
        }
    }

    #[test]
    fn groups_are_symmetric() {
        for cont in Cont::ALL {
            for s in slots(cont) {
                if s.group.is_empty() {
                    continue;
                }
                assert!(s.group.contains(&s.slot), "{} {}", cont.name(), s.slot);
                for g in s.group {
                    let other = info(cont, usize::from(*g)).unwrap();
                    assert_eq!(other.group, s.group);
                }
            }
        }
    }
}
