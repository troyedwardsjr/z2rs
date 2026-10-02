//! `hints` module: generated dialog, the title-screen hash, and the two
//! "reveal" options.
//!
//! Options owned: [`crate::flags::HintFlags`] (`ctx.flags.hints`), plus
//! [`crate::flags::CosmeticFlags::community_text`], which only changes
//! flavour lines and draws from its own random stream so it never changes
//! the seed.
//!
//! Catalog: sections 06 (text and hints, fixed patches) and 01 (title-screen
//! hash). Every line of text here is our own; nothing is taken from the
//! upstream randomizer or its community text.
//!
//! What it does, in pipeline order (this module runs after every module
//! that places items, so the hints describe the final game):
//!
//! * **Seed hash on the file-select screen**: whenever the options differ
//!   from vanilla, the six-character hash code replaces the spaced-out
//!   heading of the file-select screen (bank 5 `$BC1C`, the 11-byte PPU
//!   string at `$206A`). Players compare it to confirm they have the same
//!   seed. Vanilla options leave the heading alone.
//! * **Helpful hints**: four townsfolk name where four important items are.
//!   Old Kasuto's readable wall always carries one. With "by continent" a
//!   hint names the region only; with "towns separate" it names the town or
//!   the palace number. Every other hint-capable townsperson gets a "knows
//!   nothing" line, since the vanilla advice can be wrong once things move.
//! * **Spell item hints**: the people who ask for the trophy, medicine,
//!   child, water and mirror say what their town's wizard gives; the two
//!   closed stab-teacher doors say what the teacher inside gives.
//! * **Town name hints**: each wizard town's sign names the wizard's reward.
//! * **Community text** (cosmetic): walking townsfolk and the "nothing more
//!   to teach" lines get light-hearted lines from z2rs's own pool.
//! * **Reveal walkthrough walls**: the tile pair drawn for false walls and
//!   floors (bank 4 `$818D`, bank 5 `$81A3`) becomes plain background, so
//!   they show as gaps.
//! * **Reveal hidden jars**: the invisible "strike here" object that drops a
//!   jar (or an enemy) gets drawn as an orange jar. Original 6502 hooks in
//!   banks 4 and 5 call the common enemy draw routine and then the vanilla
//!   strike check (whose address is read from the player's ROM).
//!
//! The dialog edits go through [`crate::text::table`]; the pipeline writes
//! the table once at the end.

use crate::flags::HelpfulHints;
use crate::rng::Rng;
use crate::{text, Ctx, RandoError};

// ---------------------------------------------------------------------------
// World view used by the hints.
// ---------------------------------------------------------------------------

/// The four map regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Region {
    /// West Hyrule.
    West,
    /// Death Mountain.
    DeathMountain,
    /// East Hyrule.
    East,
    /// Maze Island.
    MazeIsland,
}

impl Region {
    /// How a hint names the region.
    #[must_use]
    pub fn phrase(self) -> &'static str {
        match self {
            Region::West => "THE WEST",
            Region::DeathMountain => "DEATH MOUNTAIN",
            Region::East => "THE EAST",
            Region::MazeIsland => "MAZE ISLAND",
        }
    }
}

/// The towns, in vanilla spell order (the game's town index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Bagu's cabin.
    Bagu,
}

impl Town {
    /// The eight wizard towns.
    pub const WIZARDS: [Town; 8] = [
        Town::Rauru,
        Town::Ruto,
        Town::Saria,
        Town::Mido,
        Town::Nabooru,
        Town::Darunia,
        Town::NewKasuto,
        Town::OldKasuto,
    ];

    /// Name as shown in dialog (at most 11 characters).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Town::Rauru => "RAURU",
            Town::Ruto => "RUTO",
            Town::Saria => "SARIA",
            Town::Mido => "MIDO",
            Town::Nabooru => "NABOORU",
            Town::Darunia => "DARUNIA",
            Town::NewKasuto => "NEW KASUTO",
            Town::OldKasuto => "OLD KASUTO",
            Town::Bagu => "BAGU",
        }
    }

    /// Region of the town in the vanilla layout.
    #[must_use]
    pub fn region(self) -> Region {
        match self {
            Town::Rauru | Town::Ruto | Town::Saria | Town::Mido | Town::Bagu => Region::West,
            _ => Region::East,
        }
    }

    /// Dialog index of the town's sign.
    #[must_use]
    pub fn sign_dialog(self) -> Option<usize> {
        Some(match self {
            Town::Rauru => 11,
            Town::Ruto => 20,
            Town::Saria => 29,
            Town::Mido => 41,
            Town::Nabooru => 62,
            Town::Darunia => 76,
            Town::NewKasuto => 86,
            Town::OldKasuto => 94,
            Town::Bagu => return None,
        })
    }
}

/// What kind of place holds an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// A town's wizard.
    Wizard(Town),
    /// A sword-technique teacher (Mido or Darunia).
    Teacher(Town),
    /// Another item in a town (Saria's table, Nabooru's fountain, the New
    /// Kasuto basement or spell tower, Bagu).
    TownItem(Town),
    /// The palace standing at palace site 1-7 (7 = the Great Palace's
    /// site). Hints name the site the player walks into, which is not the
    /// palace's identity once palaces are shuffled between sites.
    Palace(u8),
    /// Anywhere else on the map (caves, hidden tiles, drops).
    Field,
}

/// What a location holds, as hints talk about it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    /// Region of the location.
    pub region: Region,
    /// Kind of place.
    pub place: Place,
    /// Item name as shown in dialog (at most 11 characters, upper case).
    pub item: String,
    /// Whether the item is a spell (wizards "teach" spells and "give"
    /// everything else).
    pub spell: bool,
}

/// The eight items helpful hints talk about.
pub const KEY_ITEMS: [&str; 8] = [
    "CANDLE",
    "GLOVE",
    "RAFT",
    "BOOTS",
    "FLUTE",
    "CROSS",
    "HAMMER",
    "MAGIC KEY",
];

