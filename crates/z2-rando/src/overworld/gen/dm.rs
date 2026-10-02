//! Death Mountain: small open pockets inside solid mountain, joined only by
//! caves.
//!
//! 1. Smaller sizes drop cave pairs (and at most one four-way cave) until
//!    the cave count fits (Large 37, Medium 27, Small 17, Tiny 9, counting
//!    the hammer cave).
//! 2. Pockets are scattered over the mountain: little blobs of ground.
//! 3. Every cave end becomes a door in a pocket wall (a cave tile walled by
//!    mountain on all sides but the one facing the pocket). The pairs (and
//!    four-way caves) are dealt out so that they join the pockets into one
//!    network first, then add loops. The hammer cave, Spectacle Rock and the
//!    two exits to West Hyrule go into random pockets.
//!
//! So every pocket, and every location, can be reached by construction.

use super::core::{ClimateDef, Dsu, Work, DIRS};
use crate::flags::{Biome, DmSize};
use crate::overworld::continent::Continent;
use crate::overworld::loc::{self, Class};
use crate::overworld::map::RAW_ROW_BASE;
use crate::overworld::terrain::Terrain;
use crate::overworld::Resolved;
use crate::rng::Rng;
use crate::RandoError;

fn retry(m: impl Into<String>) -> RandoError {
    RandoError::Retry(m.into())
}

/// Map size (rows, cols) for a Death Mountain size.
#[must_use]
pub fn dims(s: DmSize) -> (usize, usize) {
    match s {
        DmSize::Large => (45, 64),
        DmSize::Medium => (45, 45),
        DmSize::Small => (34, 34),
        DmSize::Tiny => (26, 26),
    }
}

/// Which cave groups survive a size reduction.
fn surviving_groups(size: DmSize, rng: &mut Rng) -> Vec<&'static [u8]> {
    let mut pairs: Vec<&'static [u8]> = Vec::new();
    let mut quads: Vec<&'static [u8]> = Vec::new();
    for s in loc::DM {
        if s.group.is_empty() || s.slot != s.group[0] {
            continue;
        }
        if s.group.len() == 4 {
            quads.push(s.group);
        } else {
            pairs.push(s.group);
        }
    }
    rng.shuffle(&mut pairs);
    rng.shuffle(&mut quads);
    let (drop_pairs, drop_quads) = match size {
        DmSize::Large => (0, 0),
        DmSize::Medium => (5, 0),
        DmSize::Small => (10, 0),
        DmSize::Tiny => (12, 1),
    };
    pairs.truncate(pairs.len().saturating_sub(drop_pairs));
    quads.truncate(quads.len().saturating_sub(drop_quads));
    let mut v = quads;
    v.extend(pairs);
    v
}

