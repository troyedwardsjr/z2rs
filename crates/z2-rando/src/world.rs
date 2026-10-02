//! Shared world model: items, overworld spots, item locations, requirements
//! and the reachability solver that decides whether a seed is beatable.
//!
//! Every module that moves things around the game world works through this
//! file, so its pieces are kept small and explicit:
//!
//! * [`ItemId`]: the in-game item numbers (the byte the game stores).
//! * [`Requirement`]: an OR of AND-groups of [`Token`]s (items, spells or a
//!   magic-container count).
//! * [`OverworldMap`]: one continent's terrain grid (64 x 75 tiles).
//! * [`Spot`]: one slot of a continent's overworld location table (a cave,
//!   town, palace, connector or encounter tile), with its map position and the
//!   requirement to enter it.
//! * [`Link`]: a passage between two spots (the two ends of a cave, the two
//!   doors of Saria, a raft or bridge between continents).
//! * [`ItemLoc`]: a place that holds an item (or a pure logic event such as a
//!   boss), attached to a spot, with the requirement to collect it once the
//!   spot is reached and the ROM bytes that store the item.
//! * [`World`]: all of the above plus the starting [`Inventory`].
//!
//! [`World::vanilla`] builds the vanilla world from the player's ROM at run
//! time (maps, location tables and item bytes are read from the ROM; only
//! addresses, slot numbers and logic rules are constants here).
//!
//! # Who edits what
//!
//! * `start` decides [`World::start`] (starting items, spells, containers).
//! * `overworld` may replace [`World::maps`], move spots (`x`, `y`,
//!   `on_map`), change [`Spot::access`], add or remove [`Link`]s, and mark
//!   [`Spot::required`].
//! * `palaces` may move palaces between palace spots (see
//!   [`World::swap_palace_spots`]) and replace a palace's item rooms and
//!   boss events with [`World::set_palace_locations`].
//! * `spells`/`towns` may change wizard rewards and requirements
//!   ([`World::loc_mut`] with [`LocKey::Town`], [`World::set_wizard_magic_requirements`],
//!   [`World::set_spell_containers`], [`World::set_new_kasuto_basement`]).
//! * `items` places items into [`ItemLoc::item`] and writes them to the ROM.
//!
//! Additive changes (new fields with defaults, new helper methods) are
//! welcome; please keep the solver semantics documented on [`World::solve`].
//!
//! # Solver semantics
//!
//! [`World::solve`] runs "spheres": starting from [`World::start`], it
//! flood-fills every continent from the reachable spots, follows links, then
//! collects every item location whose spot is reached and whose requirement
//! is met. The collected items join the inventory and the next sphere
//! starts. It stops when a sphere collects nothing. A tile is entered when
//! its terrain is passable with the current inventory (walkable water needs
//! the boots, boulders the hammer, the river devil the flute) and, if spots
//! sit on it, at least one of them has its access requirement met.
//! [`World::beatable`] asks that every location marked `required` was
//! collected and every spot marked `required` was reached.

use std::collections::BTreeSet;
use std::fmt;

use crate::rom::Rom;
use crate::RandoError;

// ---------------------------------------------------------------------------
// Items.
// ---------------------------------------------------------------------------

