//! `palaces` module: palace layouts, lengths, item rooms, bosses, palettes.
//!
//! Options: [`crate::flags::PalaceFlags`] (`ctx.flags.palaces`).
//!
//! Room data comes from the player's ROM: every vanilla palace room is read
//! at run time ([`rooms`]); no room data ships with z2rs. The generators
//! ([`gen`]) rebuild palaces from those rooms, [`layout`] checks them, and
//! [`write`] puts them back into the game's tables.
//!
//! With the default options nothing is written.

pub mod gen;
pub mod layout;
pub mod reach;
pub mod rooms;
pub mod write;

use crate::flags::{
    BossRoomsExit, DarkLinkDistance, ItemRoomCount, PalaceDropStyle, PalaceLength, PalaceStyle, Tri,
};
use crate::rng::Rng;
use crate::sideview::{self, Sideview};
use crate::world::{ItemId, ItemLoc, ItemStore, LocClass, LocKey, Requirement};
use crate::{Ctx, RandoError};

use gen::{GrowParams, Parts, Style};
use layout::{DropRule, Layout, Rules};
use rooms::{Group, Needs, Role, Room, VanillaPool, VANILLA_LENGTHS};

/// Shortest a vanilla-based palace may be shortened to (P1..GP).
pub const VANILLA_MIN_LENGTHS: [u8; 7] = [11, 16, 10, 19, 23, 20, 31];

/// Item id written into extra item rooms until the `items` module places
/// something there (an extra life).
pub const EXTRA_ROOM_ITEM: u8 = 0x12;

/// Small object id of the Iron Knuckle statue that stands in front of a
/// boss room's exit.
const IK_STATUE: u8 = 0x09;

/// What this module decided, for later modules and the spoiler.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PalaceState {
    /// Whether any palace was rebuilt (false: the vanilla tables are
    /// untouched).
    pub generated: bool,
    /// One entry per palace 1-7 (empty when nothing was generated).
    pub palaces: Vec<PalaceInfo>,
    /// Ranges of the palace banks this module wrote new room data into
    /// (`(bank, start, end)`); other modules must not use them.
    pub space_used: Vec<(u8, u16, u16)>,
    /// Crystals to place before the Great Palace opens (`None` = 6).
    pub crystals_needed: Option<u8>,
}

/// One palace as generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalaceInfo {
    /// Palace (1-7).
    pub palace: u8,
    /// Generator used.
    pub style: String,
    /// Rooms in the palace.
    pub rooms: Vec<RoomInfo>,
    /// The boss room continues to more palace.
    pub boss_continues: bool,
    /// What the palace may ask for, as names (`jump`, `glove`, ...).
    pub needs: Vec<String>,
}

/// One room of a generated palace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomInfo {
    /// PRG bank of the palace tables (4 or 5).
    pub bank: u8,
    /// Map number in the palace's group.
    pub map: u8,
    /// Sideview pointer (CPU address in `bank`).
    pub sideview: u16,
    /// Enemy-list pointer (the `$7000` RAM copy of the bank's enemy data).
    pub enemies: u16,
    /// Role.
    pub role: Role,
    /// Vanilla origin `(palace, map)`.
    pub from: (u8, u8),
    /// Headerless PRG offset of the item byte of an item room.
    pub item_offset: Option<usize>,
}

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let f = ctx.flags.palaces.clone();

    // Crystals to place before the Great Palace opens (always one draw).
    let lo = f.palaces_to_complete_min.min(6);
    let hi = f.palaces_to_complete_max.clamp(lo, 6);
    let crystals = ctx.rng.range_u8(lo, hi);
    if crystals != 6 {
        ctx.rom.write_cpu(5, CRYSTALS_ADDR, &[crystals])?;
        ctx.state.palaces.crystals_needed = Some(crystals);
        ctx.spoiler.line(
            "Palaces",
            format!("Crystals needed for the Great Palace: {crystals}"),
        );
    }

    if f.change_palace_palettes {
        shuffle_palettes(ctx)?;
    }

    let plan = Plan::resolve(ctx)?;
    if plan.is_vanilla() {
        return Ok(());
    }
    let pool = match VanillaPool::read(&ctx.vanilla) {
        Ok(p) if p.looks_vanilla() => p,
        _ => {
            ctx.log(
                "palaces: the ROM's palace tables are not the vanilla ones; palaces left alone",
            );
            return Ok(());
        }
    };
    let mut layouts: Vec<Layout> = Vec::with_capacity(7);
    for p in 1..=7u8 {
        // Hard room budget: the group's map slots, minus the rooms its
        // already-built palaces took, minus the lengths reserved for the
        // ones still to come. It is never below this palace's own length
        // (the lengths were capped per group), so it only stops a palace
        // from eating the slots its group mates need.
        let group = Group::of_palace(p);
        let built: usize = layouts
            .iter()
            .filter(|l| Group::of_palace(l.palace) == group)
            .map(|l| l.rooms.len())
            .sum();
        let reserved: usize = group
            .palaces()
            .iter()
            .filter(|&&q| q > p)
            .map(|&q| plan.lengths[usize::from(q - 1)])
            .sum();
        let max_rooms = group_capacity(group).saturating_sub(built + reserved);
        let mut l = build_palace(ctx, &pool, &plan, p, max_rooms)?;
        // With random item room counts, the entrance's clouds tell how
        // many item rooms the palace has.
        let random_count = matches!(
            f.item_rooms_per_palace,
            ItemRoomCount::Random | ItemRoomCount::RandomIncludeZero
        );
        if random_count && p <= 6 {
            let n = l.all(Role::Item).len();
            let e = l.entrance();
            l.rooms[e].sideview = Some(item_count_clouds(ctx, &l.rooms[e].room, n)?);
        }
        layouts.push(l);
    }
    write_all(ctx, &pool, &plan, &mut layouts)
}

