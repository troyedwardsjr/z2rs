//! Palace rooms as the generators see them, read from the player's ROM.
//!
//! The game keeps palace rooms in three groups of 63 "maps": palaces 1, 2
//! and 5 share one set of tables in bank 4, palaces 3, 4 and 6 a second set
//! in bank 4, and the Great Palace its own set in bank 5. Per map there is a
//! sideview pointer, an enemy-list pointer (into the `$7000` RAM copy of the
//! bank's enemy data), four connection bytes and one nibble of the "item
//! still here" bits. Nothing here is copied from the ROM into the source:
//! every table is read at run time from the addresses below.
//!
//! # Connection bytes
//!
//! Byte `i` of a map is used when Link leaves through page `i`'s side of
//! the room: the game indexes `map * 4 + page`. For the 4-page rooms that
//! palaces use: byte 0 = leaving left (or falling through a hole on page
//! 0), byte 1 = riding the elevator down (or falling on page 1), byte 2 =
//! riding up (or falling on page 2), byte 3 = leaving right (or falling on
//! page 3). A value is `target_map * 4 + page`, where `page` is where Link
//! appears; `$FC` and above lead outside.

use std::collections::BTreeMap;

use crate::rom::Rom;
use crate::sideview::{self, FloorTable, ObjectSet, Sideview};
use crate::RandoError;

/// Maps per group.
pub const MAPS: usize = 63;

/// One of the three palace table groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// Palaces 1, 2 and 5 (bank 4, first table set).
    A,
    /// Palaces 3, 4 and 6 (bank 4, second table set).
    B,
    /// The Great Palace (bank 5).
    Gp,
}

impl Group {
    /// All groups.
    pub const ALL: [Group; 3] = [Group::A, Group::B, Group::Gp];

    /// Group of palace `p` (1-7).
    #[must_use]
    pub fn of_palace(p: u8) -> Group {
        match p {
            1 | 2 | 5 => Group::A,
            3 | 4 | 6 => Group::B,
            _ => Group::Gp,
        }
    }

    /// Palaces sharing this group, in map order.
    #[must_use]
    pub fn palaces(self) -> &'static [u8] {
        match self {
            Group::A => &[1, 2, 5],
            Group::B => &[3, 4, 6],
            Group::Gp => &[7],
        }
    }

    /// PRG bank holding the tables and room data.
    #[must_use]
    pub fn bank(self) -> u8 {
        match self {
            Group::A | Group::B => 4,
            Group::Gp => 5,
        }
    }

    /// Sideview pointer table (63 words).
    #[must_use]
    pub fn sideview_table(self) -> u16 {
        match self {
            Group::A | Group::Gp => 0x8523,
            Group::B => 0xA000,
        }
    }

    /// Enemy pointer table (63 words).
    #[must_use]
    pub fn enemy_table(self) -> u16 {
        match self {
            Group::A | Group::Gp => 0x85A1,
            Group::B => 0xA07E,
        }
    }

    /// Connection table (63 x 4 bytes).
    #[must_use]
    pub fn connection_table(self) -> u16 {
        match self {
            Group::A | Group::Gp => 0x871B,
            Group::B => 0xA1F8,
        }
    }

    /// Bank 5 address of this group's 32-byte "item still here" template
    /// (two maps per byte, even map in the high nibble).
    #[must_use]
    pub fn item_bits(self) -> u16 {
        match self {
            Group::A => 0xBB95,
            Group::B => 0xBBB5,
            Group::Gp => 0xBBD5,
        }
    }

    /// Object meanings for this group's rooms.
    #[must_use]
    pub fn object_set(self) -> ObjectSet {
        match self {
            Group::A | Group::B => ObjectSet::Palace,
            Group::Gp => ObjectSet::GreatPalace,
        }
    }

    /// Maps the game reserves in this group (never reassigned): the Great
    /// Palace's two ending scenes.
    #[must_use]
    pub fn reserved_maps(self) -> &'static [u8] {
        match self {
            Group::Gp => &[61, 62],
            _ => &[],
        }
    }
}

