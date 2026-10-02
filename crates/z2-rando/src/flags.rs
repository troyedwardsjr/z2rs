//! Randomizer options, the compact flag string, and presets.
//!
//! # Layout
//!
//! [`Flags`] holds one sub-struct per pipeline module ([`StartFlags`],
//! [`OverworldFlags`], ...). Every struct is `#[serde(default)]` and its
//! [`Default`] produces a **vanilla ROM**: with default flags the randomizer
//! returns the input body unchanged. Some fields default to "on" because
//! that is what the unmodified game already does (for example
//! [`OverworldFlags::hide_kasuto`]: New Kasuto *is* hidden in the vanilla
//! game); such fields only matter once the module that owns them is
//! changing something.
//!
//! Option types:
//!
//! * `bool` for plain switches.
//! * [`Tri`] for "off / on / random" switches. `Random` is resolved per seed
//!   with probability [`StartFlags::random_flag_rate`]
//!   (see `Ctx::tri` in the crate root).
//! * Enums for multiple choice, often with a `Random` variant.
//! * `u8` for counts, with the allowed range written on the field and
//!   enforced by the flag-string codec ([`RANGES`] lists them all).
//!
//! # Flag string format (version `1`)
//!
//! `<version char><payload>`, where the payload is URL-safe base64
//! (`A-Z a-z 0-9 - _`, 6 bits per character, most significant bit first) of
//! a bit stream made of one **section per module**, in this fixed order:
//! start, overworld, palaces, items, enemies, stats, spells, drops, hints,
//! towns, qol, cosmetic.
//!
//! Each section is a length (a varint of 5-bit groups, each followed by a
//! "more" bit) and then that many bits of fields. Each field is stored in a
//! fixed width as `value XOR default` (enums by index, ranged integers as
//! `value - min`). Defaults therefore encode as zero bits, trailing zero
//! bits are trimmed from every section, and trailing `A` characters are
//! trimmed from the string. Reading past the end of a section or of the
//! string yields zero bits, that is, defaults. The vanilla flags encode as
//! just `"1"`.
//!
//! Why this shape: a module can **append** a field to the end of its
//! section without breaking older strings (they decode with the new field at
//! its default) and without moving any other module's bits. Removing or
//! reordering fields, or changing a field's width or default, does break old
//! strings: bump [`FLAGS_VERSION`] when you do.
//!
//! How to add an option:
//!
//! 1. Add the field to the module struct (with a doc comment and, for
//!    ranges, the allowed range) and to its `Default`.
//! 2. Append one line to that struct's `visit` (at the **end**).
//! 3. Add the widget in `crates/z2-launcher/src/rando_ui/<module>.rs`.
//!
//! The codec, round-trip tests and the spoiler's option listing pick the new
//! field up through `visit`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::rng::Rng;

/// Current flag-string version character.
pub const FLAGS_VERSION: char = '1';

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Flag-string parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagError {
    /// Empty input.
    Empty,
    /// The leading version character is not one this build reads.
    Version(char),
    /// A character outside the alphabet.
    BadChar(char),
    /// A field holds a value outside its range.
    BadValue {
        /// Module key.
        module: &'static str,
        /// Field name.
        field: &'static str,
        /// The raw stored value.
        raw: u32,
    },
    /// A section length is malformed.
    BadLength,
}

impl fmt::Display for FlagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlagError::Empty => write!(f, "the flag string is empty"),
            FlagError::Version(c) => write!(
                f,
                "flag string version '{c}' is not supported (this build reads '{FLAGS_VERSION}')"
            ),
            FlagError::BadChar(c) => write!(f, "'{c}' is not a flag-string character"),
            FlagError::BadValue { module, field, raw } => {
                write!(f, "{module}.{field} has an invalid value ({raw})")
            }
            FlagError::BadLength => write!(f, "the flag string is malformed (bad section length)"),
        }
    }
}

impl std::error::Error for FlagError {}

/// Bits needed to store values `0..count`.
#[must_use]
pub const fn bits_for(count: u32) -> u32 {
    if count <= 1 {
        1
    } else {
        32 - (count - 1).leading_zeros()
    }
}

/// A value that can live in the flag string.
pub trait FlagValue: Copy + PartialEq + fmt::Debug {
    /// Width in bits.
    fn bits() -> u32;
    /// Stored form (`< 2^bits`).
    fn to_raw(self) -> u32;
    /// Parse the stored form.
    fn from_raw(raw: u32) -> Option<Self>;
    /// Short human-readable value (spoiler, tooltips).
    fn describe(self) -> String {
        format!("{self:?}")
    }
    /// A uniformly random valid value (tests and "surprise me").
    fn random(rng: &mut Rng) -> Self;
}

impl FlagValue for bool {
    fn bits() -> u32 {
        1
    }
    fn to_raw(self) -> u32 {
        u32::from(self)
    }
    fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn describe(self) -> String {
        if self { "on" } else { "off" }.to_string()
    }
    fn random(rng: &mut Rng) -> Self {
        rng.coin()
    }
}

/// A multiple-choice option (every enum declared in this module), for
/// generic user-interface code.
pub trait FlagEnum: FlagValue + Eq + 'static {
    /// Every variant in flag order.
    fn all() -> &'static [Self];
    /// Label shown in user interfaces.
    fn label_of(self) -> &'static str;
}

/// Field visitor: one call per option, in flag-string order.
pub trait Visitor {
    /// A [`FlagValue`] field.
    fn value<T: FlagValue>(&mut self, name: &'static str, v: &mut T, default: T);
    /// A `u8` count limited to `lo..=hi`.
    fn range(&mut self, name: &'static str, v: &mut u8, lo: u8, hi: u8, default: u8);
}

/// A module's option struct.
pub trait FlagGroup: Default + Clone + PartialEq + fmt::Debug {
    /// Module key (`"start"`, `"palaces"`, ...).
    const KEY: &'static str;
    /// Visit every field in flag-string order. Append new fields at the end.
    fn visit<V: Visitor>(&mut self, v: &mut V);
}

macro_rules! flag_enum {
    (
        $(#[$m:meta])*
        $name:ident {
            $( $(#[$vm:meta])* $var:ident => $label:expr ),+ $(,)?
        }
        default $def:ident
    ) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$vm])* $var ),+
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$def
            }
        }

        impl $name {
            /// Every variant in flag order.
            pub const ALL: &'static [$name] = &[ $( $name::$var ),+ ];

            /// Label shown in user interfaces.
            #[must_use]
            pub fn label(self) -> &'static str {
                match self {
                    $( $name::$var => $label ),+
                }
            }

            /// Position in [`Self::ALL`].
            #[must_use]
            pub fn index(self) -> usize {
                self as usize
            }

            /// Variant at `i` in [`Self::ALL`].
            #[must_use]
            pub fn from_index(i: usize) -> Option<Self> {
                Self::ALL.get(i).copied()
            }
        }

        impl FlagEnum for $name {
            fn all() -> &'static [Self] {
                Self::ALL
            }
            fn label_of(self) -> &'static str {
                self.label()
            }
        }

        impl FlagValue for $name {
            fn bits() -> u32 {
                bits_for(Self::ALL.len() as u32)
            }
            fn to_raw(self) -> u32 {
                self as u32
            }
            fn from_raw(raw: u32) -> Option<Self> {
                Self::from_index(raw as usize)
            }
            fn describe(self) -> String {
                self.label().to_string()
            }
            fn random(rng: &mut Rng) -> Self {
                Self::ALL[rng.index(Self::ALL.len())]
            }
        }
    };
}

flag_enum! {
    /// Off / on / pick at random per seed.
    Tri {
        Off => "Off",
        On => "On",
        Random => "Random",
    }
    default Off
}

flag_enum! {
    /// Cap on how many starting items (or spells) a shuffle may add.
    StartLimit {
        NoLimit => "No limit",
        One => "1",
        Two => "2",
        Four => "4",
    }
    default NoLimit
}

flag_enum! {
    /// Total heart containers in the game.
    MaxHearts {
        One => "1", Two => "2", Three => "3", Four => "4",
        Five => "5", Six => "6", Seven => "7", Eight => "8",
        PlusOne => "Start +1", PlusTwo => "Start +2", PlusThree => "Start +3",
        PlusFour => "Start +4", Random => "Random",
    }
    default Eight
}

flag_enum! {
    /// Sword techniques Link starts with.
    StartingTechs {
        None => "None",
        Downstab => "Downstab",
        Upstab => "Upstab",
        Both => "Both",
        Random => "Random",
    }
    default None
}

flag_enum! {
    /// Lives at the start and after a game over.
    StartingLives {
        L1 => "1", L2 => "2", L3 => "3", L4 => "4", L5 => "5",
        L8 => "8", L16 => "16", Random => "Random (2-5)",
    }
    default L3
}

flag_enum! {
    /// Probability that a [`Tri::Random`] switch comes out on.
    RandomFlagRate {
        Quarter => "25%",
        Half => "50%",
        ThreeQuarters => "75%",
        NinetyPercent => "90%",
    }
    default Half
}

impl RandomFlagRate {
    /// `(numerator, denominator)` of the probability.
    #[must_use]
    pub fn ratio(self) -> (u64, u64) {
        match self {
            RandomFlagRate::Quarter => (1, 4),
            RandomFlagRate::Half => (1, 2),
            RandomFlagRate::ThreeQuarters => (3, 4),
            RandomFlagRate::NinetyPercent => (9, 10),
        }
    }
}

flag_enum! {
    /// West / East continent size.
    ContinentSize {
        Large => "Large",
        Medium => "Medium",
        Small => "Small",
    }
    default Large
}

flag_enum! {
    /// Death Mountain size (smaller sizes remove caves).
    DmSize {
        Large => "Large",
        Medium => "Medium",
        Small => "Small",
        Tiny => "Tiny",
    }
    default Large
}

flag_enum! {
    /// Maze Island size (smaller sizes remove trap tiles).
    MazeSize {
        Large => "Large",
        Medium => "Medium",
        Small => "Small",
    }
    default Large
}

flag_enum! {
    /// Continent shape generator.
    Biome {
        Vanilla => "Vanilla",
        VanillaShuffle => "Vanilla (shuffled locations)",
        Vanillalike => "Vanilla-like",
        Islands => "Islands",
        Canyon => "Canyon",
        DryCanyon => "Dry canyon",
        Mountainous => "Mountainous",
        Volcano => "Volcano",
        Caldera => "Caldera",
        RandomNoVanillaOrShuffle => "Random (no vanilla or shuffle)",
        RandomNoVanilla => "Random (no vanilla)",
        Random => "Random",
    }
    default Vanilla
}

flag_enum! {
    /// Terrain mix used to fill a generated continent.
    Climate {
        Classic => "Classic",
        VanillaWeighted => "Vanilla-weighted",
        Chaos => "Chaos",
        Wetlands => "Wetlands",
        GreatLakes => "Great lakes",
        Scrubland => "Scrubland",
        Random => "Random",
    }
    default VanillaWeighted
}

flag_enum! {
    /// How the continents connect.
    ContinentConnections {
        Normal => "Normal",
        TransportationShuffle => "Transportation shuffle",
        AnythingGoes => "Anything goes",
    }
    default Normal
}

flag_enum! {
    /// What happens to minor overworld tiles (jars, fairies, P-bags).
    LessImportantLocations {
        BlendIn => "Blend in",
        Isolate => "Isolate",
        Remove => "Remove",
        Random => "Random",
    }
    default BlendIn
}

flag_enum! {
    /// What the river devil blocks.
    RiverDevilBlocker {
        Path => "Path",
        Cave => "Cave",
        Siege => "Blocks a town",
        Random => "Random",
    }
    default Path
}

