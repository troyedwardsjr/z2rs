//! Maze Island: a square island of road corridors between mountain walls,
//! carved as a perfect maze (every cell reachable, one way between any two).
//!
//! * The island is `n x n` (Large 23, Medium 19, Small 15) inside a margin
//!   of sea; its rim is shallow water.
//! * Cells sit on even coordinates, walls on odd ones; a randomized
//!   depth-first walk knocks out walls between cells.
//! * The fourth palace sits on a wall crossing with its ring cleared to
//!   road (a small plaza).
//! * Trap tiles go on straight corridor tiles (smaller islands drop one
//!   trap per step down), the two drops on dead ends.
//! * The bridge to East Hyrule leaves from the rim next to a corridor.

use super::core::{Work, DIRS};
use crate::flags::MazeSize;
use crate::overworld::continent::Continent;
use crate::overworld::loc;
use crate::overworld::terrain::Terrain;
use crate::overworld::Resolved;
use crate::rng::Rng;
use crate::RandoError;

fn retry(m: impl Into<String>) -> RandoError {
    RandoError::Retry(m.into())
}

/// Island side for a size.
#[must_use]
pub fn side(s: MazeSize) -> usize {
    match s {
        MazeSize::Large => 23,
        MazeSize::Medium => 19,
        MazeSize::Small => 15,
    }
}

const MARGIN: i32 = 3;

