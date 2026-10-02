//! Overworld and palace enemy placement shuffles.
//!
//! # Where the lists are
//!
//! Each sideview bank keeps its rooms' enemy lists in one blob at `$88A0`,
//! which the game copies to WRAM `$7000` when an area loads. Every world (or
//! palace group) has a table of 63 sideview pointers and, right after it, a
//! table of 63 enemy-list pointers (`$7xxx`, i.e. offsets into the blob):
//!
//! | Area | Bank | Sideview pointers | Enemy pointers |
//! |---|---|---|---|
//! | West Hyrule | 1 | `$8523` | `$85A1` |
//! | Death Mountain | 1 | `$A000` | `$A07E` |
//! | East Hyrule | 2 | `$8523` | `$85A1` |
//! | Maze Island | 2 | `$A000` | `$A07E` |
//! | Palaces 1/2/5 | 4 | `$8523` | `$85A1` |
//! | Palaces 3/4/6 | 4 | `$A000` | `$A07E` |
//! | Great Palace | 5 | `$8523` | `$85A1` |
//!
//! A list is a length byte (the whole list, itself included) then two bytes
//! per enemy: `YX` (Y row code in the high nibble, column low bits in the
//! low nibble) and `PI` (page in bits 7-6, enemy ID in bits 5-0).
//! Overworld encounter scenes point at two lists back to back (the small
//! encounter, then the large one).
//!
//! # What the shuffle does
//!
//! The lists are edited **in place**: their lengths never change, only enemy
//! IDs and rows (and, for encounter scenes, columns pushed away from Link's
//! entry point). The pointers are read from the current ROM, so rooms that
//! an earlier module rebuilt are shuffled as they are now. A list shared by
//! several rooms must suit every one of them.
//!
//! * Generators are replaced by generators (all the same one per list with
//!   "generators always match").
//! * Flyers are replaced by flyers. Ceiling flyers (Ache, Acheman, Deelers)
//!   are moved to the top row. In palaces 1-6 the orange Moa stays a Moa.
//!   Great Palace bubbles and King Bot need open space around them.
//! * Ground enemies are replaced by ground enemies of the same size (or of
//!   either size with "mix large and small"), then dropped onto the floor
//!   below their spot. Enemies with special needs are only placed where they
//!   work (Megmet, Leever, Geldarm, Mago/Wizard, Stalfos, Doomknocker).
//! * Anything else (bosses, doors, elevators, items, statues, fairies) is
//!   never touched. A slot that cannot be filled keeps its vanilla enemy.

use std::collections::BTreeMap;

use super::data::{EnemySets, Group};
use crate::flags::EnemyFlags;
use crate::rng::Rng;
use crate::rom::Rom;
use crate::sideview::{self, FloorTable, Grid, ObjectSet, ROWS};
use crate::{Ctx, RandoError};

/// Where the enemy lists of the shared blob live in ROM.
pub const BLOB_ADDR: u16 = 0x88A0;
/// Size of the blob (`$7000-$73FF` in WRAM).
pub const BLOB_LEN: u16 = 0x400;
/// WRAM address the blob is copied to.
pub const BLOB_WRAM: u16 = 0x7000;
/// Rooms per area table.
pub const ROOMS: u16 = 63;
/// Column where Link enters an encounter scene.
pub const ENCOUNTER_SPAWN_X: i16 = 24;

/// One area (world or palace group) with its tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    /// Display name.
    pub name: &'static str,
    /// Enemy group (ID space).
    pub group: Group,
    /// Bank.
    pub bank: u8,
    /// Sideview pointer table.
    pub sideviews: u16,
    /// Enemy pointer table.
    pub enemies: u16,
    /// Rooms whose pointer leads to a small then a large encounter list.
    pub encounters: &'static [u16],
}

const WEST_ENCOUNTERS: &[u16] = &[29, 30, 34, 35, 39, 40, 47, 48, 52, 53, 57, 58];
const DM_ENCOUNTERS: &[u16] = &[29, 30, 34, 35, 39, 40, 47, 48, 52, 53, 57, 58];
const EAST_ENCOUNTERS: &[u16] = &[29, 30, 34, 35, 39, 40, 47, 48, 52, 53, 57, 58, 59, 60];