flag_enum! {
    /// Overworld encounter frequency.
    EncounterRate {
        None => "None",
        Half => "Half",
        Normal => "Normal",
        Random => "Random",
    }
    default Normal
}

flag_enum! {
    /// Palace layout generator.
    PalaceStyle {
        Vanilla => "Vanilla",
        Shuffled => "Vanilla shuffle",
        Sequential => "Sequential",
        RandomWalk => "Random walk",
        VanillaWeighted => "Vanilla-weighted",
        Tower => "Tower",
        Mirror => "Mirror",
        Reconstructed => "Reconstructed",
        ReconstructedLoopy => "Loopy",
        Chaos => "Chaos",
        Random => "Random",
        RandomAll => "Random (same for all)",
        RandomPerPalace => "Random (per palace)",
    }
    default Vanilla
}

flag_enum! {
    /// Palace length multiplier.
    PalaceLength {
        Short => "Short",
        Medium => "Medium",
        Full => "Full",
        Random => "Random",
    }
    default Full
}

flag_enum! {
    /// Where boss rooms lead.
    BossRoomsExit {
        Overworld => "Overworld",
        Palace => "More palace",
        RandomAll => "Random (same for all)",
        RandomPerPalace => "Random (per palace)",
    }
    default Overworld
}

flag_enum! {
    /// Minimum rooms from the Great Palace entrance to Dark Link.
    DarkLinkDistance {
        None => "None",
        Short => "Short (8 rooms)",
        Medium => "Medium (12 rooms)",
        Max => "Max",
    }
    default None
}

flag_enum! {
    /// Item rooms per palace.
    ItemRoomCount {
        Zero => "0",
        One => "1",
        Two => "2",
        Random => "Random (1+)",
        RandomIncludeZero => "Random (0+)",
    }
    default One
}

flag_enum! {
    /// Where palace drops (holes) must eventually lead.
    PalaceDropStyle {
        Entrance => "Entrance",
        AnyExit => "Entrance or boss exit",
        Balanced => "Balanced",
        AnythingGoes => "Anything",
    }
    default AnyExit
}

flag_enum! {
    /// Sword damage table.
    AttackEffectiveness {
        Vanilla => "Vanilla",
        Low => "Low",
        AverageLow => "Average (low)",
        Average => "Average",
        AverageHigh => "Average (high)",
        High => "High",
        Ohko => "One hit kills",
    }
    default Vanilla
}

flag_enum! {
    /// Spell costs.
    MagicEffectiveness {
        Vanilla => "Vanilla",
        HighCost => "High cost",
        AverageHighCost => "Average (high cost)",
        Average => "Average",
        AverageLowCost => "Average (low cost)",
        LowCost => "Low cost",
        Free => "Free",
    }
    default Vanilla
}

flag_enum! {
    /// Damage Link takes per life level.
    LifeEffectiveness {
        Vanilla => "Vanilla",
        Ohko => "One hit kills Link",
        AverageLow => "Average (more damage)",
        Average => "Average",
        AverageHigh => "Average (less damage)",
        High => "High",
        Invincible => "Invincible",
    }
    default Vanilla
}

flag_enum! {
    /// Shift of enemy experience along the experience ladder.
    XpEffectiveness {
        Vanilla => "Vanilla",
        RandomLow => "Random (low)",
        Random => "Random",
        LowVariance => "Low variance",
        SlightlyHigh => "Slightly high",
        RandomHigh => "Random (high)",
        Wide => "Wide",
        None => "None (no experience)",
    }
    default Vanilla
}

flag_enum! {
    /// Enemy hit point scaling.
    EnemyLife {
        Vanilla => "Vanilla",
        Narrow => "Narrow (x0.75-1.25)",
        Medium => "Medium (x0.5-1.5)",
        Wide => "Wide (x0.25-3.0)",
        MediumHigh => "Medium high (x0.5-2.0)",
        High => "High (x1.0-2.0)",
    }
    default Vanilla
}

flag_enum! {
    /// What the palace drippers spawn.
    DripperEnemy {
        OnlyBots => "Only Bots",
        AnyGroundEnemy => "Any ground enemy",
        EasierGroundEnemies => "Easier ground enemies",
        EasierGroundEnemiesFullHp => "Easier ground enemies (full HP)",
    }
    default OnlyBots
}

flag_enum! {
    /// Which enemies need Fire to be hurt.
    SwordImmunity {
        Vanilla => "Vanilla",
        Shuffle => "Shuffle",
        ShuffleConditional => "Shuffle (none if Fire is unusable)",
        None => "None",
    }
    default Vanilla
}

flag_enum! {
    /// The Fire spell.
    FireOption {
        Normal => "Normal",
        PairWithRandom => "Linked with another spell",
        ReplaceWithDash => "Replaced by Dash",
        Random => "Random",
    }
    default Normal
}

flag_enum! {
    /// Flute warp destinations.
    FluteWarp {
        None => "Off",
        ClearedPalaces => "Cleared palaces",
        VisitedPalaces => "Visited palaces",
        VisitedTowns => "Visited towns",
    }
    default None
}

flag_enum! {
    /// Helpful NPC hints.
    HelpfulHints {
        None => "None",
        ContinentOnly => "By continent",
        TownsSeparate => "Towns separate",
    }
    default None
}

flag_enum! {
    /// Low-health beep threshold.
    BeepThreshold {
        Normal => "Normal",
        HalfBar => "Half a bar",
        QuarterBar => "Quarter bar",
        TwoBars => "Two bars",
    }
    default Normal
}

flag_enum! {
    /// Low-health beep speed.
    BeepFrequency {
        Normal => "Normal",
        HalfSpeed => "Half speed",
        QuarterSpeed => "Quarter speed",
        Off => "Off",
    }
    default Normal
}

flag_enum! {
    /// Sword beam graphic.
    BeamSprite {
        Default => "Default",
        Fire => "Fire",
        Bubble => "Bubble",
        Rock => "Rock",
        EnergyBall => "Energy ball",
        WizardBeam => "Wizard beam",
        Axe => "Axe",
        Hammer => "Mace",
        GeruMace => "Geru mace",
        GumaMace => "Guma mace",
        Boomerang => "Boomerang",
        SpicyChicken => "Spicy chicken",
        Random => "Random",
    }
    default Default
}

/// A palette choice: leave the ROM colour, pick one per seed, or an NES
/// palette index `$00-$3F`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum NesColor {
    /// Keep the colour the ROM has.
    #[default]
    Default,
    /// Pick a colour per seed.
    Random,
    /// This NES palette index (`$00-$3F`).
    Color(u8),
}

impl FlagValue for NesColor {
    fn bits() -> u32 {
        7
    }
    fn to_raw(self) -> u32 {
        match self {
            NesColor::Default => 0,
            NesColor::Random => 1,
            NesColor::Color(c) => 2 + u32::from(c & 0x3F),
        }
    }
    fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(NesColor::Default),
            1 => Some(NesColor::Random),
            2..=65 => Some(NesColor::Color((raw - 2) as u8)),
            _ => None,
        }
    }
    fn describe(self) -> String {
        match self {
            NesColor::Default => "Default".into(),
            NesColor::Random => "Random".into(),
            NesColor::Color(c) => format!("${c:02X}"),
        }
    }
    fn random(rng: &mut Rng) -> Self {
        Self::from_raw(rng.below(66) as u32).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Module option structs.
// ---------------------------------------------------------------------------

/// Starting inventory, stats and global switches (`start` module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartFlags {
    /// Each unchecked item gets a 25% chance to be added to the start.
    pub shuffle_starting_items: bool,
    /// Start with the candle.
    pub start_with_candle: bool,
    /// Start with the glove.
    pub start_with_glove: bool,
    /// Start with the raft.
    pub start_with_raft: bool,
    /// Start with the boots.
    pub start_with_boots: bool,
    /// Start with the flute.
    pub start_with_flute: bool,
    /// Start with the cross.
    pub start_with_cross: bool,
    /// Start with the hammer.
    pub start_with_hammer: bool,
    /// Start with the magic key.
    pub start_with_magic_key: bool,
    /// Cap on starting items.
    pub start_items_limit: StartLimit,
    /// Each unchecked spell gets a 25% chance to be known at the start.
    pub shuffle_starting_spells: bool,
    /// Start with Shield.
    pub start_with_shield: bool,
    /// Start with Jump.
    pub start_with_jump: bool,
    /// Start with Life.
    pub start_with_life: bool,
    /// Start with Fairy.
    pub start_with_fairy: bool,
    /// Start with Fire (or Dash when Fire is replaced).
    pub start_with_fire: bool,
    /// Start with Reflect.
    pub start_with_reflect: bool,
    /// Start with Spell.
    pub start_with_spell: bool,
    /// Start with Thunder.
    pub start_with_thunder: bool,
    /// Cap on starting spells.
    pub start_spells_limit: StartLimit,
    /// Lower bound of starting heart containers (1-8).
    pub heart_containers_min: u8,
    /// Upper bound of starting heart containers (1-8).
    pub heart_containers_max: u8,
    /// Lower bound of starting magic containers (1-8).
    pub magic_containers_min: u8,
    /// Upper bound of starting magic containers (1-8).
    pub magic_containers_max: u8,
    /// Total heart containers in the game.
    pub max_heart_containers: MaxHearts,
    /// Starting sword techniques.
    pub starting_techs: StartingTechs,
    /// Starting lives.
    pub starting_lives: StartingLives,
    /// Starting attack level (1-8).
    pub attack_level: u8,
    /// Starting magic level (1-8).
    pub magic_level: u8,
    /// Starting life level (1-8).
    pub life_level: u8,
    /// Probability that a "Random" switch comes out on.
    pub random_flag_rate: RandomFlagRate,
    /// Let racers share one base seed while picking different difficulty
    /// options (only affects the hash code and difficulty-only rolls).
    pub share_seed_across_difficulty: bool,
}

impl Default for StartFlags {
    fn default() -> Self {
        StartFlags {
            shuffle_starting_items: false,
            start_with_candle: false,
            start_with_glove: false,
            start_with_raft: false,
            start_with_boots: false,
            start_with_flute: false,
            start_with_cross: false,
            start_with_hammer: false,
            start_with_magic_key: false,
            start_items_limit: StartLimit::NoLimit,
            shuffle_starting_spells: false,
            start_with_shield: false,
            start_with_jump: false,
            start_with_life: false,
            start_with_fairy: false,
            start_with_fire: false,
            start_with_reflect: false,
            start_with_spell: false,
            start_with_thunder: false,
            start_spells_limit: StartLimit::NoLimit,
            heart_containers_min: 4,
            heart_containers_max: 4,
            magic_containers_min: 4,
            magic_containers_max: 4,
            max_heart_containers: MaxHearts::Eight,
            starting_techs: StartingTechs::None,
            starting_lives: StartingLives::L3,
            attack_level: 1,
            magic_level: 1,
            life_level: 1,
            random_flag_rate: RandomFlagRate::Half,
            share_seed_across_difficulty: false,
        }
    }
}