/// Bank holding the item-bit templates.
pub const ITEM_BITS_BANK: u8 = 5;

/// Vanilla entrance map of palace `p` (1-7) in its group.
#[must_use]
pub fn entrance_map(p: u8) -> u8 {
    match p {
        2 => 14,
        4 => 15,
        5 => 35,
        6 => 36,
        _ => 0,
    }
}

/// Vanilla boss map of palace `p` (1-7); for the Great Palace, Dark Link.
#[must_use]
pub fn boss_map(p: u8) -> u8 {
    match p {
        1 => 13,
        2 => 34,
        3 => 14,
        4 => 28,
        5 => 41,
        6 => 58,
        _ => 54,
    }
}

/// Vanilla item room map of palace `p` (1-6).
#[must_use]
pub fn item_map(p: u8) -> Option<u8> {
    match p {
        1 => Some(8),
        2 => Some(20),
        3 => Some(11),
        4 => Some(31),
        5 => Some(61),
        6 => Some(44),
        _ => None,
    }
}

/// Vanilla room counts per palace (P1..GP), for length options.
pub const VANILLA_LENGTHS: [u8; 7] = [14, 21, 15, 21, 28, 27, 55];

/// Identity of a vanilla room: group and map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoomKey {
    /// Group the room's data lives in.
    pub group: Group,
    /// Vanilla map number.
    pub map: u8,
}

/// The role a room plays in its palace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// Where Link walks in from the overworld.
    Entrance,
    /// Boss room (Dark Link in the Great Palace).
    Boss,
    /// Holds the palace's major item.
    Item,
    /// The Great Palace's Thunderbird room.
    Thunderbird,
    /// Anything else.
    Normal,
}

/// What a room offers to its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Shape {
    /// A passage on the left edge.
    pub left: bool,
    /// A passage on the right edge.
    pub right: bool,
    /// The elevator goes up.
    pub up: bool,
    /// The elevator goes down.
    pub down: bool,
    /// A hole in the floor on this page leads to a room below.
    pub drop: Option<u8>,
    /// Can be fallen into from above; Link appears on this page.
    pub zone: Option<u8>,
}

impl Shape {
    /// Number of ways in or out (a landing counts as one).
    #[must_use]
    pub fn exits(&self) -> u8 {
        u8::from(self.left)
            + u8::from(self.right)
            + u8::from(self.up)
            + u8::from(self.down)
            + u8::from(self.drop.is_some())
            + u8::from(self.zone.is_some())
    }

    /// Same set of connections, ignoring which page a drop or landing uses.
    #[must_use]
    pub fn class(&self) -> (bool, bool, bool, bool, bool, bool) {
        (
            self.left,
            self.right,
            self.up,
            self.down,
            self.drop.is_some(),
            self.zone.is_some(),
        )
    }

    /// Left and right swapped.
    #[must_use]
    pub fn mirrored(&self) -> Shape {
        Shape {
            left: self.right,
            right: self.left,
            ..*self
        }
    }

    /// Short text form, e.g. `L-R-U-D-d1-z2`.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        if self.left {
            v.push("L".into());
        }
        if self.right {
            v.push("R".into());
        }
        if self.up {
            v.push("U".into());
        }
        if self.down {
            v.push("D".into());
        }
        if let Some(p) = self.drop {
            v.push(format!("drop{p}"));
        }
        if let Some(p) = self.zone {
            v.push(format!("land{p}"));
        }
        if v.is_empty() {
            "none".into()
        } else {
            v.join("-")
        }
    }
}

/// Abilities a room may ask for (bit set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Needs(pub u16);

impl Needs {
    /// Small keys for locked doors.
    pub const KEY: Needs = Needs(1);
    /// Jump spell.
    pub const JUMP: Needs = Needs(2);
    /// Fairy spell.
    pub const FAIRY: Needs = Needs(4);
    /// Power glove (breaking blocks).
    pub const GLOVE: Needs = Needs(8);
    /// Downward thrust.
    pub const DOWNSTAB: Needs = Needs(16);
    /// Upward thrust.
    pub const UPSTAB: Needs = Needs(32);
    /// Reflect spell.
    pub const REFLECT: Needs = Needs(64);
    /// Nothing.
    pub const NONE: Needs = Needs(0);

