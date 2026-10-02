//! `overworld` module: continent shapes, terrain, location placement,
//! hidden locations, palace placement and encounters.
//!
//! Options owned: [`crate::flags::OverworldFlags`] (`ctx.flags.overworld`).
//! Catalog: section 03.
//!
//! # How it works
//!
//! Every continent starts as the vanilla one read from the player's ROM
//! ([`continent::Continent::vanilla`]). Depending on its biome it is left
//! alone, gets its locations shuffled on the vanilla terrain
//! ([`shuffle`]), or is generated from scratch ([`gen`]). Then the hidden
//! locations, palace identities and encounters are settled, the result is
//! checked with the shared logic model (every place that must be reachable
//! is reachable with every item), and finally it is written to the ROM
//! ([`write`]) and mirrored into `ctx.state.world` ([`sync`]) for the item
//! shuffle's beatability check.
//!
//! With vanilla options nothing is written.

pub mod connect;
pub mod continent;
pub mod encounters;
pub mod gen;
pub mod hidden;
pub mod loc;
pub mod map;
pub mod shuffle;
pub mod sync;
pub mod terrain;
pub mod write;

use crate::flags::FlagEnum;
use crate::flags::Tri;
use crate::flags::{
    Biome, Climate, ContinentConnections, ContinentSize, DmSize, EncounterRate,
    LessImportantLocations, MazeSize, RiverDevilBlocker,
};
use crate::rng::Rng;
use crate::world::{Inventory, World};
use crate::{Ctx, RandoError};
use continent::{Continent, Reveal};
use loc::Class;
use map::Cont;

/// Generation attempts inside one pipeline attempt before asking the
/// pipeline to start over.
const ATTEMPTS: u32 = 200;

/// Options with every "random" choice settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Biome per continent ([`Cont::index`]); never a "random" variant.
    pub biome: [Biome; 4],
    /// Climate for West, Death Mountain, East.
    pub climate: [Climate; 3],
    /// West / East size.
    pub size: [ContinentSize; 2],
    /// Death Mountain size.
    pub dm_size: DmSize,
    /// Maze Island size.
    pub maze_size: MazeSize,
    /// Boots walk on any generated water.
    pub good_boots: bool,
    /// Shuffled vanilla maps keep the vanilla icons.
    pub legacy: bool,
    /// Continent connections.
    pub connections: ContinentConnections,
    /// Minor tiles (never `Random`).
    pub less: LessImportantLocations,
    /// Bagu's woods on a generated West.
    pub bagu_woods: bool,
    /// Connection caves face each other.
    pub sane_caves: bool,
    /// Rocks may block connection caves.
    pub caves_blockable: bool,
    /// River devil placement (never `Random`).
    pub devil: RiverDevilBlocker,
    /// East path blocks may be rocks.
    pub east_rocks: bool,
    /// The east rock blocks a path (else a cave).
    pub east_rock_is_path: bool,
    /// A flute-revealed location.
    pub hide_palace: bool,
    /// A hammer-revealed location.
    pub hide_kasuto: bool,
    /// Any East location may be the hidden one.
    pub shuffle_hidden: bool,
    /// Palaces trade spots across continents.
    pub swap_palaces: bool,
    /// The Great Palace joins that trade.
    pub shuffle_gp: bool,
    /// Encounter rate (never `Random`; with `Random` this is the West
    /// Hyrule roll, see [`Resolved::rates`]).
    pub rate: EncounterRate,
    /// Rate per region: West, Death Mountain / Maze Island, East.
    pub rates: [EncounterRate; 3],
    /// Shuffle encounter scenes.
    pub shuffle_encounters: bool,
    /// Roads join the scene shuffle.
    pub path_encounters: bool,
    /// Lava joins the scene shuffle.
    pub lava_encounters: bool,
    /// West, Death Mountain, East laid out along the horizontal axis.
    pub horizontal: [bool; 3],
}

impl Resolved {
    /// The settings vanilla flags resolve to (handy for tests and tools).
    #[must_use]
    pub fn vanilla() -> Resolved {
        Resolved {
            biome: [Biome::Vanilla; 4],
            climate: [
                Climate::VanillaWeighted,
                Climate::Classic,
                Climate::VanillaWeighted,
            ],
            size: [ContinentSize::Large; 2],
            dm_size: DmSize::Large,
            maze_size: MazeSize::Large,
            good_boots: false,
            legacy: false,
            connections: ContinentConnections::Normal,
            less: LessImportantLocations::BlendIn,
            bagu_woods: true,
            sane_caves: true,
            caves_blockable: false,
            devil: RiverDevilBlocker::Path,
            east_rocks: true,
            east_rock_is_path: false,
            hide_palace: true,
            hide_kasuto: true,
            shuffle_hidden: false,
            swap_palaces: false,
            shuffle_gp: false,
            rate: EncounterRate::Normal,
            rates: [EncounterRate::Normal; 3],
            shuffle_encounters: false,
            path_encounters: false,
            lava_encounters: false,
            horizontal: [false; 3],
        }
    }

