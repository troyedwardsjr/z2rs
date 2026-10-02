//! `items` module: item placement and the beatability check.
//!
//! Options owned: [`crate::flags::ItemFlags`] (`ctx.flags.items`).
//!
//! Catalog: section 01 (items and logic), 02 (Items tab), 05 (P-bag
//! amounts).
//!
//! The module works on the shared [`World`] that `start` built and the
//! world-shaping modules (`palaces`, `overworld`, `towns`, `spells`) edited:
//!
//! 1. Resolve the switches (mixing needs both shuffles, P-bag caves need the
//!    overworld shuffle).
//! 2. Starting tools (and, with "start with spell items", the trophy,
//!    medicine and child; or a spell item whose wizard's spell is already
//!    known) are taken out of the world: their locations get a random minor
//!    item instead, and spell items get their save-file flag.
//! 3. Heart and magic containers are brought to the totals the `start`
//!    module chose (extra copies become minor items; missing ones, and
//!    palace items left without a palace room, replace minor items).
//! 4. The chosen location groups are filled with an assumed fill (each
//!    progression item goes to a random location that is reachable when the
//!    player is assumed to hold every item not placed yet), so the result is
//!    beatable by construction; it is then checked with
//!    [`World::beatable`]. If the world cannot be made beatable the module
//!    asks the pipeline for a retry.
//! 5. Optional extras: duplicates of important items over minor items,
//!    spell-item chain prevention, small-item shuffles, extra keys, P-bag
//!    experience amounts.
//! 6. Item bytes are written to the ROM and the placement goes into the
//!    spoiler.
//!
//! Town NPC rewards (wizards, trainers, Bagu, the mirror table, the
//! fountain) take part in the logic, but "include spells / sword techniques
//! / quest items in the shuffle" need a 6502 patch that lets those NPCs
//! hand out arbitrary items; that patch is not written yet, so the three
//! switches are reported in the log and otherwise ignored.
//!
//! Default options leave the ROM untouched.

use crate::world::{ItemId, ItemLoc, ItemStore, LocClass, LocKey, Town, TownSlot, World};
use crate::{Ctx, RandoError};

/// Placement attempts per pipeline attempt before asking for a retry.
pub const FILL_ATTEMPTS: u32 = 24;

/// Minor items used to replace removed items.
const FILLER: [ItemId; 7] = [
    ItemId::SmallBag,
    ItemId::MediumBag,
    ItemId::LargeBag,
    ItemId::XlBag,
    ItemId::BlueJar,
    ItemId::RedJar,
    ItemId::OneUp,
];

/// Important-item duplicate priority (most useful first).
const DUPLICATE_PRIORITY: [ItemId; 12] = [
    ItemId::Glove,
    ItemId::Downstab,
    ItemId::Fairy,
    ItemId::Thunder,
    ItemId::Reflect,
    ItemId::MagicKey,
    ItemId::Raft,
    ItemId::Boots,
    ItemId::Hammer,
    ItemId::Flute,
    ItemId::Upstab,
    ItemId::Jump,
];

/// Save-file spell-item flags: (item, iNES offset, value).
const SPELL_ITEM_SAVE: [(ItemId, usize, u8); 5] = [
    (ItemId::Trophy, 0x17B14, 0x10),
    (ItemId::Mirror, 0x17B15, 0x01),
    (ItemId::Medicine, 0x17B16, 0x40),
    (ItemId::Water, 0x17B17, 0x01),
    (ItemId::Child, 0x17B18, 0x20),
];

/// P-bag experience table (iNES offset), one experience-ladder index per
/// bag size (50, 100, 200, 500).
const PBAG_XP_INES: usize = 0x1E800;

/// Heart/magic container sprite tiles (CHR offset, 64 bytes) and the other
/// CHR banks that need a copy so containers draw anywhere.
const CONTAINER_TILES: usize = 0x7800;
const CONTAINER_TILE_COPIES: [usize; 7] =
    [0x9800, 0xB800, 0xD800, 0x13800, 0x15800, 0x17800, 0x19800];