/// Map slots a group's palaces can use (the Great Palace keeps maps 61-62
/// for the ending scenes).
fn group_capacity(g: Group) -> usize {
    rooms::MAPS - g.reserved_maps().len()
}

/// Bank 5 address of the starting "crystals left" value.
const CRYSTALS_ADDR: u16 = 0xBB00;

/// Everything decided before building.
#[derive(Debug, Clone)]
struct Plan {
    styles: [PalaceStyle; 7],
    lengths: [usize; 7],
    items: [usize; 7],
    boss_continues: [bool; 7],
    remove_thunderbird: bool,
    require_thunderbird: bool,
    dark_link_distance: usize,
    drops: [DropRule; 7],
    blocking_anywhere: bool,
    no_dup_layout: bool,
    no_dup_content: bool,
}

fn is_vanilla_pool(s: PalaceStyle) -> bool {
    matches!(s, PalaceStyle::Vanilla | PalaceStyle::Shuffled)
}

impl Plan {
    fn is_vanilla(&self) -> bool {
        self.styles.iter().all(|&s| s == PalaceStyle::Vanilla)
            && self
                .lengths
                .iter()
                .zip(VANILLA_LENGTHS)
                .all(|(&a, b)| a == usize::from(b))
            && self.items[..6].iter().all(|&n| n == 1)
            && !self.remove_thunderbird
    }

    fn resolve(ctx: &mut Ctx) -> Result<Plan, RandoError> {
        let f = ctx.flags.palaces.clone();
        // Styles. Random choices never pick Loopy or Chaos.
        let mut normal_pool: Vec<PalaceStyle> = vec![
            PalaceStyle::Sequential,
            PalaceStyle::RandomWalk,
            PalaceStyle::VanillaWeighted,
            PalaceStyle::Tower,
            PalaceStyle::Mirror,
            PalaceStyle::Reconstructed,
        ];
        if f.random_styles_allow_vanilla {
            normal_pool.insert(0, PalaceStyle::Shuffled);
            normal_pool.insert(0, PalaceStyle::Vanilla);
        }
        let gp_pool: Vec<PalaceStyle> = normal_pool
            .iter()
            .copied()
            .filter(|s| !matches!(s, PalaceStyle::VanillaWeighted | PalaceStyle::Mirror))
            .collect();
        let gp_style = match f.gp_style {
            PalaceStyle::Random | PalaceStyle::RandomAll | PalaceStyle::RandomPerPalace => *ctx
                .rng
                .pick(&gp_pool)
                .unwrap_or(&PalaceStyle::Reconstructed),
            PalaceStyle::VanillaWeighted | PalaceStyle::Mirror => PalaceStyle::Reconstructed,
            s => s,
        };
        let single = *ctx
            .rng
            .pick(&normal_pool)
            .unwrap_or(&PalaceStyle::Reconstructed);
        let mut styles = [PalaceStyle::Vanilla; 7];
        for s in styles.iter_mut().take(6) {
            *s = match f.normal_style {
                PalaceStyle::RandomAll | PalaceStyle::Random => single,
                PalaceStyle::RandomPerPalace => *ctx
                    .rng
                    .pick(&normal_pool)
                    .unwrap_or(&PalaceStyle::Reconstructed),
                s => s,
            };
        }
        styles[6] = gp_style;

        // Lengths.
        let mut lengths = [0usize; 7];
        for i in 0..7 {
            let opt = if i == 6 { f.gp_length } else { f.normal_length };
            lengths[i] = roll_length(&mut ctx.rng, i, opt, styles[i]);
        }
        cap_group(&mut lengths, &styles, &[0, 1, 4], group_capacity(Group::A));
        cap_group(&mut lengths, &styles, &[2, 3, 5], group_capacity(Group::B));
        cap_group(&mut lengths, &styles, &[6], group_capacity(Group::Gp));

        // Item rooms.
        let mut items = [0usize; 7];
        for i in 0..6 {
            let max = if lengths[i] < 16 {
                1
            } else if lengths[i] < 26 {
                2
            } else {
                3
            };
            let mut n = match f.item_rooms_per_palace {
                ItemRoomCount::Zero => 0,
                ItemRoomCount::One => 1,
                ItemRoomCount::Two => 2,
                ItemRoomCount::Random => usize::from(ctx.rng.range_u8(1, max)),
                ItemRoomCount::RandomIncludeZero => usize::from(ctx.rng.range_u8(0, max)),
            };
            n = match styles[i] {
                PalaceStyle::Vanilla => n.min(1),
                PalaceStyle::Shuffled => n.min(2),
                _ => n,
            };
            items[i] = n;
        }
        let shuffled = ctx.flags.items.shuffle_palace_items == Tri::On;
        let mixed = ctx.flags.items.mix_overworld_and_palace_items == Tri::On;
        if !shuffled {
            for n in items.iter_mut().take(6) {
                *n = (*n).max(1);
            }
        }
        if !mixed {
            let mut guard = 0;
            while items[..6].iter().sum::<usize>() < 6 && guard < 100 {
                let i = ctx.rng.index(6);
                let cap = if styles[i] == PalaceStyle::Vanilla {
                    1
                } else {
                    3
                };
                if items[i] < cap {
                    items[i] += 1;
                }
                guard += 1;
            }
        }

        // Boss rooms.
        let all = ctx.rng.coin();
        let mut boss_continues = [false; 7];
        for (i, b) in boss_continues.iter_mut().enumerate().take(6) {
            *b = match f.boss_rooms_exit {
                BossRoomsExit::Overworld => false,
                BossRoomsExit::Palace => true,
                BossRoomsExit::RandomAll => all,
                BossRoomsExit::RandomPerPalace => ctx.rng.coin(),
            } && !matches!(
                styles[i],
                PalaceStyle::Vanilla | PalaceStyle::Shuffled | PalaceStyle::Tower
            );
        }

        let require_thunderbird = ctx.tri(f.thunderbird_required);
        let remove_thunderbird =
            f.remove_thunderbird && !require_thunderbird && styles[6] != PalaceStyle::Vanilla;
        let dark_link_distance = match f.dark_link_min_distance {
            DarkLinkDistance::None => 0,
            DarkLinkDistance::Short => 8,
            DarkLinkDistance::Medium => 12,
            DarkLinkDistance::Max => {
                if styles[6] == PalaceStyle::Reconstructed {
                    16
                } else {
                    20
                }
            }
        };
        let mut drops = [DropRule::EntranceOrBoss; 7];
        for d in &mut drops {
            *d = match f.drop_style {
                PalaceDropStyle::Entrance => DropRule::Entrance,
                PalaceDropStyle::AnyExit => DropRule::EntranceOrBoss,
                PalaceDropStyle::AnythingGoes => DropRule::Anything,
                PalaceDropStyle::Balanced => {
                    if ctx.rng.chance(2, 5) {
                        DropRule::EntranceOrBoss
                    } else {
                        DropRule::Entrance
                    }
                }
            };
        }
        if f.include_vanilla_rooms == Tri::Off {
            ctx.log("palaces: no room pack loaded, using the original rooms anyway");
        }
        Ok(Plan {
            styles,
            lengths,
            items,
            boss_continues,
            remove_thunderbird,
            require_thunderbird,
            dark_link_distance,
            drops,
            blocking_anywhere: f.blocking_rooms_in_any_palace,
            no_dup_layout: f.no_duplicate_rooms_by_layout,
            no_dup_content: f.no_duplicate_rooms_by_enemies,
        })
    }
}