    /// Whether these options leave the overworld exactly as in the vanilla
    /// game.
    #[must_use]
    pub fn is_vanilla(&self) -> bool {
        self.biome.iter().all(|&b| b == Biome::Vanilla)
            && self.connections == ContinentConnections::Normal
            && !self.swap_palaces
            && self.hide_palace
            && self.hide_kasuto
            && self.rates == [EncounterRate::Normal; 3]
            && !self.shuffle_encounters
    }

    /// A biome that generates its own terrain.
    #[must_use]
    pub fn generated(&self, c: Cont) -> bool {
        !matches!(
            self.biome[c.index()],
            Biome::Vanilla | Biome::VanillaShuffle
        )
    }
}

fn resolve_biome(rng: &mut Rng, c: Cont, b: Biome) -> Biome {
    let pool: &[Biome] = &[
        Biome::Vanillalike,
        Biome::Islands,
        Biome::Canyon,
        Biome::Caldera,
        Biome::Mountainous,
        Biome::VanillaShuffle,
        Biome::Vanilla,
    ];
    let b = match b {
        Biome::Random => pool[rng.index(7)],
        Biome::RandomNoVanilla => pool[rng.index(6)],
        Biome::RandomNoVanillaOrShuffle => pool[rng.index(5)],
        Biome::Canyon | Biome::DryCanyon if c != Cont::Maze => {
            if rng.coin() {
                Biome::DryCanyon
            } else {
                Biome::Canyon
            }
        }
        other => other,
    };
    match (c, b) {
        (Cont::Maze, Biome::Vanilla | Biome::VanillaShuffle) => b,
        (Cont::Maze, _) => Biome::Vanillalike,
        (Cont::East, Biome::Caldera) => Biome::Volcano,
        (Cont::West | Cont::DeathMountain, Biome::Volcano) => Biome::Caldera,
        (Cont::DeathMountain, Biome::DryCanyon) => Biome::Canyon,
        _ => b,
    }
}

fn resolve_climate(rng: &mut Rng, c: Climate, dm: bool) -> Climate {
    match c {
        Climate::Random => {
            let pool: &[Climate] = if dm {
                &[
                    Climate::Classic,
                    Climate::Chaos,
                    Climate::GreatLakes,
                    Climate::Scrubland,
                ]
            } else {
                &[
                    Climate::Classic,
                    Climate::VanillaWeighted,
                    Climate::Chaos,
                    Climate::GreatLakes,
                    Climate::Scrubland,
                ]
            };
            pool[rng.index(pool.len())]
        }
        Climate::VanillaWeighted if dm => Climate::Classic,
        other => other,
    }
}