/// The overworld areas.
pub const OVERWORLD: [Area; 4] = [
    Area {
        name: "West Hyrule",
        group: Group::West,
        bank: 1,
        sideviews: 0x8523,
        enemies: 0x85A1,
        encounters: WEST_ENCOUNTERS,
    },
    Area {
        name: "Death Mountain",
        group: Group::West,
        bank: 1,
        sideviews: 0xA000,
        enemies: 0xA07E,
        encounters: DM_ENCOUNTERS,
    },
    Area {
        name: "East Hyrule",
        group: Group::East,
        bank: 2,
        sideviews: 0x8523,
        enemies: 0x85A1,
        encounters: EAST_ENCOUNTERS,
    },
    Area {
        name: "Maze Island",
        group: Group::East,
        bank: 2,
        sideviews: 0xA000,
        enemies: 0xA07E,
        encounters: &[],
    },
];

/// The palace groups.
pub const PALACES: [Area; 3] = [
    Area {
        name: "Palaces 1/2/5",
        group: Group::Palace125,
        bank: 4,
        sideviews: 0x8523,
        enemies: 0x85A1,
        encounters: &[],
    },
    Area {
        name: "Palaces 3/4/6",
        group: Group::Palace346,
        bank: 4,
        sideviews: 0xA000,
        enemies: 0xA07E,
        encounters: &[],
    },
    Area {
        name: "Great Palace",
        group: Group::GreatPalace,
        bank: 5,
        sideviews: 0x8523,
        enemies: 0x85A1,
        encounters: &[],
    },
];

/// One enemy of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enemy {
    /// Column 0-63.
    pub x: u8,
    /// Row (decoded: raw 0 is row 1, raw n is row n + 2).
    pub y: u8,
    /// Enemy ID (0-63).
    pub id: u8,
}

impl Enemy {
    /// Decode one 2-byte entry.
    #[must_use]
    pub fn decode(b0: u8, b1: u8) -> Enemy {
        let raw_y = b0 >> 4;
        Enemy {
            x: ((b1 >> 6) << 4) | (b0 & 0x0F),
            y: if raw_y == 0 { 1 } else { raw_y + 2 },
            id: b1 & 0x3F,
        }
    }

    /// Encode; rows below 3 become raw 0 (row 1).
    #[must_use]
    pub fn encode(self) -> [u8; 2] {
        let raw_y = if self.y < 3 { 0 } else { (self.y - 2).min(15) };
        [
            (raw_y << 4) | (self.x & 0x0F),
            ((self.x >> 4) & 3) << 6 | (self.id & 0x3F),
        ]
    }

    /// Can `y` be stored exactly?
    #[must_use]
    pub fn encodable_row(y: u8) -> bool {
        y == 1 || (3..=17).contains(&y)
    }
}

/// Enemy category inside a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Small ground enemy.
    Small,
    /// Large ground enemy.
    Large,
    /// Flyer.
    Flying,
    /// Generator.
    Generator,
    /// West's dumb-Moblin generator (may become a normal generator).
    DumbMoblinGenerator,
}

/// West's dumb-Moblin generator.
pub const DUMB_MOBLIN_GENERATOR: u8 = 0x21;

fn kind_in(sets: &EnemySets, id: u8) -> Option<Kind> {
    if sets.small.contains(&id) {
        Some(Kind::Small)
    } else if sets.large.contains(&id) {
        Some(Kind::Large)
    } else if sets.flying.contains(&id) {
        Some(Kind::Flying)
    } else if sets.generators.contains(&id) {
        Some(Kind::Generator)
    } else {
        None
    }
}

/// Category of `id` in `g`. Palace 1/2/5 and 3/4/6 rooms also accept the
/// other group's IDs (rooms can move between the two in rebuilt palaces).
#[must_use]
pub fn kind_of(g: Group, id: u8) -> Option<Kind> {
    if g == Group::West && id == DUMB_MOBLIN_GENERATOR {
        return Some(Kind::DumbMoblinGenerator);
    }
    // 0x0A is the unhorsed Rebonack in palaces 3/4/6, never a generator.
    if g == Group::Palace346 && id == 0x0A {
        return None;
    }
    kind_in(g.sets(), id).or_else(|| match g {
        Group::Palace125 => kind_in(Group::Palace346.sets(), id),
        Group::Palace346 => kind_in(Group::Palace125.sets(), id),
        _ => None,
    })
}