impl FlagGroup for StartFlags {
    const KEY: &'static str = "start";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "shuffle_starting_items",
            &mut self.shuffle_starting_items,
            d.shuffle_starting_items,
        );
        v.value(
            "start_with_candle",
            &mut self.start_with_candle,
            d.start_with_candle,
        );
        v.value(
            "start_with_glove",
            &mut self.start_with_glove,
            d.start_with_glove,
        );
        v.value(
            "start_with_raft",
            &mut self.start_with_raft,
            d.start_with_raft,
        );
        v.value(
            "start_with_boots",
            &mut self.start_with_boots,
            d.start_with_boots,
        );
        v.value(
            "start_with_flute",
            &mut self.start_with_flute,
            d.start_with_flute,
        );
        v.value(
            "start_with_cross",
            &mut self.start_with_cross,
            d.start_with_cross,
        );
        v.value(
            "start_with_hammer",
            &mut self.start_with_hammer,
            d.start_with_hammer,
        );
        v.value(
            "start_with_magic_key",
            &mut self.start_with_magic_key,
            d.start_with_magic_key,
        );
        v.value(
            "start_items_limit",
            &mut self.start_items_limit,
            d.start_items_limit,
        );
        v.value(
            "shuffle_starting_spells",
            &mut self.shuffle_starting_spells,
            d.shuffle_starting_spells,
        );
        v.value(
            "start_with_shield",
            &mut self.start_with_shield,
            d.start_with_shield,
        );
        v.value(
            "start_with_jump",
            &mut self.start_with_jump,
            d.start_with_jump,
        );
        v.value(
            "start_with_life",
            &mut self.start_with_life,
            d.start_with_life,
        );
        v.value(
            "start_with_fairy",
            &mut self.start_with_fairy,
            d.start_with_fairy,
        );
        v.value(
            "start_with_fire",
            &mut self.start_with_fire,
            d.start_with_fire,
        );
        v.value(
            "start_with_reflect",
            &mut self.start_with_reflect,
            d.start_with_reflect,
        );
        v.value(
            "start_with_spell",
            &mut self.start_with_spell,
            d.start_with_spell,
        );
        v.value(
            "start_with_thunder",
            &mut self.start_with_thunder,
            d.start_with_thunder,
        );
        v.value(
            "start_spells_limit",
            &mut self.start_spells_limit,
            d.start_spells_limit,
        );
        v.range(
            "heart_containers_min",
            &mut self.heart_containers_min,
            1,
            8,
            d.heart_containers_min,
        );
        v.range(
            "heart_containers_max",
            &mut self.heart_containers_max,
            1,
            8,
            d.heart_containers_max,
        );
        v.range(
            "magic_containers_min",
            &mut self.magic_containers_min,
            1,
            8,
            d.magic_containers_min,
        );
        v.range(
            "magic_containers_max",
            &mut self.magic_containers_max,
            1,
            8,
            d.magic_containers_max,
        );
        v.value(
            "max_heart_containers",
            &mut self.max_heart_containers,
            d.max_heart_containers,
        );
        v.value("starting_techs", &mut self.starting_techs, d.starting_techs);
        v.value("starting_lives", &mut self.starting_lives, d.starting_lives);
        v.range("attack_level", &mut self.attack_level, 1, 8, d.attack_level);
        v.range("magic_level", &mut self.magic_level, 1, 8, d.magic_level);
        v.range("life_level", &mut self.life_level, 1, 8, d.life_level);
        v.value(
            "random_flag_rate",
            &mut self.random_flag_rate,
            d.random_flag_rate,
        );
        v.value(
            "share_seed_across_difficulty",
            &mut self.share_seed_across_difficulty,
            d.share_seed_across_difficulty,
        );
    }
}

/// Overworld shape and locations (`overworld` module; upstream Biomes and
/// Overworld tabs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverworldFlags {
    /// West Hyrule size.
    pub west_size: ContinentSize,
    /// East Hyrule size.
    pub east_size: ContinentSize,
    /// Death Mountain size.
    pub dm_size: DmSize,
    /// Maze Island size.
    pub maze_size: MazeSize,
    /// West Hyrule shape.
    pub west_biome: Biome,
    /// East Hyrule shape (Volcano is East-only; Caldera is not offered).
    pub east_biome: Biome,
    /// Death Mountain shape.
    pub dm_biome: Biome,
    /// Maze Island shape (Vanilla, Vanilla shuffle, Vanilla-like, Random).
    pub maze_biome: Biome,
    /// West Hyrule terrain mix.
    pub west_climate: Climate,
    /// East Hyrule terrain mix.
    pub east_climate: Climate,
    /// Death Mountain terrain mix (Vanilla-weighted is not offered).
    pub dm_climate: Climate,
    /// Boots walk on any water, not only marked paths.
    pub good_boots: Tri,
    /// Vanilla-shuffled maps keep their original location icons.
    pub legacy_vanilla_shuffled_locations: bool,
    /// How the continents connect.
    pub continent_connections: ContinentConnections,
    /// Minor overworld tiles.
    pub less_important_locations: LessImportantLocations,
    /// Generate Bagu's woods (only with a generated West map).
    pub generate_bagu_woods: Tri,
    /// Connection caves lead where they point ("sane caves").
    pub restrict_connection_cave_shuffle: Tri,
    /// Rocks may block connection caves.
    pub allow_connection_caves_blocked: bool,
    /// What the river devil blocks.
    pub river_devil_blocker: RiverDevilBlocker,
    /// East path blocks may be rocks.
    pub east_rocks: Tri,
    /// A location hidden under the three-eyed rock (vanilla: on).
    pub hide_palace: Tri,
    /// New Kasuto hidden under a forest tile (vanilla: on).
    pub hide_kasuto: Tri,
    /// Any East location may be the hidden one.
    pub shuffle_hidden_locations: Tri,
    /// Palaces 1-6 may move between continents.
    pub palaces_swap_continents: Tri,
    /// The Great Palace joins the palace shuffle.
    pub shuffle_great_palace: Tri,
    /// Overworld encounter rate.
    pub encounter_rate: EncounterRate,
    /// Shuffle which encounter sits on which terrain.
    pub shuffle_encounters: Tri,
    /// Roads join the encounter shuffle.
    pub allow_unsafe_path_encounters: bool,
    /// Lava joins the encounter shuffle.
    pub include_lava_in_encounter_shuffle: bool,
}

impl Default for OverworldFlags {
    fn default() -> Self {
        OverworldFlags {
            west_size: ContinentSize::Large,
            east_size: ContinentSize::Large,
            dm_size: DmSize::Large,
            maze_size: MazeSize::Large,
            west_biome: Biome::Vanilla,
            east_biome: Biome::Vanilla,
            dm_biome: Biome::Vanilla,
            maze_biome: Biome::Vanilla,
            west_climate: Climate::VanillaWeighted,
            east_climate: Climate::VanillaWeighted,
            dm_climate: Climate::Classic,
            good_boots: Tri::Off,
            legacy_vanilla_shuffled_locations: false,
            continent_connections: ContinentConnections::Normal,
            less_important_locations: LessImportantLocations::BlendIn,
            generate_bagu_woods: Tri::On,
            restrict_connection_cave_shuffle: Tri::On,
            allow_connection_caves_blocked: false,
            river_devil_blocker: RiverDevilBlocker::Path,
            east_rocks: Tri::On,
            hide_palace: Tri::On,
            hide_kasuto: Tri::On,
            shuffle_hidden_locations: Tri::Off,
            palaces_swap_continents: Tri::Off,
            shuffle_great_palace: Tri::Off,
            encounter_rate: EncounterRate::Normal,
            shuffle_encounters: Tri::Off,
            allow_unsafe_path_encounters: false,
            include_lava_in_encounter_shuffle: false,
        }
    }
}

impl FlagGroup for OverworldFlags {
    const KEY: &'static str = "overworld";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value("west_size", &mut self.west_size, d.west_size);
        v.value("east_size", &mut self.east_size, d.east_size);
        v.value("dm_size", &mut self.dm_size, d.dm_size);
        v.value("maze_size", &mut self.maze_size, d.maze_size);
        v.value("west_biome", &mut self.west_biome, d.west_biome);
        v.value("east_biome", &mut self.east_biome, d.east_biome);
        v.value("dm_biome", &mut self.dm_biome, d.dm_biome);
        v.value("maze_biome", &mut self.maze_biome, d.maze_biome);
        v.value("west_climate", &mut self.west_climate, d.west_climate);
        v.value("east_climate", &mut self.east_climate, d.east_climate);
        v.value("dm_climate", &mut self.dm_climate, d.dm_climate);
        v.value("good_boots", &mut self.good_boots, d.good_boots);
        v.value(
            "legacy_vanilla_shuffled_locations",
            &mut self.legacy_vanilla_shuffled_locations,
            d.legacy_vanilla_shuffled_locations,
        );
        v.value(
            "continent_connections",
            &mut self.continent_connections,
            d.continent_connections,
        );
        v.value(
            "less_important_locations",
            &mut self.less_important_locations,
            d.less_important_locations,
        );
        v.value(
            "generate_bagu_woods",
            &mut self.generate_bagu_woods,
            d.generate_bagu_woods,
        );
        v.value(
            "restrict_connection_cave_shuffle",
            &mut self.restrict_connection_cave_shuffle,
            d.restrict_connection_cave_shuffle,
        );
        v.value(
            "allow_connection_caves_blocked",
            &mut self.allow_connection_caves_blocked,
            d.allow_connection_caves_blocked,
        );
        v.value(
            "river_devil_blocker",
            &mut self.river_devil_blocker,
            d.river_devil_blocker,
        );
        v.value("east_rocks", &mut self.east_rocks, d.east_rocks);
        v.value("hide_palace", &mut self.hide_palace, d.hide_palace);
        v.value("hide_kasuto", &mut self.hide_kasuto, d.hide_kasuto);
        v.value(
            "shuffle_hidden_locations",
            &mut self.shuffle_hidden_locations,
            d.shuffle_hidden_locations,
        );
        v.value(
            "palaces_swap_continents",
            &mut self.palaces_swap_continents,
            d.palaces_swap_continents,
        );
        v.value(
            "shuffle_great_palace",
            &mut self.shuffle_great_palace,
            d.shuffle_great_palace,
        );
        v.value("encounter_rate", &mut self.encounter_rate, d.encounter_rate);
        v.value(
            "shuffle_encounters",
            &mut self.shuffle_encounters,
            d.shuffle_encounters,
        );
        v.value(
            "allow_unsafe_path_encounters",
            &mut self.allow_unsafe_path_encounters,
            d.allow_unsafe_path_encounters,
        );
        v.value(
            "include_lava_in_encounter_shuffle",
            &mut self.include_lava_in_encounter_shuffle,
            d.include_lava_in_encounter_shuffle,
        );
    }
}

/// Palace generation and palace-related options (`palaces` module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PalaceFlags {
    /// Layout generator for palaces 1-6.
    pub normal_style: PalaceStyle,
    /// Layout generator for the Great Palace (no Vanilla-weighted, Mirror or
    /// the two "Random (...)" variants).
    pub gp_style: PalaceStyle,
    /// Length of palaces 1-6.
    pub normal_length: PalaceLength,
    /// Length of the Great Palace.
    pub gp_length: PalaceLength,
    /// Where boss rooms lead.
    pub boss_rooms_exit: BossRoomsExit,
    /// Minimum path to Dark Link.
    pub dark_link_min_distance: DarkLinkDistance,
    /// Item rooms per palace.
    pub item_rooms_per_palace: ItemRoomCount,
    /// Where drops must connect to.
    pub drop_style: PalaceDropStyle,
    /// Palaces to complete before the Great Palace opens: lower bound (0-6).
    pub palaces_to_complete_min: u8,
    /// Palaces to complete: upper bound (0-6).
    pub palaces_to_complete_max: u8,
    /// Random styles may pick Vanilla / Vanilla shuffle.
    pub random_styles_allow_vanilla: bool,
    /// No two rooms with the same layout.
    pub no_duplicate_rooms_by_layout: bool,
    /// No two rooms with the same layout and enemies.
    pub no_duplicate_rooms_by_enemies: bool,
    /// Game over (and Up+A) restarts at the palace entrance.
    pub restart_at_palaces_on_game_over: bool,
    /// 50/50 statues alternate red jar and red Iron Knuckle.
    pub global_5050_jar_drop: Tri,
    /// Blue drip guaranteed after a streak of reds.
    pub reduce_dripper_variance: bool,
    /// Use the original rooms in generated palaces.
    pub include_vanilla_rooms: Tri,
    /// Use extra room set A (a user-supplied room pack; unavailable without
    /// one, since no third-party room data ships with z2rs).
    pub include_extra_rooms_a: Tri,
    /// Use extra room set B (user-supplied, see above).
    pub include_extra_rooms_b: Tri,
    /// Item/spell-gated rooms may appear in any palace.
    pub blocking_rooms_in_any_palace: bool,
    /// Drop long dead-end rooms.
    pub remove_long_dead_ends: bool,
    /// Harder rooms in the pool.
    pub include_expert_rooms: bool,
    /// Bosses drop a random small item instead of a key.
    pub randomize_boss_item_drop: bool,
    /// Harder Carock.
    pub hard_bosses: bool,
    /// No Thunderbird in the Great Palace.
    pub remove_thunderbird: bool,
    /// The path to Dark Link goes through Thunderbird (vanilla: on).
    pub thunderbird_required: Tri,
    /// Thunderbird starts in its aggressive phase.
    pub aggressive_thunderbird: bool,
    /// Random palace colour schemes.
    pub change_palace_palettes: bool,
}