/// Settle every random choice.
pub fn resolve(ctx: &mut Ctx) -> Resolved {
    let f = ctx.flags.overworld.clone();
    let horizontal = [ctx.rng.coin(), ctx.rng.coin(), ctx.rng.coin()];
    let east_rock_is_path = ctx.rng.coin();
    let mut biome = [Biome::Vanilla; 4];
    for (i, b) in [f.west_biome, f.dm_biome, f.east_biome, f.maze_biome]
        .into_iter()
        .enumerate()
    {
        biome[i] = resolve_biome(&mut ctx.rng, Cont::from_index(i), b);
    }
    let climate = [
        resolve_climate(&mut ctx.rng, f.west_climate, false),
        resolve_climate(&mut ctx.rng, f.dm_climate, true),
        resolve_climate(&mut ctx.rng, f.east_climate, false),
    ];
    let good_boots = ctx.tri(f.good_boots);
    let bagu_woods = ctx.tri(f.generate_bagu_woods);
    let sane_caves = ctx.tri(f.restrict_connection_cave_shuffle);
    let east_rocks = ctx.tri(f.east_rocks);
    let hide_palace = ctx.tri(f.hide_palace);
    let hide_kasuto = ctx.tri(f.hide_kasuto);
    let shuffle_hidden = ctx.tri(f.shuffle_hidden_locations);
    let swap_palaces = ctx.tri(f.palaces_swap_continents);
    let shuffle_gp = ctx.tri(f.shuffle_great_palace);
    let shuffle_encounters = ctx.tri(f.shuffle_encounters);
    let less = match f.less_important_locations {
        LessImportantLocations::Random => [
            LessImportantLocations::BlendIn,
            LessImportantLocations::Isolate,
            LessImportantLocations::Remove,
        ][ctx.rng.index(3)],
        other => other,
    };
    let devil = match f.river_devil_blocker {
        RiverDevilBlocker::Random => [
            RiverDevilBlocker::Path,
            RiverDevilBlocker::Cave,
            RiverDevilBlocker::Siege,
        ][ctx.rng.index(3)],
        other => other,
    };
    let rates = match f.encounter_rate {
        EncounterRate::Random => {
            let pool = [
                EncounterRate::None,
                EncounterRate::Half,
                EncounterRate::Normal,
            ];
            [
                pool[ctx.rng.index(3)],
                pool[ctx.rng.index(3)],
                pool[ctx.rng.index(3)],
            ]
        }
        other => [other; 3],
    };
    let rate = rates[0];
    let east_vanilla_map = biome[Cont::East.index()] == Biome::Vanilla;
    let west_vanilla_map = matches!(
        biome[Cont::West.index()],
        Biome::Vanilla | Biome::VanillaShuffle
    );
    Resolved {
        biome,
        climate,
        size: [f.west_size, f.east_size],
        dm_size: f.dm_size,
        maze_size: f.maze_size,
        good_boots,
        legacy: f.legacy_vanilla_shuffled_locations,
        connections: f.continent_connections,
        less,
        bagu_woods: bagu_woods && !west_vanilla_map,
        sane_caves,
        caves_blockable: f.allow_connection_caves_blocked,
        devil,
        east_rocks,
        east_rock_is_path,
        hide_palace,
        hide_kasuto,
        shuffle_hidden: shuffle_hidden && !east_vanilla_map && (hide_palace || hide_kasuto),
        swap_palaces,
        shuffle_gp: shuffle_gp && swap_palaces,
        rate,
        rates,
        shuffle_encounters: shuffle_encounters && f.encounter_rate != EncounterRate::None,
        path_encounters: f.allow_unsafe_path_encounters && shuffle_encounters,
        lava_encounters: f.include_lava_in_encounter_shuffle && shuffle_encounters,
        horizontal,
    }
}

/// Palace slots: (continent, slot).
const PALACE_SLOTS: [(Cont, usize); 7] = [
    (Cont::West, 52),
    (Cont::West, 53),
    (Cont::West, 54),
    (Cont::Maze, 52),
    (Cont::East, 52),
    (Cont::East, 53),
    (Cont::East, 54),
];

/// Region number the game uses for a continent in a location's world bits
/// (Maze Island loads as region 1 from bank 2).
fn region(c: Cont) -> u8 {
    match c {
        Cont::West => 0,
        Cont::DeathMountain | Cont::Maze => 1,
        Cont::East => 2,
    }
}

/// Trade palace identities between palace slots (the Great Palace only
/// with `gp`). A palace is its sideview area, entry page and world group;
/// the low world bits follow the slot's continent.
fn swap_palaces(conts: &mut [Continent], rng: &mut Rng, gp: bool) {
    let slots: Vec<(Cont, usize)> = PALACE_SLOTS
        .iter()
        .copied()
        .filter(|&(c, s)| gp || !(c == Cont::East && s == 54))
        .collect();
    let ids: Vec<(u8, u8, u8, Option<u8>)> = slots
        .iter()
        .map(|&(c, s)| {
            let k = &conts[c.index()];
            let l = k.locs[s];
            (l.map, l.page, l.world & !3, k.palace[s])
        })
        .collect();
    let mut order: Vec<usize> = (0..slots.len()).collect();
    rng.shuffle(&mut order);
    for (k, &(c, s)) in slots.iter().enumerate() {
        let (map, page, base, num) = ids[order[k]];
        let cc = &mut conts[c.index()];
        let l = &mut cc.locs[s];
        if (l.map, l.page, l.world & !3) != (map, page, base) {
            cc.table_changed = true;
        }
        l.map = map;
        l.page = page;
        l.world = base | region(c);
        cc.palace[s] = num;
    }
}

/// Settle hidden locations on a vanilla-terrain East: unhide them when the
/// options say so.
fn settle_hidden(east: &mut Continent, o: &Resolved) {
    for s in 0..east.locs.len() {
        let drop = match east.hidden[s] {
            Some(Reveal::Flute) => !o.hide_palace,
            Some(Reveal::Hammer) => !o.hide_kasuto,
            None => false,
        };
        if drop {
            east.hidden[s] = None;
            east.table_changed = true;
            if let Some((r, c)) = east.locs[s].pos() {
                east.grid.set(r, c, east.icon[s]);
                east.map_changed = true;
            }
        }
    }
}