/// Roll palace `i`'s room count (integer arithmetic, percent units).
fn roll_length(rng: &mut Rng, i: usize, opt: PalaceLength, style: PalaceStyle) -> usize {
    let vanilla = usize::from(VANILLA_LENGTHS[i]);
    let vpool = is_vanilla_pool(style);
    if opt == PalaceLength::Full && vpool {
        return vanilla;
    }
    let (lo, hi, pull) = match opt {
        PalaceLength::Short => (50, 65, 45),
        PalaceLength::Medium => (60, 80, 30),
        PalaceLength::Full => (85, 115, 0),
        PalaceLength::Random => (50, 115, 0),
    };
    // Pull normal palaces toward the mean length (21) first.
    let base100 = if i < 6 {
        let v = vanilla as i64 * 100;
        v + (2100 - v) * pull / 100
    } else {
        vanilla as i64 * 100
    };
    let a = (base100 * lo + 5000) / 10000;
    let b = (base100 * hi + 5000) / 10000;
    let n = rng.range(a, b.max(a)) as usize;
    let (min, max) = if vpool {
        (usize::from(VANILLA_MIN_LENGTHS[i]), vanilla)
    } else {
        (8, if i == 6 { 61 } else { 63 })
    };
    n.clamp(min, max)
}

/// Shortest a palace of `style` may be made (`i` = palace index).
fn min_length(i: usize, style: PalaceStyle) -> usize {
    if is_vanilla_pool(style) {
        usize::from(VANILLA_MIN_LENGTHS[i])
    } else {
        8
    }
}

/// Keep a group's palaces within its `cap` map slots, leaving 3 rooms of
/// slack per generated palace (tying up loose ends can add a few rooms).
/// The longest shrinkable palace loses one room at a time, generated
/// palaces first, never below its minimum length; the minimums of every
/// group fit with the slack (A: 11+16+23, B: 10+19+20, GP: 31), so this
/// always ends within the cap. Deterministic: no random draws.
fn cap_group(lengths: &mut [usize; 7], styles: &[PalaceStyle; 7], ix: &[usize], cap: usize) {
    let slack = ix.iter().filter(|&&i| !is_vanilla_pool(styles[i])).count() * 3;
    let cap = cap.saturating_sub(slack);
    while ix.iter().map(|&i| lengths[i]).sum::<usize>() > cap {
        let Some(&i) = ix
            .iter()
            .filter(|&&i| lengths[i] > min_length(i, styles[i]))
            .max_by_key(|&&i| {
                (
                    !is_vanilla_pool(styles[i]),
                    lengths[i],
                    std::cmp::Reverse(i),
                )
            })
        else {
            break;
        };
        lengths[i] -= 1;
    }
}

fn style_of(s: PalaceStyle) -> Option<Style> {
    Some(match s {
        PalaceStyle::Sequential => Style::Sequential,
        PalaceStyle::RandomWalk => Style::RandomWalk,
        PalaceStyle::VanillaWeighted => Style::VanillaWeighted,
        PalaceStyle::Tower => Style::Tower,
        PalaceStyle::Mirror => Style::Mirror,
        PalaceStyle::Reconstructed => Style::Reconstructed,
        PalaceStyle::ReconstructedLoopy => Style::Loopy,
        PalaceStyle::Chaos => Style::Chaos,
        _ => return None,
    })
}

/// Item rooms that can stand in for one of `shape` (dead ends by side).
fn item_room_sources(pool: &VanillaPool) -> Vec<Room> {
    (1..=6u8)
        .filter_map(|p| pool.role_room(p, Role::Item))
        .map(|k| pool.rooms[&k].clone())
        .collect()
}

/// A copy of item room `r`'s layout with its item set to `item`.
fn item_room_bytes(ctx: &Ctx, r: &Room, item: u8) -> Result<Vec<u8>, RandoError> {
    let bank = r.key.group.bank();
    let (_, raw) = sideview::read_at(&ctx.vanilla, bank, r.sideview)?;
    let (sv, offs) = Sideview::parse_with_offsets(&raw)?;
    let mut out = raw.clone();
    if let Some((i, _)) = sv.collectables().next() {
        out[offs[i] + 2] = item;
    }
    Ok(out)
}