    /// Union.
    #[must_use]
    pub const fn with(self, o: Needs) -> Needs {
        Needs(self.0 | o.0)
    }

    /// Remove `o`.
    #[must_use]
    pub const fn without(self, o: Needs) -> Needs {
        Needs(self.0 & !o.0)
    }

    /// Every bit of `self` is in `o`.
    #[must_use]
    pub const fn within(self, o: Needs) -> bool {
        self.0 & !o.0 == 0
    }

    /// Has `o`.
    #[must_use]
    pub const fn has(self, o: Needs) -> bool {
        self.0 & o.0 == o.0
    }

    /// Names, for the spoiler.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        let all = [
            (Needs::KEY, "key"),
            (Needs::JUMP, "jump"),
            (Needs::FAIRY, "fairy"),
            (Needs::GLOVE, "glove"),
            (Needs::DOWNSTAB, "downstab"),
            (Needs::UPSTAB, "upstab"),
            (Needs::REFLECT, "reflect"),
        ];
        all.iter()
            .filter(|(n, _)| self.has(*n))
            .map(|(_, s)| *s)
            .collect()
    }
}

/// What may block progress in palace `p` (1-7) when rooms move between
/// palaces: a room is only used where its needs fit (see
/// [`crate::flags::PalaceFlags::blocking_rooms_in_any_palace`]).
#[must_use]
pub fn allowed_needs(p: u8) -> Needs {
    let n = Needs::KEY;
    match p {
        1 => n,
        2 => n.with(Needs::JUMP).with(Needs::GLOVE),
        3 => n
            .with(Needs::DOWNSTAB)
            .with(Needs::UPSTAB)
            .with(Needs::GLOVE),
        4 | 5 => n.with(Needs::FAIRY).with(Needs::JUMP),
        6 => n.with(Needs::FAIRY).with(Needs::JUMP).with(Needs::GLOVE),
        _ => Needs::FAIRY
            .with(Needs::UPSTAB)
            .with(Needs::DOWNSTAB)
            .with(Needs::JUMP)
            .with(Needs::GLOVE)
            .with(Needs::KEY),
    }
}

/// What a room taken from vanilla palace `p` is assumed to ask for (the
/// ability its palace is designed around; the glove only where the room
/// has breakable blocks, see [`VanillaPool::read`]). Conservative stand-ins
/// for per-room analysis, matching the vanilla palace logic.
#[must_use]
pub fn origin_needs(p: u8) -> Needs {
    match p {
        1 => Needs::NONE,
        2 => Needs::JUMP.with(Needs::GLOVE),
        3 => Needs::DOWNSTAB.with(Needs::GLOVE),
        4 | 5 => Needs::FAIRY,
        6 => Needs::FAIRY.with(Needs::GLOVE),
        _ => Needs::FAIRY
            .with(Needs::GLOVE)
            .with(Needs::DOWNSTAB)
            .with(Needs::UPSTAB),
    }
}

/// Everything any palace may ask for.
#[must_use]
pub fn all_needs() -> Needs {
    (1..=7).fold(Needs::NONE, |a, p| a.with(allowed_needs(p)))
}

/// One room: where its data is, how it connects, what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    /// Vanilla identity (also the source of the default connection bytes).
    pub key: RoomKey,
    /// Vanilla palace (1-7).
    pub palace: u8,
    /// Role in its palace.
    pub role: Role,
    /// Sideview pointer (CPU address in the group's bank).
    pub sideview: u16,
    /// Enemy-list pointer (CPU address in the `$7000` copy).
    pub enemies: u16,
    /// "Item still here" nibble (bit 3 = page 0).
    pub nibble: u8,
    /// Vanilla connection bytes (kept for bytes the layout does not use).
    pub conn: [u8; 4],
    /// Connection bytes that are holes in the floor (bit `i` = byte `i`);
    /// all of them lead to the room's drop target.
    pub drop_bytes: u8,
    /// Pages (1-4).
    pub pages: u8,
    /// Page of the elevator.
    pub elevator: Option<u8>,
    /// Connections offered.
    pub shape: Shape,
    /// Major item id (candle .. magic key), for item rooms.
    pub item: Option<u8>,
    /// Small keys lying in the room.
    pub keys: u8,
    /// Locked doors in the room.
    pub doors: u8,
    /// What the room may ask for.
    pub needs: Needs,
    /// The room has breakable blocks.
    pub breakable: bool,
}