/// Take the minor locations off the map ("Remove").
fn remove_minor(c: &mut Continent) {
    for s in c.placed() {
        if c.info(s).map(|i| i.class) == Some(Class::Minor) {
            c.remove(s);
        }
    }
}

/// Every item and every magic container: the inventory used to check
/// that the layout can be finished at all.
fn everything() -> Inventory {
    let mut inv = Inventory {
        items: !0,
        hearts: 8,
        magic: 8,
    };
    inv.items &= (1u64 << 0x25) - 1;
    inv
}

/// Why the layout is impossible even with every item, if it is.
fn impossible(w: &World) -> Option<String> {
    if !w.built {
        return None;
    }
    let r = w.reachable(&everything());
    let mut missing: Vec<&str> = w
        .spots
        .iter()
        .enumerate()
        .filter(|(i, s)| s.required && s.on_map && !r.spots[*i])
        .map(|(_, s)| s.name.as_str())
        .collect();
    missing.extend(
        w.locs
            .iter()
            .enumerate()
            .filter(|(i, l)| {
                l.required && w.spots.get(l.spot).is_some_and(|s| s.on_map) && !r.locs[*i]
            })
            .map(|(_, l)| l.name.as_str()),
    );
    if !missing.is_empty() {
        return Some(format!(
            "unreachable with every item: {}",
            missing.join(", ")
        ));
    }
    // The start must lead somewhere with what Link starts with.
    if w.solve().locs.iter().filter(|&&b| b).count() < 3 {
        return Some("almost nothing is reachable from the start".into());
    }
    None
}

/// Build one candidate overworld.
fn build(
    base: &[Continent],
    o: &Resolved,
    rng: &mut Rng,
) -> Result<(Vec<Continent>, connect::Links), RandoError> {
    let mut conts = base.to_vec();
    let vanilla_map =
        |c: Cont| matches!(o.biome[c.index()], Biome::Vanilla | Biome::VanillaShuffle);
    let links = connect::assign(o.connections, rng, vanilla_map);
    connect::apply(&mut conts, &links, vanilla_map);
    for c in Cont::ALL {
        let k = &mut conts[c.index()];
        match o.biome[c.index()] {
            Biome::Vanilla => {}
            Biome::VanillaShuffle => shuffle::shuffle(
                k,
                rng,
                shuffle::ShuffleOpts {
                    legacy: o.legacy,
                    shuffle_hidden: o.shuffle_hidden,
                },
            )?,
            _ => gen::generate(k, o, rng)?,
        }
        if o.biome[c.index()] != Biome::Vanilla && o.less == LessImportantLocations::Remove {
            remove_minor(k);
        }
    }
    settle_hidden(&mut conts[Cont::East.index()], o);
    if o.swap_palaces {
        swap_palaces(&mut conts, rng, o.shuffle_gp);
    }
    if let Some(why) = dock_clash(&conts) {
        return Err(RandoError::Retry(why));
    }
    Ok((conts, links))
}