fn spot(region: Region, place: Place, item: &str, spell: bool) -> Spot {
    Spot {
        region,
        place,
        item: item.to_string(),
        spell,
    }
}

/// Where everything is in the unshuffled game. Used when no module recorded
/// a different placement.
#[must_use]
pub fn vanilla_spots() -> Vec<Spot> {
    use Place::*;
    use Region::*;
    let spells = [
        "SHIELD", "JUMP", "LIFE", "FAIRY", "FIRE", "REFLECT", "SPELL", "THUNDER",
    ];
    let mut v: Vec<Spot> = Town::WIZARDS
        .iter()
        .zip(spells)
        .map(|(&t, s)| spot(t.region(), Wizard(t), s, true))
        .collect();
    v.extend([
        spot(West, Teacher(Town::Mido), "DOWNSTAB", false),
        spot(East, Teacher(Town::Darunia), "UPSTAB", false),
        spot(West, Palace(1), "CANDLE", false),
        spot(West, Palace(2), "GLOVE", false),
        spot(West, Palace(3), "RAFT", false),
        spot(MazeIsland, Palace(4), "BOOTS", false),
        spot(East, Palace(5), "FLUTE", false),
        spot(East, Palace(6), "CROSS", false),
        spot(DeathMountain, Field, "HAMMER", false),
        spot(East, TownItem(Town::OldKasuto), "MAGIC KEY", false),
        spot(West, Field, "TROPHY", false),
        spot(West, Field, "MEDICINE", false),
        spot(MazeIsland, Field, "CHILD", false),
        spot(West, TownItem(Town::Saria), "MIRROR", false),
        spot(East, TownItem(Town::Nabooru), "WATER", false),
        spot(West, TownItem(Town::Bagu), "BAGUS NOTE", false),
    ]);
    v
}

/// The placement the hints describe: the shared world model when the
/// pipeline built one ([`crate::world::World`], kept current by every module
/// that moves items, spells or palaces), else what modules recorded in
/// [`crate::State::hint_spots`], else the vanilla layout.
#[must_use]
pub fn world_spots(ctx: &Ctx) -> Vec<Spot> {
    if ctx.state.world.built {
        let v = spots_from_world(&ctx.state.world);
        if !v.is_empty() {
            return v;
        }
    }
    if !ctx.state.hint_spots.is_empty() {
        return ctx.state.hint_spots.clone();
    }
    vanilla_spots()
}

/// Dialog-safe name of an item (upper case, only glyphs the font has).
#[must_use]
pub fn dialog_item_name(i: crate::world::ItemId) -> String {
    use crate::world::ItemId as I;
    match i {
        I::SmallBag | I::MediumBag | I::LargeBag | I::XlBag => "P-BAG".into(),
        I::OneUp => "1-UP DOLL".into(),
        I::BaguNote => "BAGUS NOTE".into(),
        other => other
            .name()
            .to_ascii_uppercase()
            .chars()
            .filter(|&c| text::char_to_byte(c).is_some())
            .collect(),
    }
}

fn town_of(t: crate::world::Town) -> Town {
    use crate::world::Town as W;
    match t {
        W::Rauru => Town::Rauru,
        W::Ruto => Town::Ruto,
        W::Saria => Town::Saria,
        W::Mido => Town::Mido,
        W::Nabooru => Town::Nabooru,
        W::Darunia => Town::Darunia,
        W::NewKasuto => Town::NewKasuto,
        W::OldKasuto => Town::OldKasuto,
        W::Bagu => Town::Bagu,
    }
}

/// Hint spots for every item location of `w` that holds an item.
#[must_use]
pub fn spots_from_world(w: &crate::world::World) -> Vec<Spot> {
    use crate::world::{Continent, LocKey, TownSlot};
    let mut out = Vec::new();
    for loc in &w.locs {
        let Some(item) = loc.item else { continue };
        let Some(spot) = w.spots.get(loc.spot) else {
            continue;
        };
        let region = match spot.continent {
            Continent::West => Region::West,
            Continent::DeathMountain => Region::DeathMountain,
            Continent::East => Region::East,
            Continent::MazeIsland => Region::MazeIsland,
        };
        let place = match loc.key {
            LocKey::Area(..) => Place::Field,
            LocKey::PalaceItem { palace, .. } => {
                Place::Palace(palace_site(spot.continent, spot.slot).unwrap_or(palace))
            }
            LocKey::Town(TownSlot::Wizard(t)) => Place::Wizard(town_of(t)),
            LocKey::Town(ts @ (TownSlot::MidoTrainer | TownSlot::DaruniaTrainer)) => {
                Place::Teacher(town_of(ts.town()))
            }
            LocKey::Town(ts) => Place::TownItem(town_of(ts.town())),
            LocKey::Boss(_) | LocKey::Thunderbird | LocKey::DarkLink => continue,
        };
        out.push(Spot {
            region,
            place,
            item: dialog_item_name(item),
            spell: item.is_spell() || item == crate::world::ItemId::Dash,
        });
    }
    out
}

/// Palace site number (1-7) of an overworld location-table slot: palaces
/// 1-3 stand at West slots 52-54, palace 4 at Maze Island slot 52, palaces
/// 5, 6 and the Great Palace at East slots 52-54.
#[must_use]
pub fn palace_site(c: crate::world::Continent, slot: u8) -> Option<u8> {
    use crate::world::Continent as C;
    match (c, slot) {
        (C::West, 52..=54) => Some(slot - 51),
        (C::MazeIsland, 52) => Some(4),
        (C::East, 52..=54) => Some(slot - 47),
        _ => None,
    }
}