/// The entrance layout with its clouds replaced by one short cloud per item
/// room, in a row across the first screen's sky.
fn item_count_clouds(ctx: &Ctx, r: &Room, n: usize) -> Result<Vec<u8>, RandoError> {
    let bank = r.key.group.bank();
    let (mut sv, _) = sideview::read_at(&ctx.vanilla, bank, r.sideview)?;
    let cloud = |c: &sideview::Command| c.is_small() && matches!(c.b, 7 | 8 | 0x0A..=0x0E);
    sv.commands.retain(|c| !cloud(c));
    for i in 0..n.min(4) {
        sv.commands
            .push(sideview::Command::new(3 + 3 * i as u8, 2, SHORT_CLOUD));
    }
    // Keep column order (floor changes first in a column) so the game's
    // floor sweep sees every change.
    sv.commands.sort_by_key(|c| (c.x, u8::from(!c.is_floor())));
    Ok(sv.encode()?)
}

/// Palace small object: short cloud.
const SHORT_CLOUD: u8 = 0x08;

/// The boss room's layout without the statue in front of its exit.
fn open_boss_room(ctx: &Ctx, r: &Room) -> Result<Vec<u8>, RandoError> {
    let bank = r.key.group.bank();
    let (mut sv, raw) = sideview::read_at(&ctx.vanilla, bank, r.sideview)?;
    let before = sv.commands.len();
    sv.commands
        .retain(|c| !(c.is_small() && c.b == IK_STATUE && c.x >= 56));
    if sv.commands.len() == before {
        return Ok(raw);
    }
    Ok(sv.encode()?)
}

fn rules_for(plan: &Plan, p: u8, vanilla_based: bool) -> Rules {
    Rules {
        require_thunderbird: p == 7 && plan.require_thunderbird && !plan.remove_thunderbird,
        // A vanilla-based palace keeps the vanilla distances.
        boss_min_distance: if p == 7 && !vanilla_based {
            plan.dark_link_distance
        } else {
            0
        },
        drops: if vanilla_based {
            DropRule::Anything
        } else {
            plan.drops[usize::from(p - 1)]
        },
    }
}

fn build_palace(
    ctx: &mut Ctx,
    pool: &VanillaPool,
    plan: &Plan,
    p: u8,
    max_rooms: usize,
) -> Result<Layout, RandoError> {
    let i = usize::from(p - 1);
    let style = plan.styles[i];
    match style_of(style) {
        None => build_vanilla_based(ctx, pool, plan, p, max_rooms),
        Some(s) => build_grown(ctx, pool, plan, p, s, max_rooms),
    }
}

fn build_vanilla_based(
    ctx: &mut Ctx,
    pool: &VanillaPool,
    plan: &Plan,
    p: u8,
    max_rooms: usize,
) -> Result<Layout, RandoError> {
    let i = usize::from(p - 1);
    let rules = rules_for(plan, p, true);
    let sources = item_room_sources(pool);
    for _attempt in 0..100 {
        let mut l = gen::vanilla(pool, p);
        if p == 7 && plan.remove_thunderbird {
            if let Some(t) = l.find(Role::Thunderbird) {
                if !gen::splice(&mut l, t) {
                    l.rooms[t].room.role = Role::Normal;
                }
            }
        }
        if plan.styles[i] == PalaceStyle::Shuffled {
            gen::shuffle(&mut l, &mut ctx.rng);
        }
        let target = plan.lengths[i].min(max_rooms);
        if l.rooms.len() > target {
            gen::shorten(&mut l, target, &mut ctx.rng);
        }
        if l.rooms.len() > max_rooms {
            continue;
        }
        if p <= 6 {
            adjust_item_rooms(ctx, &mut l, plan.items[i], &sources, pool)?;
        }
        match layout::validate(&l, &Rules { ..rules }) {
            Ok(()) | Err(layout::Invalid::OpenExit(..)) => return Ok(l),
            Err(_) => continue,
        }
    }
    Err(RandoError::Retry(format!(
        "palace {p}: no valid vanilla-based layout"
    )))
}

/// Make a vanilla-based palace have `n` item rooms: none (the item room
/// becomes an ordinary room of the same shape) or extra ones (plain dead
/// ends become copies of other palaces' item rooms facing the same way).
fn adjust_item_rooms(
    ctx: &mut Ctx,
    l: &mut Layout,
    n: usize,
    sources: &[Room],
    pool: &VanillaPool,
) -> Result<(), RandoError> {
    let have = l.all(Role::Item);
    if n == 0 {
        for &ix in &have {
            let shape = l.rooms[ix].room.shape.class();
            let repl = pool
                .rooms
                .values()
                .filter(|r| {
                    r.role == Role::Normal && r.shape.class() == shape && r.key.group != Group::Gp
                })
                .cloned()
                .collect::<Vec<_>>();
            if let Some(r) = ctx.rng.pick(&repl) {
                let mut r = r.clone();
                r.conn = l.rooms[ix].room.conn;
                l.rooms[ix].room = r;
            }
        }
        return Ok(());
    }
    let mut extra = n.saturating_sub(have.len());
    let mut dead: Vec<usize> = (0..l.rooms.len())
        .filter(|&j| {
            let s = l.rooms[j].room.shape;
            l.rooms[j].room.role == Role::Normal
                && s.exits() == 1
                && (s.left || s.right)
                && l.rooms[j].sideview.is_none()
        })
        .collect();
    ctx.rng.shuffle(&mut dead);
    for j in dead {
        if extra == 0 {
            break;
        }
        let s = l.rooms[j].room.shape;
        let fits: Vec<&Room> = sources
            .iter()
            .filter(|r| r.shape.left == s.left && r.shape.right == s.right && r.shape.exits() == 1)
            .collect();
        let Some(src) = ctx.rng.pick(&fits).copied() else {
            continue;
        };
        let mut r = src.clone();
        r.conn = l.rooms[j].room.conn;
        r.shape = s;
        l.rooms[j].sideview = Some(item_room_bytes(ctx, src, EXTRA_ROOM_ITEM)?);
        l.rooms[j].room = r;
        extra -= 1;
    }
    Ok(())
}