impl Default for PalaceFlags {
    fn default() -> Self {
        PalaceFlags {
            normal_style: PalaceStyle::Vanilla,
            gp_style: PalaceStyle::Vanilla,
            normal_length: PalaceLength::Full,
            gp_length: PalaceLength::Full,
            boss_rooms_exit: BossRoomsExit::Overworld,
            dark_link_min_distance: DarkLinkDistance::None,
            item_rooms_per_palace: ItemRoomCount::One,
            drop_style: PalaceDropStyle::AnyExit,
            palaces_to_complete_min: 6,
            palaces_to_complete_max: 6,
            random_styles_allow_vanilla: false,
            no_duplicate_rooms_by_layout: false,
            no_duplicate_rooms_by_enemies: false,
            restart_at_palaces_on_game_over: false,
            global_5050_jar_drop: Tri::Off,
            reduce_dripper_variance: false,
            include_vanilla_rooms: Tri::On,
            include_extra_rooms_a: Tri::Off,
            include_extra_rooms_b: Tri::Off,
            blocking_rooms_in_any_palace: false,
            remove_long_dead_ends: false,
            include_expert_rooms: false,
            randomize_boss_item_drop: false,
            hard_bosses: false,
            remove_thunderbird: false,
            thunderbird_required: Tri::On,
            aggressive_thunderbird: false,
            change_palace_palettes: false,
        }
    }
}

impl FlagGroup for PalaceFlags {
    const KEY: &'static str = "palaces";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value("normal_style", &mut self.normal_style, d.normal_style);
        v.value("gp_style", &mut self.gp_style, d.gp_style);
        v.value("normal_length", &mut self.normal_length, d.normal_length);
        v.value("gp_length", &mut self.gp_length, d.gp_length);
        v.value(
            "boss_rooms_exit",
            &mut self.boss_rooms_exit,
            d.boss_rooms_exit,
        );
        v.value(
            "dark_link_min_distance",
            &mut self.dark_link_min_distance,
            d.dark_link_min_distance,
        );
        v.value(
            "item_rooms_per_palace",
            &mut self.item_rooms_per_palace,
            d.item_rooms_per_palace,
        );
        v.value("drop_style", &mut self.drop_style, d.drop_style);
        v.range(
            "palaces_to_complete_min",
            &mut self.palaces_to_complete_min,
            0,
            6,
            d.palaces_to_complete_min,
        );
        v.range(
            "palaces_to_complete_max",
            &mut self.palaces_to_complete_max,
            0,
            6,
            d.palaces_to_complete_max,
        );
        v.value(
            "random_styles_allow_vanilla",
            &mut self.random_styles_allow_vanilla,
            d.random_styles_allow_vanilla,
        );
        v.value(
            "no_duplicate_rooms_by_layout",
            &mut self.no_duplicate_rooms_by_layout,
            d.no_duplicate_rooms_by_layout,
        );
        v.value(
            "no_duplicate_rooms_by_enemies",
            &mut self.no_duplicate_rooms_by_enemies,
            d.no_duplicate_rooms_by_enemies,
        );
        v.value(
            "restart_at_palaces_on_game_over",
            &mut self.restart_at_palaces_on_game_over,
            d.restart_at_palaces_on_game_over,
        );
        v.value(
            "global_5050_jar_drop",
            &mut self.global_5050_jar_drop,
            d.global_5050_jar_drop,
        );
        v.value(
            "reduce_dripper_variance",
            &mut self.reduce_dripper_variance,
            d.reduce_dripper_variance,
        );
        v.value(
            "include_vanilla_rooms",
            &mut self.include_vanilla_rooms,
            d.include_vanilla_rooms,
        );
        v.value(
            "include_extra_rooms_a",
            &mut self.include_extra_rooms_a,
            d.include_extra_rooms_a,
        );
        v.value(
            "include_extra_rooms_b",
            &mut self.include_extra_rooms_b,
            d.include_extra_rooms_b,
        );
        v.value(
            "blocking_rooms_in_any_palace",
            &mut self.blocking_rooms_in_any_palace,
            d.blocking_rooms_in_any_palace,
        );
        v.value(
            "remove_long_dead_ends",
            &mut self.remove_long_dead_ends,
            d.remove_long_dead_ends,
        );
        v.value(
            "include_expert_rooms",
            &mut self.include_expert_rooms,
            d.include_expert_rooms,
        );
        v.value(
            "randomize_boss_item_drop",
            &mut self.randomize_boss_item_drop,
            d.randomize_boss_item_drop,
        );
        v.value("hard_bosses", &mut self.hard_bosses, d.hard_bosses);
        v.value(
            "remove_thunderbird",
            &mut self.remove_thunderbird,
            d.remove_thunderbird,
        );
        v.value(
            "thunderbird_required",
            &mut self.thunderbird_required,
            d.thunderbird_required,
        );
        v.value(
            "aggressive_thunderbird",
            &mut self.aggressive_thunderbird,
            d.aggressive_thunderbird,
        );
        v.value(
            "change_palace_palettes",
            &mut self.change_palace_palettes,
            d.change_palace_palettes,
        );
    }
}

/// Item placement (`items` module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ItemFlags {
    /// Palace items swap palaces.
    pub shuffle_palace_items: Tri,
    /// Overworld items shuffle among themselves.
    pub shuffle_overworld_items: Tri,
    /// Any item anywhere (needs both shuffles).
    pub mix_overworld_and_palace_items: Tri,
    /// The three P-bag caves become item locations.
    pub include_pbag_caves: Tri,
    /// Wizards give items; spells join the item pool.
    pub include_spells: Tri,
    /// Stab teachers give items; stabs join the pool.
    pub include_sword_techs: Tri,
    /// Bagu's note, mirror and water join the pool.
    pub include_quest_items: Tri,
    /// A spell quest item is never needed to get another town item.
    pub prevent_spell_item_chains: bool,
    /// Each small item becomes a random small item.
    pub shuffle_small_items: bool,
    /// Start with the trophy, medicine, child, water and mirror.
    pub start_with_spell_items: Tri,
    /// P-bag experience values shift +/-2 steps.
    pub shuffle_pbag_amounts: Tri,
    /// All palace small items become keys.
    pub palaces_contain_extra_keys: Tri,
    /// Copies of key items may replace minor items.
    pub allow_important_item_duplicates: bool,
}

impl Default for ItemFlags {
    fn default() -> Self {
        ItemFlags {
            shuffle_palace_items: Tri::Off,
            shuffle_overworld_items: Tri::Off,
            mix_overworld_and_palace_items: Tri::Off,
            include_pbag_caves: Tri::Off,
            include_spells: Tri::Off,
            include_sword_techs: Tri::Off,
            include_quest_items: Tri::Off,
            prevent_spell_item_chains: false,
            shuffle_small_items: false,
            start_with_spell_items: Tri::Off,
            shuffle_pbag_amounts: Tri::Off,
            palaces_contain_extra_keys: Tri::Off,
            allow_important_item_duplicates: false,
        }
    }
}

impl FlagGroup for ItemFlags {
    const KEY: &'static str = "items";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "shuffle_palace_items",
            &mut self.shuffle_palace_items,
            d.shuffle_palace_items,
        );
        v.value(
            "shuffle_overworld_items",
            &mut self.shuffle_overworld_items,
            d.shuffle_overworld_items,
        );
        v.value(
            "mix_overworld_and_palace_items",
            &mut self.mix_overworld_and_palace_items,
            d.mix_overworld_and_palace_items,
        );
        v.value(
            "include_pbag_caves",
            &mut self.include_pbag_caves,
            d.include_pbag_caves,
        );
        v.value("include_spells", &mut self.include_spells, d.include_spells);
        v.value(
            "include_sword_techs",
            &mut self.include_sword_techs,
            d.include_sword_techs,
        );
        v.value(
            "include_quest_items",
            &mut self.include_quest_items,
            d.include_quest_items,
        );
        v.value(
            "prevent_spell_item_chains",
            &mut self.prevent_spell_item_chains,
            d.prevent_spell_item_chains,
        );
        v.value(
            "shuffle_small_items",
            &mut self.shuffle_small_items,
            d.shuffle_small_items,
        );
        v.value(
            "start_with_spell_items",
            &mut self.start_with_spell_items,
            d.start_with_spell_items,
        );
        v.value(
            "shuffle_pbag_amounts",
            &mut self.shuffle_pbag_amounts,
            d.shuffle_pbag_amounts,
        );
        v.value(
            "palaces_contain_extra_keys",
            &mut self.palaces_contain_extra_keys,
            d.palaces_contain_extra_keys,
        );
        v.value(
            "allow_important_item_duplicates",
            &mut self.allow_important_item_duplicates,
            d.allow_important_item_duplicates,
        );
    }
}

/// Enemy placement and enemy stats (`enemies` module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnemyFlags {
    /// Encounter enemies shuffled within their continent.
    pub shuffle_overworld_enemies: Tri,
    /// Room enemies shuffled within palace groups.
    pub shuffle_palace_enemies: Tri,
    /// Size classes may swap (flyers stay flyers).
    pub mix_large_and_small: Tri,
    /// Left/right spawners in a room spawn the same enemy.
    pub generators_always_match: bool,
    /// What drippers spawn.
    pub dripper_enemy: DripperEnemy,
    /// Regular enemy HP.
    pub enemy_hp: EnemyLife,
    /// Boss HP (capped at 255; Great Palace bosses excluded; no Wide).
    pub boss_hp: EnemyLife,
    /// Reassign which enemies steal experience.
    pub shuffle_xp_stealers: bool,
    /// Stolen experience 50-150% of vanilla.
    pub shuffle_xp_stolen_amount: bool,
    /// Which enemies need Fire.
    pub sword_immunity: SwordImmunity,
    /// Enemy experience drops.
    pub xp_drops: XpEffectiveness,
    /// Chaotic knockback.
    pub randomize_knockback: bool,
}

impl Default for EnemyFlags {
    fn default() -> Self {
        EnemyFlags {
            shuffle_overworld_enemies: Tri::Off,
            shuffle_palace_enemies: Tri::Off,
            mix_large_and_small: Tri::Off,
            generators_always_match: true,
            dripper_enemy: DripperEnemy::OnlyBots,
            enemy_hp: EnemyLife::Vanilla,
            boss_hp: EnemyLife::Vanilla,
            shuffle_xp_stealers: false,
            shuffle_xp_stolen_amount: false,
            sword_immunity: SwordImmunity::Vanilla,
            xp_drops: XpEffectiveness::Vanilla,
            randomize_knockback: false,
        }
    }
}