/// Bank 0 starts a raft ride whenever Link enters any location standing on
/// a raft dock's coordinates, whatever the continent. So no other location
/// may share a dock's row and column on any map.
fn dock_clash(conts: &[Continent]) -> Option<String> {
    let docks: Vec<(Cont, usize, (usize, usize))> = conts
        .iter()
        .filter_map(|c| c.pos(loc::RAFT).map(|p| (c.cont, loc::RAFT, p)))
        .collect();
    for c in conts {
        for s in c.placed() {
            let Some(p) = c.pos(s) else { continue };
            for &(dc, ds, dp) in &docks {
                if p == dp && !(c.cont == dc && s == ds) {
                    return Some(format!(
                        "{} slot {s} sits on the coordinates of the {} raft dock",
                        c.cont.name(),
                        dc.name()
                    ));
                }
            }
        }
    }
    None
}

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let o = resolve(ctx);
    if o.is_vanilla() {
        return Ok(());
    }
    let base: Vec<Continent> = match Cont::ALL
        .iter()
        .map(|&c| Continent::vanilla(&ctx.vanilla, c))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) => v,
        Err(e) => {
            // Not the game's data (synthetic test images): leave it alone.
            ctx.log(format!("overworld: skipped ({e})"));
            return Ok(());
        }
    };
    let items_stay = ctx.flags.items.shuffle_palace_items == Tri::Off
        && ctx.flags.items.shuffle_overworld_items == Tri::Off;
    let mut last = String::from("no attempt");
    let mut why: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for tries in 0..ATTEMPTS {
        if !last.is_empty() && tries > 0 {
            // Developer log: what made earlier attempts fail (prefix only,
            // the details vary).
            let key: String = last.chars().take(48).collect();
            *why.entry(key).or_insert(0) += 1;
        }
        let (conts, links) = match build(&base, &o, &mut ctx.rng) {
            Ok(c) => c,
            Err(RandoError::Retry(m)) => {
                last = m;
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut world = ctx.state.world.clone();
        sync::sync(&mut world, &conts);
        if links != connect::NORMAL {
            connect::sync(&mut world, &conts, &links);
        }
        if let Some(why) = impossible(&world) {
            last = why;
            continue;
        }
        // Items that never move must already be collectable in order.
        if items_stay {
            if let Some(why) = world.unbeatable_reason() {
                last = format!("with the items where they are: {why}");
                continue;
            }
        }
        let mut rom = ctx.rom.clone();
        let east_changed =
            conts[Cont::East.index()].table_changed || conts[Cont::East.index()].map_changed;
        // "Legacy" icons only mean something on a vanilla-shuffled East.
        let legacy = o.legacy && o.biome[Cont::East.index()] == Biome::VanillaShuffle;
        match write::write_all(&mut rom, &conts, legacy, east_changed) {
            Ok(()) => {}
            Err(RandoError::Retry(m)) => {
                last = m;
                continue;
            }
            Err(e) => return Err(e),
        }
        encounters::set_rates(&mut rom, o.rates)?;
        let mut scene_log = Vec::new();
        if o.shuffle_encounters {
            for bank in [1u8, 2] {
                let lava = o.lava_encounters && bank == 2;
                for (i, v) in encounters::shuffle_scenes(
                    &mut rom,
                    bank,
                    &mut ctx.rng,
                    o.path_encounters,
                    lava,
                )? {
                    scene_log.push(format!(
                        "{}: {} -> area {} page {}",
                        if bank == 1 { "West/DM" } else { "East/Maze" },
                        encounters::selector_name(i),
                        v & 0x3F,
                        v >> 6
                    ));
                }
            }
        }
        ctx.rom = rom;
        ctx.state.world = world;
        if !why.is_empty() {
            ctx.log(format!("overworld: {tries} failed attempts first: {why:?}"));
        }
        if std::env::var_os("OW_DEBUG").is_some() {
            eprintln!("overworld done after {tries} retries: {why:?}");
        }
        spoil(ctx, &conts, &o, &scene_log);
        if links != connect::NORMAL {
            for (k, (a, b)) in links.ends.iter().enumerate() {
                let what = ["Bridge", "Raft", "Cave 1", "Cave 2"][k];
                ctx.spoiler.line(
                    "Overworld",
                    format!("Connection {what}: {} - {}", a.name(), b.name()),
                );
            }
        }
        return Ok(());
    }
    if std::env::var_os("OW_DEBUG").is_some() {
        eprintln!("overworld gave up this attempt: {why:?}");
    }
    Err(RandoError::Retry(format!("overworld: {last}")))
}

/// Spoiler section.
fn spoil(ctx: &mut Ctx, conts: &[Continent], o: &Resolved, scenes: &[String]) {
    let sec = "Overworld";
    for c in conts {
        ctx.spoiler.line(
            sec,
            format!("{}: {}", c.cont.name(), o.biome[c.cont.index()].label_of()),
        );
    }
    for c in conts {
        if !(c.table_changed || c.map_changed) {
            continue;
        }
        for s in c.placed() {
            let Some(info) = c.info(s) else { continue };
            let (r, col) = c.locs[s].pos().unwrap_or((0, 0));
            let mut line = format!(
                "  {} / {}: column {}, row {}",
                c.cont.name(),
                info.name,
                col,
                r
            );
            if let Some(n) = c.palace[s] {
                line.push_str(&format!(" (palace {n})"));
            }
            match c.hidden[s] {
                Some(Reveal::Flute) => line.push_str(" [hidden: flute]"),
                Some(Reveal::Hammer) => line.push_str(" [hidden: hammer]"),
                None => {}
            }
            ctx.spoiler.line(sec, line);
        }
    }
    if o.rates != [EncounterRate::Normal; 3] {
        ctx.spoiler.line(
            sec,
            format!(
                "Encounter rate: West {}, Death Mountain / Maze Island {}, East {}",
                o.rates[0].label_of(),
                o.rates[1].label_of(),
                o.rates[2].label_of()
            ),
        );
    }
    for l in scenes {
        ctx.spoiler.line(sec, format!("Encounter scene {l}"));
    }
}