fn build_grown(
    ctx: &mut Ctx,
    pool: &VanillaPool,
    plan: &Plan,
    p: u8,
    style: Style,
    max_rooms: usize,
) -> Result<Layout, RandoError> {
    let i = usize::from(p - 1);
    let gp = p == 7;
    let mut allowed = if plan.blocking_anywhere {
        rooms::all_needs()
    } else {
        rooms::allowed_needs(p)
    };
    // The glove's own palace must not need the glove (it may not be
    // shuffled away).
    if p == 2 && ctx.flags.items.shuffle_palace_items != Tri::On {
        allowed = allowed.without(Needs::GLOVE);
    }
    let role = |r: Role| pool.role_room(p, r).map(|k| pool.rooms[&k].clone());
    let entrance = role(Role::Entrance)
        .ok_or_else(|| RandoError::Other(format!("palace {p}: no entrance room")))?;
    let boss = role(Role::Boss).ok_or_else(|| RandoError::Other(format!("palace {p}: no boss")))?;
    let thunderbird = if gp && !plan.remove_thunderbird {
        role(Role::Thunderbird)
    } else {
        None
    };
    // The pool: ordinary rooms of the same bank whose needs fit here, one
    // of each distinct layout + enemy set.
    let mut normal: Vec<Room> = Vec::new();
    for r in pool.rooms.values() {
        let same_bank = (r.key.group == Group::Gp) == gp;
        if !same_bank || r.role != Role::Normal || !r.needs.within(allowed) {
            continue;
        }
        if normal.iter().any(|o| o.same_content(r)) {
            continue;
        }
        normal.push(r.clone());
    }
    // Item rooms: the palace's own first, then copies of others.
    let sources = item_room_sources(pool);
    let mut items: Vec<Room> = Vec::new();
    if !gp && plan.items[i] > 0 {
        if let Some(own) = role(Role::Item) {
            items.push(own);
        }
        let mut k = 0;
        while items.len() < plan.items[i] && !sources.is_empty() {
            let src = &sources[(ctx.rng.index(sources.len()) + k) % sources.len()];
            k += 1;
            if src.shape.exits() == 1 {
                items.push(src.clone());
            }
            if k > 20 {
                break;
            }
        }
    }
    let parts = Parts {
        entrance,
        boss,
        items,
        thunderbird,
        pool: normal,
    };
    // Duplicate prevention needs a pool larger than the palace: with only
    // the original rooms that holds for short palaces 1-6.
    let dedupe = !gp && plan.lengths[i] < 22;
    let mut params = GrowParams {
        palace: p,
        style,
        target: plan.lengths[i].min(max_rooms),
        boss_continues: plan.boss_continues[i],
        thunderbird_gate: gp && plan.require_thunderbird,
        no_dup_layout: plan.no_dup_layout && dedupe,
        no_dup_content: plan.no_dup_content && dedupe,
    };
    let rules = rules_for(plan, p, false);
    let own_item = role(Role::Item).map(|r| r.key);
    // A style that keeps getting stuck (a grid style with a small pool)
    // falls back to Reconstructed, which can always tie its ends together.
    let mut too_big = 0;
    for attempt in 0..900 {
        // A palace that keeps overflowing its budget aims a room lower.
        if too_big >= 40 && params.target > 8 {
            params.target -= 1;
            too_big = 0;
        }
        if attempt == 600 && params.style != Style::Reconstructed {
            ctx.log(format!(
                "palaces: palace {p}: {} kept getting stuck; built as Reconstructed",
                params.style.name()
            ));
            params.style = Style::Reconstructed;
            params.no_dup_layout = false;
            params.no_dup_content = false;
        }
        let Some(mut l) = gen::grow(&params, &parts, &mut ctx.rng) else {
            continue;
        };
        if l.rooms.len() > (plan.lengths[i] + 6).min(max_rooms) {
            too_big += 1;
            continue;
        }
        if layout::validate(&l, &rules).is_err() {
            continue;
        }
        if params.style == Style::Chaos && !gen::strongly_connected(&l) {
            continue;
        }
        // Extra item rooms get their own copy holding a placeholder item;
        // a continuing boss room loses the statue in front of its exit.
        let mut own_used = false;
        for j in 0..l.rooms.len() {
            let r = l.rooms[j].room.clone();
            if r.role == Role::Item {
                if Some(r.key) == own_item && !own_used {
                    own_used = true;
                } else {
                    l.rooms[j].sideview = Some(item_room_bytes(ctx, &r, EXTRA_ROOM_ITEM)?);
                }
            }
            if r.role == Role::Boss && l.boss_continues {
                l.rooms[j].sideview = Some(open_boss_room(ctx, &r)?);
            }
        }
        return Ok(l);
    }
    Err(RandoError::Retry(format!(
        "palace {p}: {} found no valid layout",
        style.name()
    )))
}