/// Overworld and town sideview pointer tables: (bank, CPU address).
const AREA_SIDEVIEW_TABLES: [(u8, u16); 5] = [
    (1, 0x8523),
    (1, 0xA000),
    (2, 0x8523),
    (2, 0xA000),
    (3, 0x8523),
];
/// Palace 1-6 sideview pointer tables: (bank, CPU address).
const PALACE_SIDEVIEW_TABLES: [(u8, u16); 2] = [(4, 0x8523), (4, 0xA000)];

/// Resolved item switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemSettings {
    /// Palace items shuffle.
    pub palace: bool,
    /// Overworld items shuffle.
    pub overworld: bool,
    /// Palace and overworld items share one pool.
    pub mixed: bool,
    /// P-bag caves are item locations.
    pub pbag_caves: bool,
    /// Spells in the pool (needs the NPC patch).
    pub spells: bool,
    /// Sword techniques in the pool (needs the NPC patch).
    pub techs: bool,
    /// Quest items in the pool (needs the NPC patch).
    pub quest: bool,
    /// Start with every spell item.
    pub start_with_spell_items: bool,
    /// Shuffle P-bag experience.
    pub pbag_amounts: bool,
    /// Palace small items become keys.
    pub extra_keys: bool,
}

/// Resolve the tri-state switches in a fixed order.
pub fn resolve(ctx: &mut Ctx) -> ItemSettings {
    let f = ctx.flags.items.clone();
    let palace = ctx.tri(f.shuffle_palace_items);
    let overworld = ctx.tri(f.shuffle_overworld_items);
    let mixed = ctx.tri(f.mix_overworld_and_palace_items) && palace && overworld;
    let pbag_caves = ctx.tri(f.include_pbag_caves) && overworld;
    let spells = ctx.tri(f.include_spells);
    let techs = ctx.tri(f.include_sword_techs);
    let quest = ctx.tri(f.include_quest_items);
    let start_with_spell_items = ctx.tri(f.start_with_spell_items);
    let pbag_amounts = ctx.tri(f.shuffle_pbag_amounts);
    let extra_keys = ctx.tri(f.palaces_contain_extra_keys);
    ItemSettings {
        palace,
        overworld,
        mixed,
        pbag_caves,
        spells,
        techs,
        quest,
        start_with_spell_items,
        pbag_amounts,
        extra_keys,
    }
}

fn filler(ctx: &mut Ctx) -> ItemId {
    FILLER[ctx.rng.index(FILLER.len())]
}

/// Whether the item at location `i` may be swapped for another item by
/// writing its byte (an item object the unmodified game understands).
fn writable(w: &World, i: usize) -> bool {
    matches!(w.locs[i].store, ItemStore::Prg(_))
}

/// Replace the first writable copy of `item` with a minor item. Returns
/// whether a copy was found.
fn remove_first(ctx: &mut Ctx, item: ItemId) -> bool {
    let w = &ctx.state.world;
    let Some(i) = (0..w.locs.len()).find(|&i| w.locs[i].item == Some(item) && writable(w, i))
    else {
        return false;
    };
    let f = filler(ctx);
    ctx.state.world.locs[i].item = Some(f);
    true
}

/// Step 2: take starting items out of the world.
fn remove_starting_items(ctx: &mut Ctx, s: &ItemSettings) -> Result<Vec<ItemId>, RandoError> {
    let mut owned_spell_items = Vec::new();
    let start = ctx.state.world.start;
    for t in ItemId::TOOLS {
        if start.has(t) {
            while remove_first(ctx, t) {}
        }
    }
    if s.start_with_spell_items {
        for (it, _, _) in SPELL_ITEM_SAVE {
            owned_spell_items.push(it);
        }
    } else {
        // A spell item is pointless when its wizard's reward is already
        // known (the wizard has nothing to teach).
        for t in [
            Town::Ruto,
            Town::Mido,
            Town::Darunia,
            Town::Saria,
            Town::Nabooru,
        ] {
            let w = &ctx.state.world;
            let Some(need) = t.wizard_item() else {
                continue;
            };
            let Some(reward) = w
                .loc(LocKey::Town(TownSlot::Wizard(t)))
                .and_then(|l| l.item)
            else {
                continue;
            };
            if reward.is_spell() && start.has(reward) {
                owned_spell_items.push(need);
            }
        }
    }
    for &it in &owned_spell_items {
        ctx.state.world.start.add(it);
        while remove_first(ctx, it) {}
        if let Some(&(_, ines, v)) = SPELL_ITEM_SAVE.iter().find(|(i, _, _)| *i == it) {
            let off = ctx.rom.prg_from_ines(ines)?;
            ctx.rom.write(off, &[v])?;
        }
    }
    Ok(owned_spell_items)
}