macro_rules! items {
    ($( $(#[$m:meta])* $v:ident = $b:literal => $name:literal ),+ $(,)?) => {
        /// An item as the game numbers it (the byte stored in item objects
        /// and location data).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u8)]
        pub enum ItemId {
            $( $(#[$m])* $v = $b ),+
        }

        impl ItemId {
            /// Every item, in byte order.
            pub const ALL: &'static [ItemId] = &[ $( ItemId::$v ),+ ];

            /// Display name (used by the spoiler and hints).
            #[must_use]
            pub fn name(self) -> &'static str {
                match self { $( ItemId::$v => $name ),+ }
            }

            /// The item for a game byte, if it is a known item.
            #[must_use]
            pub fn from_byte(b: u8) -> Option<ItemId> {
                match b { $( $b => Some(ItemId::$v), )+ _ => None }
            }
        }
    };
}

items! {
    /// Candle.
    Candle = 0x00 => "Candle",
    /// Power glove.
    Glove = 0x01 => "Glove",
    /// Raft.
    Raft = 0x02 => "Raft",
    /// Boots.
    Boots = 0x03 => "Boots",
    /// Flute.
    Flute = 0x04 => "Flute",
    /// Cross.
    Cross = 0x05 => "Cross",
    /// Hammer.
    Hammer = 0x06 => "Hammer",
    /// Magic key.
    MagicKey = 0x07 => "Magic Key",
    /// Palace small key.
    SmallKey = 0x08 => "Small Key",
    /// 50-experience P-bag.
    SmallBag = 0x0A => "P-Bag (50)",
    /// 100-experience P-bag.
    MediumBag = 0x0B => "P-Bag (100)",
    /// 200-experience P-bag.
    LargeBag = 0x0C => "P-Bag (200)",
    /// 500-experience P-bag.
    XlBag = 0x0D => "P-Bag (500)",
    /// Magic container.
    MagicContainer = 0x0E => "Magic Container",
    /// Heart container.
    HeartContainer = 0x0F => "Heart Container",
    /// Blue magic jar.
    BlueJar = 0x10 => "Blue Jar",
    /// Red magic jar.
    RedJar = 0x11 => "Red Jar",
    /// Extra life (Link doll).
    OneUp = 0x12 => "1-Up",
    /// The kidnapped child.
    Child = 0x13 => "Child",
    /// Trophy.
    Trophy = 0x14 => "Trophy",
    /// Medicine.
    Medicine = 0x15 => "Medicine",
    /// Upward thrust technique.
    Upstab = 0x17 => "Upstab",
    /// Downward thrust technique.
    Downstab = 0x18 => "Downstab",
    /// Bagu's note.
    BaguNote = 0x19 => "Bagu's Note",
    /// Mirror.
    Mirror = 0x1A => "Mirror",
    /// Water of life.
    Water = 0x1B => "Water",
    /// Shield spell.
    Shield = 0x1C => "Shield",
    /// Jump spell.
    Jump = 0x1D => "Jump",
    /// Life spell.
    Life = 0x1E => "Life",
    /// Fairy spell.
    Fairy = 0x1F => "Fairy",
    /// Fire spell.
    Fire = 0x20 => "Fire",
    /// Reflect spell.
    Reflect = 0x21 => "Reflect",
    /// Spell spell.
    Spell = 0x22 => "Spell",
    /// Thunder spell.
    Thunder = 0x23 => "Thunder",
    /// Dash spell (replaces Fire when that option is on).
    Dash = 0x24 => "Dash",
}

impl ItemId {
    /// The byte the game uses.
    #[must_use]
    pub fn byte(self) -> u8 {
        self as u8
    }

    /// The eight tools found in palaces and caves (candle .. magic key).
    pub const TOOLS: [ItemId; 8] = [
        ItemId::Candle,
        ItemId::Glove,
        ItemId::Raft,
        ItemId::Boots,
        ItemId::Flute,
        ItemId::Cross,
        ItemId::Hammer,
        ItemId::MagicKey,
    ];

    /// The six vanilla palace items, P1..P6.
    pub const PALACE_ITEMS: [ItemId; 6] = [
        ItemId::Candle,
        ItemId::Glove,
        ItemId::Raft,
        ItemId::Boots,
        ItemId::Flute,
        ItemId::Cross,
    ];

    /// Spells in vanilla menu order (Fire's slot also holds Dash).
    pub const SPELLS: [ItemId; 8] = [
        ItemId::Shield,
        ItemId::Jump,
        ItemId::Life,
        ItemId::Fairy,
        ItemId::Fire,
        ItemId::Reflect,
        ItemId::Spell,
        ItemId::Thunder,
    ];

    /// Wizard prerequisites (spell items).
    pub const SPELL_ITEMS: [ItemId; 5] = [
        ItemId::Trophy,
        ItemId::Mirror,
        ItemId::Medicine,
        ItemId::Water,
        ItemId::Child,
    ];

    /// Minor items (bags, jars, small key, 1-up).
    pub const MINOR: [ItemId; 8] = [
        ItemId::SmallKey,
        ItemId::SmallBag,
        ItemId::MediumBag,
        ItemId::LargeBag,
        ItemId::XlBag,
        ItemId::BlueJar,
        ItemId::RedJar,
        ItemId::OneUp,
    ];

    /// Bags, jars, small key or 1-up.
    #[must_use]
    pub fn is_minor(self) -> bool {
        Self::MINOR.contains(&self)
    }

    /// Heart or magic container.
    #[must_use]
    pub fn is_container(self) -> bool {
        matches!(self, ItemId::HeartContainer | ItemId::MagicContainer)
    }

    /// One of the spells (including Dash).
    #[must_use]
    pub fn is_spell(self) -> bool {
        (0x1C..=0x24).contains(&self.byte())
    }

    /// Menu slot of a spell (Dash shares Fire's slot).
    #[must_use]
    pub fn spell_index(self) -> Option<usize> {
        match self {
            ItemId::Dash => Some(4),
            s if s.is_spell() => Some(usize::from(s.byte() - 0x1C)),
            _ => None,
        }
    }

    /// Trophy, mirror, medicine, water or child.
    #[must_use]
    pub fn is_spell_item(self) -> bool {
        Self::SPELL_ITEMS.contains(&self)
    }

    /// Bagu's note, mirror or water.
    #[must_use]
    pub fn is_quest_item(self) -> bool {
        matches!(self, ItemId::BaguNote | ItemId::Mirror | ItemId::Water)
    }

    /// Upstab or downstab.
    #[must_use]
    pub fn is_tech(self) -> bool {
        matches!(self, ItemId::Upstab | ItemId::Downstab)
    }

    /// One of the eight tools.
    #[must_use]
    pub fn is_tool(self) -> bool {
        self.byte() <= 0x07
    }

    /// An item the logic must prove collectable (tools, spell items, quest
    /// items, techs, spells). Containers and minor items are not.
    #[must_use]
    pub fn is_major(self) -> bool {
        !(self.is_minor() || self.is_container())
    }

    /// Whether the unmodified game's item-get code handles this item when it
    /// sits in an overworld area or palace item object (bytes `$00-$15`).
    /// Larger numbers (techs, quest items, spells) need the full item
    /// shuffle patch.
    #[must_use]
    pub fn is_vanilla_object_item(self) -> bool {
        self.byte() <= 0x15
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---------------------------------------------------------------------------
// Requirements.
// ---------------------------------------------------------------------------

/// One thing a requirement can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Token {
    /// Owning an item. For spells this also needs the magic containers
    /// listed in [`World::spell_containers`] (the spell must be castable).
    Item(ItemId),
    /// At least this many magic containers.
    MagicContainers(u8),
    /// Never satisfied (a blocked path).
    Never,
}

/// An OR of AND-groups. An empty requirement is always met.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Requirement {
    /// Alternatives; the requirement is met when every token of any one
    /// group is met. No groups at all means "always".
    pub any_of: Vec<Vec<Token>>,
}

impl Requirement {
    /// Always met.
    #[must_use]
    pub fn none() -> Self {
        Requirement { any_of: Vec::new() }
    }

    /// Never met.
    #[must_use]
    pub fn never() -> Self {
        Requirement {
            any_of: vec![vec![Token::Never]],
        }
    }

    /// A single item.
    #[must_use]
    pub fn item(i: ItemId) -> Self {
        Requirement {
            any_of: vec![vec![Token::Item(i)]],
        }
    }

    /// Any one of these items.
    #[must_use]
    pub fn any(items: &[ItemId]) -> Self {
        Requirement {
            any_of: items.iter().map(|&i| vec![Token::Item(i)]).collect(),
        }
    }

    /// All of these items.
    #[must_use]
    pub fn all(items: &[ItemId]) -> Self {
        if items.is_empty() {
            return Self::none();
        }
        Requirement {
            any_of: vec![items.iter().map(|&i| Token::Item(i)).collect()],
        }
    }

    /// At least `n` magic containers (0 means always).
    #[must_use]
    pub fn containers(n: u8) -> Self {
        if n == 0 {
            Self::none()
        } else {
            Requirement {
                any_of: vec![vec![Token::MagicContainers(n)]],
            }
        }
    }

    /// Whether this is always met.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.any_of.is_empty() || self.any_of.iter().any(Vec::is_empty)
    }

    /// `self AND other`.
    #[must_use]
    pub fn and(&self, other: &Requirement) -> Requirement {
        if self.is_none() {
            return other.clone();
        }
        if other.is_none() {
            return self.clone();
        }
        let mut out = Vec::new();
        for a in &self.any_of {
            for b in &other.any_of {
                let mut g = a.clone();
                for t in b {
                    if !g.contains(t) {
                        g.push(*t);
                    }
                }
                out.push(g);
            }
        }
        Requirement { any_of: out }
    }

    /// `self OR other`.
    #[must_use]
    pub fn or(&self, other: &Requirement) -> Requirement {
        if self.is_none() || other.is_none() {
            return Self::none();
        }
        let mut out = self.any_of.clone();
        out.extend(other.any_of.iter().cloned());
        Requirement { any_of: out }
    }

    /// AND a single token into every group (an always-met requirement
    /// becomes exactly that token).
    #[must_use]
    pub fn with_hard(&self, t: Token) -> Requirement {
        self.and(&Requirement {
            any_of: vec![vec![t]],
        })
    }

    /// Remove `items` from every group (a group left empty makes the whole
    /// requirement always met).
    #[must_use]
    pub fn without(&self, items: &[ItemId]) -> Requirement {
        let any_of: Vec<Vec<Token>> = self
            .any_of
            .iter()
            .map(|g| {
                g.iter()
                    .copied()
                    .filter(|t| !matches!(t, Token::Item(i) if items.contains(i)))
                    .collect()
            })
            .collect();
        if any_of.iter().any(Vec::is_empty) {
            Self::none()
        } else {
            Requirement { any_of }
        }
    }

    /// Remove every magic-container token.
    #[must_use]
    pub fn without_containers(&self) -> Requirement {
        let any_of: Vec<Vec<Token>> = self
            .any_of
            .iter()
            .map(|g| {
                g.iter()
                    .copied()
                    .filter(|t| !matches!(t, Token::MagicContainers(_)))
                    .collect()
            })
            .collect();
        if any_of.iter().any(Vec::is_empty) {
            Self::none()
        } else {
            Requirement { any_of }
        }
    }

    /// Whether every alternative needs `t`.
    #[must_use]
    pub fn has_hard(&self, t: Token) -> bool {
        !self.any_of.is_empty() && self.any_of.iter().all(|g| g.contains(&t))
    }

    /// Every item this requirement mentions.
    #[must_use]
    pub fn items(&self) -> BTreeSet<ItemId> {
        self.any_of
            .iter()
            .flatten()
            .filter_map(|t| match t {
                Token::Item(i) => Some(*i),
                _ => None,
            })
            .collect()
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            return f.write_str("-");
        }
        let groups: Vec<String> = self
            .any_of
            .iter()
            .map(|g| {
                g.iter()
                    .map(|t| match t {
                        Token::Item(i) => i.name().to_string(),
                        Token::MagicContainers(n) => format!("{n} MC"),
                        Token::Never => "never".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" + ")
            })
            .collect();
        f.write_str(&groups.join(" | "))
    }
}

/// What the player holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inventory {
    /// Bit `n` = item byte `n` is owned.
    pub items: u64,
    /// Heart containers (starting ones included).
    pub hearts: u8,
    /// Magic containers (starting ones included).
    pub magic: u8,
}

impl Inventory {
    /// Whether `i` is owned.
    #[must_use]
    pub fn has(&self, i: ItemId) -> bool {
        self.items & (1u64 << i.byte()) != 0
    }

    /// Add one item (containers bump the counters).
    pub fn add(&mut self, i: ItemId) {
        match i {
            ItemId::HeartContainer => self.hearts = self.hearts.saturating_add(1),
            ItemId::MagicContainer => self.magic = self.magic.saturating_add(1),
            _ => self.items |= 1u64 << i.byte(),
        }
    }

    /// Remove an item (not containers).
    pub fn remove(&mut self, i: ItemId) {
        self.items &= !(1u64 << i.byte());
    }

    /// Every owned item (containers not listed).
    #[must_use]
    pub fn list(&self) -> Vec<ItemId> {
        ItemId::ALL
            .iter()
            .copied()
            .filter(|&i| self.has(i))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Overworld.
// ---------------------------------------------------------------------------

/// The four overworld maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Continent {
    /// West Hyrule.
    West = 0,
    /// Death Mountain.
    DeathMountain = 1,
    /// East Hyrule.
    East = 2,
    /// Maze Island.
    MazeIsland = 3,
}

impl Continent {
    /// All four, in table order.
    pub const ALL: [Continent; 4] = [
        Continent::West,
        Continent::DeathMountain,
        Continent::East,
        Continent::MazeIsland,
    ];

    /// Table index 0-3.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Continent for a table index.
    #[must_use]
    pub fn from_index(i: usize) -> Option<Continent> {
        Self::ALL.get(i).copied()
    }

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Continent::West => "West Hyrule",
            Continent::DeathMountain => "Death Mountain",
            Continent::East => "East Hyrule",
            Continent::MazeIsland => "Maze Island",
        }
    }

    /// iNES file offset of the location table (4 parallel arrays of
    /// [`LOCATION_SLOTS`] bytes).
    #[must_use]
    pub fn location_table_ines(self) -> usize {
        match self {
            Continent::West => 0x462F,
            Continent::DeathMountain => 0x610C,
            Continent::East => 0x862F,
            Continent::MazeIsland => 0xA10C,
        }
    }

    /// iNES file offset of the vanilla compressed map.
    #[must_use]
    pub fn vanilla_map_ines(self) -> usize {
        match self {
            Continent::West => 0x506C,
            Continent::DeathMountain => 0x665C,
            Continent::East => 0x9056,
            Continent::MazeIsland => 0xA65C,
        }
    }
}

/// Slots per continent location table.
pub const LOCATION_SLOTS: usize = 0x3F;
/// Map width in tiles.
pub const MAP_COLS: usize = 64;
/// Map height in tiles.
pub const MAP_ROWS: usize = 75;
/// Location-table Y values are map rows plus this.
pub const MAP_Y_OFFSET: u8 = 30;

/// Overworld tile type (the 4-bit code in the map data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Terrain {
    /// Town entrance.
    Town = 0,
    /// Cave entrance.
    Cave = 1,
    /// Palace entrance.
    Palace = 2,
    /// Bridge.
    Bridge = 3,
    /// Desert.
    Desert = 4,
    /// Grass.
    Grass = 5,
    /// Forest.
    Forest = 6,
    /// Swamp.
    Swamp = 7,
    /// Graveyard.
    Grave = 8,
    /// Road.
    Road = 9,
    /// Lava.
    Lava = 10,
    /// Mountain (impassable).
    Mountain = 11,
    /// Water (impassable).
    Water = 12,
    /// Shallow water (needs the boots).
    WalkableWater = 13,
    /// Boulder (needs the hammer).
    Rock = 14,
    /// River devil (needs the flute).
    RiverDevil = 15,
}

impl Terrain {
    /// Terrain for a 4-bit code.
    #[must_use]
    pub fn from_code(c: u8) -> Terrain {
        match c & 0x0F {
            0 => Terrain::Town,
            1 => Terrain::Cave,
            2 => Terrain::Palace,
            3 => Terrain::Bridge,
            4 => Terrain::Desert,
            5 => Terrain::Grass,
            6 => Terrain::Forest,
            7 => Terrain::Swamp,
            8 => Terrain::Grave,
            9 => Terrain::Road,
            10 => Terrain::Lava,
            11 => Terrain::Mountain,
            12 => Terrain::Water,
            13 => Terrain::WalkableWater,
            14 => Terrain::Rock,
            _ => Terrain::RiverDevil,
        }
    }