/// A list's location and the rooms that use it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListSite {
    bank: u8,
    addr: u16,
    encounter: bool,
    grids: Vec<Grid>,
    group: Group,
}

/// Collect every distinct list of `areas` with the grids of the rooms that
/// use it.
fn collect_lists(rom: &Rom, areas: &[Area]) -> Result<Vec<ListSite>, RandoError> {
    let floors = FloorTable::from_rom(rom)?;
    let mut sites: BTreeMap<(u8, u16), ListSite> = BTreeMap::new();
    for a in areas {
        let set = match a.group {
            Group::Palace125 | Group::Palace346 => ObjectSet::Palace,
            Group::GreatPalace => ObjectSet::GreatPalace,
            _ => ObjectSet::Other,
        };
        for room in 0..ROOMS {
            let ptr = rom.read_cpu_word(a.bank, a.enemies + 2 * room)?;
            if !(BLOB_WRAM..BLOB_WRAM + BLOB_LEN).contains(&ptr) {
                continue;
            }
            let sv_ptr = rom.read_cpu_word(a.bank, a.sideviews + 2 * room)?;
            if !(0x8000..0xC000).contains(&sv_ptr) {
                continue;
            }
            let Ok((sv, _)) = sideview::read_at(rom, a.bank, sv_ptr) else {
                continue;
            };
            let grid = sv.grid(set, &floors);
            let first = BLOB_ADDR + (ptr - BLOB_WRAM);
            let mut addrs = vec![(first, false)];
            if a.encounters.contains(&room) {
                let len = u16::from(rom.read_cpu(a.bank, first)?);
                addrs = vec![(first, true), (first + len.max(1), true)];
            }
            for (addr, encounter) in addrs {
                if addr + 1 > BLOB_ADDR + BLOB_LEN {
                    continue;
                }
                let site = sites.entry((a.bank, addr)).or_insert_with(|| ListSite {
                    bank: a.bank,
                    addr,
                    encounter,
                    grids: Vec::new(),
                    group: a.group,
                });
                site.encounter |= encounter;
                site.grids.push(grid.clone());
            }
        }
    }
    Ok(sites.into_values().collect())
}

/// Every distinct enemy list of `areas` as `(bank, addr, group)`.
pub fn list_sites(rom: &Rom, areas: &[Area]) -> Result<Vec<(u8, u16, Group)>, RandoError> {
    Ok(collect_lists(rom, areas)?
        .into_iter()
        .map(|s| (s.bank, s.addr, s.group))
        .collect())
}

/// Decode the enemy list at `(bank, addr)` (empty when it has no enemies).
pub fn read_list(rom: &Rom, bank: u8, addr: u16) -> Result<Vec<Enemy>, RandoError> {
    let len = rom.read_cpu(bank, addr)?;
    if len < 3 || len % 2 == 0 {
        return Ok(Vec::new());
    }
    let mut v = Vec::new();
    for i in 0..u16::from(len / 2) {
        let a = addr + 1 + 2 * i;
        v.push(Enemy::decode(
            rom.read_cpu(bank, a)?,
            rom.read_cpu(bank, a + 1)?,
        ));
    }
    Ok(v)
}

fn write_list(rom: &mut Rom, bank: u8, addr: u16, list: &[Enemy]) -> Result<(), RandoError> {
    for (i, e) in list.iter().enumerate() {
        rom.write_cpu(bank, addr + 1 + 2 * i as u16, &e.encode())?;
    }
    Ok(())
}