impl FlagGroup for EnemyFlags {
    const KEY: &'static str = "enemies";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "shuffle_overworld_enemies",
            &mut self.shuffle_overworld_enemies,
            d.shuffle_overworld_enemies,
        );
        v.value(
            "shuffle_palace_enemies",
            &mut self.shuffle_palace_enemies,
            d.shuffle_palace_enemies,
        );
        v.value(
            "mix_large_and_small",
            &mut self.mix_large_and_small,
            d.mix_large_and_small,
        );
        v.value(
            "generators_always_match",
            &mut self.generators_always_match,
            d.generators_always_match,
        );
        v.value("dripper_enemy", &mut self.dripper_enemy, d.dripper_enemy);
        v.value("enemy_hp", &mut self.enemy_hp, d.enemy_hp);
        v.value("boss_hp", &mut self.boss_hp, d.boss_hp);
        v.value(
            "shuffle_xp_stealers",
            &mut self.shuffle_xp_stealers,
            d.shuffle_xp_stealers,
        );
        v.value(
            "shuffle_xp_stolen_amount",
            &mut self.shuffle_xp_stolen_amount,
            d.shuffle_xp_stolen_amount,
        );
        v.value("sword_immunity", &mut self.sword_immunity, d.sword_immunity);
        v.value("xp_drops", &mut self.xp_drops, d.xp_drops);
        v.value(
            "randomize_knockback",
            &mut self.randomize_knockback,
            d.randomize_knockback,
        );
    }
}

/// Experience, levels and effectiveness tables (`stats` module; upstream
/// Levels tab).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsFlags {
    /// Attack level-up costs +/-25%.
    pub shuffle_attack_exp: bool,
    /// Magic level-up costs +/-25%.
    pub shuffle_magic_exp: bool,
    /// Life level-up costs +/-25%.
    pub shuffle_life_exp: bool,
    /// Attack level cap (1-8); levels past it become 1-ups.
    pub attack_level_cap: u8,
    /// Magic level cap (1-8).
    pub magic_level_cap: u8,
    /// Life level cap (1-8).
    pub life_level_cap: u8,
    /// Scale experience costs to the reduced caps.
    pub scale_level_requirements_to_cap: bool,
    /// Sword damage.
    pub attack_effectiveness: AttackEffectiveness,
    /// Spell costs.
    pub magic_effectiveness: MagicEffectiveness,
    /// Damage taken.
    pub life_effectiveness: LifeEffectiveness,
}

impl Default for StatsFlags {
    fn default() -> Self {
        StatsFlags {
            shuffle_attack_exp: false,
            shuffle_magic_exp: false,
            shuffle_life_exp: false,
            attack_level_cap: 8,
            magic_level_cap: 8,
            life_level_cap: 8,
            scale_level_requirements_to_cap: false,
            attack_effectiveness: AttackEffectiveness::Vanilla,
            magic_effectiveness: MagicEffectiveness::Vanilla,
            life_effectiveness: LifeEffectiveness::Vanilla,
        }
    }
}

impl FlagGroup for StatsFlags {
    const KEY: &'static str = "stats";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "shuffle_attack_exp",
            &mut self.shuffle_attack_exp,
            d.shuffle_attack_exp,
        );
        v.value(
            "shuffle_magic_exp",
            &mut self.shuffle_magic_exp,
            d.shuffle_magic_exp,
        );
        v.value(
            "shuffle_life_exp",
            &mut self.shuffle_life_exp,
            d.shuffle_life_exp,
        );
        v.range(
            "attack_level_cap",
            &mut self.attack_level_cap,
            1,
            8,
            d.attack_level_cap,
        );
        v.range(
            "magic_level_cap",
            &mut self.magic_level_cap,
            1,
            8,
            d.magic_level_cap,
        );
        v.range(
            "life_level_cap",
            &mut self.life_level_cap,
            1,
            8,
            d.life_level_cap,
        );
        v.value(
            "scale_level_requirements_to_cap",
            &mut self.scale_level_requirements_to_cap,
            d.scale_level_requirements_to_cap,
        );
        v.value(
            "attack_effectiveness",
            &mut self.attack_effectiveness,
            d.attack_effectiveness,
        );
        v.value(
            "magic_effectiveness",
            &mut self.magic_effectiveness,
            d.magic_effectiveness,
        );
        v.value(
            "life_effectiveness",
            &mut self.life_effectiveness,
            d.life_effectiveness,
        );
    }
}

/// Spells and movement abilities (`spells` module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellFlags {
    /// Life spell heals 1-5 bars (fixed per seed).
    pub shuffle_life_refill: bool,
    /// Spells shuffled among wizards (menu reordered).
    pub shuffle_spell_locations: Tri,
    /// East wizards no longer need 5-8 magic containers.
    pub disable_magic_container_requirements: Tri,
    /// The Spell spell turns enemies into something other than a Bot.
    pub randomize_spell_spell_enemy: Tri,
    /// The two stab teachers swap.
    pub swap_up_and_down_stab: Tri,
    /// The Fire spell (normal, linked, or replaced by Dash).
    pub fire_option: FireOption,
    /// Permanent high jump.
    pub jump_always_on: bool,
    /// Permanent dash speed.
    pub dash_always_on: bool,
    /// Sword beam at any health.
    pub permanent_beam_sword: bool,
    /// The flute opens a warp to palaces or towns.
    pub flute_warp: FluteWarp,
}

impl Default for SpellFlags {
    fn default() -> Self {
        SpellFlags {
            shuffle_life_refill: false,
            shuffle_spell_locations: Tri::Off,
            disable_magic_container_requirements: Tri::Off,
            randomize_spell_spell_enemy: Tri::Off,
            swap_up_and_down_stab: Tri::Off,
            fire_option: FireOption::Normal,
            jump_always_on: false,
            dash_always_on: false,
            permanent_beam_sword: false,
            flute_warp: FluteWarp::None,
        }
    }
}

impl FlagGroup for SpellFlags {
    const KEY: &'static str = "spells";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "shuffle_life_refill",
            &mut self.shuffle_life_refill,
            d.shuffle_life_refill,
        );
        v.value(
            "shuffle_spell_locations",
            &mut self.shuffle_spell_locations,
            d.shuffle_spell_locations,
        );
        v.value(
            "disable_magic_container_requirements",
            &mut self.disable_magic_container_requirements,
            d.disable_magic_container_requirements,
        );
        v.value(
            "randomize_spell_spell_enemy",
            &mut self.randomize_spell_spell_enemy,
            d.randomize_spell_spell_enemy,
        );
        v.value(
            "swap_up_and_down_stab",
            &mut self.swap_up_and_down_stab,
            d.swap_up_and_down_stab,
        );
        v.value("fire_option", &mut self.fire_option, d.fire_option);
        v.value("jump_always_on", &mut self.jump_always_on, d.jump_always_on);
        v.value("dash_always_on", &mut self.dash_always_on, d.dash_always_on);
        v.value(
            "permanent_beam_sword",
            &mut self.permanent_beam_sword,
            d.permanent_beam_sword,
        );
        v.value("flute_warp", &mut self.flute_warp, d.flute_warp);
    }
}

/// One enemy drop pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DropPool {
    /// Blue jar.
    pub blue_jar: bool,
    /// Red jar.
    pub red_jar: bool,
    /// 50-point bag.
    pub small_bag: bool,
    /// 100-point bag.
    pub medium_bag: bool,
    /// 200-point bag.
    pub large_bag: bool,
    /// 500-point bag.
    pub xl_bag: bool,
    /// 1-up doll.
    pub one_up: bool,
    /// Key.
    pub key: bool,
}

impl DropPool {
    /// `(label, field)` pairs for user interfaces, in flag order.
    pub fn entries_mut(&mut self) -> [(&'static str, &mut bool); 8] {
        [
            ("Blue jar", &mut self.blue_jar),
            ("Red jar", &mut self.red_jar),
            ("50 bag", &mut self.small_bag),
            ("100 bag", &mut self.medium_bag),
            ("200 bag", &mut self.large_bag),
            ("500 bag", &mut self.xl_bag),
            ("1-up", &mut self.one_up),
            ("Key", &mut self.key),
        ]
    }

    fn visit<V: Visitor>(&mut self, v: &mut V, prefix: &'static [&'static str; 8]) {
        for (name, field) in prefix.iter().zip(self.entries_mut()) {
            v.value(name, field.1, false);
        }
    }
}

const SMALL_POOL_NAMES: [&str; 8] = [
    "small_blue_jar",
    "small_red_jar",
    "small_small_bag",
    "small_medium_bag",
    "small_large_bag",
    "small_xl_bag",
    "small_one_up",
    "small_key",
];
const LARGE_POOL_NAMES: [&str; 8] = [
    "large_blue_jar",
    "large_red_jar",
    "large_small_bag",
    "large_medium_bag",
    "large_large_bag",
    "large_xl_bag",
    "large_one_up",
    "large_key",
];

/// Enemy drops (`drops` module). When no pool item is ticked and
/// [`DropFlags::randomize_drops`] is off, the vanilla pools are kept.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DropFlags {
    /// Drop every 4-8 kills instead of 6.
    pub shuffle_drop_frequency: bool,
    /// Each unticked pool item gets a 50% chance to join its pool.
    pub randomize_drops: bool,
    /// Drops follow a fixed per-seed sequence.
    pub standardize_drops: bool,
    /// What small enemies can drop.
    pub small_pool: DropPool,
    /// What large enemies can drop.
    pub large_pool: DropPool,
}

impl FlagGroup for DropFlags {
    const KEY: &'static str = "drops";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "shuffle_drop_frequency",
            &mut self.shuffle_drop_frequency,
            d.shuffle_drop_frequency,
        );
        v.value(
            "randomize_drops",
            &mut self.randomize_drops,
            d.randomize_drops,
        );
        v.value(
            "standardize_drops",
            &mut self.standardize_drops,
            d.standardize_drops,
        );
        self.small_pool.visit(v, &SMALL_POOL_NAMES);
        self.large_pool.visit(v, &LARGE_POOL_NAMES);
    }
}

/// Hints and spoiler-related switches (`hints` module).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HintFlags {
    /// NPC hints naming where items are.
    pub helpful_hints: HelpfulHints,
    /// Quest-item givers say what their wizard gives.
    pub spell_item_hints: Tri,
    /// Town signs show the wizard's reward.
    pub town_name_hints: Tri,
    /// Marks the seed as a spoiler seed. Like upstream, this also changes
    /// the generated world (it is part of the seed); the spoiler log itself
    /// is always available through `--rando-spoiler`.
    pub generate_spoiler: bool,
    /// False walls and floors are drawn differently.
    pub reveal_walkthrough_walls: bool,
    /// Hidden jar spots get a marker.
    pub reveal_hidden_jars: bool,
}

impl FlagGroup for HintFlags {
    const KEY: &'static str = "hints";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value("helpful_hints", &mut self.helpful_hints, d.helpful_hints);
        v.value(
            "spell_item_hints",
            &mut self.spell_item_hints,
            d.spell_item_hints,
        );
        v.value(
            "town_name_hints",
            &mut self.town_name_hints,
            d.town_name_hints,
        );
        v.value(
            "generate_spoiler",
            &mut self.generate_spoiler,
            d.generate_spoiler,
        );
        v.value(
            "reveal_walkthrough_walls",
            &mut self.reveal_walkthrough_walls,
            d.reveal_walkthrough_walls,
        );
        v.value(
            "reveal_hidden_jars",
            &mut self.reveal_hidden_jars,
            d.reveal_hidden_jars,
        );
    }
}

/// Town changes (`towns` module).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TownFlags {
    /// The New Kasuto basement needs 5-7 magic containers (vanilla 7).
    pub randomize_new_kasuto_jar_requirements: bool,
    /// Town doors lead straight to the wizard and back.
    pub shorten_wizards: bool,
}