    /// Whether Link can walk on it with `inv`.
    #[must_use]
    pub fn passable(self, inv: &Inventory) -> bool {
        match self {
            Terrain::Mountain | Terrain::Water => false,
            Terrain::WalkableWater => inv.has(ItemId::Boots),
            Terrain::Rock => inv.has(ItemId::Hammer),
            Terrain::RiverDevil => inv.has(ItemId::Flute),
            _ => true,
        }
    }
}

/// One continent's terrain grid.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OverworldMap {
    /// Width in tiles.
    pub cols: usize,
    /// Height in tiles.
    pub rows: usize,
    /// Row-major tiles.
    pub tiles: Vec<Terrain>,
}

impl OverworldMap {
    /// Decode the run-length map format (high nibble = run length - 1, low
    /// nibble = terrain; runs never cross a row) starting at headerless PRG
    /// offset `off`. Stops at the end of PRG if the data runs out.
    #[must_use]
    pub fn decode(rom: &Rom, off: usize, rows: usize, cols: usize) -> OverworldMap {
        let mut tiles = Vec::with_capacity(rows * cols);
        let mut p = off;
        'rows: for _ in 0..rows {
            let mut col = 0;
            while col < cols {
                let Ok(b) = rom.read(p) else {
                    break 'rows;
                };
                p += 1;
                let run = usize::from(b >> 4) + 1;
                let t = Terrain::from_code(b);
                for _ in 0..run.min(cols - col) {
                    tiles.push(t);
                }
                col += run;
            }
        }
        tiles.resize(rows * cols, Terrain::Mountain);
        OverworldMap { cols, rows, tiles }
    }

    /// Tile at `(x, row)`; out of range reads as mountain.
    #[must_use]
    pub fn get(&self, x: usize, row: usize) -> Terrain {
        if x < self.cols && row < self.rows {
            self.tiles[row * self.cols + x]
        } else {
            Terrain::Mountain
        }
    }

    /// Set a tile (ignored when out of range).
    pub fn set(&mut self, x: usize, row: usize, t: Terrain) {
        if x < self.cols && row < self.rows {
            self.tiles[row * self.cols + x] = t;
        }
    }
}

/// Which town an NPC or entrance belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Town {
    /// Rauru.
    Rauru,
    /// Ruto.
    Ruto,
    /// Saria.
    Saria,
    /// Mido.
    Mido,
    /// Nabooru.
    Nabooru,
    /// Darunia.
    Darunia,
    /// New Kasuto.
    NewKasuto,
    /// Old Kasuto.
    OldKasuto,
    /// Bagu's house (one room, counted as a town).
    Bagu,
}

impl Town {
    /// The eight wizard towns in menu order (Rauru gives the first spell).
    pub const WIZARD_TOWNS: [Town; 8] = [
        Town::Rauru,
        Town::Ruto,
        Town::Saria,
        Town::Mido,
        Town::Nabooru,
        Town::Darunia,
        Town::NewKasuto,
        Town::OldKasuto,
    ];

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Town::Rauru => "Rauru",
            Town::Ruto => "Ruto",
            Town::Saria => "Saria",
            Town::Mido => "Mido",
            Town::Nabooru => "Nabooru",
            Town::Darunia => "Darunia",
            Town::NewKasuto => "New Kasuto",
            Town::OldKasuto => "Old Kasuto",
            Town::Bagu => "Bagu's House",
        }
    }

    /// Vanilla magic containers the wizard asks for (0 = none).
    #[must_use]
    pub fn wizard_containers(self) -> u8 {
        match self {
            Town::Rauru => 1,
            Town::Ruto => 2,
            Town::Saria => 3,
            Town::Mido => 4,
            Town::Nabooru => 5,
            Town::Darunia => 6,
            Town::NewKasuto => 7,
            Town::OldKasuto => 8,
            Town::Bagu => 0,
        }
    }

    /// The spell item the wizard asks for, if any.
    #[must_use]
    pub fn wizard_item(self) -> Option<ItemId> {
        match self {
            Town::Ruto => Some(ItemId::Trophy),
            Town::Saria => Some(ItemId::Mirror),
            Town::Mido => Some(ItemId::Medicine),
            Town::Nabooru => Some(ItemId::Water),
            Town::Darunia => Some(ItemId::Child),
            _ => None,
        }
    }

    /// Vanilla spell taught here.
    #[must_use]
    pub fn vanilla_spell(self) -> Option<ItemId> {
        Self::WIZARD_TOWNS
            .iter()
            .position(|&t| t == self)
            .map(|i| ItemId::SPELLS[i])
    }
}

/// Item-giving NPCs and fixtures inside towns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TownSlot {
    /// The town's wizard.
    Wizard(Town),
    /// Saria's mirror table.
    SariaTable,
    /// Nabooru's fountain (water).
    NabooruFountain,
    /// Bagu (the note).
    BaguHouse,
    /// Mido's downstab teacher.
    MidoTrainer,
    /// Darunia's upstab teacher.
    DaruniaTrainer,
    /// New Kasuto spell tower (magic key).
    SpellTower,
    /// New Kasuto granny's basement (magic container).
    GrannysBasement,
}

impl TownSlot {
    /// The town this slot is in.
    #[must_use]
    pub fn town(self) -> Town {
        match self {
            TownSlot::Wizard(t) => t,
            TownSlot::SariaTable => Town::Saria,
            TownSlot::NabooruFountain => Town::Nabooru,
            TownSlot::BaguHouse => Town::Bagu,
            TownSlot::MidoTrainer => Town::Mido,
            TownSlot::DaruniaTrainer => Town::Darunia,
            TownSlot::SpellTower | TownSlot::GrannysBasement => Town::NewKasuto,
        }
    }
}

/// What kind of place a spot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpotKind {
    /// A palace entrance; the number is the palace that is entered (1-7).
    Palace(u8),
    /// A town entrance.
    Town(Town),
    /// A continent connector (slots 40-43); the destination continent.
    Connector(Continent),
    /// A cave, item tile, encounter tile or anything else.
    Area,
}

/// One overworld location-table slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    /// Continent.
    pub continent: Continent,
    /// Slot in that continent's table (0..63).
    pub slot: u8,
    /// Display name.
    pub name: String,
    /// Column on the map.
    pub x: u8,
    /// Row on the map (location-table Y minus [`MAP_Y_OFFSET`]).
    pub row: u8,
    /// Whether the spot is on the map (removed spots are skipped by the
    /// solver).
    pub on_map: bool,
    /// Hidden until revealed (vanilla: the sixth palace and New Kasuto).
    /// The reveal item is part of [`Spot::access`].
    pub hidden: bool,
    /// What kind of place.
    pub kind: SpotKind,
    /// Requirement to enter the spot's tile from the overworld.
    pub access: Requirement,
    /// The beatability check asks that this spot is reached.
    pub required: bool,
    /// The raw four location-table bytes as loaded (Y, X/entrance,
    /// map/page, flags/world).
    pub raw: [u8; 4],
}

impl Spot {
    /// Whether the area is entered at its right edge (page bits or the
    /// force-right flag).
    #[must_use]
    pub fn enters_from_right(&self) -> bool {
        (self.raw[2] >> 6) != 0 || self.raw[3] & 0x20 != 0
    }
}

/// A two-way passage between spots, usable when `req` is met and either end
/// is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// One end (spot index).
    pub a: usize,
    /// The other end (spot index).
    pub b: usize,
    /// Requirement to use the passage.
    pub req: Requirement,
    /// Only from `a` to `b`.
    pub one_way: bool,
}

/// Stable key for an item location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LocKey {
    /// The item in an overworld area (cave, tile, drop) at this slot.
    Area(Continent, u8),
    /// An NPC or fixture inside a town.
    Town(TownSlot),
    /// Palace `palace` (1-7) item room number `room` (0-based).
    PalaceItem {
        /// Palace number (1-7), the palace's identity, not its spot.
        palace: u8,
        /// Item room index within the palace.
        room: u8,
    },
    /// Pure logic event: defeating palace `n`'s boss (1-6) and placing its
    /// crystal.
    Boss(u8),
    /// Pure logic event: getting past Thunderbird.
    Thunderbird,
    /// Pure logic event: reaching Dark Link.
    DarkLink,
}

/// How an item location is grouped for shuffling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocClass {
    /// Overworld caves and tiles, the New Kasuto tower and basement, Maze
    /// Island drops.
    Overworld,
    /// The three P-bag caves.
    PbagCave,
    /// A palace item room.
    Palace,
    /// A town wizard (spell location).
    Wizard,
    /// Bagu, the Saria table, the Nabooru fountain.
    Quest,
    /// A stab teacher.
    Trainer,
    /// No item, logic only.
    Event,
}

/// Where the ROM stores the item of a location.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ItemStore {
    /// Nothing to write (events, or a location another module writes).
    #[default]
    None,
    /// The item byte at each of these headerless PRG offsets (vanilla
    /// layout; map through [`Rom::vanilla_offset`] before writing).
    Prg(Vec<usize>),
    /// A town NPC whose reward is set by code (needs the full item shuffle
    /// patch to give anything other than its vanilla reward).
    Npc,
}

/// A place that holds an item, or a logic event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemLoc {
    /// Stable key.
    pub key: LocKey,
    /// Display name.
    pub name: String,
    /// Index of the spot that leads here.
    pub spot: usize,
    /// Requirement to collect once the spot is reached.
    pub requirement: Requirement,
    /// The vanilla item (None for events).
    pub vanilla: Option<ItemId>,
    /// The item placed here now (None for events).
    pub item: Option<ItemId>,
    /// Shuffle group.
    pub class: LocClass,
    /// ROM storage.
    pub store: ItemStore,
    /// The beatability check asks that this is collected.
    pub required: bool,
}

// ---------------------------------------------------------------------------
// World.
// ---------------------------------------------------------------------------