/// One attempt.
fn attempt(
    base: &Continent,
    o: &Resolved,
    cl: &ClimateDef,
    rng: &mut Rng,
) -> Result<Continent, RandoError> {
    let (rows, cols) = dims(o.dm_size);
    let biome = o.biome[1];
    let mut w = Work::new(rows, cols);
    let groups = surviving_groups(o.dm_size, rng);
    // Doors needed: every group end, plus hammer cave, spectacle rock and
    // the two exits.
    let singles: Vec<usize> = [28usize, 56, loc::CAVE1, loc::CAVE2, loc::BRIDGE, loc::RAFT]
        .into_iter()
        .filter(|&s| base.used[s])
        .collect();
    let doors: usize = groups.iter().map(|g| g.len()).sum::<usize>() + singles.len();
    let n_pockets = (doors * 2 / 5).max(groups.len().min(3) + 1).max(2);
    // Pocket terrain.
    let ground = |rng: &mut Rng| -> Terrain {
        if biome == Biome::Vanillalike || rng.chance(1, 2) {
            Terrain::Road
        } else {
            cl.pick_ground(rng, &[])
        }
    };
    // Scatter pockets.
    let mut pockets: Vec<Vec<(i32, i32)>> = Vec::new();
    let mut owner = vec![usize::MAX; rows * cols];
    let min_gap = if rows * cols > 1500 { 6 } else { 4 };
    let mut misses = 0;
    while pockets.len() < n_pockets && misses < 4000 {
        let r = 2 + rng.index(rows - 4) as i32;
        let c = 2 + rng.index(cols - 4) as i32;
        let far = pockets.iter().all(|p| {
            let (pr, pc) = p[0];
            (pr - r).abs() + (pc - c).abs() >= min_gap
        });
        if !far {
            misses += 1;
            continue;
        }
        let t = ground(rng);
        let k = pockets.len();
        let size = rng.range(6, 18) as usize;
        let mut blob = vec![(r, c)];
        let mut tries = 0;
        while blob.len() < size && tries < 200 {
            tries += 1;
            let (br, bc) = blob[rng.index(blob.len())];
            let (dr, dc) = DIRS[rng.index(4)];
            let (nr, nc) = (br + dr, bc + dc);
            if nr < 2 || nc < 2 || nr >= rows as i32 - 2 || nc >= cols as i32 - 2 {
                continue;
            }
            let i = nr as usize * cols + nc as usize;
            // Keep pockets apart: no tile next to another pocket.
            let touches = DIRS.iter().chain(&[(0, 0)]).any(|&(a, b)| {
                let (rr, cc) = (nr + a, nc + b);
                rr >= 0
                    && cc >= 0
                    && (rr as usize) < rows
                    && (cc as usize) < cols
                    && owner[rr as usize * cols + cc as usize] != usize::MAX
                    && owner[rr as usize * cols + cc as usize] != k
            });
            if touches || blob.contains(&(nr, nc)) {
                continue;
            }
            let _ = i;
            blob.push((nr, nc));
        }
        for &(br, bc) in &blob {
            owner[br as usize * cols + bc as usize] = k;
            w.set(br, bc, t);
        }
        pockets.push(blob);
    }
    if pockets.len() < 2 {
        return Err(retry("no room for pockets"));
    }
    // Door candidates per pocket: (cave tile, facing toward the pocket).
    let is_pocket = |r: i32, c: i32| -> bool {
        r >= 0
            && c >= 0
            && (r as usize) < rows
            && (c as usize) < cols
            && owner[r as usize * cols + c as usize] != usize::MAX
    };
    let mut taken = vec![false; rows * cols];
    let door_for = |pk: usize, rng: &mut Rng, taken: &mut Vec<bool>| -> Option<(i32, i32)> {
        let blob = &pockets[pk];
        for _ in 0..200 {
            let (pr, pc) = blob[rng.index(blob.len())];
            let (dr, dc) = DIRS[rng.index(4)];
            let (r, c) = (pr + dr, pc + dc);
            if r < 1 || c < 1 || r >= rows as i32 - 1 || c >= cols as i32 - 1 || is_pocket(r, c) {
                continue;
            }
            // Walled: every other neighbour (8-way) is not a pocket tile
            // and no door is near.
            let mut ok = true;
            for a in -1..=1 {
                for b in -1..=1 {
                    if a == 0 && b == 0 {
                        continue;
                    }
                    let (rr, cc) = (r + a, c + b);
                    if (rr, cc) == (pr, pc) {
                        continue;
                    }
                    if is_pocket(rr, cc) && (a == 0 || b == 0) {
                        ok = false;
                    }
                }
            }
            for a in -1..=1 {
                for b in -1..=1 {
                    let (rr, cc) = (r + a, c + b);
                    if rr >= 0
                        && cc >= 0
                        && (rr as usize) < rows
                        && (cc as usize) < cols
                        && taken[rr as usize * cols + cc as usize]
                    {
                        ok = false;
                    }
                }
            }
            if ok {
                taken[r as usize * cols + c as usize] = true;
                return Some((r, c));
            }
        }
        None
    };
    let mut c = base.clone();
    for s in 0..loc::SLOTS {
        c.on_map[s] = false;
        c.hidden[s] = None;
    }
    // Deal the groups: first join pockets into a tree.
    let mut dsu = Dsu::new(pockets.len());
    let mut order: Vec<usize> = (0..pockets.len()).collect();
    rng.shuffle(&mut order);
    let mut gi = 0;
    let place = |c: &mut Continent,
                 slot: usize,
                 pk: usize,
                 rng: &mut Rng,
                 taken: &mut Vec<bool>|
     -> Result<(), RandoError> {
        let (r, col) = door_for(pk, rng, taken).ok_or_else(|| retry("no door spot"))?;
        c.locs[slot].set_pos(r as usize, col as usize);
        c.on_map[slot] = true;
        Ok(())
    };
    for k in 1..order.len() {
        if gi >= groups.len() {
            break;
        }
        let g = groups[gi];
        gi += 1;
        let a = order[k];
        let b = order[rng.index(k)];
        let mut ends = vec![a, b];
        while ends.len() < g.len() {
            ends.push(order[rng.index(order.len())]);
        }
        for (j, &m) in g.iter().enumerate() {
            place(&mut c, usize::from(m), ends[j], rng, &mut taken)?;
        }
        for e in &ends {
            dsu.union(a, *e);
        }
    }
    for &g in &groups[gi..] {
        for &m in g {
            let pk = rng.index(pockets.len());
            place(&mut c, usize::from(m), pk, rng, &mut taken)?;
        }
    }
    let first = dsu.find(order[0]);
    // Pockets left out of the tree get their own visit from a single.
    let joined: Vec<usize> = (0..pockets.len())
        .filter(|&p| dsu.find(p) == first)
        .collect();
    for &s in &singles {
        let pk = joined[rng.index(joined.len())];
        place(&mut c, s, pk, rng, &mut taken)?;
    }
    // Paint: pockets are already set; doors; everything else mountain (or
    // water away from the doors on Islands).
    for s in c.placed() {
        let (r, col) = c.locs[s].pos().unwrap_or((0, 0));
        // Raft docks and bridges to other continents show as bridge tiles
        // (the raft ride and the crossing do not depend on the terrain).
        let t = if s == 56 {
            Terrain::Rock
        } else if s == loc::RAFT || s == loc::BRIDGE {
            Terrain::Bridge
        } else {
            Terrain::Cave
        };
        w.set(r as i32, col as i32, t);
        c.icon[s] = t;
    }
    // Islands: open water wherever the rock is at least two tiles thick
    // around the pockets and doors; everything else is mountain.
    let solid = |r: i32, col: i32, w: &Work| {
        (-2..=2).all(|a: i32| {
            (-2..=2).all(|b: i32| {
                let t = w.get(r + a, col + b);
                t.is_none() || t == Some(Terrain::Mountain) || t == Some(Terrain::Water)
            })
        })
    };
    let mut fill = Vec::new();
    for r in 0..rows as i32 {
        for col in 0..cols as i32 {
            if w.get(r, col).is_none() {
                let t = if biome == Biome::Islands && solid(r, col, &w) {
                    Terrain::Water
                } else {
                    Terrain::Mountain
                };
                fill.push((r, col, t));
            }
        }
    }
    for (r, col, t) in fill {
        w.set(r, col, t);
    }
    // Every pocket that holds a location must be in the joined set.
    for s in c.placed() {
        let (r, col) = c.locs[s].pos().unwrap_or((0, 0));
        let pk = DIRS.iter().find_map(|&(a, b)| {
            let (rr, cc) = (r as i32 + a, col as i32 + b);
            if is_pocket(rr, cc) {
                Some(owner[rr as usize * cols + cc as usize])
            } else {
                None
            }
        });
        match pk {
            Some(p) if dsu.find(p) == first => {}
            _ => return Err(retry("a cave leads to a cut-off pocket")),
        }
    }
    c.grid = w.to_grid(Terrain::Mountain);
    c.rows = rows;
    c.cols = cols;
    c.map_changed = true;
    c.table_changed = true;
    c.separator = Some(RAW_ROW_BASE + (rows / 2) as u8);
    for s in 0..loc::SLOTS {
        if c.used[s] && c.info(s).map(|i| i.class) == Some(Class::Link) {
            c.locs[s].pass = false;
        }
    }
    Ok(c)
}

/// Generate Death Mountain.
pub fn generate(
    base: &Continent,
    o: &Resolved,
    cl: &ClimateDef,
    rng: &mut Rng,
) -> Result<Continent, RandoError> {
    let mut last = String::new();
    for _ in 0..300 {
        match attempt(base, o, cl, rng) {
            Ok(c) => return Ok(c),
            Err(RandoError::Retry(m)) => last = m,
            Err(e) => return Err(e),
        }
    }
    Err(retry(format!("Death Mountain: {last}")))
}