impl FlagGroup for TownFlags {
    const KEY: &'static str = "towns";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value(
            "randomize_new_kasuto_jar_requirements",
            &mut self.randomize_new_kasuto_jar_requirements,
            d.randomize_new_kasuto_jar_requirements,
        );
        v.value(
            "shorten_wizards",
            &mut self.shorten_wizards,
            d.shorten_wizards,
        );
    }
}

/// Quality-of-life patches (`qol` module). These never change the generated
/// world or the hash code.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QolFlags {
    /// Dialog prints without the per-letter delay.
    pub fast_text: bool,
    /// Fix vanilla quirks that matter once things are shuffled.
    pub bug_fixes: bool,
    /// Suppress full-screen flashing.
    pub remove_flashing: bool,
    /// Black out the bricks in Thunderbird's room during Thunder.
    pub darken_thunderbird: bool,
    /// HUD shows lives, keys and crystals.
    pub updated_hud: bool,
    /// Keep the HUD steady during lag frames.
    pub disable_hud_lag: bool,
    /// Select recasts the current spell.
    pub fast_spell_casting: bool,
    /// Up+Select on controller 1 acts as Up+A on controller 2.
    pub up_a_on_controller_1: bool,
    /// Low-health beep threshold.
    pub beep_threshold: BeepThreshold,
    /// Low-health beep speed.
    pub beep_frequency: BeepFrequency,
}

impl FlagGroup for QolFlags {
    const KEY: &'static str = "qol";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value("fast_text", &mut self.fast_text, d.fast_text);
        v.value("bug_fixes", &mut self.bug_fixes, d.bug_fixes);
        v.value(
            "remove_flashing",
            &mut self.remove_flashing,
            d.remove_flashing,
        );
        v.value(
            "darken_thunderbird",
            &mut self.darken_thunderbird,
            d.darken_thunderbird,
        );
        v.value("updated_hud", &mut self.updated_hud, d.updated_hud);
        v.value(
            "disable_hud_lag",
            &mut self.disable_hud_lag,
            d.disable_hud_lag,
        );
        v.value(
            "fast_spell_casting",
            &mut self.fast_spell_casting,
            d.fast_spell_casting,
        );
        v.value(
            "up_a_on_controller_1",
            &mut self.up_a_on_controller_1,
            d.up_a_on_controller_1,
        );
        v.value("beep_threshold", &mut self.beep_threshold, d.beep_threshold);
        v.value("beep_frequency", &mut self.beep_frequency, d.beep_frequency);
    }
}

/// Looks and sound (`cosmetic` module). Never changes the generated world
/// or the hash code. Character sprites are bring-your-own IPS files passed
/// separately (`--sprite-ips`), not flags.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CosmeticFlags {
    /// Tunic colour.
    pub tunic: NesColor,
    /// Skin colour.
    pub skin_tone: NesColor,
    /// Tunic outline colour.
    pub tunic_outline: NesColor,
    /// Tunic colour while the Shield spell is on.
    pub shield_tunic: NesColor,
    /// Sword beam graphic.
    pub beam_sprite: BeamSprite,
    /// Shuffle enemy, NPC and health-bar palettes.
    pub shuffle_sprite_palettes: bool,
    /// Let the sprite IPS also change item graphics.
    pub change_item_sprites: bool,
    /// Replace stock NPC lines with z2rs's own text pool.
    pub community_text: bool,
    /// Silence the background music (sound effects stay).
    pub disable_music: bool,
    /// Use bring-your-own music if provided.
    pub randomize_music: bool,
    /// Mix custom and original music.
    pub mix_custom_and_original_music: bool,
    /// Include more diverse custom tracks.
    pub include_diverse_music: bool,
    /// Skip tracks marked unsafe for streaming.
    pub disable_unsafe_music: bool,
}

impl FlagGroup for CosmeticFlags {
    const KEY: &'static str = "cosmetic";
    fn visit<V: Visitor>(&mut self, v: &mut V) {
        let d = Self::default();
        v.value("tunic", &mut self.tunic, d.tunic);
        v.value("skin_tone", &mut self.skin_tone, d.skin_tone);
        v.value("tunic_outline", &mut self.tunic_outline, d.tunic_outline);
        v.value("shield_tunic", &mut self.shield_tunic, d.shield_tunic);
        v.value("beam_sprite", &mut self.beam_sprite, d.beam_sprite);
        v.value(
            "shuffle_sprite_palettes",
            &mut self.shuffle_sprite_palettes,
            d.shuffle_sprite_palettes,
        );
        v.value(
            "change_item_sprites",
            &mut self.change_item_sprites,
            d.change_item_sprites,
        );
        v.value("community_text", &mut self.community_text, d.community_text);
        v.value("disable_music", &mut self.disable_music, d.disable_music);
        v.value(
            "randomize_music",
            &mut self.randomize_music,
            d.randomize_music,
        );
        v.value(
            "mix_custom_and_original_music",
            &mut self.mix_custom_and_original_music,
            d.mix_custom_and_original_music,
        );
        v.value(
            "include_diverse_music",
            &mut self.include_diverse_music,
            d.include_diverse_music,
        );
        v.value(
            "disable_unsafe_music",
            &mut self.disable_unsafe_music,
            d.disable_unsafe_music,
        );
    }
}

// ---------------------------------------------------------------------------
// Aggregate.
// ---------------------------------------------------------------------------

/// Every randomizer option.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Flags {
    /// Starting inventory and global switches.
    pub start: StartFlags,
    /// Overworld.
    pub overworld: OverworldFlags,
    /// Palaces.
    pub palaces: PalaceFlags,
    /// Item placement.
    pub items: ItemFlags,
    /// Enemies.
    pub enemies: EnemyFlags,
    /// Experience and effectiveness.
    pub stats: StatsFlags,
    /// Spells.
    pub spells: SpellFlags,
    /// Drops.
    pub drops: DropFlags,
    /// Hints.
    pub hints: HintFlags,
    /// Towns.
    pub towns: TownFlags,
    /// Quality-of-life patches (not part of the seed).
    pub qol: QolFlags,
    /// Cosmetics (not part of the seed).
    pub cosmetic: CosmeticFlags,
}

/// Call `$func(&mut group, extra args...)` for every module in flag-string
/// order (a macro because each call has a different group type).
macro_rules! for_each_group {
    ($flags:expr, $func:ident $(, $arg:expr)*) => {{
        let fl = $flags;
        $func(&mut fl.start $(, $arg)*);
        $func(&mut fl.overworld $(, $arg)*);
        $func(&mut fl.palaces $(, $arg)*);
        $func(&mut fl.items $(, $arg)*);
        $func(&mut fl.enemies $(, $arg)*);
        $func(&mut fl.stats $(, $arg)*);
        $func(&mut fl.spells $(, $arg)*);
        $func(&mut fl.drops $(, $arg)*);
        $func(&mut fl.hints $(, $arg)*);
        $func(&mut fl.towns $(, $arg)*);
        $func(&mut fl.qol $(, $arg)*);
        $func(&mut fl.cosmetic $(, $arg)*);
    }};
}

/// Module keys in flag-string order.
pub const MODULE_KEYS: [&str; 12] = [
    "start",
    "overworld",
    "palaces",
    "items",
    "enemies",
    "stats",
    "spells",
    "drops",
    "hints",
    "towns",
    "qol",
    "cosmetic",
];

/// Modules that never change the generated world (left out of the seed
/// string and the hash code).
pub const NON_SEED_MODULES: [&str; 2] = ["qol", "cosmetic"];

// --- bit writer / reader ---------------------------------------------------

#[derive(Default)]
struct Bits {
    bits: Vec<bool>,
}

impl Bits {
    fn push(&mut self, value: u32, width: u32) {
        for i in (0..width).rev() {
            self.bits.push((value >> i) & 1 == 1);
        }
    }

    fn push_varint(&mut self, mut n: usize) {
        loop {
            let group = (n & 0x1F) as u32;
            n >>= 5;
            self.push(group, 5);
            self.push(u32::from(n != 0), 1);
            if n == 0 {
                break;
            }
        }
    }

    fn trim_trailing_zeros(&mut self) {
        while self.bits.last() == Some(&false) {
            self.bits.pop();
        }
    }
}

struct Reader<'a> {
    bits: &'a [bool],
    pos: usize,
}

impl Reader<'_> {
    fn read(&mut self, width: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..width {
            let b = self.bits.get(self.pos).copied().unwrap_or(false);
            self.pos += 1;
            v = (v << 1) | u32::from(b);
        }
        v
    }

    fn read_varint(&mut self) -> Result<usize, FlagError> {
        let mut n: usize = 0;
        let mut shift = 0;
        loop {
            let group = self.read(5) as usize;
            if shift > 20 {
                return Err(FlagError::BadLength);
            }
            n |= group << shift;
            shift += 5;
            if self.read(1) == 0 {
                break;
            }
        }
        Ok(n)
    }
}

struct WriteVisitor {
    out: Bits,
}

impl Visitor for WriteVisitor {
    fn value<T: FlagValue>(&mut self, _name: &'static str, v: &mut T, default: T) {
        let w = T::bits();
        self.out.push(v.to_raw() ^ default.to_raw(), w);
    }
    fn range(&mut self, _name: &'static str, v: &mut u8, lo: u8, hi: u8, default: u8) {
        let w = bits_for(u32::from(hi - lo) + 1);
        let raw = u32::from((*v).clamp(lo, hi) - lo);
        let def = u32::from(default - lo);
        self.out.push(raw ^ def, w);
    }
}

struct ReadVisitor<'a> {
    module: &'static str,
    r: Reader<'a>,
    err: Option<FlagError>,
}

impl Visitor for ReadVisitor<'_> {
    fn value<T: FlagValue>(&mut self, name: &'static str, v: &mut T, default: T) {
        let raw = self.r.read(T::bits()) ^ default.to_raw();
        match T::from_raw(raw) {
            Some(x) => *v = x,
            None => {
                self.err.get_or_insert(FlagError::BadValue {
                    module: self.module,
                    field: name,
                    raw,
                });
            }
        }
    }
    fn range(&mut self, name: &'static str, v: &mut u8, lo: u8, hi: u8, default: u8) {
        let w = bits_for(u32::from(hi - lo) + 1);
        let raw = self.r.read(w) ^ u32::from(default - lo);
        if raw > u32::from(hi - lo) {
            self.err.get_or_insert(FlagError::BadValue {
                module: self.module,
                field: name,
                raw,
            });
        } else {
            *v = lo + raw as u8;
        }
    }
}

fn group_bits<G: FlagGroup>(g: &G) -> Bits {
    let mut w = WriteVisitor {
        out: Bits::default(),
    };
    g.clone().visit(&mut w);
    w.out.trim_trailing_zeros();
    w.out
}

fn read_group<G: FlagGroup>(bits: &[bool]) -> Result<G, FlagError> {
    let mut g = G::default();
    let mut rv = ReadVisitor {
        module: G::KEY,
        r: Reader { bits, pos: 0 },
        err: None,
    };
    g.visit(&mut rv);
    match rv.err {
        Some(e) => Err(e),
        None => Ok(g),
    }
}

fn encode_sections(sections: &[Bits]) -> String {
    let mut all = Bits::default();
    for s in sections {
        all.push_varint(s.bits.len());
        all.bits.extend_from_slice(&s.bits);
    }
    let mut out = String::new();
    out.push(FLAGS_VERSION);
    for chunk in all.bits.chunks(6) {
        let mut v = 0usize;
        for i in 0..6 {
            v = (v << 1) | usize::from(chunk.get(i).copied().unwrap_or(false));
        }
        out.push(ALPHABET[v] as char);
    }
    while out.len() > 1 && out.ends_with('A') {
        out.pop();
    }
    out
}