/// The whole game world as the logic sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct World {
    /// Terrain per continent ([`Continent::index`]).
    pub maps: Vec<OverworldMap>,
    /// Every overworld spot.
    pub spots: Vec<Spot>,
    /// Passages between spots.
    pub links: Vec<Link>,
    /// Item locations and events.
    pub locs: Vec<ItemLoc>,
    /// Where the game starts (spot index of the North Palace).
    pub start_spot: usize,
    /// Starting inventory (items, spells, techs, containers).
    pub start: Inventory,
    /// Magic containers needed to cast each spell (menu order); only
    /// checked for tokens naming that spell. Vanilla logic: Jump 2, Fairy,
    /// Reflect and Spell 4, others 0.
    pub spell_containers: [u8; 8],
    /// Total heart containers in the game (starting ones included).
    pub max_hearts: u8,
    /// Total magic containers in the game (starting ones included).
    pub max_magic: u8,
    /// Whether this world was built (an empty default world is never
    /// checked).
    pub built: bool,
}

/// Result of [`World::solve`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reach {
    /// Spot reached, per spot index.
    pub spots: Vec<bool>,
    /// Location collected, per location index.
    pub locs: Vec<bool>,
    /// Inventory after everything reachable was collected.
    pub inventory: Inventory,
    /// Location indices collected in each sphere.
    pub spheres: Vec<Vec<usize>>,
}

/// Vanilla spot names (continent, slot, name). Slot numbers are facts about
/// the game's location tables, names are ours.
const SPOT_NAMES: &[(Continent, u8, &str)] = &[
    (Continent::West, 0, "North Palace"),
    (Continent::West, 1, "Trophy Cave"),
    (Continent::West, 2, "Forest near the start"),
    (Continent::West, 3, "Magic Container Cave"),
    (Continent::West, 4, "Forest by Saria"),
    (Continent::West, 5, "Grass Tile"),
    (Continent::West, 6, "Bagu's Woods 1"),
    (Continent::West, 7, "Trap Road"),
    (Continent::West, 8, "Swamp 1"),
    (Continent::West, 9, "Graveyard 1"),
    (Continent::West, 10, "Parapa Cave North"),
    (Continent::West, 11, "Parapa Cave South"),
    (Continent::West, 12, "Jump Cave North"),
    (Continent::West, 13, "Jump Cave South"),
    (Continent::West, 14, "West P-Bag Cave"),
    (Continent::West, 15, "Medicine Cave"),
    (Continent::West, 16, "Heart Container Cave"),
    (Continent::West, 17, "Fairy Cave Drop"),
    (Continent::West, 18, "Fairy Cave Exit"),
    (Continent::West, 19, "Bridge north of Saria"),
    (Continent::West, 20, "Bridge east of Saria"),
    (Continent::West, 21, "Bridge after DM (west)"),
    (Continent::West, 22, "Bridge after DM (east)"),
    (Continent::West, 23, "Forest by the Jump Cave"),
    (Continent::West, 24, "Swamp 2"),
    (Continent::West, 25, "Forest east of Saria"),
    (Continent::West, 26, "Bagu's Woods 2"),
    (Continent::West, 27, "Bagu's Woods 3"),
    (Continent::West, 28, "Bagu's Woods 4"),
    (Continent::West, 29, "Bagu's Woods 5"),
    (Continent::West, 30, "Road"),
    (Continent::West, 32, "Desert"),
    (Continent::West, 41, "Raft Dock (West)"),
    (Continent::West, 42, "Death Mountain Entrance"),
    (Continent::West, 43, "Death Mountain Exit"),
    (Continent::West, 44, "King's Tomb"),
    (Continent::West, 45, "Rauru"),
    (Continent::West, 47, "Ruto"),
    (Continent::West, 48, "Saria (south)"),
    (Continent::West, 49, "Saria (north)"),
    (Continent::West, 50, "Bagu's House"),
    (Continent::West, 51, "Mido"),
    (Continent::West, 52, "Palace 1 site"),
    (Continent::West, 53, "Palace 2 site"),
    (Continent::West, 54, "Palace 3 site"),
    (Continent::DeathMountain, 28, "Hammer Cave"),
    (Continent::DeathMountain, 42, "DM Connector 1"),
    (Continent::DeathMountain, 43, "DM Connector 2"),
    (Continent::DeathMountain, 56, "Spectacle Rock"),
    (Continent::East, 0, "Forest by Nabooru"),
    (Continent::East, 1, "Forest by Palace 6"),
    (Continent::East, 2, "Trap Road 1"),
    (Continent::East, 3, "Trap Road 2"),
    (Continent::East, 4, "Trap Road 3"),
    (Continent::East, 5, "Trap Road to the Valley"),
    (Continent::East, 6, "Bridge to Palace 6"),
    (Continent::East, 7, "Bridge to Kasuto"),
    (Continent::East, 8, "Desert Trap 1"),
    (Continent::East, 9, "Desert Trap 2"),
    (Continent::East, 10, "Water Tile"),
    (Continent::East, 11, "Nabooru Passthrough South"),
    (Continent::East, 12, "Nabooru Passthrough North"),
    (Continent::East, 13, "Sunken P-Bag Cave"),
    (Continent::East, 14, "Risen P-Bag Cave"),
    (Continent::East, 15, "New Kasuto Passthrough West"),
    (Continent::East, 16, "New Kasuto Passthrough East"),
    (Continent::East, 17, "Valley Passthrough 2 Start"),
    (Continent::East, 18, "Valley Passthrough 2 End"),
    (Continent::East, 19, "Valley Passthrough 1 End"),
    (Continent::East, 20, "Valley Passthrough 1 Start"),
    (Continent::East, 21, "Swamp"),
    (Continent::East, 22, "Lava (unused)"),
    (Continent::East, 23, "Desert 1"),
    (Continent::East, 24, "Desert 2"),
    (Continent::East, 25, "Desert 3"),
    (Continent::East, 26, "Desert Tile"),
    (Continent::East, 27, "Forest 2"),
    (Continent::East, 28, "Lava 1"),
    (Continent::East, 29, "Lava 2"),
    (Continent::East, 30, "Lava Trap 1"),
    (Continent::East, 31, "Lava Trap 2"),
    (Continent::East, 32, "Lava Trap 3"),
    (Continent::East, 40, "Bridge to Maze Island"),
    (Continent::East, 41, "Raft Dock (East)"),
    (Continent::East, 45, "Nabooru"),
    (Continent::East, 47, "Darunia"),
    (Continent::East, 49, "New Kasuto"),
    (Continent::East, 51, "Old Kasuto"),
    (Continent::East, 52, "Palace 5 site"),
    (Continent::East, 53, "Palace 6 site"),
    (Continent::East, 54, "Great Palace site"),
    (Continent::MazeIsland, 37, "Maze Trap 1"),
    (Continent::MazeIsland, 38, "Maze Trap 2"),
    (Continent::MazeIsland, 39, "Magic Container Drop"),
    (Continent::MazeIsland, 40, "Bridge to East"),
    (Continent::MazeIsland, 52, "Palace 4 site"),
    (Continent::MazeIsland, 55, "Child Drop"),
    (Continent::MazeIsland, 57, "Maze Trap 3"),
    (Continent::MazeIsland, 58, "Maze Trap 4"),
    (Continent::MazeIsland, 59, "Maze Trap 5"),
    (Continent::MazeIsland, 60, "Maze Trap 6"),
    (Continent::MazeIsland, 61, "Maze Trap 7"),
];

/// Death Mountain cave halves (slots 0..=36) are named generically.
fn spot_name(c: Continent, slot: u8) -> String {
    if let Some((_, _, n)) = SPOT_NAMES.iter().find(|(cc, s, _)| *cc == c && *s == slot) {
        return (*n).to_string();
    }
    if c == Continent::DeathMountain && slot <= 36 {
        return format!("DM Cave (slot {slot})");
    }
    format!("{} slot {slot}", c.name())
}

/// Slots that exist in each continent's vanilla table (DM and Maze Island
/// share one table, each uses its own slots).
fn vanilla_slots(c: Continent) -> Vec<u8> {
    match c {
        Continent::West => (0..=30)
            .chain([32, 41, 42, 43, 44, 45, 47, 48, 49, 50, 51, 52, 53, 54])
            .collect(),
        Continent::DeathMountain => (0..=36).chain([42, 43, 56]).collect(),
        Continent::East => (0..=32)
            .chain([40, 41, 45, 47, 49, 51, 52, 53, 54])
            .collect(),
        Continent::MazeIsland => [37, 38, 39, 40, 52, 55, 57, 58, 59, 60, 61].to_vec(),
    }
}

/// Map position of the two spots the vanilla game hides until revealed:
/// (continent, slot, column, row, reveal item).
const VANILLA_HIDDEN: &[(Continent, u8, u8, u8, ItemId)] = &[
    (Continent::East, 53, 45, 102 - MAP_Y_OFFSET, ItemId::Flute),
    (Continent::East, 49, 61, 51, ItemId::Hammer),
];

/// Overworld item bytes (iNES file offsets): (continent, slot, offset,
/// class).
const AREA_ITEM_BYTES: &[(Continent, u8, usize, LocClass)] = &[
    (Continent::West, 1, 0x4DEA, LocClass::Overworld),
    (Continent::West, 3, 0x502A, LocClass::Overworld),
    (Continent::West, 5, 0x4DD7, LocClass::Overworld),
    (Continent::West, 14, 0x4FE2, LocClass::PbagCave),
    (Continent::West, 15, 0x5069, LocClass::Overworld),
    (Continent::West, 16, 0x4FF5, LocClass::Overworld),
    (Continent::DeathMountain, 28, 0x6512, LocClass::Overworld),
    (Continent::DeathMountain, 56, 0x65C3, LocClass::Overworld),
    (Continent::East, 10, 0x8FAA, LocClass::Overworld),
    (Continent::East, 13, 0x8ECC, LocClass::PbagCave),
    (Continent::East, 14, 0x8FB3, LocClass::PbagCave),
    (Continent::East, 26, 0x9011, LocClass::Overworld),
    (Continent::MazeIsland, 39, 0xA5A8, LocClass::Overworld),
    (Continent::MazeIsland, 55, 0xA58B, LocClass::Overworld),
];