fn write_all(
    ctx: &mut Ctx,
    pool: &VanillaPool,
    plan: &Plan,
    layouts: &mut [Layout],
) -> Result<(), RandoError> {
    let mut space = write::Space::from_rom(&ctx.rom);
    let mut written: Vec<write::WrittenRoom> = Vec::new();
    for group in Group::ALL {
        let ps = group.palaces();
        let mut refs: Vec<&mut Layout> = layouts
            .iter_mut()
            .filter(|l| ps.contains(&l.palace))
            .collect();
        write::assign_maps(group, &mut refs)?;
        let views: Vec<&Layout> = refs.iter().map(|l| &**l).collect();
        written.extend(write::write_group(&mut ctx.rom, group, &views, &mut space)?);
    }
    ctx.state.palaces.space_used = space.used.clone();
    ctx.state.palaces.generated = true;

    // Logic, state and spoiler.
    for l in layouts.iter() {
        let p = l.palace;
        let i = usize::from(p - 1);
        let mut needs = Needs::NONE;
        for r in &l.rooms {
            needs = needs.with(r.room.needs);
        }
        let rooms_info: Vec<RoomInfo> = l
            .rooms
            .iter()
            .enumerate()
            .map(|(j, r)| {
                let w = written.iter().find(|w| w.palace == p && w.index == j);
                RoomInfo {
                    bank: r.room.key.group.bank(),
                    map: r.map.unwrap_or(0),
                    sideview: w.map_or(r.room.sideview, |w| w.sideview),
                    enemies: r.room.enemies,
                    role: r.room.role,
                    from: (r.room.palace, r.room.key.map),
                    item_offset: if r.room.role == Role::Item {
                        w.and_then(|w| w.item_offset)
                    } else {
                        None
                    },
                }
            })
            .collect();
        update_world(ctx, l, &rooms_info, needs, is_vanilla_pool(plan.styles[i]));
        spoil(ctx, l, &rooms_info);
        ctx.state.palaces.palaces.push(PalaceInfo {
            palace: p,
            style: l.style.to_string(),
            rooms: rooms_info,
            boss_continues: l.boss_continues,
            needs: needs.names().iter().map(|s| (*s).to_string()).collect(),
        });
    }
    let _ = pool;
    Ok(())
}