/// Locations that may receive extra items (writable, holding a minor item),
/// limited to the groups being shuffled.
fn minor_slots(w: &World, groups: &[Vec<usize>]) -> Vec<usize> {
    let mut v: Vec<usize> = groups
        .iter()
        .flatten()
        .copied()
        .filter(|&i| writable(w, i) && w.locs[i].item.is_some_and(ItemId::is_minor))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Step 3: bring containers to the chosen totals and give palace items
/// without a room a place. Returns log lines.
fn adjust_counts(ctx: &mut Ctx, groups: &[Vec<usize>]) -> Vec<String> {
    let mut log = Vec::new();
    let mut excess: Vec<ItemId> = Vec::new();
    let (hearts_wanted, magic_wanted) = {
        let w = &ctx.state.world;
        (
            w.max_hearts.saturating_sub(w.start.hearts),
            w.max_magic.saturating_sub(w.start.magic),
        )
    };
    // Vanilla totals (4 + 4 of each) need no adjustment.
    let vanilla_totals = hearts_wanted == 4 && magic_wanted == 4;
    for (item, wanted) in [
        (ItemId::HeartContainer, hearts_wanted),
        (ItemId::MagicContainer, magic_wanted),
    ] {
        if vanilla_totals {
            break;
        }
        let have = ctx
            .state
            .world
            .placed_items()
            .iter()
            .filter(|&&i| i == item)
            .count();
        let wanted = usize::from(wanted);
        if have > wanted {
            for _ in wanted..have {
                let w = &ctx.state.world;
                let cands: Vec<usize> = (0..w.locs.len())
                    .filter(|&i| w.locs[i].item == Some(item) && writable(w, i))
                    .collect();
                if cands.is_empty() {
                    break;
                }
                let i = cands[ctx.rng.index(cands.len())];
                let f = filler(ctx);
                ctx.state.world.locs[i].item = Some(f);
            }
        } else {
            excess.extend(std::iter::repeat_n(item, wanted - have));
        }
    }
    // Palace items that no location holds (palaces without item rooms).
    let start = ctx.state.world.start;
    for it in ItemId::PALACE_ITEMS {
        if !start.has(it) && !ctx.state.world.placed_items().contains(&it) {
            excess.push(it);
        }
    }
    for it in excess {
        let mut slots = minor_slots(&ctx.state.world, groups);
        if slots.is_empty() {
            // Fall back to any writable minor-item location.
            let w = &ctx.state.world;
            slots = (0..w.locs.len())
                .filter(|&i| writable(w, i) && w.locs[i].item.is_some_and(ItemId::is_minor))
                .collect();
        }
        if slots.is_empty() {
            log.push(format!("items: no room for an extra {}", it.name()));
            continue;
        }
        let i = slots[ctx.rng.index(slots.len())];
        ctx.state.world.locs[i].item = Some(it);
    }
    log
}

/// Items that the fill places with logic (everything except filler).
fn is_progression(i: ItemId) -> bool {
    i.is_major() || i == ItemId::MagicContainer
}

/// Assumed fill of `groups` (each a list of location indices; an item stays
/// within its group). Returns false when some progression item had no
/// reachable location.
pub fn assumed_fill(w: &mut World, rng: &mut crate::rng::Rng, groups: &[Vec<usize>]) -> bool {
    let mut prog: Vec<(ItemId, usize)> = Vec::new();
    let mut rest: Vec<Vec<ItemId>> = vec![Vec::new(); groups.len()];
    for (g, locs) in groups.iter().enumerate() {
        for &i in locs {
            if let Some(it) = w.locs[i].item.take() {
                if is_progression(it) {
                    prog.push((it, g));
                } else {
                    rest[g].push(it);
                }
            }
        }
    }
    rng.shuffle(&mut prog);
    let mut ok = true;
    while let Some((it, g)) = prog.pop() {
        let mut assumed = w.start;
        for &(o, _) in &prog {
            assumed.add(o);
        }
        let r = w.solve_from(&assumed);
        let cands: Vec<usize> = groups[g]
            .iter()
            .copied()
            .filter(|&i| w.locs[i].item.is_none() && r.locs[i])
            .collect();
        let pick = if cands.is_empty() {
            ok = false;
            let any: Vec<usize> = groups[g]
                .iter()
                .copied()
                .filter(|&i| w.locs[i].item.is_none())
                .collect();
            if any.is_empty() {
                continue;
            }
            any[rng.index(any.len())]
        } else {
            cands[rng.index(cands.len())]
        };
        w.locs[pick].item = Some(it);
    }
    for (g, locs) in groups.iter().enumerate() {
        let mut empty: Vec<usize> = locs
            .iter()
            .copied()
            .filter(|&i| w.locs[i].item.is_none())
            .collect();
        rng.shuffle(&mut empty);
        for (slot, it) in empty.into_iter().zip(rest[g].drain(..)) {
            w.locs[slot].item = Some(it);
        }
    }
    ok
}

/// The location groups to shuffle.
fn groups(w: &World, s: &ItemSettings) -> Vec<Vec<usize>> {
    let mut palace: Vec<usize> = w
        .locs_in(LocClass::Palace)
        .into_iter()
        .filter(|&i| writable(w, i))
        .collect();
    let mut overworld: Vec<usize> = w
        .locs_in(LocClass::Overworld)
        .into_iter()
        .filter(|&i| writable(w, i))
        .collect();
    if s.pbag_caves {
        overworld.extend(
            w.locs_in(LocClass::PbagCave)
                .into_iter()
                .filter(|&i| writable(w, i)),
        );
    }
    let mut out = Vec::new();
    if s.mixed {
        palace.extend(overworld);
        out.push(palace);
    } else {
        if s.palace {
            out.push(palace);
        }
        if s.overworld {
            out.push(overworld);
        }
    }
    out
}

/// Step 5a: copies of important items replace minor items.
fn add_duplicates(ctx: &mut Ctx, groups: &[Vec<usize>]) -> Vec<String> {
    let w = &ctx.state.world;
    let present = w.placed_items();
    let slots = minor_slots(w, groups);
    let mut dups: Vec<ItemId> = DUPLICATE_PRIORITY
        .iter()
        .copied()
        .filter(|i| present.contains(i) && i.is_vanilla_object_item())
        .collect();
    dups.truncate(slots.len());
    ctx.rng.shuffle(&mut dups);
    let mut log = Vec::new();
    for it in dups {
        let slots = minor_slots(&ctx.state.world, groups);
        if slots.is_empty() {
            break;
        }
        let i = slots[ctx.rng.index(slots.len())];
        log.push(format!(
            "Duplicate {} at {}",
            it.name(),
            ctx.state.world.locs[i].name
        ));
        ctx.state.world.locs[i].item = Some(it);
    }
    log
}

/// Step 5b: a wizard (or quest NPC) whose reward is a spell item swaps it
/// with a non-spell-item reward elsewhere, so spell items never chain.
fn prevent_spell_item_chains(ctx: &mut Ctx) {
    let w = &ctx.state.world;
    let wizards: Vec<usize> = (0..w.locs.len())
        .filter(|&i| {
            matches!(w.locs[i].class, LocClass::Wizard | LocClass::Quest)
                && w.locs[i].item.is_some_and(ItemId::is_spell_item)
                && w.locs[i]
                    .requirement
                    .items()
                    .iter()
                    .any(|r| r.is_spell_item())
        })
        .collect();
    for wi in wizards {
        let w = &ctx.state.world;
        let wiz_store = matches!(w.locs[wi].store, ItemStore::Npc);
        let mut others: Vec<usize> = (0..w.locs.len())
            .filter(|&j| {
                j != wi
                    && w.locs[j].item.is_some_and(|i| !i.is_spell_item())
                    && w.locs[j].class != LocClass::Event
                    && (matches!(w.locs[j].store, ItemStore::Npc) == wiz_store)
            })
            .collect();
        ctx.rng.shuffle(&mut others);
        if let Some(&j) = others.first() {
            let a = ctx.state.world.locs[wi].item;
            ctx.state.world.locs[wi].item = ctx.state.world.locs[j].item;
            ctx.state.world.locs[j].item = a;
        }
    }
}

/// Item-byte offsets of every item object reachable from the sideview
/// pointer `tables` of `rom` (deduplicated, in offset order).
fn sideview_item_objects(ctx: &Ctx, tables: &[(u8, u16)]) -> Vec<(usize, u8)> {
    let rom = &ctx.rom;
    let mut out = Vec::new();
    for &(bank, addr) in tables {
        for map in 0..crate::world::LOCATION_SLOTS as u16 {
            let Ok(ptr) = rom.read_cpu_word(bank, addr + 2 * map) else {
                continue;
            };
            if ptr < 0x8000 {
                continue;
            }
            let Ok(start) = rom.cpu_offset(if ptr >= 0xC000 { 7 } else { bank }, ptr) else {
                continue;
            };
            out.extend(crate::world::sideview_items(rom, start));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Offsets that hold a location's item (left alone by the small-item
/// shuffles).
fn location_offsets(ctx: &Ctx) -> Vec<usize> {
    ctx.state
        .world
        .locs
        .iter()
        .filter_map(|l| match &l.store {
            ItemStore::Prg(v) => Some(v.clone()),
            _ => None,
        })
        .flatten()
        .map(|o| ctx.rom.vanilla_offset(o))
        .collect()
}

fn is_small(b: u8) -> bool {
    ItemId::from_byte(b).is_some_and(ItemId::is_minor)
}

/// Step 5c: permute the overworld/town small items.
fn shuffle_area_small_items(ctx: &mut Ctx) -> Result<usize, RandoError> {
    let skip = location_offsets(ctx);
    let objs: Vec<(usize, u8)> = sideview_item_objects(ctx, &AREA_SIDEVIEW_TABLES)
        .into_iter()
        .filter(|(o, b)| is_small(*b) && !skip.contains(o))
        .collect();
    let mut bytes: Vec<u8> = objs.iter().map(|&(_, b)| b).collect();
    ctx.rng.shuffle(&mut bytes);
    for (&(o, _), b) in objs.iter().zip(bytes) {
        ctx.rom.write(o, &[b])?;
    }
    Ok(objs.len())
}

/// Step 5d: re-roll palace small items (or make them all keys).
fn reroll_palace_small_items(ctx: &mut Ctx, all_keys: bool) -> Result<usize, RandoError> {
    let skip = location_offsets(ctx);
    let objs: Vec<(usize, u8)> = sideview_item_objects(ctx, &PALACE_SIDEVIEW_TABLES)
        .into_iter()
        .filter(|(o, b)| is_small(*b) && !skip.contains(o))
        .collect();
    for &(o, _) in &objs {
        let it = if all_keys {
            ItemId::SmallKey
        } else {
            // 35% key, 10% blue jar, 10% red jar, 10% 50-bag, 15% 100-bag,
            // 10% 200-bag, 5% 500-bag, 5% 1-up.
            let r = ctx.rng.below(100);
            match r {
                0..=34 => ItemId::SmallKey,
                35..=44 => ItemId::BlueJar,
                45..=54 => ItemId::RedJar,
                55..=64 => ItemId::SmallBag,
                65..=79 => ItemId::MediumBag,
                80..=89 => ItemId::LargeBag,
                90..=94 => ItemId::XlBag,
                _ => ItemId::OneUp,
            }
        };
        ctx.rom.write(o, &[it.byte()])?;
    }
    Ok(objs.len())
}

/// Step 5e: P-bag experience indices move up to two ladder steps.
fn shuffle_pbag_amounts(ctx: &mut Ctx) -> Result<[u8; 4], RandoError> {
    let off = ctx.rom.prg_from_ines(PBAG_XP_INES)?;
    let mut v = [0u8; 4];
    for (k, lo) in [5u8, 7, 9, 11].into_iter().enumerate() {
        v[k] = ctx.rng.range_u8(lo, lo + 4);
    }
    ctx.rom.write(off, &v)?;
    Ok(v)
}

/// Whether a town NPC reward class (spells, sword techniques, quest items)
/// really joins the item pool for this include switch. Always `false`
/// until the town NPC item patch exists: the switches are accepted and
/// logged, and the NPCs keep their own rewards, so the `spells` module
/// still shuffles the spell menu and swaps the stab teachers.
#[must_use]
pub fn npc_rewards_in_pool(_include: crate::flags::Tri) -> bool {
    false
}

/// Copy the container sprite tiles into every CHR bank that draws items.
fn copy_container_tiles(ctx: &mut Ctx) -> Result<(), RandoError> {
    let chr = ctx.rom.chr();
    if chr.len() < CONTAINER_TILE_COPIES[6] + 64 {
        return Ok(());
    }
    let tiles: Vec<u8> = chr[CONTAINER_TILES..CONTAINER_TILES + 64].to_vec();
    for dst in CONTAINER_TILE_COPIES {
        ctx.rom.write_chr(dst, &tiles)?;
    }
    Ok(())
}

/// Write every location's item byte that changed.
fn write_items(ctx: &mut Ctx) -> Result<bool, RandoError> {
    let mut any = false;
    let locs: Vec<ItemLoc> = ctx.state.world.locs.clone();
    for l in &locs {
        let (Some(it), ItemStore::Prg(offs)) = (l.item, &l.store) else {
            continue;
        };
        if l.item == l.vanilla {
            continue;
        }
        for &o in offs {
            let o = ctx.rom.vanilla_offset(o);
            if ctx.rom.read(o)? != it.byte() {
                ctx.rom.write(o, &[it.byte()])?;
                any = true;
            }
        }
    }
    Ok(any)
}

/// Spoiler lines for every item location, grouped by continent.
fn spoiler(ctx: &mut Ctx) {
    let w = &ctx.state.world;
    let mut lines = Vec::new();
    for c in crate::world::Continent::ALL {
        let mut here: Vec<String> = w
            .locs
            .iter()
            .filter(|l| l.item.is_some() && w.spots.get(l.spot).is_some_and(|s| s.continent == c))
            .map(|l| {
                let item = l.item.map_or("-", ItemId::name);
                let site = &w.spots[l.spot].name;
                if l.name == *site {
                    format!("  {}: {item}", l.name)
                } else {
                    format!("  {} ({site}): {item}", l.name)
                }
            })
            .collect();
        if here.is_empty() {
            continue;
        }
        lines.push(format!("{}:", c.name()));
        lines.append(&mut here);
    }
    let r = w.solve();
    let order: Vec<String> = r
        .spheres
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let got: Vec<&str> = s
                .iter()
                .filter_map(|&i| w.locs[i].item.filter(|it| it.is_major()).map(ItemId::name))
                .collect();
            format!(
                "  Sphere {}: {}",
                n + 1,
                if got.is_empty() {
                    "-".into()
                } else {
                    got.join(", ")
                }
            )
        })
        .collect();
    for l in lines {
        ctx.spoiler.line("Items", l);
    }
    ctx.spoiler.line("Items", "Collection order (major items):");
    for l in order {
        ctx.spoiler.line("Items", l);
    }
}

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let s = resolve(ctx);
    if !ctx.state.world.built {
        return Ok(());
    }
    if let Some(n) = ctx.state.new_kasuto_containers {
        ctx.state.world.set_new_kasuto_basement(n);
    }
    for (on, what) in [
        (s.spells, "spells"),
        (s.techs, "sword techniques"),
        (s.quest, "quest items"),
    ] {
        if on {
            ctx.log(format!(
                "items: including {what} in the shuffle needs the town NPC item patch, which is not implemented yet; kept vanilla"
            ));
        }
    }
    let pristine = ctx.state.world.clone();
    let mut changed_world = false;
    let mut last = String::new();
    let mut extra_log = Vec::new();
    for attempt in 0..FILL_ATTEMPTS {
        ctx.state.world = pristine.clone();
        let removed = remove_starting_items(ctx, &s)?;
        let groups = groups(&ctx.state.world, &s);
        extra_log = adjust_counts(ctx, &groups);
        let mut rng = ctx.rng.derive(&format!("fill#{attempt}"));
        if !groups.is_empty() {
            assumed_fill(&mut ctx.state.world, &mut rng, &groups);
        }
        if ctx.flags.items.prevent_spell_item_chains {
            prevent_spell_item_chains(ctx);
        }
        match ctx.state.world.unbeatable_reason() {
            None => {
                if ctx.flags.items.allow_important_item_duplicates {
                    extra_log.extend(add_duplicates(ctx, &groups));
                }
                changed_world = !groups.is_empty() || !removed.is_empty();
                last.clear();
                break;
            }
            Some(why) => last = why,
        }
    }
    if !last.is_empty() {
        return Err(RandoError::Retry(format!("items: {last}")));
    }
    let wrote = write_items(ctx)?;
    if wrote {
        copy_container_tiles(ctx)?;
    }
    if ctx.flags.items.shuffle_small_items {
        let n = shuffle_area_small_items(ctx)?;
        ctx.log(format!("items: permuted {n} overworld/town small items"));
    }
    if ctx.flags.items.shuffle_small_items || s.extra_keys {
        let n = reroll_palace_small_items(ctx, s.extra_keys)?;
        ctx.log(format!("items: re-rolled {n} palace small items"));
    }
    if s.pbag_amounts {
        let v = shuffle_pbag_amounts(ctx)?;
        ctx.spoiler.line(
            "Items",
            format!("P-bag experience ladder steps (50/100/200/500 bags): {v:?}"),
        );
    }
    for l in extra_log {
        ctx.spoiler.line("Items", l);
    }
    if changed_world || wrote {
        spoiler(ctx);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::Flags;
    use crate::flags::Tri;
    use crate::rng::Rng;
    use crate::rom::{Rom, VANILLA_BODY_LEN};
    use crate::spoiler::Spoiler;
    use crate::{Extras, State};

    fn synth_ctx(flags: Flags) -> Ctx {
        let mut r = Rng::new(4);
        let body: Vec<u8> = (0..VANILLA_BODY_LEN).map(|_| r.next_u32() as u8).collect();
        let rom = Rom::from_body(&body).unwrap();
        Ctx {
            rom: rom.clone(),
            vanilla: rom,
            rng: Rng::new(7),
            flags,
            seed: String::new(),
            attempt: 0,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: Extras::default(),
        }
    }

    #[test]
    fn default_flags_are_a_no_op() {
        let mut c = synth_ctx(Flags::default());
        crate::start::apply(&mut c).unwrap();
        let before = c.rom.body();
        apply(&mut c).unwrap();
        assert_eq!(c.rom.body(), before);
        assert!(c.spoiler.is_empty());
    }

    #[test]
    fn mixing_needs_both_shuffles() {
        let mut f = Flags::default();
        f.items.mix_overworld_and_palace_items = Tri::On;
        f.items.shuffle_palace_items = Tri::On;
        let mut c = synth_ctx(f);
        let s = resolve(&mut c);
        assert!(s.palace && !s.overworld && !s.mixed);
    }

    #[test]
    fn assumed_fill_keeps_items_in_their_group() {
        let mut c = synth_ctx(Flags::default());
        crate::start::apply(&mut c).unwrap();
        let s = ItemSettings {
            palace: true,
            overworld: true,
            ..ItemSettings::default()
        };
        let g = groups(&c.state.world, &s);
        let before: Vec<Vec<Option<ItemId>>> = g
            .iter()
            .map(|l| {
                let mut v: Vec<_> = l.iter().map(|&i| c.state.world.locs[i].item).collect();
                v.sort();
                v
            })
            .collect();
        let mut rng = Rng::new(3);
        assumed_fill(&mut c.state.world, &mut rng, &g);
        for (k, l) in g.iter().enumerate() {
            let mut v: Vec<_> = l.iter().map(|&i| c.state.world.locs[i].item).collect();
            v.sort();
            assert_eq!(v, before[k]);
        }
    }

    fn rom_ctx(flags: Flags, seed: u64) -> Ctx {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let rom = Rom::from_body(&body).unwrap();
        Ctx {
            rom: rom.clone(),
            vanilla: rom,
            rng: Rng::new(seed),
            flags,
            seed: String::new(),
            attempt: 0,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: Extras::default(),
        }
    }

    /// ROM-gated: full shuffles over many seeds stay beatable, keep the item
    /// multiset, and the bytes in the ROM match the world.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_mixed_shuffle_is_beatable_and_written() {
        let mut f = Flags::default();
        f.items.shuffle_palace_items = Tri::On;
        f.items.shuffle_overworld_items = Tri::On;
        f.items.mix_overworld_and_palace_items = Tri::On;
        f.items.include_pbag_caves = Tri::On;
        let mut moved = 0;
        for seed in 0..40 {
            let mut c = rom_ctx(f.clone(), seed);
            crate::start::apply(&mut c).unwrap();
            let mut before = c.state.world.placed_items();
            apply(&mut c).unwrap();
            assert!(c.state.world.beatable(), "seed {seed}");
            let mut after = c.state.world.placed_items();
            before.sort();
            after.sort();
            assert_eq!(before, after, "seed {seed}");
            for l in &c.state.world.locs {
                if let (Some(it), ItemStore::Prg(o)) = (l.item, &l.store) {
                    assert_eq!(c.rom.read(o[0]).unwrap(), it.byte(), "{}", l.name);
                    if Some(it) != l.vanilla {
                        moved += 1;
                    }
                }
            }
            assert!(c.spoiler.render().contains("Sphere 1"));
        }
        assert!(moved > 200, "{moved}");
    }

    /// ROM-gated: starting items leave the world and container totals follow
    /// the start options.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_start_items_and_containers() {
        let mut f = Flags::default();
        f.start.start_with_hammer = true;
        f.start.start_with_glove = true;
        f.start.heart_containers_min = 2;
        f.start.heart_containers_max = 2;
        f.start.max_heart_containers = crate::flags::MaxHearts::Eight;
        f.start.magic_containers_min = 6;
        f.start.magic_containers_max = 6;
        f.items.shuffle_overworld_items = Tri::On;
        f.items.shuffle_palace_items = Tri::On;
        f.items.start_with_spell_items = Tri::On;
        f.items.allow_important_item_duplicates = true;
        for seed in 0..10 {
            let mut c = rom_ctx(f.clone(), seed);
            crate::start::apply(&mut c).unwrap();
            apply(&mut c).unwrap();
            let items = c.state.world.placed_items();
            let count = |i| items.iter().filter(|&&x| x == i).count();
            assert_eq!(count(ItemId::Hammer), 0);
            assert_eq!(count(ItemId::Trophy), 0);
            assert!(count(ItemId::Glove) <= 1);
            assert_eq!(count(ItemId::MagicContainer), 2);
            assert_eq!(count(ItemId::HeartContainer), 6);
            assert!(c.state.world.beatable());
        }
    }

    /// ROM-gated: the small-item options touch only small-item bytes.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_small_items_and_keys() {
        let mut f = Flags::default();
        f.items.shuffle_small_items = true;
        f.items.palaces_contain_extra_keys = Tri::On;
        f.items.shuffle_pbag_amounts = Tri::On;
        let mut c = rom_ctx(f, 1);
        crate::start::apply(&mut c).unwrap();
        let before = c.rom.body();
        apply(&mut c).unwrap();
        let after = c.rom.body();
        let diffs: Vec<usize> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert!(!diffs.is_empty());
        for d in diffs {
            assert!(
                is_small(before[d]) || (0x1E7F0..0x1E7F4).contains(&d),
                "{d:#X}"
            );
            assert!(
                is_small(after[d]) || (0x1E7F0..0x1E7F4).contains(&d),
                "{d:#X}"
            );
        }
    }
}