/// New Kasuto item bytes (iNES file offsets).
const NK_BASEMENT_INES: usize = 0xDB8C;
const NK_TOWER_INES: usize = 0xDB95;

/// Palace sideview pointer tables: (iNES offset of the table, bank of the
/// room data, palaces sharing it).
const PALACE_POINTER_TABLES: &[(usize, u8, &[u8])] = &[
    (0x10533, 4, &[1, 2, 5]),
    (0x12010, 4, &[3, 4, 6]),
    (0x14533, 5, &[7]),
];

/// Palace number (1-7) at a vanilla palace spot.
fn vanilla_palace_at(c: Continent, slot: u8) -> Option<u8> {
    match (c, slot) {
        (Continent::West, 52) => Some(1),
        (Continent::West, 53) => Some(2),
        (Continent::West, 54) => Some(3),
        (Continent::MazeIsland, 52) => Some(4),
        (Continent::East, 52) => Some(5),
        (Continent::East, 53) => Some(6),
        (Continent::East, 54) => Some(7),
        _ => None,
    }
}

/// Where a vanilla continent connector leads (raft, bridge and the two
/// Death Mountain caves). Each pair uses the same slot number on both ends.
fn vanilla_connector_dest(c: Continent, slot: u8) -> Option<Continent> {
    match (c, slot) {
        (Continent::West, 41) => Some(Continent::East),
        (Continent::East, 41) => Some(Continent::West),
        (Continent::West, 42 | 43) => Some(Continent::DeathMountain),
        (Continent::DeathMountain, 42 | 43) => Some(Continent::West),
        (Continent::East, 40) => Some(Continent::MazeIsland),
        (Continent::MazeIsland, 40) => Some(Continent::East),
        _ => None,
    }
}

/// Town at a vanilla town spot.
fn vanilla_town_at(c: Continent, slot: u8) -> Option<Town> {
    match (c, slot) {
        (Continent::West, 45) => Some(Town::Rauru),
        (Continent::West, 47) => Some(Town::Ruto),
        (Continent::West, 48 | 49) => Some(Town::Saria),
        (Continent::West, 50) => Some(Town::Bagu),
        (Continent::West, 51) => Some(Town::Mido),
        (Continent::East, 45) => Some(Town::Nabooru),
        (Continent::East, 47) => Some(Town::Darunia),
        (Continent::East, 49) => Some(Town::NewKasuto),
        (Continent::East, 51) => Some(Town::OldKasuto),
        _ => None,
    }
}

/// Conservative summary of what the vanilla palaces ask for: (item room
/// requirement, boss requirement) per palace 1-6, then the Great Palace
/// path. The `palaces` module replaces these when it generates layouts.
fn vanilla_palace_logic(palace: u8) -> (Requirement, Requirement) {
    use ItemId as I;
    let jump_or_fairy = Requirement::any(&[I::Jump, I::Fairy]);
    match palace {
        1 => (Requirement::none(), Requirement::none()),
        2 => (
            jump_or_fairy.clone(),
            jump_or_fairy.and(&Requirement::item(I::Glove)),
        ),
        3 => {
            let r = Requirement::item(I::Glove).and(&Requirement::any(&[I::Downstab, I::Upstab]));
            (r.clone(), r)
        }
        4 => (
            Requirement::item(I::Fairy),
            Requirement::all(&[I::Fairy, I::Reflect]),
        ),
        5 => (Requirement::item(I::Fairy), Requirement::item(I::Fairy)),
        6 => {
            let r = Requirement::all(&[I::Glove, I::Fairy]);
            (r.clone(), r)
        }
        _ => (
            Requirement::all(&[I::Glove, I::Fairy, I::Downstab, I::Upstab]),
            Requirement::all(&[I::Glove, I::Fairy, I::Downstab, I::Upstab]),
        ),
    }
}

impl World {
    /// Build the vanilla world from `rom` (a vanilla-layout image: maps,
    /// location tables and item bytes are read from it).
    ///
    /// Never fails on a well-sized image; data that does not look like the
    /// game (a synthetic test body) just yields odd maps and missing palace
    /// item rooms.
    pub fn vanilla(rom: &Rom) -> Result<World, RandoError> {
        let mut w = World {
            spell_containers: [0, 2, 0, 4, 0, 4, 4, 0],
            max_hearts: 8,
            max_magic: 8,
            built: true,
            ..World::default()
        };
        w.start.hearts = 4;
        w.start.magic = 4;
        for c in Continent::ALL {
            let off = rom.prg_from_ines(c.vanilla_map_ines())?;
            w.maps
                .push(OverworldMap::decode(rom, off, MAP_ROWS, MAP_COLS));
        }
        // Spots.
        for c in Continent::ALL {
            let base = rom.prg_from_ines(c.location_table_ines())?;
            for slot in vanilla_slots(c) {
                let s = usize::from(slot);
                let raw = [
                    rom.read(base + s)?,
                    rom.read(base + s + LOCATION_SLOTS)?,
                    rom.read(base + s + 2 * LOCATION_SLOTS)?,
                    rom.read(base + s + 3 * LOCATION_SLOTS)?,
                ];
                let y = raw[0] & 0x7F;
                let mut spot = Spot {
                    continent: c,
                    slot,
                    name: spot_name(c, slot),
                    x: raw[1] & 0x3F,
                    row: y.saturating_sub(MAP_Y_OFFSET),
                    on_map: y >= MAP_Y_OFFSET,
                    hidden: false,
                    kind: SpotKind::Area,
                    access: Requirement::none(),
                    required: false,
                    raw,
                };
                if let Some(&(_, _, x, row, reveal)) = VANILLA_HIDDEN
                    .iter()
                    .find(|(cc, ss, ..)| *cc == c && *ss == slot)
                {
                    if !spot.on_map {
                        spot.x = x;
                        spot.row = row;
                        spot.on_map = true;
                        spot.hidden = true;
                        spot.access = Requirement::item(reveal);
                    }
                }
                if let Some(p) = vanilla_palace_at(c, slot) {
                    spot.kind = SpotKind::Palace(p);
                    // Palaces need the fairy or the magic key so small-key
                    // counts never matter to the logic.
                    spot.access = spot
                        .access
                        .and(&Requirement::any(&[ItemId::Fairy, ItemId::MagicKey]));
                    spot.required = true;
                } else if let Some(t) = vanilla_town_at(c, slot) {
                    spot.kind = SpotKind::Town(t);
                    spot.required = true;
                } else if let Some(dest) = vanilla_connector_dest(c, slot) {
                    spot.kind = SpotKind::Connector(dest);
                }
                w.spots.push(spot);
            }
        }
        w.start_spot = w.spot_index(Continent::West, 0).unwrap_or(0);
        // Access requirements of special spots.
        let fairy_or = |w: &mut World, c, s, r: Requirement| {
            if let Some(i) = w.spot_index(c, s) {
                w.spots[i].access = w.spots[i].access.and(&r);
            }
        };
        fairy_or(
            &mut w,
            Continent::West,
            17,
            Requirement::item(ItemId::Fairy),
        );
        fairy_or(
            &mut w,
            Continent::West,
            12,
            Requirement::any(&[ItemId::Jump, ItemId::Fairy]),
        );
        fairy_or(
            &mut w,
            Continent::East,
            10,
            Requirement::item(ItemId::Boots),
        );
        // Passthrough tiles that need a spell to cross (East desert/swamp).
        for (slot, item) in [(23u8, ItemId::Jump), (21, ItemId::Fairy)] {
            if let Some(i) = w.spot_index(Continent::East, slot) {
                if w.spots[i].raw[3] & 0x40 != 0 {
                    w.spots[i].access = w.spots[i].access.with_hard(Token::Item(item));
                }
            }
        }
        // Internal links: slots sharing a sideview area (entrance index).
        for i in 0..w.spots.len() {
            let sp = &w.spots[i];
            let e = sp.raw[1] >> 6;
            // Town doors and the King's Tomb use the field for something
            // else; only Saria's two doors share one town.
            let townish = |k: SpotKind| matches!(k, SpotKind::Town(t) if t != Town::Saria);
            if e == 0
                || sp.slot < e
                || townish(sp.kind)
                || (sp.continent, sp.slot) == (Continent::West, 44)
            {
                continue;
            }
            if let Some(j) = w.spot_index(sp.continent, sp.slot - e) {
                if townish(w.spots[j].kind)
                    || (w.spots[j].continent, w.spots[j].slot) == (Continent::West, 44)
                {
                    continue;
                }
                let req = if matches!(sp.kind, SpotKind::Town(Town::Saria)) {
                    saria_moat()
                } else {
                    Requirement::none()
                };
                w.links.push(Link {
                    a: j,
                    b: i,
                    req,
                    one_way: false,
                });
            }
        }
        // Vanilla Saria: make sure its two doors are linked through the moat.
        if let (Some(a), Some(b)) = (
            w.spot_index(Continent::West, 48),
            w.spot_index(Continent::West, 49),
        ) {
            if !w
                .links
                .iter()
                .any(|l| (l.a == a && l.b == b) || (l.a == b && l.b == a))
            {
                w.links.push(Link {
                    a,
                    b,
                    req: saria_moat(),
                    one_way: false,
                });
            }
        }
        // Cave pairs whose ends are separate areas in the table (Parapa,
        // jump cave, fairy cave): linked explicitly.
        for (a, b) in [(10u8, 11u8), (12, 13), (17, 18)] {
            w.link_slots(Continent::West, a, b, Requirement::none());
        }
        for (a, b) in [(11u8, 12u8), (15, 16), (17, 18), (19, 20)] {
            w.link_slots(Continent::East, a, b, Requirement::none());
        }
        // Continent connectors: slot N on one continent leads to slot N on
        // the destination continent. The raft needs the raft.
        let mut conn = Vec::new();
        for (i, sp) in w.spots.iter().enumerate() {
            if let SpotKind::Connector(dest) = sp.kind {
                if let Some(j) = w.spot_index(dest, sp.slot) {
                    if i < j {
                        let req = if sp.slot == 41 {
                            Requirement::item(ItemId::Raft)
                        } else {
                            Requirement::none()
                        };
                        conn.push(Link {
                            a: i,
                            b: j,
                            req,
                            one_way: false,
                        });
                    }
                }
            }
        }
        w.links.extend(conn);
        // Item locations: overworld areas.
        for &(c, slot, ines, class) in AREA_ITEM_BYTES {
            let Some(spot) = w.spot_index(c, slot) else {
                continue;
            };
            let off = rom.prg_from_ines(ines)?;
            let item = ItemId::from_byte(rom.read(off)?);
            w.locs.push(ItemLoc {
                key: LocKey::Area(c, slot),
                name: w.spots[spot].name.clone(),
                spot,
                requirement: Requirement::none(),
                vanilla: item,
                item,
                class,
                store: ItemStore::Prg(vec![off]),
                required: item.is_some(),
            });
        }
        // Towns.
        w.add_town_locations(rom)?;
        // Palaces.
        w.add_vanilla_palaces(rom)?;
        Ok(w)
    }