/// Is the box `[x0, x1] x [y0, y1]` open (not solid, not lava) in `g`?
fn open_box(g: &Grid, x0: i32, x1: i32, y0: i32, y1: i32) -> bool {
    for x in x0..=x1 {
        for y in y0..=y1 {
            if x < 0 || y < 0 || x as usize >= g.width() || y as usize >= ROWS {
                return false;
            }
            let c = g.cell(x as usize, y as usize);
            if c.is_solid() || c == sideview::Cell::Lava {
                return false;
            }
        }
    }
    true
}

/// Where a `height`-tall ground enemy at column `x` ends up when dropped
/// from top row `y` (or onto the nearest floor above, if there is none
/// below): `(top, feet)` rows, with `feet` standing on a solid cell and
/// every row from `top` to `feet` open. `None` if no floor has room.
fn floor_snap(g: &Grid, x: u8, y: u8, height: u8) -> Option<(u8, u8)> {
    let x = usize::from(x);
    if x >= g.width() {
        return None;
    }
    let h = usize::from(height.max(1));
    let rows = g.standable_rows(x);
    let fits =
        |r: usize| r + 1 >= h && open_box(g, x as i32, x as i32, (r + 1 - h) as i32, r as i32);
    let start = usize::from(y) + h - 1;
    let below = rows.iter().copied().find(|&r| r >= start && fits(r));
    let above = rows.iter().rev().copied().find(|&r| r < start && fits(r));
    below.or(above).map(|r| ((r + 1 - h) as u8, r as u8))
}

/// Shuffle parameters.
#[derive(Debug, Clone, Copy)]
struct Rules {
    mix: bool,
    generators_match: bool,
    palace: bool,
}

/// Overworld group facts used by the placement rules.
const MEGMET: u8 = 0x1F;
const LEEVER: u8 = 0x16;
const GELDARM: u8 = 0x20;
const CEILING_FLYERS: [u8; 4] = [0x07, 0x0A, 0x0D, 0x0E];
/// Palace facts.
const ORANGE_MOA: u8 = 0x07;
const MAGO_OR_WIZARD: u8 = 0x1D;
const DOOMKNOCKER: u8 = 0x1E;
const STALFOS: [u8; 2] = [0x1F, 0x23];
/// Great Palace facts.
const GP_SMALL_BUBBLES: [u8; 2] = [0x14, 0x15];
const GP_BIG_BUBBLE: u8 = 0x17;
const GP_KING_BOT: u8 = 0x1E;

/// Try to put ground enemy `id` at `(x, orig.y)` in every grid. Returns the
/// (top) row to store.
fn place_ground(site: &ListSite, orig: Enemy, id: u8, large: bool) -> Option<u8> {
    let g = site.group;
    let palace = matches!(g, Group::Palace125 | Group::Palace346);
    let mut y = orig.y;
    if large && kind_of(g, orig.id) == Some(Kind::Small) {
        y = y.saturating_sub(1);
    }
    // Mago / Wizard: only where the original stood on row 9.
    if palace && id == MAGO_OR_WIZARD {
        return (orig.y == 9).then_some(9);
    }
    let x = i32::from(orig.x);
    let mut row: Option<u8> = None;
    for grid in &site.grids {
        let top = if palace && STALFOS.contains(&id) {
            // Stalfos drop in from the ceiling: the column above must be
            // open (unless a Stalfos was already there).
            let (top, feet) = floor_snap(grid, orig.x, y, 2)?;
            if !STALFOS.contains(&orig.id)
                && !open_box(grid, x, x, 4.min(i32::from(top)), feet.into())
            {
                return None;
            }
            top
        } else if palace && id == DOOMKNOCKER {
            // Swings a mace: needs room on both sides.
            let (top, feet) = floor_snap(grid, orig.x, y, 2)?;
            let (t, f) = ((i32::from(feet) - 3).max(0), i32::from(feet));
            if !open_box(grid, x - 4, x - 1, t, f) || !open_box(grid, x + 1, x + 4, t, f) {
                return None;
            }
            top
        } else if !palace && id == GELDARM {
            // Tall and stretchy: the tallest body (5 down to 1 rows) that
            // fits; never with its head at the top of the screen.
            let (_, feet) = floor_snap(grid, orig.x, y, 1)?;
            let top = (1..=5u8)
                .rev()
                .filter(|&h| feet + 1 >= h)
                .map(|h| feet + 1 - h)
                .find(|&t| open_box(grid, x, x, t.into(), feet.into()))?;
            if top <= 2 {
                return None;
            }
            top
        } else if !palace && g == Group::East && id == LEEVER {
            // Leevers burrow: only at ground level.
            let (top, feet) = floor_snap(grid, orig.x, y, 1)?;
            if feet < 8 {
                return None;
            }
            top
        } else if !palace && g == Group::West && id == MEGMET {
            // Megmets hop: one row up when there is room.
            let (top, _) = floor_snap(grid, orig.x, y, 1)?;
            if top > 0 && !grid.is_solid(orig.x.into(), usize::from(top) - 1) {
                top - 1
            } else {
                top
            }
        } else {
            floor_snap(grid, orig.x, y, if large { 2 } else { 1 })?.0
        };
        match row {
            None => row = Some(top),
            Some(prev) if prev == top => {}
            Some(_) => return None,
        }
    }
    let r = row?;
    // Overworld rows stay on screen above the status area.
    if !palace && r > 9 {
        return None;
    }
    Enemy::encodable_row(r).then_some(r)
}

