//! "Vanilla (shuffled locations)": the vanilla terrain with the locations
//! moved around.
//!
//! Our approach keeps the map's structure intact:
//!
//! * Single locations (towns, palaces, item caves, encounter tiles) trade
//!   places freely.
//! * Linked groups (the two ends of a passthrough cave, Saria's two doors,
//!   Death Mountain's four-way caves) trade places only with groups of the
//!   same size, end for end. Wherever a pair of mouths joined two parts of
//!   the map before, some pair still joins them, so the terrain stays
//!   connected the way it was.
//! * The North Palace (the start) and the continent connectors stay put.
//! * Where a location lands, the tile takes its icon (a town tile shows a
//!   town), unless the legacy option keeps the vanilla icons.
//!
//! A position keeps its "passthrough" bit (the encounter tiles in narrow
//! passes let Link out on the far side), except that caves, towns, palaces
//! and item caves never pass through.

use super::continent::{Continent, Reveal};
use super::loc::{self, Class};
use super::map::Cont;
use super::terrain::Terrain;
use crate::rng::Rng;
use crate::RandoError;

/// Options for one continent's shuffle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShuffleOpts {
    /// Keep the vanilla icons (do not repaint the tiles).
    pub legacy: bool,
    /// The hidden spots take whatever lands on them.
    pub shuffle_hidden: bool,
}

/// A spot a location can occupy.
#[derive(Debug, Clone, Copy)]
struct Place {
    r: usize,
    c: usize,
    pass: bool,
    hidden: Option<Reveal>,
    cover: Terrain,
}

fn place_of(c: &Continent, slot: usize) -> Option<Place> {
    let (r, col) = c.pos(slot)?;
    Some(Place {
        r,
        c: col,
        pass: c.locs[slot].pass,
        hidden: c.hidden[slot],
        cover: c.cover[slot],
    })
}

fn put(c: &mut Continent, slot: usize, p: Place) {
    c.locs[slot].set_pos(p.r, p.c);
    c.locs[slot].pass = p.pass;
    c.hidden[slot] = p.hidden;
    c.cover[slot] = p.cover;
    c.table_changed = true;
}

/// Whether a slot's location may pass through (exit on the far side).
#[must_use]
pub fn may_pass(c: &Continent, slot: usize) -> bool {
    match c.info(slot).map(|i| i.class) {
        Some(Class::Palace(_) | Class::Town | Class::Special | Class::Link) => false,
        Some(Class::Item) => c.icon[slot] != Terrain::Cave,
        Some(Class::Connector | Class::Start | Class::Cave) | None => false,
        Some(Class::Minor | Class::Encounter) => true,
    }
}

/// Slots that are hidden twins of another slot (the second "minor lava"
/// entry in East Hyrule sits on the same tile as slot 28 and follows it).
fn twin_of(cont: Cont, slot: usize) -> Option<usize> {
    match (cont, slot) {
        (Cont::East, 22) => Some(28),
        _ => None,
    }
}