impl Room {
    /// Same layout and enemies (a duplicate in every way that shows).
    #[must_use]
    pub fn same_content(&self, o: &Room) -> bool {
        self.sideview == o.sideview && self.enemies == o.enemies
    }
}

/// Neighbours of one vanilla map, decoded from its connection bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VanillaLinks {
    /// Left neighbour.
    pub left: Option<u8>,
    /// Right neighbour.
    pub right: Option<u8>,
    /// Room above (elevator).
    pub up: Option<u8>,
    /// Room below (elevator).
    pub down: Option<u8>,
    /// `(page, target, landing page)` of the drop.
    pub drop: Option<(u8, u8, u8)>,
    /// Every connection byte that is a drop (bit `i` = byte `i`).
    pub drop_mask: u8,
}

/// One group's tables as read from the ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupData {
    /// Which group.
    pub group: Group,
    /// Sideview pointers.
    pub sideview: [u16; MAPS],
    /// Enemy pointers.
    pub enemies: [u16; MAPS],
    /// Connection bytes.
    pub conn: [[u8; 4]; MAPS],
    /// Item nibbles.
    pub nibble: [u8; MAPS],
}

impl GroupData {
    /// Read `group`'s tables from `rom`.
    pub fn read(rom: &Rom, group: Group) -> Result<GroupData, RandoError> {
        let b = group.bank();
        let mut g = GroupData {
            group,
            sideview: [0; MAPS],
            enemies: [0; MAPS],
            conn: [[0; 4]; MAPS],
            nibble: [0; MAPS],
        };
        for m in 0..MAPS {
            let m16 = m as u16;
            g.sideview[m] = rom.read_cpu_word(b, group.sideview_table() + 2 * m16)?;
            g.enemies[m] = rom.read_cpu_word(b, group.enemy_table() + 2 * m16)?;
            for i in 0..4u16 {
                g.conn[m][i as usize] = rom.read_cpu(b, group.connection_table() + 4 * m16 + i)?;
            }
            let bits = rom.read_cpu(ITEM_BITS_BANK, group.item_bits() + m16 / 2)?;
            g.nibble[m] = if m % 2 == 0 { bits >> 4 } else { bits & 0x0F };
        }
        Ok(g)
    }

    /// Bank of a sideview pointer of this group.
    #[must_use]
    pub fn sideview_bank(&self, ptr: u16) -> u8 {
        if ptr >= 0xC000 {
            7
        } else {
            self.group.bank()
        }
    }
}

/// A byte is a real link: below `$FC` and not the `$00` filler (no room
/// links to page 0 of map 0, the entrance).
fn live(b: u8) -> Option<u8> {
    (b < 0xFC && b != 0).then_some(b >> 2)
}