/// Great Palace flyers that need room around them.
fn place_gp_flyer(site: &ListSite, orig: Enemy, id: u8) -> Option<u8> {
    let x = i32::from(orig.x);
    let ok_at = |y: u8| {
        let yi = i32::from(y);
        site.grids.iter().all(|g| match id {
            _ if GP_SMALL_BUBBLES.contains(&id) => open_box(g, x, x, yi, yi),
            GP_BIG_BUBBLE => open_box(g, x, x + 1, yi, yi + 1),
            GP_KING_BOT => {
                let near_elevator = g.elevator.is_some_and(|e| {
                    let c = i32::from(e / 16) * 16 + 7;
                    (c - 3..=c + 2).contains(&x)
                });
                let drop_zone = !g.drop_pages().is_empty() && (14..=48).contains(&x);
                !near_elevator && !drop_zone && open_box(g, x - 1, x + 1, yi, yi + 5)
            }
            _ => true,
        })
    };
    if id == GP_KING_BOT {
        return [3u8, 4, 1, 5].into_iter().find(|&y| ok_at(y));
    }
    ok_at(orig.y).then_some(orig.y)
}

fn shuffle_list(rng: &mut Rng, site: &ListSite, list: &mut [Enemy], rules: Rules) -> usize {
    let g = site.group;
    let sets = g.sets();
    let mut first_generator: Option<u8> = None;
    let mut changed = 0;
    for e in list.iter_mut() {
        let Some(kind) = kind_of(g, e.id) else {
            continue;
        };
        let before = *e;
        match kind {
            Kind::Generator | Kind::DumbMoblinGenerator => {
                let pick = match first_generator {
                    Some(id) if rules.generators_match => id,
                    _ => {
                        let n = sets.generators.len();
                        if kind == Kind::DumbMoblinGenerator {
                            // Stay a Moblin generator, or become a normal one.
                            let i = rng.index(n + 1);
                            if i == n {
                                DUMB_MOBLIN_GENERATOR
                            } else {
                                sets.generators[i]
                            }
                        } else {
                            sets.generators[rng.index(n)]
                        }
                    }
                };
                if first_generator.is_none() && pick != DUMB_MOBLIN_GENERATOR {
                    first_generator = Some(pick);
                }
                e.id = pick;
            }
            Kind::Flying => {
                if rules.palace && g != Group::GreatPalace && e.id == ORANGE_MOA {
                    continue;
                }
                for _ in 0..16 {
                    let id = sets.flying[rng.index(sets.flying.len())];
                    if g == Group::GreatPalace {
                        if let Some(y) = place_gp_flyer(site, before, id) {
                            *e = Enemy { id, y, ..before };
                            break;
                        }
                    } else if matches!(g, Group::West | Group::East) && CEILING_FLYERS.contains(&id)
                    {
                        *e = Enemy { id, y: 1, ..before };
                        break;
                    } else {
                        *e = Enemy { id, ..before };
                        break;
                    }
                }
            }
            Kind::Small | Kind::Large => {
                let pool: Vec<u8> = if rules.mix {
                    sets.ground()
                } else if kind == Kind::Small {
                    sets.small.to_vec()
                } else {
                    sets.large.to_vec()
                };
                // Palace small enemies keep their spot when sizes do not mix.
                let keep_spot = rules.palace && !rules.mix && kind == Kind::Small;
                for _ in 0..16 {
                    let id = pool[rng.index(pool.len())];
                    let large = sets.large.contains(&id);
                    let y = if keep_spot {
                        Some(before.y)
                    } else {
                        place_ground(site, before, id, large)
                    };
                    let Some(y) = y else { continue };
                    let mut x = before.x;
                    if site.encounter {
                        let reach = if id == GELDARM { 1 } else { 4 };
                        let dx = i16::from(x) - ENCOUNTER_SPAWN_X;
                        if dx.abs() <= reach {
                            x = if dx < 0 { 19 } else { 29 };
                            // The new column must also hold the enemy.
                            let moved = Enemy { x, ..before };
                            let Some(y2) = place_ground(site, moved, id, large) else {
                                continue;
                            };
                            *e = Enemy { x, y: y2, id };
                            break;
                        }
                    }
                    *e = Enemy { x, y, id };
                    break;
                }
            }
        }
        if *e != before {
            changed += 1;
        }
    }
    changed
}