fn attempt(base: &Continent, o: &Resolved, rng: &mut Rng) -> Result<Continent, RandoError> {
    let n = side(o.maze_size) as i32;
    let rows = (n + 2 * MARGIN) as usize;
    let cols = (n + 2 * MARGIN) as usize;
    let mut w = Work::new(rows, cols);
    let at = |r: i32, c: i32| (r + MARGIN, c + MARGIN);
    for r in 0..rows as i32 {
        for c in 0..cols as i32 {
            w.set(r, c, Terrain::Water);
        }
    }
    for r in 0..n {
        for c in 0..n {
            let (mr, mc) = at(r, c);
            let t = if r == 0 || c == 0 || r == n - 1 || c == n - 1 {
                Terrain::WalkableWater
            } else if r % 2 == 1 || c % 2 == 1 {
                Terrain::Mountain
            } else {
                Terrain::Road
            };
            w.set(mr, mc, t);
        }
    }
    // Carve: cells at even coordinates 2..=n-3.
    let cells = (n - 3) / 2;
    let cell = |i: i32, j: i32| (2 + 2 * i, 2 + 2 * j);
    let mut seen = vec![false; (cells * cells) as usize];
    let start = (rng.index(cells as usize) as i32, 0);
    let mut stack = vec![start];
    seen[(start.0 * cells + start.1) as usize] = true;
    while let Some(&(i, j)) = stack.last() {
        let mut opts: Vec<(i32, i32)> = DIRS
            .iter()
            .map(|&(a, b)| (i + a, j + b))
            .filter(|&(a, b)| {
                a >= 0 && b >= 0 && a < cells && b < cells && !seen[(a * cells + b) as usize]
            })
            .collect();
        if opts.is_empty() {
            stack.pop();
            continue;
        }
        rng.shuffle(&mut opts);
        let (ni, nj) = opts[0];
        let (r0, c0) = cell(i, j);
        let (r1, c1) = cell(ni, nj);
        let (wr, wc) = at((r0 + r1) / 2, (c0 + c1) / 2);
        w.set(wr, wc, Terrain::Road);
        seen[(ni * cells + nj) as usize] = true;
        stack.push((ni, nj));
    }
    let mut c = base.clone();
    for s in 0..loc::SLOTS {
        c.on_map[s] = false;
        c.hidden[s] = None;
    }
    let mut used_tiles: Vec<(i32, i32)> = Vec::new();
    let mark = |c: &mut Continent, s: usize, r: i32, col: i32, used: &mut Vec<(i32, i32)>| {
        c.locs[s].set_pos(r as usize, col as usize);
        c.on_map[s] = true;
        used.push((r, col));
    };
    // Palace on a wall crossing (odd, odd), ring cleared to road.
    let mut palace_ok = false;
    for _ in 0..200 {
        let r = 1 + 2 * rng.index(((n - 1) / 2) as usize) as i32;
        let col = 1 + 2 * rng.index(((n - 1) / 2) as usize) as i32;
        if r < 3 || col < 3 || r > n - 4 || col > n - 4 {
            continue;
        }
        let (mr, mc) = at(r, col);
        for a in -1..=1 {
            for b in -1..=1 {
                w.set(mr + a, mc + b, Terrain::Road);
            }
        }
        w.set(mr, mc, Terrain::Palace);
        mark(&mut c, 52, mr, mc, &mut used_tiles);
        c.icon[52] = Terrain::Palace;
        palace_ok = true;
        break;
    }
    if !palace_ok {
        return Err(retry("no room for the maze palace"));
    }
    let open = |w: &Work, r: i32, col: i32| {
        DIRS.iter()
            .filter(|&&(a, b)| w.get(r + a, col + b).is_some_and(Terrain::is_open))
            .count()
    };
    let far_from = |used: &[(i32, i32)], r: i32, col: i32| {
        used.iter()
            .all(|&(a, b)| (a - r).abs() + (b - col).abs() > 1)
    };
    // Traps on straight corridor tiles.
    let mut traps: Vec<usize> = [37usize, 38, 57, 58, 59, 60, 61]
        .into_iter()
        .filter(|&s| base.used[s])
        .collect();
    rng.shuffle(&mut traps);
    let keep = match o.maze_size {
        MazeSize::Large => traps.len(),
        MazeSize::Medium => traps.len().saturating_sub(1),
        MazeSize::Small => traps.len().saturating_sub(2),
    };
    traps.truncate(keep);
    let mut corridor: Vec<(i32, i32)> = Vec::new();
    let mut dead_ends: Vec<(i32, i32)> = Vec::new();
    for r in 1..n - 1 {
        for col in 1..n - 1 {
            let (mr, mc) = at(r, col);
            if w.get(mr, mc) != Some(Terrain::Road) {
                continue;
            }
            let k = open(&w, mr, mc);
            let straight = (w.get(mr, mc - 1) == Some(Terrain::Road)
                && w.get(mr, mc + 1) == Some(Terrain::Road)
                && k == 2)
                || (w.get(mr - 1, mc) == Some(Terrain::Road)
                    && w.get(mr + 1, mc) == Some(Terrain::Road)
                    && k == 2);
            if straight {
                corridor.push((mr, mc));
            }
            if k == 1 {
                dead_ends.push((mr, mc));
            }
        }
    }
    rng.shuffle(&mut corridor);
    rng.shuffle(&mut dead_ends);
    for s in traps {
        let Some(k) = corridor
            .iter()
            .position(|&(r, col)| far_from(&used_tiles, r, col))
        else {
            return Err(retry("no corridor for a trap"));
        };
        let (r, col) = corridor.remove(k);
        mark(&mut c, s, r, col, &mut used_tiles);
        c.locs[s].pass = true;
        c.icon[s] = Terrain::Road;
    }
    for s in [39usize, 55] {
        if !base.used[s] {
            continue;
        }
        let Some(k) = dead_ends
            .iter()
            .position(|&(r, col)| far_from(&used_tiles, r, col))
        else {
            return Err(retry("no dead end for a drop"));
        };
        let (r, col) = dead_ends.remove(k);
        mark(&mut c, s, r, col, &mut used_tiles);
        c.icon[s] = Terrain::Road;
    }
    // Bridge and raft: on the rim next to a corridor cell, plus one bridge
    // tile out (the raft on the coast it sails from).
    for slot in [loc::BRIDGE, loc::RAFT] {
        if !base.used[slot] {
            continue;
        }
        let mut ok = false;
        for _ in 0..200 {
            let side = match (slot, base.dock) {
                (s, Some(0)) if s == loc::RAFT => 1,
                (s, Some(1)) if s == loc::RAFT => 0,
                _ => rng.index(4),
            };
            let (r, col, out) = match side {
                0 => (2 * (1 + rng.index(cells as usize) as i32), 0, (0, -1)),
                1 => (2 * (1 + rng.index(cells as usize) as i32), n - 1, (0, 1)),
                2 => (0, 2 * (1 + rng.index(cells as usize) as i32), (-1, 0)),
                _ => (n - 1, 2 * (1 + rng.index(cells as usize) as i32), (1, 0)),
            };
            let (mr, mc) = at(r, col);
            // The wall between the rim and the first cell.
            let (wr, wc) = (mr - out.0, mc - out.1);
            if !far_from(&used_tiles, mr, mc) || !far_from(&used_tiles, wr, wc) {
                continue;
            }
            w.set(wr, wc, Terrain::Road);
            w.set(mr, mc, Terrain::Bridge);
            w.set(mr + out.0, mc + out.1, Terrain::Bridge);
            mark(&mut c, slot, mr, mc, &mut used_tiles);
            c.icon[slot] = Terrain::Bridge;
            c.locs[slot].pass = slot == loc::BRIDGE;
            ok = true;
            break;
        }
        if !ok {
            return Err(retry("no spot for a maze connector"));
        }
    }
    // Cave connectors at dead ends.
    for slot in [loc::CAVE1, loc::CAVE2] {
        if !base.used[slot] {
            continue;
        }
        let Some(k) = dead_ends
            .iter()
            .position(|&(r, col)| far_from(&used_tiles, r, col))
        else {
            return Err(retry("no dead end for a cave"));
        };
        let (r, col) = dead_ends.remove(k);
        w.set(r, col, Terrain::Cave);
        mark(&mut c, slot, r, col, &mut used_tiles);
        c.icon[slot] = Terrain::Cave;
    }
    c.grid = w.to_grid(Terrain::Water);
    c.rows = rows;
    c.cols = cols;
    c.map_changed = true;
    c.table_changed = true;
    c.separator = None;
    Ok(c)
}

/// Generate Maze Island.
pub fn generate(base: &Continent, o: &Resolved, rng: &mut Rng) -> Result<Continent, RandoError> {
    let mut last = String::new();
    for _ in 0..200 {
        match attempt(base, o, rng) {
            Ok(c) => return Ok(c),
            Err(RandoError::Retry(m)) => last = m,
            Err(e) => return Err(e),
        }
    }
    Err(retry(format!("Maze Island: {last}")))
}