    fn link_slots(&mut self, c: Continent, a: u8, b: u8, req: Requirement) {
        if let (Some(i), Some(j)) = (self.spot_index(c, a), self.spot_index(c, b)) {
            if !self
                .links
                .iter()
                .any(|l| (l.a == i && l.b == j) || (l.a == j && l.b == i))
            {
                self.links.push(Link {
                    a: i,
                    b: j,
                    req,
                    one_way: false,
                });
            }
        }
    }

    fn add_town_locations(&mut self, rom: &Rom) -> Result<(), RandoError> {
        use ItemId as I;
        let town_spot = |w: &World, t: Town| -> Option<usize> {
            let cands: Vec<usize> = w
                .spots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.kind == SpotKind::Town(t))
                .map(|(i, _)| i)
                .collect();
            // Saria: items sit past the moat for the left door, so attach
            // them to the door that enters from the right.
            cands
                .iter()
                .copied()
                .find(|&i| w.spots[i].enters_from_right())
                .or_else(|| cands.last().copied())
        };
        let jump_or_fairy = Requirement::any(&[I::Jump, I::Fairy]);
        let add = |w: &mut World,
                   slot: TownSlot,
                   name: String,
                   item: Option<ItemId>,
                   req: Requirement,
                   class: LocClass,
                   store: ItemStore| {
            if let Some(spot) = town_spot(w, slot.town()) {
                w.locs.push(ItemLoc {
                    key: LocKey::Town(slot),
                    name,
                    spot,
                    requirement: req,
                    vanilla: item,
                    item,
                    class,
                    store,
                    required: item.is_some(),
                });
            }
        };
        for t in Town::WIZARD_TOWNS {
            let mut req = Requirement::containers(t.wizard_containers());
            if let Some(i) = t.wizard_item() {
                req = req.and(&Requirement::item(i));
            }
            let spell = t.vanilla_spell().unwrap_or(I::Shield);
            add(
                self,
                TownSlot::Wizard(t),
                format!("{} Wizard", t.name()),
                Some(spell),
                req,
                LocClass::Wizard,
                ItemStore::Npc,
            );
        }
        add(
            self,
            TownSlot::SariaTable,
            "Saria Table".into(),
            Some(I::Mirror),
            Requirement::none(),
            LocClass::Quest,
            ItemStore::Npc,
        );
        add(
            self,
            TownSlot::NabooruFountain,
            "Nabooru Fountain".into(),
            Some(I::Water),
            Requirement::none(),
            LocClass::Quest,
            ItemStore::Npc,
        );
        add(
            self,
            TownSlot::BaguHouse,
            "Bagu".into(),
            Some(I::BaguNote),
            Requirement::none(),
            LocClass::Quest,
            ItemStore::Npc,
        );
        add(
            self,
            TownSlot::MidoTrainer,
            "Mido Downstab Trainer".into(),
            Some(I::Downstab),
            jump_or_fairy.clone(),
            LocClass::Trainer,
            ItemStore::Npc,
        );
        add(
            self,
            TownSlot::DaruniaTrainer,
            "Darunia Upstab Trainer".into(),
            Some(I::Upstab),
            jump_or_fairy,
            LocClass::Trainer,
            ItemStore::Npc,
        );
        let tower = rom.prg_from_ines(NK_TOWER_INES)?;
        let basement = rom.prg_from_ines(NK_BASEMENT_INES)?;
        let tower_item = ItemId::from_byte(rom.read(tower)?);
        let basement_item = ItemId::from_byte(rom.read(basement)?);
        add(
            self,
            TownSlot::SpellTower,
            "New Kasuto Spell Tower".into(),
            tower_item,
            Requirement::item(I::Spell),
            LocClass::Overworld,
            ItemStore::Prg(vec![tower]),
        );
        add(
            self,
            TownSlot::GrannysBasement,
            "New Kasuto Granny's Basement".into(),
            basement_item,
            Requirement::containers(7),
            LocClass::Overworld,
            ItemStore::Prg(vec![basement]),
        );
        Ok(())
    }

    fn add_vanilla_palaces(&mut self, rom: &Rom) -> Result<(), RandoError> {
        for palace in 1..=7u8 {
            let Some(spot) = self.palace_spot(palace) else {
                continue;
            };
            let (item_req, boss_req) = vanilla_palace_logic(palace);
            let mut locs = Vec::new();
            if palace <= 6 {
                let item = ItemId::PALACE_ITEMS[usize::from(palace - 1)];
                let store = match find_palace_item_offset(rom, palace, item)? {
                    Some(off) => ItemStore::Prg(vec![off]),
                    None => ItemStore::None,
                };
                locs.push(ItemLoc {
                    key: LocKey::PalaceItem { palace, room: 0 },
                    name: format!("Palace {palace} Item Room"),
                    spot,
                    requirement: item_req,
                    vanilla: Some(item),
                    item: Some(item),
                    class: LocClass::Palace,
                    store,
                    required: true,
                });
                locs.push(event(
                    LocKey::Boss(palace),
                    format!("Palace {palace} Boss"),
                    spot,
                    boss_req,
                ));
            } else {
                locs.push(event(
                    LocKey::Thunderbird,
                    "Thunderbird".into(),
                    spot,
                    item_req.and(&Requirement::item(ItemId::Thunder)),
                ));
                locs.push(event(
                    LocKey::DarkLink,
                    "Dark Link".into(),
                    spot,
                    boss_req.and(&Requirement::item(ItemId::Thunder)),
                ));
            }
            self.locs.extend(locs);
        }
        Ok(())
    }

    // -- lookups ----------------------------------------------------------

    /// Index of the spot at `(continent, slot)`.
    #[must_use]
    pub fn spot_index(&self, c: Continent, slot: u8) -> Option<usize> {
        self.spots
            .iter()
            .position(|s| s.continent == c && s.slot == slot)
    }

    /// Index of the spot whose palace is `palace` (1-7).
    #[must_use]
    pub fn palace_spot(&self, palace: u8) -> Option<usize> {
        self.spots
            .iter()
            .position(|s| s.kind == SpotKind::Palace(palace))
    }

    /// Index of the location with `key`.
    #[must_use]
    pub fn loc_index(&self, key: LocKey) -> Option<usize> {
        self.locs.iter().position(|l| l.key == key)
    }

    /// The location with `key`.
    #[must_use]
    pub fn loc(&self, key: LocKey) -> Option<&ItemLoc> {
        self.locs.iter().find(|l| l.key == key)
    }

    /// The location with `key`, mutable.
    pub fn loc_mut(&mut self, key: LocKey) -> Option<&mut ItemLoc> {
        self.locs.iter_mut().find(|l| l.key == key)
    }

    /// Indices of the locations in `class`.
    #[must_use]
    pub fn locs_in(&self, class: LocClass) -> Vec<usize> {
        (0..self.locs.len())
            .filter(|&i| self.locs[i].class == class)
            .collect()
    }

    /// Every placed item, in location order.
    #[must_use]
    pub fn placed_items(&self) -> Vec<ItemId> {
        self.locs.iter().filter_map(|l| l.item).collect()
    }

    // -- editing hooks for other modules ----------------------------------

    /// Swap which palaces sit at two palace spots (the palaces keep their
    /// own item rooms and bosses, which follow them to the new spot).
    pub fn swap_palace_spots(&mut self, a: usize, b: usize) {
        let (ka, kb) = (self.spots[a].kind, self.spots[b].kind);
        self.spots[a].kind = kb;
        self.spots[b].kind = ka;
        for l in &mut self.locs {
            if l.spot == a {
                l.spot = b;
            } else if l.spot == b {
                l.spot = a;
            }
        }
    }

    /// Replace palace `palace`'s item rooms and boss events with `locs`
    /// (their `spot` is set to the palace's spot). Use [`LocKey::PalaceItem`]
    /// keys for item rooms and [`LocKey::Boss`] / [`LocKey::Thunderbird`] /
    /// [`LocKey::DarkLink`] for events.
    pub fn set_palace_locations(&mut self, palace: u8, mut locs: Vec<ItemLoc>) {
        let Some(spot) = self.palace_spot(palace) else {
            return;
        };
        self.locs.retain(|l| match l.key {
            LocKey::PalaceItem { palace: p, .. } => p != palace,
            LocKey::Boss(p) => p != palace,
            LocKey::Thunderbird | LocKey::DarkLink => palace != 7,
            _ => true,
        });
        for l in &mut locs {
            l.spot = spot;
        }
        self.locs.extend(locs);
    }

    /// Turn the wizards' magic-container requirements on or off.
    pub fn set_wizard_magic_requirements(&mut self, on: bool) {
        for t in Town::WIZARD_TOWNS {
            if let Some(l) = self.loc_mut(LocKey::Town(TownSlot::Wizard(t))) {
                let mut r = match t.wizard_item() {
                    Some(i) => Requirement::item(i),
                    None => Requirement::none(),
                };
                if on {
                    r = r.and(&Requirement::containers(t.wizard_containers()));
                }
                l.requirement = r;
            }
        }
    }

    /// Set the magic containers needed for granny's basement in New Kasuto.
    pub fn set_new_kasuto_basement(&mut self, containers: u8) {
        if let Some(l) = self.loc_mut(LocKey::Town(TownSlot::GrannysBasement)) {
            l.requirement = Requirement::containers(containers);
        }
    }

    /// Set the magic containers the logic asks for before a spell counts as
    /// usable (menu order).
    pub fn set_spell_containers(&mut self, c: [u8; 8]) {
        self.spell_containers = c;
    }

    // -- solver -----------------------------------------------------------

    fn token_met(&self, t: Token, inv: &Inventory) -> bool {
        match t {
            Token::Item(i) => {
                if !inv.has(i) {
                    return false;
                }
                match i.spell_index() {
                    Some(s) => inv.magic >= self.spell_containers[s],
                    None => true,
                }
            }
            Token::MagicContainers(n) => inv.magic >= n,
            Token::Never => false,
        }
    }

    /// Whether `r` is met by `inv`.
    #[must_use]
    pub fn met(&self, r: &Requirement, inv: &Inventory) -> bool {
        r.any_of.is_empty()
            || r.any_of
                .iter()
                .any(|g| g.iter().all(|&t| self.token_met(t, inv)))
    }

    /// Spots reached and locations collectable with a fixed inventory (no
    /// collecting along the way).
    #[must_use]
    pub fn reachable(&self, inv: &Inventory) -> Reach {
        let spots = self.reach_spots(inv, None);
        let locs = self
            .locs
            .iter()
            .map(|l| spots.get(l.spot).copied().unwrap_or(false) && self.met(&l.requirement, inv))
            .collect();
        Reach {
            spots,
            locs,
            inventory: *inv,
            spheres: Vec::new(),
        }
    }

    /// Flood-fill the overworld with `inv`, optionally continuing from a
    /// previous result (reachability only grows as the inventory grows).
    fn reach_spots(&self, inv: &Inventory, prev: Option<&[bool]>) -> Vec<bool> {
        let n = self.spots.len();
        let mut reached: Vec<bool> = prev.map_or_else(|| vec![false; n], <[bool]>::to_vec);
        reached.resize(n, false);
        if self.start_spot < n {
            reached[self.start_spot] = true;
        }
        // Spots per tile, per continent.
        let mut at: Vec<Vec<Vec<usize>>> = vec![Vec::new(); self.maps.len()];
        for (ci, m) in self.maps.iter().enumerate() {
            at[ci] = vec![Vec::new(); m.cols.max(1) * m.rows.max(1)];
        }
        for (i, s) in self.spots.iter().enumerate() {
            let ci = s.continent.index();
            if !s.on_map || ci >= self.maps.len() {
                continue;
            }
            let m = &self.maps[ci];
            if usize::from(s.x) < m.cols && usize::from(s.row) < m.rows {
                at[ci][usize::from(s.row) * m.cols + usize::from(s.x)].push(i);
            }
        }
        let mut visited: Vec<Vec<bool>> = self
            .maps
            .iter()
            .map(|m| vec![false; m.cols * m.rows])
            .collect();
        loop {
            let before = reached.iter().filter(|&&r| r).count();
            // Flood from every reached spot.
            let mut stack: Vec<(usize, usize, usize)> = Vec::new();
            for (i, s) in self.spots.iter().enumerate() {
                if reached[i] && s.on_map {
                    let ci = s.continent.index();
                    if ci < self.maps.len() {
                        stack.push((ci, usize::from(s.x), usize::from(s.row)));
                    }
                }
            }
            while let Some((ci, x, y)) = stack.pop() {
                let m = &self.maps[ci];
                if x >= m.cols || y >= m.rows {
                    continue;
                }
                let k = y * m.cols + x;
                if visited[ci][k] {
                    continue;
                }
                visited[ci][k] = true;
                for &si in &at[ci][k] {
                    if self.met(&self.spots[si].access, inv) {
                        reached[si] = true;
                    }
                }
                let nbrs = [
                    (x.wrapping_sub(1), y),
                    (x + 1, y),
                    (x, y.wrapping_sub(1)),
                    (x, y + 1),
                ];
                for (nx, ny) in nbrs {
                    if nx >= m.cols || ny >= m.rows {
                        continue;
                    }
                    let nk = ny * m.cols + nx;
                    if visited[ci][nk] {
                        continue;
                    }
                    if !m.tiles[nk].passable(inv) {
                        continue;
                    }
                    let here = &at[ci][nk];
                    if !here.is_empty()
                        && !here.iter().any(|&si| self.met(&self.spots[si].access, inv))
                    {
                        continue;
                    }
                    stack.push((ci, nx, ny));
                }
            }
            // Links.
            for l in &self.links {
                if !self.met(&l.req, inv) {
                    continue;
                }
                let from_a =
                    reached.get(l.a).copied().unwrap_or(false) && self.spot_usable(l.a, inv);
                let from_b = !l.one_way
                    && reached.get(l.b).copied().unwrap_or(false)
                    && self.spot_usable(l.b, inv);
                if from_a && l.b < n {
                    reached[l.b] = true;
                }
                if from_b && l.a < n {
                    reached[l.a] = true;
                }
            }
            if reached.iter().filter(|&&r| r).count() == before {
                break;
            }
        }
        reached
    }

    /// A reached spot can be entered (its access met), or it is the start.
    fn spot_usable(&self, i: usize, inv: &Inventory) -> bool {
        i == self.start_spot || self.met(&self.spots[i].access, inv)
    }

    /// Collect everything reachable from [`World::start`], sphere by sphere.
    #[must_use]
    pub fn solve(&self) -> Reach {
        self.solve_from(&self.start)
    }

    /// [`World::solve`] from a given inventory.
    #[must_use]
    pub fn solve_from(&self, start: &Inventory) -> Reach {
        let mut inv = *start;
        let mut taken = vec![false; self.locs.len()];
        let mut spots: Vec<bool> = Vec::new();
        let mut spheres = Vec::new();
        loop {
            spots = self.reach_spots(&inv, Some(&spots));
            let mut got = Vec::new();
            for (i, l) in self.locs.iter().enumerate() {
                if !taken[i]
                    && spots.get(l.spot).copied().unwrap_or(false)
                    && self.met(&l.requirement, &inv)
                {
                    got.push(i);
                }
            }
            if got.is_empty() {
                break;
            }
            for &i in &got {
                taken[i] = true;
                if let Some(it) = self.locs[i].item {
                    inv.add(it);
                }
            }
            spheres.push(got);
        }
        Reach {
            spots,
            locs: taken,
            inventory: inv,
            spheres,
        }
    }

    /// Why the world is not beatable, or `None` when it is.
    #[must_use]
    pub fn unbeatable_reason(&self) -> Option<String> {
        if !self.built {
            return None;
        }
        let r = self.solve();
        let missing: Vec<&str> = self
            .locs
            .iter()
            .enumerate()
            .filter(|(i, l)| l.required && !r.locs[*i])
            .map(|(_, l)| l.name.as_str())
            .collect();
        if !missing.is_empty() {
            return Some(format!("unreachable: {}", missing.join(", ")));
        }
        let spots: Vec<&str> = self
            .spots
            .iter()
            .enumerate()
            .filter(|(i, s)| s.required && s.on_map && !r.spots[*i])
            .map(|(_, s)| s.name.as_str())
            .collect();
        if !spots.is_empty() {
            return Some(format!("unreached: {}", spots.join(", ")));
        }
        None
    }

    /// Whether every required location is collectable and every required
    /// spot reachable from the start.
    #[must_use]
    pub fn beatable(&self) -> bool {
        self.unbeatable_reason().is_none()
    }

    /// Drop the `required` mark from everything the current placement cannot
    /// reach from [`World::start`]. `start` calls this on the freshly built
    /// vanilla world, where it changes nothing for the real game; it only
    /// matters for images that are not the game (synthetic test bodies).
    pub fn relax_to_solvable(&mut self) -> Vec<String> {
        let r = self.solve();
        let mut out = Vec::new();
        for (i, l) in self.locs.iter_mut().enumerate() {
            if l.required && !r.locs[i] {
                l.required = false;
                out.push(l.name.clone());
            }
        }
        for (i, s) in self.spots.iter_mut().enumerate() {
            if s.required && !r.spots[i] {
                s.required = false;
                out.push(s.name.clone());
            }
        }
        out
    }

    /// Drop the `required` mark from everything that is not collectable even
    /// with every item in the game (a guard for data that does not match the
    /// expected layout, such as synthetic test images). Returns the names of
    /// what was relaxed.
    pub fn relax_impossible(&mut self) -> Vec<String> {
        let mut all = Inventory {
            items: !0,
            hearts: 8,
            magic: 8,
        };
        all.items &= (1u64 << 0x25) - 1;
        let r = self.solve_from(&all);
        let mut out = Vec::new();
        for (i, l) in self.locs.iter_mut().enumerate() {
            if l.required && !r.locs[i] {
                l.required = false;
                out.push(l.name.clone());
            }
        }
        for (i, s) in self.spots.iter_mut().enumerate() {
            if s.required && !r.spots[i] {
                s.required = false;
                out.push(s.name.clone());
            }
        }
        out
    }
}