/// Shuffle the locations of one vanilla-terrain continent.
pub fn shuffle(c: &mut Continent, rng: &mut Rng, o: ShuffleOpts) -> Result<(), RandoError> {
    let cont = c.cont;
    // Singles and groups.
    let mut singles: Vec<usize> = Vec::new();
    let mut groups: Vec<&'static [u8]> = Vec::new();
    for s in c.placed() {
        let Some(info) = c.info(s) else { continue };
        if matches!(info.class, Class::Start | Class::Connector) || twin_of(cont, s).is_some() {
            continue;
        }
        if !o.shuffle_hidden && c.hidden[s].is_some() {
            continue;
        }
        if info.group.is_empty() {
            singles.push(s);
        } else if !groups.contains(&info.group)
            && info.group.iter().all(|&g| c.pos(usize::from(g)).is_some())
        {
            groups.push(info.group);
        }
    }

    // Singles.
    let places: Vec<Place> = singles.iter().filter_map(|&s| place_of(c, s)).collect();
    let mut order = singles.clone();
    rng.shuffle(&mut order);
    for (k, &s) in order.iter().enumerate() {
        put(c, s, places[k]);
    }

    // Groups trade places only with groups of the same size that are
    // plain two-way passages: the jump cave (needs a spell), the fairy cave
    // (one way, needs the fairy) and Saria's doors (the moat) keep their
    // own spots, so they cannot shut the start area in.
    let special = |g: &[u8]| cont == Cont::West && [12u8, 17, 48].contains(&g[0]);
    groups.retain(|g| !special(g));
    let mut sizes: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    sizes.sort_unstable();
    sizes.dedup();
    for n in sizes {
        let same: Vec<&'static [u8]> = groups.iter().copied().filter(|g| g.len() == n).collect();
        let homes: Vec<Vec<Place>> = same
            .iter()
            .map(|g| {
                g.iter()
                    .filter_map(|&m| place_of(c, usize::from(m)))
                    .collect()
            })
            .collect();
        let mut order = same.clone();
        rng.shuffle(&mut order);
        for (k, g) in order.iter().enumerate() {
            // Either orientation: a cave works both ways round.
            let flip = n == 2 && rng.coin();
            for (j, &m) in g.iter().enumerate() {
                let jj = if flip { n - 1 - j } else { j };
                put(c, usize::from(m), homes[k][jj]);
            }
        }
    }

    // Twins follow.
    for s in 0..loc::SLOTS {
        if let Some(t) = twin_of(cont, s) {
            if c.used[s] && c.used[t] {
                let (r, col) = c.locs[t].pos().unwrap_or((0, 0));
                c.locs[s].set_pos(r, col);
                c.on_map[s] = c.on_map[t];
            }
        }
    }

    // Passthrough cleanup.
    for s in c.placed() {
        let want = if cont == Cont::Maze {
            may_pass(c, s)
        } else {
            c.locs[s].pass && may_pass(c, s)
        };
        c.locs[s].pass = want;
    }
    if cont == Cont::DeathMountain {
        // Spectacle rock shows a boulder wherever it goes.
        c.icon[56] = Terrain::Rock;
    }
    if !o.legacy {
        c.paint_icons();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overworld::loc::Loc;
    use crate::overworld::map::Grid;

    fn toy() -> Continent {
        let mut c = Continent {
            cont: Cont::West,
            grid: Grid::filled(Terrain::Grass),
            rows: 75,
            cols: 64,
            locs: vec![Loc::from_bytes([0; 4]); loc::SLOTS],
            used: vec![false; loc::SLOTS],
            on_map: vec![false; loc::SLOTS],
            hidden: vec![None; loc::SLOTS],
            icon: vec![Terrain::Grass; loc::SLOTS],
            cover: vec![Terrain::Grass; loc::SLOTS],
            map_changed: false,
            table_changed: false,
            separator: None,
            vanilla_ptr: 0,
            palace: vec![None; loc::SLOTS],
            dock: None,
        };
        for (k, s) in [0usize, 1, 3, 10, 11, 12, 13, 45, 52].iter().enumerate() {
            c.used[*s] = true;
            c.on_map[*s] = true;
            c.locs[*s].set_pos(10 + k, 5 + 2 * k);
            c.icon[*s] = match s {
                45 => Terrain::Town,
                52 => Terrain::Palace,
                _ => Terrain::Cave,
            };
        }
        c
    }

    #[test]
    fn start_stays_and_pairs_move_together() {
        for seed in 0..20 {
            let mut c = toy();
            let before: Vec<_> = (0..loc::SLOTS).map(|s| c.pos(s)).collect();
            shuffle(
                &mut c,
                &mut Rng::new(seed),
                ShuffleOpts {
                    legacy: false,
                    shuffle_hidden: false,
                },
            )
            .unwrap();
            assert_eq!(c.pos(0), before[0]);
            // The set of positions is unchanged.
            let mut a: Vec<_> = before.iter().flatten().copied().collect();
            let mut b: Vec<_> = (0..loc::SLOTS).filter_map(|s| c.pos(s)).collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b);
            // Pairs occupy a former pair's two homes.
            let p1 = [c.pos(10).unwrap(), c.pos(11).unwrap()];
            let homes = [
                [before[10].unwrap(), before[11].unwrap()],
                [before[12].unwrap(), before[13].unwrap()],
            ];
            assert!(homes
                .iter()
                .any(|h| (h[0] == p1[0] && h[1] == p1[1]) || (h[0] == p1[1] && h[1] == p1[0])));
            // Icons are painted.
            let (r, col) = c.pos(45).unwrap();
            assert_eq!(c.grid.get(r, col), Terrain::Town);
        }
    }
}