/// Decode every map's neighbours. A side byte is a side passage (the game
/// has a few one-way ones); an elevator byte counts only when the room has
/// an elevator and the other room points back; any other live byte on pages
/// 1-2 is a hole in the floor (a drop) into the target.
#[must_use]
pub fn vanilla_links(g: &GroupData, elevators: &[Option<u8>; MAPS]) -> [VanillaLinks; MAPS] {
    let mut out = [VanillaLinks::default(); MAPS];
    let back = |t: u8, idx: usize, me: usize| -> bool {
        let t = usize::from(t);
        t < MAPS && live(g.conn[t][idx]) == Some(me as u8)
    };
    for m in 0..MAPS {
        let c = g.conn[m];
        let mut l = VanillaLinks::default();
        for (i, &b) in c.iter().enumerate() {
            let Some(t) = live(b) else { continue };
            if usize::from(t) >= MAPS {
                continue;
            }
            let elevator = match i {
                1 => elevators[m].is_some() && back(t, 2, m),
                2 => elevators[m].is_some() && back(t, 1, m),
                _ => false,
            };
            match i {
                0 => l.left = Some(t),
                3 => l.right = Some(t),
                1 if elevator => l.down = Some(t),
                2 if elevator => l.up = Some(t),
                _ => {
                    l.drop_mask |= 1 << i;
                    if l.drop.is_none() {
                        l.drop = Some((i as u8, t, b & 3));
                    }
                }
            }
        }
        out[m] = l;
    }
    out
}

/// The vanilla pool: every room of every palace, with its vanilla links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VanillaPool {
    /// Per group tables.
    pub groups: Vec<GroupData>,
    /// Rooms, by key.
    pub rooms: BTreeMap<RoomKey, Room>,
    /// Vanilla links, by key.
    pub links: BTreeMap<RoomKey, VanillaLinks>,
    /// Rooms of each palace (index 0 = palace 1), in map order.
    pub palace_rooms: Vec<Vec<RoomKey>>,
}

impl VanillaPool {
    /// Group tables.
    #[must_use]
    pub fn group(&self, g: Group) -> &GroupData {
        &self.groups[Group::ALL.iter().position(|&x| x == g).unwrap_or(0)]
    }