/// Bits of one group with its trimmed section, for `seed`-relevant groups.
fn sections_of(flags: &Flags, include_non_seed: bool) -> Vec<Bits> {
    let mut v = Vec::with_capacity(MODULE_KEYS.len());
    let f = flags.clone();
    v.push(group_bits(&f.start));
    v.push(group_bits(&f.overworld));
    v.push(group_bits(&f.palaces));
    v.push(group_bits(&f.items));
    v.push(group_bits(&f.enemies));
    v.push(group_bits(&f.stats));
    v.push(group_bits(&f.spells));
    v.push(group_bits(&f.drops));
    v.push(group_bits(&f.hints));
    v.push(group_bits(&f.towns));
    if include_non_seed {
        v.push(group_bits(&f.qol));
        v.push(group_bits(&f.cosmetic));
    }
    v
}

impl Flags {
    /// The compact flag string (every option).
    #[must_use]
    pub fn to_flag_string(&self) -> String {
        encode_sections(&sections_of(self, true))
    }

    /// The flag string without the QoL and cosmetic sections: what the seed
    /// and the hash code are derived from.
    #[must_use]
    pub fn seed_flag_string(&self) -> String {
        encode_sections(&sections_of(self, false))
    }

    /// Parse a flag string (surrounding whitespace is ignored).
    pub fn from_flag_string(s: &str) -> Result<Flags, FlagError> {
        let s = s.trim();
        let mut chars = s.chars();
        let ver = chars.next().ok_or(FlagError::Empty)?;
        if ver != FLAGS_VERSION {
            return Err(FlagError::Version(ver));
        }
        let mut bits = Vec::with_capacity(s.len() * 6);
        for c in chars {
            let v = ALPHABET
                .iter()
                .position(|&a| a as char == c)
                .ok_or(FlagError::BadChar(c))?;
            for i in (0..6).rev() {
                bits.push((v >> i) & 1 == 1);
            }
        }
        let mut r = Reader {
            bits: &bits,
            pos: 0,
        };
        let mut sections: Vec<Vec<bool>> = Vec::with_capacity(MODULE_KEYS.len());
        for _ in MODULE_KEYS {
            let len = r.read_varint()?;
            if len > 4096 {
                return Err(FlagError::BadLength);
            }
            let start = r.pos.min(bits.len());
            let end = (r.pos + len).min(bits.len());
            let mut sec = bits[start..end].to_vec();
            sec.resize(len, false);
            r.pos += len;
            sections.push(sec);
        }
        Ok(Flags {
            start: read_group(&sections[0])?,
            overworld: read_group(&sections[1])?,
            palaces: read_group(&sections[2])?,
            items: read_group(&sections[3])?,
            enemies: read_group(&sections[4])?,
            stats: read_group(&sections[5])?,
            spells: read_group(&sections[6])?,
            drops: read_group(&sections[7])?,
            hints: read_group(&sections[8])?,
            towns: read_group(&sections[9])?,
            qol: read_group(&sections[10])?,
            cosmetic: read_group(&sections[11])?,
        })
    }

    /// Whether these flags leave the ROM untouched.
    #[must_use]
    pub fn is_vanilla(&self) -> bool {
        *self == Flags::default()
    }

    /// `(module, field, value)` for every option that differs from its
    /// default, in flag order (spoiler and logs).
    #[must_use]
    pub fn describe_changes(&self) -> Vec<(&'static str, &'static str, String)> {
        struct Describe {
            module: &'static str,
            out: Vec<(&'static str, &'static str, String)>,
        }
        impl Visitor for Describe {
            fn value<T: FlagValue>(&mut self, name: &'static str, v: &mut T, default: T) {
                if *v != default {
                    self.out.push((self.module, name, v.describe()));
                }
            }
            fn range(&mut self, name: &'static str, v: &mut u8, _lo: u8, _hi: u8, default: u8) {
                if *v != default {
                    self.out.push((self.module, name, v.to_string()));
                }
            }
        }
        fn one<G: FlagGroup>(g: &mut G, d: &mut Describe) {
            d.module = G::KEY;
            g.visit(d);
        }
        let mut d = Describe {
            module: "",
            out: Vec::new(),
        };
        let mut f = self.clone();
        for_each_group!(&mut f, one, &mut d);
        d.out
    }

    /// Fill every option with a random valid value (tests, "surprise me").
    #[must_use]
    pub fn random(rng: &mut Rng) -> Flags {
        struct Randomize<'a> {
            rng: &'a mut Rng,
        }
        impl Visitor for Randomize<'_> {
            fn value<T: FlagValue>(&mut self, _n: &'static str, v: &mut T, _d: T) {
                *v = T::random(self.rng);
            }
            fn range(&mut self, _n: &'static str, v: &mut u8, lo: u8, hi: u8, _d: u8) {
                *v = self.rng.range_u8(lo, hi);
            }
        }
        fn one<G: FlagGroup>(g: &mut G, r: &mut Randomize<'_>) {
            g.visit(r);
        }
        let mut f = Flags::default();
        let mut r = Randomize { rng };
        for_each_group!(&mut f, one, &mut r);
        f
    }

    /// Clamp every ranged field into its range and order min/max pairs
    /// (call after editing values by hand).
    pub fn normalize(&mut self) {
        struct Clamp;
        impl Visitor for Clamp {
            fn value<T: FlagValue>(&mut self, _n: &'static str, _v: &mut T, _d: T) {}
            fn range(&mut self, _n: &'static str, v: &mut u8, lo: u8, hi: u8, _d: u8) {
                *v = (*v).clamp(lo, hi);
            }
        }
        fn one<G: FlagGroup>(g: &mut G) {
            g.visit(&mut Clamp);
        }
        for_each_group!(&mut *self, one);
        let s = &mut self.start;
        if s.heart_containers_min > s.heart_containers_max {
            s.heart_containers_max = s.heart_containers_min;
        }
        if s.magic_containers_min > s.magic_containers_max {
            s.magic_containers_max = s.magic_containers_min;
        }
        let p = &mut self.palaces;
        if p.palaces_to_complete_min > p.palaces_to_complete_max {
            p.palaces_to_complete_max = p.palaces_to_complete_min;
        }
    }

    /// Number of options (all modules).
    #[must_use]
    pub fn option_count() -> usize {
        struct Count(usize);
        impl Visitor for Count {
            fn value<T: FlagValue>(&mut self, _n: &'static str, _v: &mut T, _d: T) {
                self.0 += 1;
            }
            fn range(&mut self, _n: &'static str, _v: &mut u8, _lo: u8, _hi: u8, _d: u8) {
                self.0 += 1;
            }
        }
        fn one<G: FlagGroup>(g: &mut G, c: &mut Count) {
            g.visit(c);
        }
        let mut c = Count(0);
        let mut f = Flags::default();
        for_each_group!(&mut f, one, &mut c);
        c.0
    }
}

/// Every ranged option: `(module, field, min, max)`.
pub const RANGES: &[(&str, &str, u8, u8)] = &[
    ("start", "heart_containers_min", 1, 8),
    ("start", "heart_containers_max", 1, 8),
    ("start", "magic_containers_min", 1, 8),
    ("start", "magic_containers_max", 1, 8),
    ("start", "attack_level", 1, 8),
    ("start", "magic_level", 1, 8),
    ("start", "life_level", 1, 8),
    ("palaces", "palaces_to_complete_min", 0, 6),
    ("palaces", "palaces_to_complete_max", 0, 6),
    ("stats", "attack_level_cap", 1, 8),
    ("stats", "magic_level_cap", 1, 8),
    ("stats", "life_level_cap", 1, 8),
];

// ---------------------------------------------------------------------------
// Presets.
// ---------------------------------------------------------------------------

flag_enum! {
    /// Named whole configurations.
    Preset {
        Vanilla => "Vanilla (no changes)",
        Beginner => "Beginner",
        Standard => "Standard",
        MaxRando => "Max rando",
    }
    default Vanilla
}

impl Preset {
    /// The flags this preset stands for.
    #[must_use]
    pub fn flags(self) -> Flags {
        match self {
            Preset::Vanilla => Flags::default(),
            Preset::Beginner => beginner(),
            Preset::Standard => standard(),
            Preset::MaxRando => max_rando(),
        }
    }

    /// The preset whose flags equal `flags`, if any.
    #[must_use]
    pub fn detect(flags: &Flags) -> Option<Preset> {
        Preset::ALL.iter().copied().find(|p| p.flags() == *flags)
    }

    /// One-line description.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Preset::Vanilla => "The original game: the ROM is not changed.",
            Preset::Beginner => {
                "Shuffled items with helpful hints, a stronger start and comfort patches."
            }
            Preset::Standard => {
                "Items, spells and enemies shuffled; palaces and overworld vanilla-like."
            }
            Preset::MaxRando => "Everything generated or shuffled, with random switches mixed in.",
        }
    }
}

/// Options z2rs accepts (they round-trip in flag strings and some feed the
/// seed) but does not implement yet, as `(module, field)`. Presets leave
/// them at their defaults; README.md says why each one waits.
pub const NOT_IMPLEMENTED: &[(&str, &str)] = &[
    ("start", "share_seed_across_difficulty"),
    ("palaces", "include_extra_rooms_a"),
    ("palaces", "include_extra_rooms_b"),
    ("palaces", "remove_long_dead_ends"),
    ("palaces", "include_expert_rooms"),
    ("palaces", "restart_at_palaces_on_game_over"),
    ("palaces", "global_5050_jar_drop"),
    ("palaces", "reduce_dripper_variance"),
    ("palaces", "randomize_boss_item_drop"),
    ("palaces", "hard_bosses"),
    ("items", "include_spells"),
    ("items", "include_sword_techs"),
    ("items", "include_quest_items"),
    ("spells", "flute_warp"),
    ("qol", "updated_hud"),
    ("qol", "disable_hud_lag"),
    ("cosmetic", "randomize_music"),
    ("cosmetic", "mix_custom_and_original_music"),
    ("cosmetic", "include_diverse_music"),
    ("cosmetic", "disable_unsafe_music"),
];

// Presets only switch on options that do something: the switches z2rs
// accepts but does not implement yet (see README.md, "Status")
// stay off, and a test pins that.

fn comfort(f: &mut Flags) {
    f.qol.fast_text = true;
    f.qol.bug_fixes = true;
    f.qol.remove_flashing = true;
    f.towns.shorten_wizards = true;
}

fn beginner() -> Flags {
    let mut f = Flags::default();
    comfort(&mut f);
    f.start.start_with_candle = true;
    f.start.heart_containers_min = 5;
    f.start.heart_containers_max = 5;
    f.start.magic_containers_min = 5;
    f.start.magic_containers_max = 5;
    f.start.starting_lives = StartingLives::L5;
    f.items.shuffle_palace_items = Tri::On;
    f.items.shuffle_overworld_items = Tri::On;
    f.items.start_with_spell_items = Tri::On;
    f.hints.helpful_hints = HelpfulHints::TownsSeparate;
    f.hints.spell_item_hints = Tri::On;
    f.hints.town_name_hints = Tri::On;
    f.hints.reveal_walkthrough_walls = true;
    f.drops.randomize_drops = false;
    f.stats.life_effectiveness = LifeEffectiveness::AverageHigh;
    f.qol.fast_spell_casting = true;
    f
}