fn saria_moat() -> Requirement {
    Requirement::any(&[ItemId::Fairy, ItemId::BaguNote])
        .or(&Requirement::all(&[ItemId::Jump, ItemId::Dash]))
}

fn event(key: LocKey, name: String, spot: usize, requirement: Requirement) -> ItemLoc {
    ItemLoc {
        key,
        name,
        spot,
        requirement,
        vanilla: None,
        item: None,
        class: LocClass::Event,
        store: ItemStore::None,
        required: true,
    }
}

/// Headerless offset of the item byte of palace `palace`'s item object
/// holding `item` (the first match in the palace's sideview group), read
/// from the ROM's room data.
pub fn find_palace_item_offset(
    rom: &Rom,
    palace: u8,
    item: ItemId,
) -> Result<Option<usize>, RandoError> {
    let Some(&(table_ines, bank, _)) = PALACE_POINTER_TABLES
        .iter()
        .find(|(_, _, ps)| ps.contains(&palace))
    else {
        return Ok(None);
    };
    let table = rom.prg_from_ines(table_ines)?;
    for map in 0..LOCATION_SLOTS {
        let lo = rom.read(table + 2 * map)?;
        let hi = rom.read(table + 2 * map + 1)?;
        let ptr = u16::from_le_bytes([lo, hi]);
        if ptr < 0x8000 {
            continue;
        }
        let b = if ptr >= 0xC000 { 7 } else { bank };
        let Ok(start) = rom.cpu_offset(b, ptr) else {
            continue;
        };
        for (off, it) in sideview_items(rom, start) {
            if it == item.byte() {
                return Ok(Some(off));
            }
        }
    }
    Ok(None)
}