    /// Read every palace room from `rom`.
    pub fn read(rom: &Rom) -> Result<VanillaPool, RandoError> {
        let floors = FloorTable::from_rom(rom)?;
        let mut pool = VanillaPool {
            groups: Vec::new(),
            rooms: BTreeMap::new(),
            links: BTreeMap::new(),
            palace_rooms: vec![Vec::new(); 7],
        };
        for group in Group::ALL {
            let g = GroupData::read(rom, group)?;
            let mut views: Vec<Option<Sideview>> = Vec::with_capacity(MAPS);
            let mut elevators = [None; MAPS];
            for (el, &ptr) in elevators.iter_mut().zip(g.sideview.iter()) {
                let sv = sideview::read_at(rom, g.sideview_bank(ptr), ptr)
                    .ok()
                    .map(|(s, _)| s);
                *el = sv.as_ref().and_then(Sideview::elevator_page);
                views.push(sv);
            }
            let links = vanilla_links(&g, &elevators);
            // Palace membership: flood from each entrance.
            let mut owner = [0u8; MAPS];
            for &p in group.palaces() {
                let mut stack = vec![entrance_map(p)];
                while let Some(m) = stack.pop() {
                    let mu = usize::from(m);
                    if mu >= MAPS || owner[mu] != 0 || group.reserved_maps().contains(&m) {
                        continue;
                    }
                    owner[mu] = p;
                    let l = links[mu];
                    for t in [l.left, l.right, l.up, l.down, l.drop.map(|d| d.1)]
                        .into_iter()
                        .flatten()
                    {
                        stack.push(t);
                    }
                }
            }
            // Landing pages: a drop into a room makes it a landing room.
            // A side passage into a room opens that side of it (even when
            // the vanilla byte for the way back leads outside).
            let mut zone: [Option<u8>; MAPS] = [None; MAPS];
            let mut open_left = [false; MAPS];
            let mut open_right = [false; MAPS];
            for l in &links {
                if let Some((_, t, page)) = l.drop {
                    if usize::from(t) < MAPS {
                        zone[usize::from(t)] = Some(page);
                    }
                }
                if let Some(t) = l.right.filter(|&t| usize::from(t) < MAPS) {
                    open_left[usize::from(t)] = true;
                }
                if let Some(t) = l.left.filter(|&t| usize::from(t) < MAPS) {
                    open_right[usize::from(t)] = true;
                }
            }
            for m in 0..MAPS {
                let p = owner[m];
                if p == 0 {
                    continue;
                }
                let key = RoomKey {
                    group,
                    map: m as u8,
                };
                let sv = views[m].clone();
                let l = links[m];
                let shape = Shape {
                    left: l.left.is_some() || open_left[m],
                    right: l.right.is_some() || open_right[m],
                    up: l.up.is_some(),
                    down: l.down.is_some(),
                    drop: l.drop.map(|d| d.0),
                    zone: zone[m],
                };
                let (grid, pages) = match &sv {
                    Some(s) => (Some(s.grid(group.object_set(), &floors)), s.pages()),
                    None => (None, 4),
                };
                let items: Vec<u8> = grid
                    .as_ref()
                    .map(|gr| gr.items.iter().map(|&(_, _, i)| i).collect())
                    .unwrap_or_default();
                let keys = items.iter().filter(|&&i| i == 0x08).count() as u8;
                let doors = grid.as_ref().map_or(0, |gr| gr.doors.len() as u8);
                let breakable = grid.as_ref().is_some_and(|gr| {
                    (0..gr.width()).any(|x| {
                        (0..sideview::ROWS).any(|y| gr.cell(x, y) == sideview::Cell::Breakable)
                    })
                });
                let mut role = Role::Normal;
                let mut item = None;
                if m as u8 == entrance_map(p) {
                    role = Role::Entrance;
                } else if m as u8 == boss_map(p) {
                    role = Role::Boss;
                } else if item_map(p) == Some(m as u8) {
                    role = Role::Item;
                    item = items.iter().copied().find(|&i| i <= 0x07);
                }
                // Small keys are not modelled (the game's own key placement
                // is kept with each room); `KEY` stands for the magic key.
                let mut needs = origin_needs(p);
                if !breakable {
                    needs = needs.without(Needs::GLOVE);
                }
                // A room Link can cross between all its ways in and out
                // with no spells and no glove asks for nothing.
                if grid
                    .as_ref()
                    .is_some_and(|gr| super::reach::needs_nothing(gr, &shape, elevators[m]))
                {
                    needs = Needs::NONE;
                }
                if p == 4 && role == Role::Boss {
                    needs = needs.with(Needs::REFLECT);
                }
                pool.rooms.insert(
                    key,
                    Room {
                        key,
                        palace: p,
                        role,
                        sideview: g.sideview[m],
                        enemies: g.enemies[m],
                        nibble: g.nibble[m],
                        conn: g.conn[m],
                        drop_bytes: l.drop_mask,
                        pages,
                        elevator: elevators[m],
                        shape,
                        item,
                        keys,
                        doors,
                        needs,
                        breakable,
                    },
                );
                pool.links.insert(key, l);
                pool.palace_rooms[usize::from(p - 1)].push(key);
            }
            pool.groups.push(g);
        }
        // The Thunderbird room: the Great Palace room whose enemy list
        // holds the Thunderbird (enemy id $18 in the Great Palace set).
        let gp = pool.group(Group::Gp).clone();
        for key in pool.palace_rooms[6].clone() {
            let e = gp.enemies[usize::from(key.map)];
            if enemy_ids(rom, Group::Gp, e).contains(&THUNDERBIRD_ID) {
                if let Some(r) = pool.rooms.get_mut(&key) {
                    if r.role == Role::Normal {
                        r.role = Role::Thunderbird;
                    }
                }
            }
        }
        Ok(pool)
    }

    /// Every palace has its vanilla room count and its entrance, boss and
    /// (palaces 1-6) item room: the tables are the game's own.
    #[must_use]
    pub fn looks_vanilla(&self) -> bool {
        (1..=7u8).all(|p| {
            let i = usize::from(p - 1);
            self.palace_rooms[i].len() == usize::from(VANILLA_LENGTHS[i])
                && self.role_room(p, Role::Entrance).is_some()
                && self.role_room(p, Role::Boss).is_some()
                && (p == 7 || self.role_room(p, Role::Item).is_some())
        })
    }