fn standard() -> Flags {
    let mut f = Flags::default();
    comfort(&mut f);
    f.start.shuffle_starting_items = true;
    f.start.start_items_limit = StartLimit::One;
    f.start.heart_containers_min = 1;
    f.start.heart_containers_max = 4;
    f.start.magic_containers_min = 1;
    f.start.magic_containers_max = 4;
    f.start.max_heart_containers = MaxHearts::Random;
    f.overworld.west_biome = Biome::Vanillalike;
    f.overworld.east_biome = Biome::Vanillalike;
    f.overworld.dm_biome = Biome::Vanillalike;
    f.overworld.maze_biome = Biome::Vanillalike;
    f.overworld.palaces_swap_continents = Tri::On;
    f.overworld.shuffle_encounters = Tri::On;
    f.palaces.normal_style = PalaceStyle::RandomAll;
    f.palaces.gp_style = PalaceStyle::Random;
    f.palaces.normal_length = PalaceLength::Medium;
    f.palaces.palaces_to_complete_min = 4;
    f.palaces.palaces_to_complete_max = 6;
    f.items.shuffle_palace_items = Tri::On;
    f.items.shuffle_overworld_items = Tri::On;
    f.items.mix_overworld_and_palace_items = Tri::On;
    f.items.include_pbag_caves = Tri::On;
    f.items.shuffle_small_items = true;
    f.enemies.shuffle_overworld_enemies = Tri::On;
    f.enemies.shuffle_palace_enemies = Tri::On;
    f.enemies.enemy_hp = EnemyLife::Medium;
    f.enemies.xp_drops = XpEffectiveness::LowVariance;
    f.stats.shuffle_attack_exp = true;
    f.stats.shuffle_magic_exp = true;
    f.stats.shuffle_life_exp = true;
    f.spells.shuffle_life_refill = true;
    f.drops.randomize_drops = true;
    f.drops.shuffle_drop_frequency = true;
    f.hints.helpful_hints = HelpfulHints::TownsSeparate;
    f.hints.spell_item_hints = Tri::On;
    f
}

fn max_rando() -> Flags {
    let mut f = standard();
    f.start.shuffle_starting_spells = true;
    f.start.starting_techs = StartingTechs::Random;
    f.start.starting_lives = StartingLives::Random;
    f.overworld.west_biome = Biome::RandomNoVanilla;
    f.overworld.east_biome = Biome::RandomNoVanilla;
    f.overworld.dm_biome = Biome::RandomNoVanilla;
    f.overworld.maze_biome = Biome::Random;
    f.overworld.west_climate = Climate::Random;
    f.overworld.east_climate = Climate::Random;
    f.overworld.dm_climate = Climate::Random;
    f.overworld.continent_connections = ContinentConnections::AnythingGoes;
    f.overworld.less_important_locations = LessImportantLocations::Random;
    f.overworld.river_devil_blocker = RiverDevilBlocker::Random;
    f.overworld.shuffle_hidden_locations = Tri::Random;
    f.overworld.shuffle_great_palace = Tri::Random;
    f.overworld.encounter_rate = EncounterRate::Random;
    f.overworld.good_boots = Tri::Random;
    f.palaces.normal_style = PalaceStyle::RandomPerPalace;
    f.palaces.normal_length = PalaceLength::Random;
    f.palaces.gp_length = PalaceLength::Random;
    f.palaces.boss_rooms_exit = BossRoomsExit::RandomPerPalace;
    f.palaces.item_rooms_per_palace = ItemRoomCount::RandomIncludeZero;
    f.palaces.palaces_to_complete_min = 0;
    f.palaces.change_palace_palettes = true;
    f.palaces.thunderbird_required = Tri::Random;
    f.items.shuffle_pbag_amounts = Tri::Random;
    f.items.palaces_contain_extra_keys = Tri::Random;
    f.enemies.mix_large_and_small = Tri::Random;
    f.enemies.dripper_enemy = DripperEnemy::AnyGroundEnemy;
    f.enemies.enemy_hp = EnemyLife::Wide;
    f.enemies.boss_hp = EnemyLife::Medium;
    f.enemies.shuffle_xp_stealers = true;
    f.enemies.shuffle_xp_stolen_amount = true;
    f.enemies.sword_immunity = SwordImmunity::ShuffleConditional;
    f.enemies.xp_drops = XpEffectiveness::Random;
    f.stats.attack_effectiveness = AttackEffectiveness::Average;
    f.stats.magic_effectiveness = MagicEffectiveness::Average;
    f.stats.life_effectiveness = LifeEffectiveness::Average;
    f.spells.shuffle_spell_locations = Tri::Random;
    f.spells.disable_magic_container_requirements = Tri::Random;
    f.spells.randomize_spell_spell_enemy = Tri::Random;
    f.spells.swap_up_and_down_stab = Tri::Random;
    f.spells.fire_option = FireOption::Random;
    f.drops.standardize_drops = false;
    f.hints.town_name_hints = Tri::Random;
    f.towns.randomize_new_kasuto_jar_requirements = true;
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_is_short_and_round_trips() {
        let f = Flags::default();
        assert!(f.is_vanilla());
        assert_eq!(f.to_flag_string(), "1");
        assert_eq!(Flags::from_flag_string("1").unwrap(), f);
        // Extra padding characters decode to the same thing.
        assert_eq!(Flags::from_flag_string(" 1AAAA ").unwrap(), f);
        assert!(f.describe_changes().is_empty());
    }

    #[test]
    fn covers_every_option() {
        // The 160 upstream flag-string options other than Mario mode, the 21
        // upstream cosmetic/customize options (the sprite choice itself is a
        // bring-your-own file, not a flag), plus three switches upstream
        // always applies (fast text, bug fixes, shortened wizards). Pinned so
        // a field added without its `visit` line is caught.
        assert_eq!(Flags::option_count(), 160 + 21 + 3);
    }

    #[test]
    fn every_preset_round_trips() {
        for p in Preset::ALL {
            let f = p.flags();
            let s = f.to_flag_string();
            assert!(s.starts_with(FLAGS_VERSION));
            let back = Flags::from_flag_string(&s).unwrap_or_else(|e| panic!("{p:?}: {e}"));
            assert_eq!(back, f, "{p:?} {s}");
            assert_eq!(Preset::detect(&back), Some(*p));
            assert_eq!(back.to_flag_string(), s, "canonical");
        }
        assert_ne!(Preset::Standard.flags(), Preset::MaxRando.flags());
    }

    #[test]
    fn presets_leave_unimplemented_options_off() {
        for p in Preset::ALL {
            for (m, f, v) in p.flags().describe_changes() {
                assert!(
                    !NOT_IMPLEMENTED.contains(&(m, f)),
                    "{p:?} sets {m}.{f} = {v}, which does nothing yet"
                );
            }
        }
        // Every listed name is a real option.
        struct Names {
            module: &'static str,
            out: Vec<(&'static str, &'static str)>,
        }
        impl Visitor for Names {
            fn value<T: FlagValue>(&mut self, name: &'static str, _v: &mut T, _d: T) {
                self.out.push((self.module, name));
            }
            fn range(&mut self, name: &'static str, _v: &mut u8, _lo: u8, _hi: u8, _d: u8) {
                self.out.push((self.module, name));
            }
        }
        fn one<G: FlagGroup>(g: &mut G, n: &mut Names) {
            n.module = G::KEY;
            g.visit(n);
        }
        let mut n = Names {
            module: "",
            out: Vec::new(),
        };
        for_each_group!(&mut Flags::default(), one, &mut n);
        for entry in NOT_IMPLEMENTED {
            assert!(n.out.contains(entry), "{entry:?} is not an option");
        }
    }

    #[test]
    fn random_flags_round_trip_many_times() {
        let mut rng = Rng::new(0xF1A6);
        for _ in 0..2000 {
            let f = Flags::random(&mut rng);
            let s = f.to_flag_string();
            let back = Flags::from_flag_string(&s).unwrap_or_else(|e| panic!("{s}: {e}"));
            assert_eq!(back, f, "{s}");
            assert_eq!(back.to_flag_string(), s);
        }
    }

    #[test]
    fn single_field_changes_round_trip() {
        let mut f = Flags::default();
        f.cosmetic.disable_unsafe_music = true; // last field of last module
        let s = f.to_flag_string();
        assert_eq!(Flags::from_flag_string(&s).unwrap(), f);
        let mut g = Flags::default();
        g.start.shuffle_starting_items = true; // first field of first module
        assert_eq!(Flags::from_flag_string(&g.to_flag_string()).unwrap(), g);
        assert_eq!(
            g.describe_changes(),
            vec![("start", "shuffle_starting_items", "on".to_string())]
        );
    }

    #[test]
    fn seed_string_ignores_qol_and_cosmetic() {
        let mut a = Preset::Standard.flags();
        let base = a.seed_flag_string();
        a.cosmetic.tunic = NesColor::Color(0x16);
        a.qol.beep_frequency = BeepFrequency::Off;
        assert_eq!(a.seed_flag_string(), base);
        assert_ne!(
            a.to_flag_string(),
            Preset::Standard.flags().to_flag_string()
        );
        a.items.shuffle_small_items = !a.items.shuffle_small_items;
        assert_ne!(a.seed_flag_string(), base);
    }

    #[test]
    fn appended_field_reads_old_strings_as_default() {
        // Simulate an older build that wrote one bit less in `towns`: the
        // shorter section decodes with the missing trailing field defaulted.
        let mut f = Flags::default();
        f.towns.randomize_new_kasuto_jar_requirements = true;
        let s = f.to_flag_string();
        let back = Flags::from_flag_string(&s).unwrap();
        assert!(back.towns.randomize_new_kasuto_jar_requirements);
        assert!(!back.towns.shorten_wizards);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert_eq!(Flags::from_flag_string(""), Err(FlagError::Empty));
        assert_eq!(
            Flags::from_flag_string("9abc"),
            Err(FlagError::Version('9'))
        );
        assert_eq!(
            Flags::from_flag_string("1ab$"),
            Err(FlagError::BadChar('$'))
        );
        // Fuzz: arbitrary alphabet strings either parse or error, never panic,
        // and anything that parses re-encodes to a string that parses the same.
        let mut rng = Rng::new(77);
        for _ in 0..3000 {
            let len = rng.index(80);
            let mut s = String::from("1");
            for _ in 0..len {
                s.push(ALPHABET[rng.index(64)] as char);
            }
            if let Ok(f) = Flags::from_flag_string(&s) {
                let again = Flags::from_flag_string(&f.to_flag_string()).unwrap();
                assert_eq!(again, f);
            }
        }
    }

    #[test]
    fn enum_widths_and_labels() {
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(3), 2);
        assert_eq!(bits_for(4), 2);
        assert_eq!(bits_for(5), 3);
        assert_eq!(bits_for(13), 4);
        assert_eq!(<Tri as FlagValue>::bits(), 2);
        assert_eq!(<PalaceStyle as FlagValue>::bits(), 4);
        assert_eq!(PalaceStyle::from_index(1), Some(PalaceStyle::Shuffled));
        assert_eq!(Biome::Random.label(), "Random");
        for c in [
            NesColor::Default,
            NesColor::Random,
            NesColor::Color(0),
            NesColor::Color(0x3F),
        ] {
            assert_eq!(NesColor::from_raw(c.to_raw()), Some(c));
        }
    }

    #[test]
    fn normalize_orders_pairs() {
        let mut f = Flags::default();
        f.start.heart_containers_min = 7;
        f.start.heart_containers_max = 2;
        f.stats.attack_level_cap = 0;
        f.normalize();
        assert_eq!(f.start.heart_containers_max, 7);
        assert_eq!(f.stats.attack_level_cap, 1);
    }

    #[test]
    fn serde_round_trip_and_defaults_for_missing_fields() {
        let f = Preset::MaxRando.flags();
        let json = serde_json::to_string(&f).unwrap();
        let back: Flags = serde_json::from_str(&json).unwrap();
        assert_eq!(back, f);
        let partial: Flags = serde_json::from_str(r#"{"start":{"attack_level":3}}"#).unwrap();
        assert_eq!(partial.start.attack_level, 3);
        assert_eq!(partial.start.magic_level, 1);
        assert_eq!(partial.palaces, PalaceFlags::default());
    }
}