/// Items Link starts with (they are not worth a hint).
fn starting_items(ctx: &Ctx) -> Vec<String> {
    if ctx.state.world.built {
        return crate::world::ItemId::TOOLS
            .iter()
            .filter(|&&i| ctx.state.world.start.has(i))
            .map(|&i| dialog_item_name(i))
            .collect();
    }
    let s = &ctx.flags.start;
    let mut v = Vec::new();
    for (on, name) in [
        (s.start_with_candle, "CANDLE"),
        (s.start_with_glove, "GLOVE"),
        (s.start_with_raft, "RAFT"),
        (s.start_with_boots, "BOOTS"),
        (s.start_with_flute, "FLUTE"),
        (s.start_with_cross, "CROSS"),
        (s.start_with_hammer, "HAMMER"),
        (s.start_with_magic_key, "MAGIC KEY"),
    ] {
        if on {
            v.push(name.to_string());
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Dialog slots.
// ---------------------------------------------------------------------------

/// A townsperson whose line can carry a helpful hint.
#[derive(Debug, Clone, Copy)]
pub struct HintSlot {
    /// Dialog index.
    pub dialog: usize,
    /// Town, for the one-hint-per-town rule.
    pub town: Option<Town>,
    /// Who says it (spoiler log).
    pub who: &'static str,
}

/// Old Kasuto's readable wall: always a real hint.
pub const OLD_KASUTO_WALL: usize = 74;

/// Every stationary townsperson whose vanilla line is advice.
pub const HINT_SLOTS: &[HintSlot] = &[
    HintSlot {
        dialog: 32,
        town: Some(Town::Rauru),
        who: "Rauru, woman in the street",
    },
    HintSlot {
        dialog: 30,
        town: Some(Town::Rauru),
        who: "Rauru, man in the first house",
    },
    HintSlot {
        dialog: 12,
        town: Some(Town::Rauru),
        who: "Rauru, child in the second house",
    },
    HintSlot {
        dialog: 18,
        town: Some(Town::Ruto),
        who: "Ruto, woman in the street",
    },
    HintSlot {
        dialog: 33,
        town: Some(Town::Ruto),
        who: "Ruto, woman in a house",
    },
    HintSlot {
        dialog: 28,
        town: Some(Town::Saria),
        who: "Saria, man at the gate",
    },
    HintSlot {
        dialog: 45,
        town: Some(Town::Mido),
        who: "Mido, man in the first house",
    },
    HintSlot {
        dialog: 51,
        town: None,
        who: "King's tomb",
    },
    HintSlot {
        dialog: 67,
        town: Some(Town::Nabooru),
        who: "Nabooru, man in the first house",
    },
    HintSlot {
        dialog: 64,
        town: Some(Town::Nabooru),
        who: "Nabooru, woman in the street",
    },
    HintSlot {
        dialog: 77,
        town: Some(Town::Darunia),
        who: "Darunia, writing on a wall",
    },
    HintSlot {
        dialog: 73,
        town: Some(Town::Darunia),
        who: "Darunia, child in the street",
    },
    HintSlot {
        dialog: 83,
        town: Some(Town::NewKasuto),
        who: "New Kasuto, woman at the gate",
    },
    HintSlot {
        dialog: 68,
        town: Some(Town::NewKasuto),
        who: "New Kasuto, writing on a wall",
    },
    HintSlot {
        dialog: OLD_KASUTO_WALL,
        town: Some(Town::OldKasuto),
        who: "Old Kasuto, writing on a wall",
    },
];

/// Number of real helpful hints per seed.
pub const HELPFUL_HINT_COUNT: usize = 4;

/// Quest-item askers: (dialog, town whose wizard they lead to, request).
pub const QUEST_ASKERS: [(usize, Town, &str); 5] = [
    (13, Town::Ruto, "FIND OUR TROPHY"),
    (43, Town::Mido, "CURE MY GIRL"),
    (79, Town::Darunia, "SAVE MY CHILD"),
    (65, Town::Nabooru, "BRING ME WATER"),
    (22, Town::Saria, "FIND MY MIRROR"),
];

/// Closed stab-teacher doors: (dialog, teacher town).
pub const TEACHER_DOORS: [(usize, Town); 2] = [(42, Town::Mido), (78, Town::Darunia)];

/// Walking townsfolk lines (shared by every walker of a kind).
pub const WALKER_DIALOGS: &[usize] = &[
    4, 5, 6, 7, 8, 9, 10, 17, 19, 27, 39, 40, 56, 57, 58, 59, 60, 61, 72, 75, 88, 89,
];

/// "Nothing more to teach you" lines (west and east tables).
pub const ALREADY_DIALOGS: [usize; 2] = [16, 71];

// ---------------------------------------------------------------------------
// Our own text.
// ---------------------------------------------------------------------------

/// Filler for hint-capable townsfolk without a real hint.
pub const KNOW_NOTHING: &[&str] = &[
    "I HAVE NOTHING USEFUL TO SAY.",
    "ASK SOMEONE ELSE. I JUST LIVE HERE.",
    "MY MEMORY IS FOGGY TODAY.",
    "SECRETS? NOT FROM ME.",
];

/// Flavour lines for walking townsfolk (community text).
pub const WALKER_LINES: &[&str] = &[
    "THE BREAD HERE IS STALE AGAIN.",
    "I SAW A BOT JUMP TWICE. NOBODY CARES.",
    "KEEP YOUR SHIELD UP AND HOPES HIGH.",
    "THE WISE MAN NEVER PAYS FOR HIS TEA.",
    "PLEASE DO NOT FEED THE MOBLINS.",
    "A FAIRY STOLE MY LUNCH. RUDE.",
    "WALKING ALL DAY IS GOOD FOR THE KNEES.",
    "SOME DOORS ARE SHY. TRY AGAIN LATER.",
    "CANDLES ARE CHEAP. DARKNESS IS FREE.",
    "I COUNT THE BRICKS. THERE ARE MANY.",
    "THE RIVER IS COLD. I CHECKED.",
    "HAVE YOU TRIED LOOKING DOWN?",
    "HEROES NEVER LISTEN TO MY STORIES.",
    "I SELL NOTHING. BUSINESS IS SLOW.",
    "PALACES ARE FULL OF RUDE STATUES.",
    "A RED JAR A DAY KEEPS THE HEALER AWAY.",
    "I THINK A CLOUD IS FOLLOWING ME.",
    "WHY DOES EVERYONE WALK BACK AND FORTH?",
    "MY FEET HURT. I WALK HERE ALL DAY.",
    "LEARN EVERY SPELL. YOU NEVER KNOW.",
    "DO NOT TRUST THE QUIET BRICKS.",
    "THE ROAD EAST IS LONG AND LONELY.",
    "IF YOU MEET A SLIME, BE POLITE.",
    "IS IT LUNCH TIME YET?",
    "I TRADED MY SWORD FOR A SANDWICH.",
    "BEWARE OF SPIDERS THAT FALL FROM ABOVE.",
    "MY COUSIN SWEEPS THE GREAT PALACE.",
    "THE BIRDS HERE SING OFF KEY.",
    "SAVE OFTEN. TRUST ME.",
    "I LOST A BET TO A BUBBLE.",
    "NEVER ARGUE WITH AN IRON KNUCKLE.",
    "ONE DAY I WILL VISIT THE EAST.",
];

/// Flavour for the "nothing more to teach" lines (community text).
pub const ALREADY_LINES: &[&str] = &[
    "YOU ALREADY KNOW THIS. GO PRACTICE.",
    "AGAIN? I HAVE NOTHING NEW FOR YOU.",
    "THAT IS ALL I KNOW. GOOD LUCK.",
];

/// Wrap `text` into a 4-line box, or `None` if it does not fit.
fn fit(text_: &str) -> Option<String> {
    text::wrap(text_, text::VANILLA_MAX_LINES)
}

/// First template of `templates` (filled with `fill`) that fits the box,
/// trying them from a random start.
fn first_fit(rng: &mut Rng, templates: &[&str], fill: &dyn Fn(&str) -> String) -> Option<String> {
    let start = rng.index(templates.len());
    (0..templates.len())
        .map(|k| templates[(start + k) % templates.len()])
        .find_map(|t| fit(&fill(t)))
}

/// An item name squeezed into the box no matter what.
fn last_resort(item: &str) -> String {
    fit(item).unwrap_or_else(|| item.chars().take(text::MAX_LINE_CHARS).collect())
}

/// Where-is text for one item.
#[must_use]
pub fn helpful_hint_text(rng: &mut Rng, mode: HelpfulHints, s: &Spot) -> String {
    let item = &s.item;
    let region = s.region.phrase();
    let named = match (mode, s.place) {
        (
            HelpfulHints::TownsSeparate,
            Place::Wizard(t) | Place::Teacher(t) | Place::TownItem(t),
        ) => Some(t.name().to_string()),
        (HelpfulHints::TownsSeparate, Place::Palace(7)) => Some("THE GREAT PALACE".to_string()),
        (HelpfulHints::TownsSeparate, Place::Palace(n)) => Some(format!("PALACE {n}")),
        _ => None,
    };
    let fill = |t: &str| {
        t.replace("{ITEM}", item)
            .replace("{REGION}", region)
            .replace("{PLACE}", named.as_deref().unwrap_or(region))
    };
    let templates: &[&str] = if named.is_some() {
        &[
            "{PLACE} HOLDS THE {ITEM}.",
            "THE {ITEM} WAITS IN {PLACE}.",
            "SEEK THE {ITEM} IN {PLACE}.",
        ]
    } else {
        match s.place {
            Place::Palace(_) => &[
                "A PALACE IN {REGION} HIDES THE {ITEM}.",
                "THE {ITEM} SLEEPS IN A PALACE OF {REGION}.",
            ],
            Place::Field => &[
                "THE {ITEM} LIES OUT IN {REGION}.",
                "SEARCH {REGION} FOR THE {ITEM}.",
            ],
            _ => &[
                "SOMEONE IN {REGION} HAS THE {ITEM}.",
                "A TOWN OF {REGION} KEEPS THE {ITEM}.",
            ],
        }
    };
    first_fit(rng, templates, &fill)
        .or_else(|| fit(&format!("THE {item} IS IN {region}.")))
        .or_else(|| fit(&format!("{item}: {region}")))
        .unwrap_or_else(|| last_resort(item))
}

/// Text for a quest-item asker: the request plus the wizard's reward.
#[must_use]
pub fn quest_hint_text(request: &str, reward: &Spot) -> String {
    let verb = if reward.spell { "LEARN" } else { "GET THE" };
    fit(&format!("{request} AND {verb} {}.", reward.item))
        .or_else(|| fit(&format!("{request}. REWARD: {}.", reward.item)))
        .or_else(|| fit(&format!("REWARD: {}.", reward.item)))
        .unwrap_or_else(|| last_resort(&reward.item))
}

/// Text for a closed stab-teacher door.
#[must_use]
pub fn teacher_door_text(reward: &Spot) -> String {
    fit(&format!(
        "THE DOOR IS SHUT. INSIDE, A KNIGHT OFFERS {}.",
        reward.item
    ))
    .or_else(|| fit(&format!("THE KNIGHT INSIDE OFFERS {}.", reward.item)))
    .unwrap_or_else(|| last_resort(&reward.item))
}

/// Text for a town sign naming the wizard's reward.
#[must_use]
pub fn town_sign_text(town: Town, reward: &Spot) -> String {
    let item =
        text::wrap(&reward.item, 2).unwrap_or_else(|| reward.item.chars().take(11).collect());
    format!("{}\nWIZARD HAS\n{item}", town.name())
}

// ---------------------------------------------------------------------------
// The pass.
// ---------------------------------------------------------------------------

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let spots = world_spots(ctx);
    let mut used: Vec<usize> = Vec::new();
    // A dialog table that cannot be read (only synthetic test images) means
    // no text options.
    let text_ok = ctx.state.text.is_some() || text::TextTable::read(&ctx.rom).is_ok();
    if !text_ok {
        ctx.log("hints: dialog table unreadable, text options skipped");
    }

    if text_ok && ctx.flags.hints.helpful_hints != HelpfulHints::None {
        let mode = ctx.flags.hints.helpful_hints;
        used.extend(place_helpful_hints(ctx, mode, &spots)?);
    }
    let spell_item = ctx.tri(ctx.flags.hints.spell_item_hints);
    if text_ok && spell_item {
        place_spell_item_hints(ctx, &spots)?;
    }
    let town_names = ctx.tri(ctx.flags.hints.town_name_hints);
    if text_ok && town_names {
        place_town_name_hints(ctx, &spots)?;
    }
    if text_ok && ctx.flags.cosmetic.community_text {
        place_community_text(ctx, &used)?;
    }
    if ctx.flags.hints.reveal_walkthrough_walls {
        reveal_walkthrough_walls(ctx)?;
    }
    if ctx.flags.hints.reveal_hidden_jars {
        reveal_hidden_jars(ctx)?;
    }
    if ctx.flags != crate::flags::Flags::default() {
        let hash = crate::hash_code(&ctx.seed, &ctx.flags);
        if !write_select_screen_hash(&mut ctx.rom, &hash)? {
            ctx.log("hints: file-select heading not found, hash not drawn");
        }
    }
    // Fail early (with a retry) rather than at the final write.
    if let Some(t) = ctx.state.text.as_ref() {
        let need = t.packed_len();
        if need > text::TEXT_BUDGET {
            return Err(RandoError::Retry(format!(
                "dialog text needs {need} bytes of {}",
                text::TEXT_BUDGET
            )));
        }
    }
    Ok(())
}

fn find_spot(spots: &[Spot], place: Place) -> Option<&Spot> {
    spots.iter().find(|s| s.place == place)
}

/// Pick slots and items for the helpful hints; returns the dialog indices
/// that got a real hint.
fn place_helpful_hints(
    ctx: &mut Ctx,
    mode: HelpfulHints,
    spots: &[Spot],
) -> Result<Vec<usize>, RandoError> {
    let start = starting_items(ctx);
    let mut targets: Vec<&Spot> = KEY_ITEMS
        .iter()
        .filter(|k| !start.iter().any(|s| s == *k))
        .filter_map(|k| spots.iter().find(|s| s.item == *k))
        .collect();
    ctx.rng.shuffle(&mut targets);
    targets.truncate(HELPFUL_HINT_COUNT);

    // Slots: Old Kasuto's wall first, then one random slot from each of
    // other towns, in random order.
    let mut slots: Vec<HintSlot> = vec![*HINT_SLOTS
        .iter()
        .find(|s| s.dialog == OLD_KASUTO_WALL)
        .expect("listed")];
    let mut towns: Vec<Option<Town>> = Vec::new();
    for s in HINT_SLOTS {
        if s.dialog != OLD_KASUTO_WALL && !towns.contains(&s.town) {
            towns.push(s.town);
        }
    }
    ctx.rng.shuffle(&mut towns);
    for t in towns {
        let choices: Vec<HintSlot> = HINT_SLOTS
            .iter()
            .copied()
            .filter(|s| s.town == t && s.dialog != OLD_KASUTO_WALL)
            .collect();
        if let Some(&s) = ctx.rng.pick(&choices) {
            slots.push(s);
        }
    }

    let filler = KNOW_NOTHING[ctx.rng.index(KNOW_NOTHING.len())];
    let filler = fit(filler).expect("filler fits");
    let mut placed = Vec::new();
    let mut lines = Vec::new();
    for (k, slot) in slots.iter().enumerate() {
        let text_ = match targets.get(k) {
            Some(target) => {
                let t = helpful_hint_text(&mut ctx.rng, mode, target);
                lines.push(format!("{}: {}", slot.who, t.replace('\n', " ")));
                placed.push(slot.dialog);
                t
            }
            None => filler.clone(),
        };
        text::table(ctx)?.set(slot.dialog, &text_)?;
    }
    // Leftover advice-givers (other slots in the same towns) know nothing.
    for s in HINT_SLOTS {
        if !slots.iter().any(|x| x.dialog == s.dialog) {
            text::table(ctx)?.set(s.dialog, &filler)?;
        }
    }
    for l in lines {
        ctx.spoiler.line("Hints", l);
    }
    Ok(placed)
}

fn place_spell_item_hints(ctx: &mut Ctx, spots: &[Spot]) -> Result<(), RandoError> {
    for (dialog, town, request) in QUEST_ASKERS {
        if let Some(reward) = find_spot(spots, Place::Wizard(town)) {
            let t = quest_hint_text(request, reward);
            ctx.spoiler.line(
                "Hints",
                format!("{} quest giver: {}", town.name(), t.replace('\n', " ")),
            );
            text::table(ctx)?.set(dialog, &t)?;
        }
    }
    for (dialog, town) in TEACHER_DOORS {
        if let Some(reward) = find_spot(spots, Place::Teacher(town)) {
            let t = teacher_door_text(reward);
            ctx.spoiler.line(
                "Hints",
                format!("{} teacher door: {}", town.name(), t.replace('\n', " ")),
            );
            text::table(ctx)?.set(dialog, &t)?;
        }
    }
    Ok(())
}

fn place_town_name_hints(ctx: &mut Ctx, spots: &[Spot]) -> Result<(), RandoError> {
    for town in Town::WIZARDS {
        let (Some(dialog), Some(reward)) =
            (town.sign_dialog(), find_spot(spots, Place::Wizard(town)))
        else {
            continue;
        };
        let t = town_sign_text(town, reward);
        ctx.spoiler.line(
            "Hints",
            format!("{} sign: {}", town.name(), t.replace('\n', " ")),
        );
        text::table(ctx)?.set(dialog, &t)?;
    }
    Ok(())
}

/// Community text: random flavour lines, drawn from a stream that does not
/// touch the seed's other choices, and only while the text still fits.
fn place_community_text(ctx: &mut Ctx, used: &[usize]) -> Result<(), RandoError> {
    let mut rng = ctx.rng.derive("community text");
    let mut pool: Vec<&str> = WALKER_LINES.to_vec();
    rng.shuffle(&mut pool);
    let mut targets: Vec<usize> = WALKER_DIALOGS
        .iter()
        .copied()
        .filter(|d| !used.contains(d))
        .collect();
    rng.shuffle(&mut targets);
    let already = ALREADY_LINES[rng.index(ALREADY_LINES.len())];
    let jobs: Vec<(usize, &str)> = targets
        .into_iter()
        .zip(pool)
        .chain(ALREADY_DIALOGS.iter().map(|&d| (d, already)))
        .collect();
    for (dialog, line) in jobs {
        let Some(t) = fit(line) else { continue };
        let table = text::table(ctx)?;
        let before = table.bytes(dialog).to_vec();
        table.set(dialog, &t)?;
        if table.packed_len() > text::TEXT_BUDGET {
            table.set_bytes(dialog, before)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ROM patches.
// ---------------------------------------------------------------------------

/// The file-select heading: PPU address and length bytes, then 11 tiles.
pub const SELECT_HEADING: u16 = 0xBC19;
/// Bank of the file-select screen data.
pub const SELECT_BANK: u8 = 5;

/// Write `hash` (six characters) over the file-select heading. Returns
/// `false` (and writes nothing) when the heading is not where the vanilla
/// game keeps it.
pub fn write_select_screen_hash(rom: &mut crate::rom::Rom, hash: &str) -> Result<bool, RandoError> {
    let head = [
        rom.read_cpu(SELECT_BANK, SELECT_HEADING)?,
        rom.read_cpu(SELECT_BANK, SELECT_HEADING + 1)?,
        rom.read_cpu(SELECT_BANK, SELECT_HEADING + 2)?,
    ];
    if head != [0x20, 0x6A, 0x0B] {
        return Ok(false);
    }
    let glyphs = hash_glyphs(hash)?;
    rom.write_cpu(SELECT_BANK, SELECT_HEADING + 3, &glyphs)?;
    Ok(true)
}

/// The 11 tile bytes for a six-character hash: glyphs separated by spaces.
pub fn hash_glyphs(hash: &str) -> Result<[u8; 11], RandoError> {
    let chars: Vec<char> = hash.chars().collect();
    if chars.len() != 6 {
        return Err(RandoError::Other(format!(
            "hash {hash:?} is not 6 characters"
        )));
    }
    let mut out = [text::SPACE; 11];
    for (i, c) in chars.into_iter().enumerate() {
        out[2 * i] =
            text::char_to_byte(c).ok_or_else(|| RandoError::Text(format!("hash glyph {c:?}")))?;
    }
    Ok(out)
}

/// (bank, address, vanilla pair) of the false-wall tile pairs.
pub const WALKTHROUGH_TILES: [(u8, u16, [u8; 2]); 2] =
    [(4, 0x818D, [0x47, 0x47]), (5, 0x81A3, [0x49, 0x49])];
/// Plain background tile.
pub const BACKGROUND_TILE: u8 = 0x02;

fn reveal_walkthrough_walls(ctx: &mut Ctx) -> Result<(), RandoError> {
    for (bank, addr, vanilla) in WALKTHROUGH_TILES {
        let cur = [
            ctx.rom.read_cpu(bank, addr)?,
            ctx.rom.read_cpu(bank, addr + 1)?,
        ];
        if cur != vanilla {
            ctx.log(format!(
                "hints: walkthrough tiles at {bank}:${addr:04X} already changed, left alone"
            ));
            continue;
        }
        ctx.rom
            .write_cpu(bank, addr, &[BACKGROUND_TILE, BACKGROUND_TILE])?;
    }
    Ok(())
}

/// Hidden-jar hook sites: (bank, main-routine pointer slots, draw pointer
/// slots, palette byte).
struct JarBank {
    bank: u8,
    mains: &'static [u16],
    draws: &'static [u16],
    palettes: &'static [u16],
}

const JAR_BANKS: [JarBank; 2] = [
    JarBank {
        bank: 4,
        mains: &[0x9497, 0xA997],
        draws: &[0x956F, 0xAA6F],
        palettes: &[0x94DA, 0xA9DA],
    },
    JarBank {
        bank: 5,
        mains: &[0x9497],
        draws: &[0x956F],
        palettes: &[0x94DA],
    },
];

/// Common enemy draw routine (fixed bank).
pub const ENEMY_DRAW: u16 = 0xDE3D;
/// Draws one tile from the common enemy tile table (fixed bank), tile in X.
pub const DRAW_COMMON_TILE: u16 = 0xF0C6;
/// Jar tile index in the common table.
pub const JAR_TILE: u8 = 0x50;
/// Orange sprite palette for the marker.
pub const JAR_PALETTE: u8 = 0x40;

fn reveal_hidden_jars(ctx: &mut Ctx) -> Result<(), RandoError> {
    for jb in &JAR_BANKS {
        let mut src =
            format!("ENEMY_DRAW = ${ENEMY_DRAW:04X}\nDRAW_TILE = ${DRAW_COMMON_TILE:04X}\n");
        // Size first, then allocate, then assemble at the real address.
        let originals: Vec<u16> = jb
            .mains
            .iter()
            .map(|&a| ctx.rom.read_cpu_word(jb.bank, a))
            .collect::<Result<_, _>>()?;
        for (k, orig) in originals.iter().enumerate() {
            src.push_str(&format!(
                "main{k}:\n    JSR ENEMY_DRAW\n    JMP ${orig:04X}\n"
            ));
        }
        src.push_str(&format!(
            "draw:\n    LDA $01\n    CLC\n    ADC #$04\n    BCS off\n    STA $01\n    \
             LDA #$00\n    STA $02\n    LDX #${JAR_TILE:02X}\n    JMP DRAW_TILE\noff:\n    RTS\n"
        ));
        let size = crate::asm::assemble_at(0x8000, &src)?.len();
        let base = ctx.rom.alloc_vanilla(jb.bank, size)?;
        let out = crate::asm::assemble(&format!(".org ${base:04X}\n{src}"))?;
        ctx.rom.apply_asm(jb.bank, &out)?;
        for (k, &slot) in jb.mains.iter().enumerate() {
            let a = out
                .symbol(&format!("main{k}"))
                .ok_or_else(|| RandoError::Asm("missing label".into()))?;
            ctx.rom.write_cpu_word(jb.bank, slot, a)?;
        }
        let draw = out
            .symbol("draw")
            .ok_or_else(|| RandoError::Asm("missing label".into()))?;
        for &slot in jb.draws {
            ctx.rom.write_cpu_word(jb.bank, slot, draw)?;
        }
        for &p in jb.palettes {
            ctx.rom.write_cpu(jb.bank, p, &[JAR_PALETTE])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::Flags;

    #[test]
    fn every_line_of_our_text_fits_and_encodes() {
        for l in KNOW_NOTHING.iter().chain(WALKER_LINES).chain(ALREADY_LINES) {
            let t = fit(l).unwrap_or_else(|| panic!("{l:?} does not fit"));
            text::validate_dialog(&t, 4).unwrap();
            text::encode_message(&t).unwrap();
        }
        // The vanilla spots cover every key item once.
        let v = vanilla_spots();
        for k in KEY_ITEMS {
            assert_eq!(v.iter().filter(|s| s.item == k).count(), 1, "{k}");
        }
    }

    #[test]
    fn hint_texts_fit_for_every_spot_and_mode() {
        let mut rng = Rng::new(9);
        for s in vanilla_spots() {
            for mode in [HelpfulHints::ContinentOnly, HelpfulHints::TownsSeparate] {
                for _ in 0..8 {
                    let t = helpful_hint_text(&mut rng, mode, &s);
                    text::validate_dialog(&t, 4).unwrap();
                    text::encode_message(&t).unwrap();
                    assert!(t.contains(s.item.split(' ').next().unwrap()), "{t}");
                }
            }
            text::validate_dialog(&quest_hint_text("SAVE MY CHILD", &s), 4).unwrap();
            text::validate_dialog(&teacher_door_text(&s), 4).unwrap();
            for town in Town::WIZARDS {
                text::validate_dialog(&town_sign_text(town, &s), 4).unwrap();
            }
        }
    }

    #[test]
    fn every_item_name_works_in_every_text() {
        let mut rng = Rng::new(11);
        let places = [
            Place::Wizard(Town::Ruto),
            Place::Teacher(Town::Mido),
            Place::TownItem(Town::NewKasuto),
            Place::Palace(7),
            Place::Field,
        ];
        for &i in crate::world::ItemId::ALL {
            let name = dialog_item_name(i);
            text::encode(&name).unwrap();
            for place in places {
                let s = Spot {
                    region: Region::DeathMountain,
                    place,
                    item: name.clone(),
                    spell: i.is_spell(),
                };
                for mode in [HelpfulHints::ContinentOnly, HelpfulHints::TownsSeparate] {
                    let t = helpful_hint_text(&mut rng, mode, &s);
                    text::validate_dialog(&t, 4).unwrap();
                }
                for (_, _, req) in QUEST_ASKERS {
                    text::validate_dialog(&quest_hint_text(req, &s), 4).unwrap();
                }
                text::validate_dialog(&teacher_door_text(&s), 4).unwrap();
                text::validate_dialog(&town_sign_text(Town::NewKasuto, &s), 4).unwrap();
            }
        }
    }

    #[test]
    fn continent_mode_never_names_a_town() {
        let mut rng = Rng::new(3);
        for s in vanilla_spots() {
            let t = helpful_hint_text(&mut rng, HelpfulHints::ContinentOnly, &s);
            for town in Town::WIZARDS {
                assert!(!t.contains(town.name()), "{t}");
            }
            for n in 1..=7 {
                assert!(!t.contains(&format!("PALACE {n}")), "{t}");
            }
        }
    }

    #[test]
    fn hash_glyph_layout() {
        let g = hash_glyphs("R9BLQ4").unwrap();
        assert_eq!(text::decode(&g), "R 9 B L Q 4");
        assert!(hash_glyphs("ABC").is_err());
    }

    #[test]
    fn synthetic_image_without_vanilla_data_is_left_alone() {
        use crate::flags::Tri;
        let mut rng = Rng::new(77);
        let body: Vec<u8> = (0..crate::rom::VANILLA_BODY_LEN)
            .map(|_| rng.next_u32() as u8)
            .collect();
        let mut f = Flags::default();
        f.hints.helpful_hints = HelpfulHints::ContinentOnly;
        f.hints.spell_item_hints = Tri::On;
        f.hints.town_name_hints = Tri::On;
        f.cosmetic.community_text = true;
        f.towns.shorten_wizards = true;
        f.towns.randomize_new_kasuto_jar_requirements = true;
        let out = crate::randomize_unverified(&body, "x", &f, &crate::Extras::default()).unwrap();
        let diff: Vec<usize> = (0..body.len())
            .filter(|&i| out.body[i] != body[i])
            .collect();
        assert!(
            diff.is_empty(),
            "{} bytes changed, first at {:#X}: {:?}",
            diff.len(),
            diff[0],
            out.log
        );
    }

    fn rom_body() -> Vec<u8> {
        z2_assets::rom::open().expect("Z2_ROM")
    }

    /// ROM-gated: every hint option on, several seeds: the dialog fits,
    /// decodes cleanly, the hints landed where expected and the hash shows
    /// on the file-select screen.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_hints_all_on() {
        use crate::flags::Tri;
        let body = rom_body();
        for seed in ["a", "b", "hints 3", "4", "five"] {
            for mode in [HelpfulHints::ContinentOnly, HelpfulHints::TownsSeparate] {
                let mut f = Flags::default();
                f.hints.helpful_hints = mode;
                f.hints.spell_item_hints = Tri::On;
                f.hints.town_name_hints = Tri::On;
                f.cosmetic.community_text = true;
                let out = crate::randomize(&body, seed, &f).unwrap();
                let rom = crate::rom::Rom::from_body(&out.body).unwrap();
                let msgs = text::read_vanilla_messages(&rom).unwrap();
                for m in &msgs {
                    let s = text::decode(m);
                    assert!(!s.contains('{'), "{s:?}");
                    text::validate_dialog(&s, 4).unwrap();
                }
                let wall = text::decode(&msgs[OLD_KASUTO_WALL]);
                assert!(
                    KEY_ITEMS
                        .iter()
                        .any(|k| wall.replace('\n', " ").contains(k)),
                    "{wall}"
                );
                let sign = text::decode(&msgs[11]);
                assert!(sign.contains("SHIELD"), "{sign}");
                let trophy = text::decode(&msgs[13]).replace('\n', " ");
                assert!(trophy.contains("JUMP"), "{trophy}");
                let heading = rom
                    .read_slice(rom.cpu_offset(5, 0xBC1C).unwrap(), 11)
                    .unwrap();
                assert_eq!(text::decode(heading).replace(' ', ""), out.hash_code);
                assert_eq!(out.spoiler.matches("== Hints ==").count(), 1);
                // Text only lives in the text block; nothing else in bank 3
                // moved.
                let van = crate::rom::Rom::from_body(&body).unwrap();
                let lo = van.cpu_offset(3, 0x8000).unwrap();
                let a = van.cpu_offset(3, text::TEXT_DATA_START).unwrap();
                let b = van.cpu_offset(3, 0xB082).unwrap();
                assert_eq!(
                    rom.read_slice(lo, a - lo).unwrap(),
                    van.read_slice(lo, a - lo).unwrap()
                );
                let hi = van.cpu_offset(3, 0xBFFF).unwrap();
                assert_eq!(
                    rom.read_slice(b, hi - b).unwrap(),
                    van.read_slice(b, hi - b).unwrap()
                );
            }
        }
    }

    /// ROM-gated: the reveal options touch only their bytes, and community
    /// text does not change anything but text.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_reveal_options_and_community_text() {
        let body = rom_body();
        let van = crate::rom::Rom::from_body(&body).unwrap();
        let mut f = Flags::default();
        f.hints.reveal_walkthrough_walls = true;
        f.hints.reveal_hidden_jars = true;
        let out = crate::randomize(&body, "walls", &f).unwrap();
        let rom = crate::rom::Rom::from_body(&out.body).unwrap();
        assert_eq!(rom.read_cpu(4, 0x818D).unwrap(), BACKGROUND_TILE);
        assert_eq!(rom.read_cpu(5, 0x81A4).unwrap(), BACKGROUND_TILE);
        for jb in &JAR_BANKS {
            for (k, &slot) in jb.mains.iter().enumerate() {
                let hook = rom.read_cpu_word(jb.bank, slot).unwrap();
                assert!(
                    crate::rom::claimed_ranges(crate::rom::Owner::ModuleAlloc("hints"))
                        .iter()
                        .any(|&(b, s, e)| b == jb.bank && hook >= s && u32::from(hook) < e),
                    "{hook:04X}"
                );
                // JSR ENEMY_DRAW ; JMP <vanilla routine>
                assert_eq!(rom.read_cpu(jb.bank, hook).unwrap(), 0x20);
                assert_eq!(rom.read_cpu_word(jb.bank, hook + 1).unwrap(), ENEMY_DRAW);
                assert_eq!(rom.read_cpu(jb.bank, hook + 3).unwrap(), 0x4C);
                assert_eq!(
                    rom.read_cpu_word(jb.bank, hook + 4).unwrap(),
                    van.read_cpu_word(jb.bank, jb.mains[k]).unwrap()
                );
            }
        }
        assert!(out.fixed_bank_changes.is_empty());

        // Community text is cosmetic: same hash, and only bank-3 text and
        // the select heading differ from a run without it.
        let mut g = Flags::default();
        g.hints.reveal_walkthrough_walls = true;
        g.cosmetic.community_text = true;
        let mut h = g.clone();
        h.cosmetic.community_text = false;
        let a = crate::randomize(&body, "c", &g).unwrap();
        let b = crate::randomize(&body, "c", &h).unwrap();
        assert_eq!(a.hash_code, b.hash_code);
        let ra = crate::rom::Rom::from_body(&a.body).unwrap();
        let rb = crate::rom::Rom::from_body(&b.body).unwrap();
        let t0 = ra.cpu_offset(3, text::TEXT_DATA_START).unwrap();
        let t1 = ra.cpu_offset(3, 0xB082).unwrap();
        for (i, (x, y)) in a.body.iter().zip(&b.body).enumerate() {
            if x != y {
                assert!((t0..t1).contains(&i), "byte {i:#X} differs");
            }
        }
        assert_ne!(
            text::read_vanilla_messages(&ra).unwrap(),
            text::read_vanilla_messages(&rb).unwrap()
        );
    }
}