fn shuffle_areas(
    ctx: &mut Ctx,
    areas: &[Area],
    rules: Rules,
    label: &str,
) -> Result<(), RandoError> {
    let sites = collect_lists(&ctx.rom, areas)?;
    let mut changed = 0;
    for site in &sites {
        let mut list = read_list(&ctx.rom, site.bank, site.addr)?;
        if list.is_empty() {
            continue;
        }
        changed += shuffle_list(&mut ctx.rng, site, &mut list, rules);
        write_list(&mut ctx.rom, site.bank, site.addr, &list)?;
    }
    ctx.spoiler.line(
        "Enemies",
        format!(
            "{label}: {changed} enemies changed in {} lists",
            sites.len()
        ),
    );
    Ok(())
}

/// Run the shuffles the options ask for.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let f: EnemyFlags = ctx.flags.enemies.clone();
    let overworld = ctx.tri(f.shuffle_overworld_enemies);
    let palace = ctx.tri(f.shuffle_palace_enemies);
    if !overworld && !palace {
        return Ok(());
    }
    let mix = ctx.tri(f.mix_large_and_small);
    if overworld {
        let rules = Rules {
            mix,
            generators_match: f.generators_always_match,
            palace: false,
        };
        shuffle_areas(ctx, &OVERWORLD, rules, "Overworld enemies shuffled")?;
    }
    if palace {
        let rules = Rules {
            mix,
            generators_match: f.generators_always_match,
            palace: true,
        };
        shuffle_areas(ctx, &PALACES, rules, "Palace enemies shuffled")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_codec_round_trips() {
        for b0 in 0..=255u8 {
            for b1 in [0x00u8, 0x45, 0x8C, 0xFF] {
                let e = Enemy::decode(b0, b1);
                assert_eq!(e.encode(), [b0, b1]);
            }
        }
        let e = Enemy { x: 40, y: 2, id: 3 };
        assert_eq!(Enemy::decode(e.encode()[0], e.encode()[1]).y, 1);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Group::West, 0x14), Some(Kind::Large));
        assert_eq!(kind_of(Group::West, 0x21), Some(Kind::DumbMoblinGenerator));
        assert_eq!(kind_of(Group::West, 0x13), None, "elevator");
        // Palace 1/2/5 IDs count in 3/4/6 rooms too, except 0x0A, which is
        // the unhorsed Rebonack there.
        assert_eq!(kind_of(Group::Palace346, 0x12), Some(Kind::Small));
        assert_eq!(kind_of(Group::Palace346, 0x0A), None);
        assert_eq!(kind_of(Group::Palace346, 0x22), None, "Carock");
        assert_eq!(kind_of(Group::GreatPalace, 0x1E), Some(Kind::Flying));
    }
}