/// `(offset of the item byte, item byte)` for every item object in the
/// sideview data at headerless offset `start` (4-byte header, the first byte
/// being the total length; objects are 2 bytes, or 3 when the object type
/// is `$0F` with a Y below 13).
#[must_use]
pub fn sideview_items(rom: &Rom, start: usize) -> Vec<(usize, u8)> {
    let mut out = Vec::new();
    let Ok(len) = rom.read(start) else {
        return out;
    };
    let len = usize::from(len);
    let mut p = 4;
    while p + 1 < len {
        let (Ok(b0), Ok(b1)) = (rom.read(start + p), rom.read(start + p + 1)) else {
            break;
        };
        if b0 >> 4 < 13 && b1 == 0x0F {
            if p + 2 < len {
                if let Ok(it) = rom.read(start + p + 2) {
                    out.push((start + p + 2, it));
                }
            }
            p += 3;
        } else {
            p += 2;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_algebra() {
        let a = Requirement::item(ItemId::Jump);
        let b = Requirement::any(&[ItemId::Fairy, ItemId::MagicKey]);
        let ab = a.and(&b);
        assert_eq!(ab.any_of.len(), 2);
        assert!(ab.has_hard(Token::Item(ItemId::Jump)));
        assert!(!ab.has_hard(Token::Item(ItemId::Fairy)));
        assert!(Requirement::none().is_none());
        assert!(a.without(&[ItemId::Jump]).is_none());
        assert_eq!(
            a.or(&Requirement::none()),
            Requirement::none(),
            "OR with always is always"
        );
        assert_eq!(ab.items().len(), 3);
        assert_eq!(format!("{ab}"), "Jump + Fairy | Jump + Magic Key");
    }

    #[test]
    fn spell_tokens_need_containers() {
        let w = World {
            spell_containers: [0, 2, 0, 4, 0, 4, 4, 0],
            ..World::default()
        };
        let mut inv = Inventory::default();
        inv.add(ItemId::Fairy);
        inv.magic = 3;
        assert!(!w.met(&Requirement::item(ItemId::Fairy), &inv));
        inv.magic = 4;
        assert!(w.met(&Requirement::item(ItemId::Fairy), &inv));
        assert!(w.met(&Requirement::containers(4), &inv));
        assert!(!w.met(&Requirement::never(), &inv));
    }

    #[test]
    fn item_classes() {
        assert!(ItemId::SmallKey.is_minor());
        assert!(ItemId::Dash.is_spell());
        assert_eq!(ItemId::Dash.spell_index(), Some(4));
        assert_eq!(ItemId::Thunder.spell_index(), Some(7));
        assert!(ItemId::Mirror.is_spell_item() && ItemId::Mirror.is_quest_item());
        assert!(ItemId::Glove.is_major());
        assert!(!ItemId::HeartContainer.is_major());
        for &i in ItemId::ALL {
            assert_eq!(ItemId::from_byte(i.byte()), Some(i));
        }
        assert_eq!(ItemId::from_byte(0x09), None);
        assert_eq!(Town::Mido.vanilla_spell(), Some(ItemId::Fairy));
    }

    fn tiny_world() -> World {
        // A 4x1 strip: start, grass, rock, cave with an item; plus a second
        // continent reached by raft.
        let mut m = OverworldMap {
            cols: 4,
            rows: 1,
            tiles: vec![
                Terrain::Palace,
                Terrain::Grass,
                Terrain::Rock,
                Terrain::Cave,
            ],
        };
        m.set(9, 9, Terrain::Water);
        let spot = |c, slot, x, kind| Spot {
            continent: c,
            slot,
            name: format!("s{slot}"),
            x,
            row: 0,
            on_map: true,
            hidden: false,
            kind,
            access: Requirement::none(),
            required: false,
            raw: [0; 4],
        };
        let m2 = OverworldMap {
            cols: 2,
            rows: 1,
            tiles: vec![Terrain::Cave, Terrain::Cave],
        };
        let loc = |key, spot, item, req| ItemLoc {
            key,
            name: format!("{key:?}"),
            spot,
            requirement: req,
            vanilla: Some(item),
            item: Some(item),
            class: LocClass::Overworld,
            store: ItemStore::None,
            required: true,
        };
        World {
            maps: vec![m, m2],
            spots: vec![
                spot(Continent::West, 0, 0, SpotKind::Area),
                spot(
                    Continent::West,
                    1,
                    1,
                    SpotKind::Connector(Continent::DeathMountain),
                ),
                spot(Continent::West, 2, 3, SpotKind::Area),
                spot(Continent::DeathMountain, 1, 0, SpotKind::Area),
                spot(Continent::DeathMountain, 2, 1, SpotKind::Area),
            ],
            links: vec![Link {
                a: 1,
                b: 3,
                req: Requirement::item(ItemId::Raft),
                one_way: false,
            }],
            locs: vec![
                loc(
                    LocKey::Area(Continent::West, 0),
                    0,
                    ItemId::Raft,
                    Requirement::none(),
                ),
                loc(
                    LocKey::Area(Continent::DeathMountain, 2),
                    4,
                    ItemId::Hammer,
                    Requirement::none(),
                ),
                loc(
                    LocKey::Area(Continent::West, 2),
                    2,
                    ItemId::Glove,
                    Requirement::none(),
                ),
            ],
            start_spot: 0,
            start: Inventory::default(),
            spell_containers: [0; 8],
            max_hearts: 4,
            max_magic: 4,
            built: true,
        }
    }

    #[test]
    fn solver_follows_items_terrain_and_links() {
        let w = tiny_world();
        let r = w.solve();
        assert!(r.locs.iter().all(|&b| b), "{r:?}");
        assert_eq!(r.spheres.len(), 3);
        assert!(w.beatable());
        // Swap the raft and the hammer: the raft now sits behind the rock,
        // the hammer behind the raft.
        let mut w2 = w.clone();
        w2.locs[0].item = Some(ItemId::Hammer);
        w2.locs[2].item = Some(ItemId::Raft);
        w2.locs[1].item = Some(ItemId::Glove);
        assert!(w2.beatable());
        let mut w3 = w;
        w3.locs[0].item = Some(ItemId::Glove);
        w3.locs[1].item = Some(ItemId::Hammer);
        w3.locs[2].item = Some(ItemId::Raft);
        assert!(!w3.beatable());
        assert!(w3.unbeatable_reason().unwrap().contains("unreachable"));
    }

    #[test]
    fn default_world_is_never_checked() {
        assert!(World::default().beatable());
    }

    /// ROM-gated: the vanilla world from the player's ROM is beatable, the
    /// expected item bytes and palace item objects are found, and nothing is
    /// collectable without the right items.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_vanilla_world_is_beatable() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let rom = Rom::from_body(&body).unwrap();
        let mut w = World::vanilla(&rom).unwrap();
        let reason = w.unbeatable_reason();
        assert_eq!(reason, None);
        assert!(w.relax_impossible().is_empty());
        for p in 1..=6u8 {
            let l = w.loc(LocKey::PalaceItem { palace: p, room: 0 }).unwrap();
            assert!(matches!(l.store, ItemStore::Prg(_)), "palace {p} item byte");
        }
        let items: Vec<ItemId> = w.placed_items();
        assert_eq!(
            items
                .iter()
                .filter(|&&i| i == ItemId::HeartContainer)
                .count(),
            4
        );
        assert_eq!(
            items
                .iter()
                .filter(|&&i| i == ItemId::MagicContainer)
                .count(),
            4
        );
        // Without anything, the palace items are out of reach (palaces need
        // fairy or key) and so is the hammer's spectacle rock.
        let bare = w.reachable(&w.start);
        let spec = w
            .loc_index(LocKey::Area(Continent::DeathMountain, 56))
            .unwrap();
        assert!(!bare.locs[spec]);
        let p1 = w
            .loc_index(LocKey::PalaceItem { palace: 1, room: 0 })
            .unwrap();
        assert!(!bare.locs[p1]);
        // Starting spot reaches Rauru.
        let rauru = w.spot_index(Continent::West, 45).unwrap();
        assert!(bare.spots[rauru]);
        let r = w.solve();
        eprintln!("spheres: {}", r.spheres.len());
        for (n, s) in r.spheres.iter().enumerate() {
            let names: Vec<String> = s
                .iter()
                .map(|&i| {
                    format!(
                        "{} [{}]",
                        w.locs[i].name,
                        w.locs[i].item.map_or("-", ItemId::name)
                    )
                })
                .collect();
            eprintln!("{n}: {}", names.join(", "));
        }
        // The flute is needed for the hidden palace.
        w.locs.iter_mut().for_each(|l| {
            if l.item == Some(ItemId::Flute) {
                l.item = Some(ItemId::SmallKey);
            }
        });
        assert!(!w.beatable());
    }
}