/// The cheapest ways (as need sets) to reach `goal` from the entrance,
/// using only rooms whose needs are in the set; minimal sets only.
fn ways_to(l: &Layout, goal: usize, extra: Needs) -> Vec<Needs> {
    let mut all = Needs::NONE;
    for r in &l.rooms {
        all = all.with(r.room.needs);
    }
    let bits: Vec<u16> = (0..16)
        .map(|b| 1u16 << b)
        .filter(|b| all.0 & b != 0)
        .collect();
    let mut subsets: Vec<Needs> = (0u32..(1 << bits.len()))
        .map(|m| {
            Needs(
                bits.iter()
                    .enumerate()
                    .filter(|(i, _)| m & (1 << i) != 0)
                    .fold(0, |a, (_, b)| a | b),
            )
        })
        .collect();
    subsets.sort_by_key(|n| (n.0.count_ones(), n.0));
    let mut found: Vec<Needs> = Vec::new();
    for s in subsets {
        if found.iter().any(|f| f.within(s)) {
            continue;
        }
        let mut seen = vec![false; l.rooms.len()];
        let start = l.entrance();
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            for (_, n) in l.rooms[i].neighbours() {
                if !seen[n] && l.rooms[n].room.needs.without(Needs::REFLECT).within(s) {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        if seen[goal] {
            found.push(s);
        }
    }
    found.into_iter().map(|f| f.with(extra)).collect()
}

/// OR of the need sets as a world requirement.
fn ways_requirement(ways: &[Needs]) -> Requirement {
    if ways.is_empty() {
        return Requirement::never();
    }
    let mut r = needs_requirement(ways[0]);
    for w in &ways[1..] {
        r = r.or(&needs_requirement(*w));
    }
    r
}

fn needs_requirement(n: Needs) -> Requirement {
    let mut v = Vec::new();
    for (bit, item) in [
        (Needs::JUMP, ItemId::Jump),
        (Needs::FAIRY, ItemId::Fairy),
        (Needs::GLOVE, ItemId::Glove),
        (Needs::DOWNSTAB, ItemId::Downstab),
        (Needs::UPSTAB, ItemId::Upstab),
        (Needs::REFLECT, ItemId::Reflect),
        (Needs::KEY, ItemId::MagicKey),
    ] {
        if n.has(bit) {
            v.push(item);
        }
    }
    if v.is_empty() {
        Requirement::none()
    } else {
        Requirement::all(&v)
    }
}

/// Tell the shared world model about the palace's item rooms and boss.
fn update_world(ctx: &mut Ctx, l: &Layout, rooms: &[RoomInfo], needs: Needs, vanilla_based: bool) {
    let p = l.palace;
    let w = &mut ctx.state.world;
    let Some(spot) = w.palace_spot(p) else {
        return;
    };
    // Vanilla-based palaces keep the vanilla requirements.
    let old_item = w
        .loc(LocKey::PalaceItem { palace: p, room: 0 })
        .map(|x| x.requirement.clone());
    let old_boss = w.loc(LocKey::Boss(p)).map(|x| x.requirement.clone());
    let old_tb = w.loc(LocKey::Thunderbird).map(|x| x.requirement.clone());
    let old_dl = w.loc(LocKey::DarkLink).map(|x| x.requirement.clone());
    let base = needs_requirement(needs.without(Needs::REFLECT));
    let path = |goal: Option<usize>, extra: Needs| -> Requirement {
        goal.map_or_else(|| base.clone(), |g| ways_requirement(&ways_to(l, g, extra)))
    };
    let mut locs = Vec::new();
    let mut k = 0u8;
    for (j, r) in rooms.iter().enumerate() {
        if r.role != Role::Item {
            continue;
        }
        let item_req = if vanilla_based {
            old_item.clone().unwrap_or_else(|| base.clone())
        } else {
            path(Some(j), Needs::NONE)
        };
        let byte = r
            .item_offset
            .and_then(|o| ctx.rom.read(o).ok())
            .unwrap_or(EXTRA_ROOM_ITEM);
        let item = ItemId::from_byte(byte);
        locs.push(ItemLoc {
            key: LocKey::PalaceItem { palace: p, room: k },
            name: if k == 0 {
                format!("Palace {p} Item Room")
            } else {
                format!("Palace {p} Item Room {}", k + 1)
            },
            spot,
            requirement: item_req.clone(),
            vanilla: item,
            item,
            class: LocClass::Palace,
            store: r
                .item_offset
                .map_or(ItemStore::None, |o| ItemStore::Prg(vec![o])),
            required: true,
        });
        k += 1;
    }
    let boss_req = if vanilla_based {
        old_boss.unwrap_or_else(|| base.clone())
    } else {
        let extra = if p == 4 { Needs::REFLECT } else { Needs::NONE };
        path(l.boss(), extra)
    };
    let event = |key: LocKey, name: String, requirement: Requirement| ItemLoc {
        key,
        name,
        spot,
        requirement,
        vanilla: None,
        item: None,
        class: LocClass::Event,
        store: ItemStore::None,
        required: true,
    };
    if p <= 6 {
        locs.push(event(LocKey::Boss(p), format!("Palace {p} Boss"), boss_req));
    } else {
        let thunder = Requirement::item(ItemId::Thunder);
        let has_tb = l.find(Role::Thunderbird).is_some();
        let tb_req = if vanilla_based {
            old_tb.unwrap_or_else(|| base.and(&thunder))
        } else {
            path(l.find(Role::Thunderbird), Needs::NONE).and(&thunder)
        };
        // Dark Link needs Thunder only when Thunderbird stands in the way.
        let gated = has_tb && {
            let t = l.find(Role::Thunderbird).unwrap_or(0);
            let b = l.boss().unwrap_or(0);
            !l.reachable_from(l.entrance(), Some(t))[b]
        };
        let dl_req = if vanilla_based {
            old_dl.unwrap_or_else(|| base.and(&thunder))
        } else if gated {
            path(l.boss(), Needs::NONE).and(&thunder)
        } else {
            path(l.boss(), Needs::NONE)
        };
        if has_tb {
            locs.push(event(LocKey::Thunderbird, "Thunderbird".into(), tb_req));
        }
        locs.push(event(LocKey::DarkLink, "Dark Link".into(), dl_req));
    }
    ctx.state.world.set_palace_locations(p, locs);
}

fn spoil(ctx: &mut Ctx, l: &Layout, rooms: &[RoomInfo]) {
    let p = l.palace;
    let title = "Palaces";
    let items: Vec<String> = rooms
        .iter()
        .filter(|r| r.role == Role::Item)
        .map(|r| format!("map {}", r.map))
        .collect();
    ctx.spoiler.line(
        title,
        format!(
            "Palace {p}: {} rooms, style {}{}, item rooms: {}",
            l.rooms.len(),
            l.style,
            if l.boss_continues {
                ", boss room continues"
            } else {
                ""
            },
            if items.is_empty() {
                "none".to_string()
            } else {
                items.join(", ")
            }
        ),
    );
    for (j, r) in l.rooms.iter().enumerate() {
        let m = |x: Option<usize>| x.map_or("-".to_string(), |k| rooms[k].map.to_string());
        ctx.spoiler.line(
            title,
            format!(
                "  P{p} map {:2} {:<9} from P{} map {:2}  L {} R {} U {} D {} drop {}",
                rooms[j].map,
                format!("{:?}", r.room.role),
                r.room.palace,
                r.room.key.map,
                m(r.left),
                m(r.right),
                m(r.up),
                m(r.down),
                m(r.drop)
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Palace colours.
// ---------------------------------------------------------------------------

/// Per palace 1-6: (entrance palette, inside palette) in bank 4; the Great
/// Palace's two palettes are in bank 5.
fn palette_addrs(p: u8) -> (u8, u16, u16) {
    if p <= 6 {
        let i = u16::from(p - 1);
        (4, 0x8470 + 0x10 * i, 0xBF00 + 0x10 * i)
    } else {
        (5, 0x800E, 0x801E)
    }
}

/// A brick colour triad (shadow, main, highlight) of our own making: one
/// hue, three brightness steps, with black shadows half the time.
fn brick_triad(rng: &mut Rng) -> [u8; 3] {
    let hue = rng.range_u8(1, 12);
    let lift = rng.range_u8(0, 1) * 0x10;
    let shadow = if rng.coin() { 0x0F } else { hue + lift };
    [shadow, hue + 0x10 + lift, (hue + 0x20 + lift).min(0x3C)]
}

/// Curtain colours (light, mid, dark) in a second hue.
fn curtain_triad(rng: &mut Rng) -> [u8; 3] {
    let hue = rng.range_u8(1, 12);
    [hue + 0x20, hue + 0x10, hue]
}

/// Give every palace new brick, window and curtain colours.
fn shuffle_palettes(ctx: &mut Ctx) -> Result<(), RandoError> {
    for p in 1..=7u8 {
        let (bank, outside, inside) = palette_addrs(p);
        let bricks = brick_triad(&mut ctx.rng);
        let inner = brick_triad(&mut ctx.rng);
        let curtains = curtain_triad(&mut ctx.rng);
        ctx.rom.write_cpu(bank, outside + 5, &bricks)?;
        ctx.rom.write_cpu(bank, inside + 5, &inner)?;
        ctx.rom.write_cpu(bank, inside + 9, &[curtains[2]])?;
        ctx.rom.write_cpu(bank, inside + 13, &curtains)?;
    }
    ctx.spoiler.line("Palaces", "Palace colours changed");
    Ok(())
}

#[cfg(test)]
mod rom_tests {
    use super::rooms::*;
    use crate::flags::{Flags, PalaceLength, PalaceStyle};
    use crate::rom::Rom;

    fn vanilla() -> Option<Rom> {
        let body = z2_assets::rom::open().ok()?;
        Rom::from_body(&body).ok()
    }

    /// ROM-gated: every palace has its vanilla room count, one entrance,
    /// one boss.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_vanilla_pool_shape() {
        let rom = vanilla().expect("Z2_ROM");
        let pool = VanillaPool::read(&rom).unwrap();
        for p in 1..=7u8 {
            let keys = &pool.palace_rooms[usize::from(p - 1)];
            let roles: Vec<Role> = keys.iter().map(|k| pool.rooms[k].role).collect();
            assert_eq!(keys.len(), usize::from(VANILLA_LENGTHS[usize::from(p - 1)]));
            assert_eq!(roles.iter().filter(|&&r| r == Role::Entrance).count(), 1);
            assert_eq!(roles.iter().filter(|&&r| r == Role::Boss).count(), 1);
            if p <= 6 {
                assert_eq!(
                    roles.iter().filter(|&&r| r == Role::Item).count(),
                    1,
                    "P{p}"
                );
            } else {
                assert_eq!(roles.iter().filter(|&&r| r == Role::Thunderbird).count(), 1);
            }
        }
    }

    /// ROM-gated: the vanilla layouts written back give the vanilla bytes.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_vanilla_layouts_round_trip() {
        use super::write;
        let rom = vanilla().expect("Z2_ROM");
        let pool = VanillaPool::read(&rom).unwrap();
        let mut out = rom.clone();
        let mut space = write::Space::from_rom(&out);
        for group in Group::ALL {
            let mut ls: Vec<super::Layout> = group
                .palaces()
                .iter()
                .map(|&p| super::gen::vanilla(&pool, p))
                .collect();
            let mut refs: Vec<&mut super::Layout> = ls.iter_mut().collect();
            write::assign_maps(group, &mut refs).unwrap();
            let views: Vec<&super::Layout> = refs.iter().map(|l| &**l).collect();
            write::write_group(&mut out, group, &views, &mut space).unwrap();
        }
        assert!(space.used.is_empty());
        assert_eq!(out.body(), rom.body());
    }

    /// ROM-gated: every style, several seeds, through the whole pipeline.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_every_style_randomizes() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let styles = [
            PalaceStyle::Shuffled,
            PalaceStyle::Reconstructed,
            PalaceStyle::ReconstructedLoopy,
            PalaceStyle::Chaos,
            PalaceStyle::RandomWalk,
            PalaceStyle::VanillaWeighted,
            PalaceStyle::Sequential,
            PalaceStyle::Tower,
            PalaceStyle::Mirror,
        ];
        for s in styles {
            for seed in ["a", "b", "c"] {
                let mut f = Flags::default();
                f.palaces.normal_style = s;
                f.palaces.gp_style =
                    if matches!(s, PalaceStyle::VanillaWeighted | PalaceStyle::Mirror) {
                        PalaceStyle::Reconstructed
                    } else {
                        s
                    };
                f.palaces.normal_length = PalaceLength::Medium;
                let out = crate::randomize(&body, seed, &f)
                    .unwrap_or_else(|e| panic!("{s:?} {seed}: {e}"));
                assert_ne!(out.body, body, "{s:?} {seed}");
                assert!(out.spoiler.contains("Palace 7:"), "{s:?}");
            }
        }
    }

    /// ROM-gated: many seeds with a spread of palace options all produce a
    /// seed (the pipeline's own checks, including beatability, pass).
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_palace_option_mix() {
        use crate::flags::{BossRoomsExit, DarkLinkDistance, ItemRoomCount, PalaceDropStyle, Tri};
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let mut rng = crate::rng::Rng::new(77);
        for n in 0..24 {
            let mut f = Flags::default();
            let p = &mut f.palaces;
            p.normal_style = *rng
                .pick(&[
                    PalaceStyle::RandomPerPalace,
                    PalaceStyle::RandomAll,
                    PalaceStyle::Chaos,
                    PalaceStyle::ReconstructedLoopy,
                ])
                .unwrap();
            p.gp_style = *rng
                .pick(&[
                    PalaceStyle::Random,
                    PalaceStyle::Tower,
                    PalaceStyle::Shuffled,
                ])
                .unwrap();
            p.normal_length = *rng.pick(PalaceLength::ALL).unwrap();
            p.gp_length = *rng.pick(PalaceLength::ALL).unwrap();
            p.boss_rooms_exit = *rng.pick(BossRoomsExit::ALL).unwrap();
            p.dark_link_min_distance = *rng.pick(DarkLinkDistance::ALL).unwrap();
            p.item_rooms_per_palace = *rng.pick(ItemRoomCount::ALL).unwrap();
            p.drop_style = *rng.pick(PalaceDropStyle::ALL).unwrap();
            p.no_duplicate_rooms_by_layout = rng.coin();
            p.blocking_rooms_in_any_palace = rng.coin();
            p.thunderbird_required = *rng.pick(Tri::ALL).unwrap();
            p.remove_thunderbird = rng.coin();
            p.random_styles_allow_vanilla = rng.coin();
            p.change_palace_palettes = rng.coin();
            p.aggressive_thunderbird = rng.coin();
            p.palaces_to_complete_min = 3;
            p.palaces_to_complete_max = 6;
            let seed = format!("mix-{n}");
            let out =
                crate::randomize(&body, &seed, &f).unwrap_or_else(|e| panic!("{seed}: {e}\n{f:?}"));
            assert!(out.spoiler.contains("Palace 1:"), "{seed}");
            let again = crate::randomize(&body, &seed, &f).unwrap();
            assert_eq!(out.body, again.body, "{seed}: deterministic");
        }
    }
}