    /// The vanilla room of palace `p` with `role`.
    #[must_use]
    pub fn role_room(&self, p: u8, role: Role) -> Option<RoomKey> {
        self.palace_rooms
            .get(usize::from(p.saturating_sub(1)))?
            .iter()
            .copied()
            .find(|k| self.rooms[k].role == role)
    }
}

/// Enemy id of the Thunderbird in the Great Palace enemy set.
pub const THUNDERBIRD_ID: u8 = 0x22;

/// Bank address of the enemy data that the game copies to `$7000`.
pub const ENEMY_DATA_ROM: u16 = 0x88A0;
/// RAM address the enemy data is copied to.
pub const ENEMY_DATA_RAM: u16 = 0x7000;
/// Bytes copied.
pub const ENEMY_DATA_LEN: u16 = 0x400;

/// Enemy ids in the list at RAM pointer `ptr` of `group` (each entry is
/// two bytes: `YX` position and `ID`; byte 0 of the list is its length).
#[must_use]
pub fn enemy_ids(rom: &Rom, group: Group, ptr: u16) -> Vec<u8> {
    let Some(addr) = enemy_rom_addr(ptr) else {
        return Vec::new();
    };
    let b = group.bank();
    let Ok(len) = rom.read_cpu(b, addr) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    let mut i = 1u16;
    while i + 1 < u16::from(len) {
        if let Ok(id) = rom.read_cpu(b, addr + i + 1) {
            v.push(id & 0x3F);
        }
        i += 2;
    }
    v
}

/// ROM address (in the group's bank) of an enemy list given its RAM
/// pointer.
#[must_use]
pub fn enemy_rom_addr(ptr: u16) -> Option<u16> {
    (ENEMY_DATA_RAM..ENEMY_DATA_RAM + ENEMY_DATA_LEN)
        .contains(&ptr)
        .then(|| ptr - ENEMY_DATA_RAM + ENEMY_DATA_ROM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_sets() {
        let n = Needs::KEY.with(Needs::JUMP);
        assert!(n.has(Needs::KEY) && !n.has(Needs::FAIRY));
        assert!(n.within(allowed_needs(2)));
        assert!(!Needs::FAIRY.within(allowed_needs(1)));
        assert!(allowed_needs(3).within(all_needs()));
        assert_eq!(n.names(), vec!["key", "jump"]);
    }

    #[test]
    fn shape_text_and_mirror() {
        let s = Shape {
            left: true,
            down: true,
            drop: Some(2),
            ..Shape::default()
        };
        assert_eq!(s.describe(), "L-D-drop2");
        assert_eq!(s.exits(), 3);
        let m = s.mirrored();
        assert!(m.right && !m.left && m.down);
    }

    #[test]
    fn links_need_a_way_back() {
        let mut g = GroupData {
            group: Group::A,
            sideview: [0; MAPS],
            enemies: [0; MAPS],
            conn: [[0; 4]; MAPS],
            nibble: [0; MAPS],
        };
        // 0 -> right 1, 1 -> left 0; 1 has a drop on page 2 into 2 (landing
        // page 1); 2 -> elevator up to 3, 3 down to 2; 3 has a one-way
        // passage right into 4.
        g.conn[0] = [0xFC, 0, 0, 1 << 2];
        g.conn[1] = [3, 0, (2 << 2) | 1, 0xFC];
        g.conn[2] = [0xFC, 0, 3 << 2, 0xFC];
        g.conn[3] = [0xFC, (2 << 2) | 2, 0, 4 << 2];
        let mut el = [None; MAPS];
        el[2] = Some(2);
        el[3] = Some(1);
        let l = vanilla_links(&g, &el);
        assert_eq!(l[0].right, Some(1));
        assert_eq!(l[1].left, Some(0));
        assert_eq!(l[1].drop, Some((2, 2, 1)));
        assert_eq!(l[1].drop_mask, 4);
        assert_eq!(l[2].up, Some(3));
        assert_eq!(l[3].down, Some(2));
        assert_eq!(l[0].left, None);
        assert_eq!(l[3].right, Some(4));
        assert_eq!(l[4].left, None);
    }
}
