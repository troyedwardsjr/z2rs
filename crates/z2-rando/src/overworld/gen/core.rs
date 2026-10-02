//! Shared pieces of the terrain generators: the work grid, climates, the
//! seed-and-grow fill, location pads, crossings and flood fills.

use crate::flags::Climate;
use crate::overworld::map::Grid;
use crate::overworld::terrain::Terrain;
use crate::rng::Rng;

/// The four directions as (row, col) steps.
pub const DIRS: [(i32, i32); 4] = [(0, 1), (0, -1), (1, 0), (-1, 0)];

/// Work grid: `None` = not decided yet.
#[derive(Debug, Clone)]
pub struct Work {
    /// Playable rows.
    pub rows: usize,
    /// Playable columns.
    pub cols: usize,
    cells: Vec<Option<Terrain>>,
    /// Tiles laid down as scaffolding (they grow with a neutral rate).
    pre: Vec<bool>,
    /// Tiles nothing may change any more (locations and their pads).
    pub locked: Vec<bool>,
}

impl Work {
    /// An empty grid.
    #[must_use]
    pub fn new(rows: usize, cols: usize) -> Work {
        Work {
            rows,
            cols,
            cells: vec![None; rows * cols],
            pre: vec![false; rows * cols],
            locked: vec![false; rows * cols],
        }
    }

    /// Inside the grid.
    #[must_use]
    pub fn inside(&self, r: i32, c: i32) -> bool {
        r >= 0 && c >= 0 && (r as usize) < self.rows && (c as usize) < self.cols
    }

    /// At least `m` tiles from every edge.
    #[must_use]
    pub fn interior(&self, r: i32, c: i32, m: i32) -> bool {
        r >= m && c >= m && r < self.rows as i32 - m && c < self.cols as i32 - m
    }

    fn idx(&self, r: i32, c: i32) -> usize {
        r as usize * self.cols + c as usize
    }

    /// Tile (out of range reads as water).
    #[must_use]
    pub fn get(&self, r: i32, c: i32) -> Option<Terrain> {
        if self.inside(r, c) {
            self.cells[self.idx(r, c)]
        } else {
            Some(Terrain::Water)
        }
    }

    /// Decided tile or `fallback`.
    #[must_use]
    pub fn get_or(&self, r: i32, c: i32, fallback: Terrain) -> Terrain {
        self.get(r, c).unwrap_or(fallback)
    }

    /// Set a tile unless it is locked or out of range.
    pub fn set(&mut self, r: i32, c: i32, t: Terrain) {
        if self.inside(r, c) {
            let i = self.idx(r, c);
            if !self.locked[i] {
                self.cells[i] = Some(t);
            }
        }
    }

    /// Set as scaffolding.
    pub fn set_pre(&mut self, r: i32, c: i32, t: Terrain) {
        if self.inside(r, c) {
            let i = self.idx(r, c);
            if !self.locked[i] {
                self.cells[i] = Some(t);
                self.pre[i] = true;
            }
        }
    }

    /// Set a tile even if it is locked (blockers on mouths and crossings,
    /// details inside a locked pad).
    pub fn set_force(&mut self, r: i32, c: i32, t: Terrain) {
        if self.inside(r, c) {
            let i = self.idx(r, c);
            self.cells[i] = Some(t);
        }
    }

    /// Clear back to undecided (unless locked).
    pub fn clear(&mut self, r: i32, c: i32) {
        if self.inside(r, c) {
            let i = self.idx(r, c);
            if !self.locked[i] {
                self.cells[i] = None;
                self.pre[i] = false;
            }
        }
    }

    /// Lock a tile.
    pub fn lock(&mut self, r: i32, c: i32) {
        if self.inside(r, c) {
            let i = self.idx(r, c);
            self.locked[i] = true;
        }
    }

    /// Whether a tile is locked.
    #[must_use]
    pub fn is_locked(&self, r: i32, c: i32) -> bool {
        self.inside(r, c) && self.locked[self.idx(r, c)]
    }

    /// Scaffolding?
    #[must_use]
    pub fn is_pre(&self, r: i32, c: i32) -> bool {
        self.inside(r, c) && self.pre[self.idx(r, c)]
    }

    /// Every tile of the (2k+1)-square around (r, c) is inside, undecided
    /// and unlocked.
    #[must_use]
    pub fn free_square(&self, r: i32, c: i32, k: i32) -> bool {
        for dr in -k..=k {
            for dc in -k..=k {
                let (rr, cc) = (r + dr, c + dc);
                if !self.inside(rr, cc) || self.get(rr, cc).is_some() || self.is_locked(rr, cc) {
                    return false;
                }
            }
        }
        true
    }

    /// Undecided tiles.
    #[must_use]
    pub fn count_none(&self) -> usize {
        self.cells.iter().filter(|c| c.is_none()).count()
    }

    /// Into a full map grid (`filler` outside the playable area and for any
    /// tile still undecided).
    #[must_use]
    pub fn to_grid(&self, filler: Terrain) -> Grid {
        let mut g = Grid::filled(filler);
        for r in 0..self.rows {
            for c in 0..self.cols {
                g.set(r, c, self.cells[r * self.cols + c].unwrap_or(filler));
            }
        }
        g
    }

    /// Text dump for debugging.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut s = String::new();
        for r in 0..self.rows {
            for c in 0..self.cols {
                s.push(self.cells[r * self.cols + c].map_or(' ', Terrain::glyph));
            }
            s.push('\n');
        }
        s
    }
}

/// A terrain mix.
#[derive(Debug, Clone)]
pub struct ClimateDef {
    /// Seed weights.
    pub weights: Vec<(Terrain, u32)>,
    /// Growth size per terrain (bigger claims more area).
    pub size: [f64; 16],
    /// Random seed tiles to drop.
    pub seeds: usize,
}

impl ClimateDef {
    /// Random seed terrain (only from `allowed`, if not empty).
    pub fn pick(&self, rng: &mut Rng, allowed: &[Terrain]) -> Terrain {
        let items: Vec<(Terrain, u32)> = self
            .weights
            .iter()
            .copied()
            .filter(|(t, _)| allowed.is_empty() || allowed.contains(t))
            .collect();
        rng.weighted_pick(&items).copied().unwrap_or(Terrain::Grass)
    }

    /// Random walkable ground (pads, mouths).
    pub fn pick_ground(&self, rng: &mut Rng, except: &[Terrain]) -> Terrain {
        let items: Vec<(Terrain, u32)> = self
            .weights
            .iter()
            .copied()
            .filter(|(t, _)| {
                matches!(
                    t,
                    Terrain::Desert
                        | Terrain::Grass
                        | Terrain::Forest
                        | Terrain::Swamp
                        | Terrain::Grave
                        | Terrain::Road
                ) && !except.contains(t)
            })
            .collect();
        rng.weighted_pick(&items).copied().unwrap_or(Terrain::Grass)
    }
}

/// Build a climate. `counts` are the vanilla map's tile counts per terrain
/// (used by the vanilla-weighted mix). `water` is the water the generator
/// lays down (walkable with good boots).
#[must_use]
pub fn climate(c: Climate, counts: &[usize; 16], water: Terrain, lava: bool) -> ClimateDef {
    use Terrain::*;
    let mut size = [1.0f64; 16];
    let ground = [Desert, Grass, Forest, Swamp, Grave, Road, Mountain, water];
    let mut weights: Vec<(Terrain, u32)> = ground.iter().map(|&t| (t, 10)).collect();
    if lava {
        weights.push((Lava, 4));
    }
    let mut seeds = 30;
    let set = |w: &mut Vec<(Terrain, u32)>, t: Terrain, v: u32| {
        if let Some(e) = w.iter_mut().find(|(x, _)| *x == t) {
            e.1 = v;
        }
    };
    match c {
        Climate::Classic | Climate::Random => {}
        Climate::VanillaWeighted => {
            for (t, w) in &mut weights {
                let n = if *t == water {
                    counts[Water as usize] + counts[WalkableWater as usize]
                } else {
                    counts[*t as usize]
                };
                *w = (n as u32 / 20).max(1);
            }
            // Mountains and water mostly come from the scaffolding.
            set(&mut weights, Mountain, 12);
        }
        Climate::Chaos => {
            seeds = 200;
            size[Road as usize] = 0.2;
            set(&mut weights, water, 3);
        }
        Climate::Wetlands => {
            set(&mut weights, Desert, 0);
            size[Swamp as usize] = 2.5;
            set(&mut weights, water, 16);
            set(&mut weights, Road, 14);
            set(&mut weights, Swamp, 14);
        }
        Climate::GreatLakes => {
            set(&mut weights, Desert, 0);
            size[water as usize] = 2.5;
            size[Grass as usize] = 1.5;
            size[Forest as usize] = 1.5;
        }
        Climate::Scrubland => {
            size[Forest as usize] = 0.6;
            size[Swamp as usize] = 0.5;
            size[Grave as usize] = 0.5;
            size[water as usize] = 0.4;
            set(&mut weights, Desert, 16);
            set(&mut weights, Grass, 14);
        }
    }
    ClimateDef {
        weights,
        size,
        seeds,
    }
}

/// Drop `n` random seed tiles of climate terrain onto undecided tiles.
pub fn place_seeds(w: &mut Work, cl: &ClimateDef, rng: &mut Rng, n: usize, allowed: &[Terrain]) {
    let mut misses = 0;
    let mut placed = 0;
    while placed < n && misses < 300 {
        let r = rng.index(w.rows) as i32;
        let c = rng.index(w.cols) as i32;
        if w.get(r, c).is_some() || w.is_locked(r, c) {
            misses += 1;
            continue;
        }
        w.set(r, c, cl.pick(rng, allowed));
        placed += 1;
        misses = 0;
    }
}

/// Fill every undecided tile with the terrain of the nearest decided tile,
/// distances scaled by `1 / size` of that terrain (scaffolding uses 1).
/// Ties go to a random candidate.
pub fn grow(w: &mut Work, cl: &ClimateDef, rng: &mut Rng) {
    // Seeds: decided tiles next to an undecided one (the nearest decided
    // tile of a region is always on its border).
    let mut seeds: Vec<(i32, i32, Terrain, f64)> = Vec::new();
    for r in 0..w.rows as i32 {
        for c in 0..w.cols as i32 {
            let Some(t) = w.get(r, c) else { continue };
            if DIRS.iter().any(|&(dr, dc)| {
                let (rr, cc) = (r + dr, c + dc);
                w.inside(rr, cc) && w.get(rr, cc).is_none()
            }) {
                let k = if w.is_pre(r, c) {
                    1.0
                } else {
                    1.0 / cl.size[t as usize].max(0.05)
                };
                seeds.push((r, c, t, k * k));
            }
        }
    }
    if seeds.is_empty() {
        for r in 0..w.rows as i32 {
            for c in 0..w.cols as i32 {
                if w.get(r, c).is_none() {
                    w.set(r, c, Terrain::Grass);
                }
            }
        }
        return;
    }
    let mut out: Vec<(i32, i32, Terrain)> = Vec::new();
    let mut ties: Vec<Terrain> = Vec::new();
    for r in 0..w.rows as i32 {
        for c in 0..w.cols as i32 {
            if w.get(r, c).is_some() {
                continue;
            }
            let mut best = f64::MAX;
            ties.clear();
            for &(sr, sc, t, k) in &seeds {
                let d = f64::from((sr - r) * (sr - r) + (sc - c) * (sc - c)) * k;
                if d < best - 1e-9 {
                    best = d;
                    ties.clear();
                    ties.push(t);
                } else if (d - best).abs() <= 1e-9 {
                    ties.push(t);
                }
            }
            let t = ties[rng.index(ties.len())];
            out.push((r, c, t));
        }
    }
    for (r, c, t) in out {
        w.set(r, c, t);
    }
}

/// Region labels over tiles where `pass` holds (4-neighbour). Returns
/// (labels, region count); impassable tiles get `usize::MAX`.
pub fn regions(w: &Work, pass: impl Fn(Terrain) -> bool) -> (Vec<usize>, usize) {
    let n = w.rows * w.cols;
    let mut lab = vec![usize::MAX; n];
    let mut k = 0;
    let mut stack = Vec::new();
    for start in 0..n {
        if lab[start] != usize::MAX {
            continue;
        }
        let (r0, c0) = ((start / w.cols) as i32, (start % w.cols) as i32);
        if !w.get(r0, c0).is_some_and(&pass) {
            continue;
        }
        lab[start] = k;
        stack.push((r0, c0));
        while let Some((r, c)) = stack.pop() {
            for (dr, dc) in DIRS {
                let (rr, cc) = (r + dr, c + dc);
                if !w.inside(rr, cc) {
                    continue;
                }
                let i = rr as usize * w.cols + cc as usize;
                if lab[i] == usize::MAX && w.get(rr, cc).is_some_and(&pass) {
                    lab[i] = k;
                    stack.push((rr, cc));
                }
            }
        }
        k += 1;
    }
    (lab, k)
}

/// Simple union-find.
#[derive(Debug, Clone)]
pub struct Dsu(Vec<usize>);

impl Dsu {
    /// `n` singletons.
    #[must_use]
    pub fn new(n: usize) -> Dsu {
        Dsu((0..n).collect())
    }

    /// Representative.
    pub fn find(&mut self, a: usize) -> usize {
        let mut a = a;
        while self.0[a] != a {
            self.0[a] = self.0[self.0[a]];
            a = self.0[a];
        }
        a
    }

    /// Join; returns whether they were apart.
    pub fn union(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            false
        } else {
            self.0[a] = b;
            true
        }
    }
}

/// Bresenham line from a to b (inclusive).
#[must_use]
pub fn line(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut r, mut c) = a;
    let dr = (b.0 - a.0).abs();
    let dc = -(b.1 - a.1).abs();
    let sr = if a.0 < b.0 { 1 } else { -1 };
    let sc = if a.1 < b.1 { 1 } else { -1 };
    let mut err = dr + dc;
    let mut v = Vec::new();
    loop {
        v.push((r, c));
        if (r, c) == b {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dc {
            err += dc;
            r += sr;
        }
        if e2 <= dr {
            err += dr;
            c += sc;
        }
    }
    v
}

/// A 4-connected version of [`line`] (adds a corner step at diagonals) so
/// walls and rivers drawn with it cannot be crossed diagonally.
#[must_use]
pub fn thick_line(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let l = line(a, b);
    let mut v = Vec::with_capacity(l.len() * 2);
    for w in l.windows(2) {
        v.push(w[0]);
        if w[0].0 != w[1].0 && w[0].1 != w[1].1 {
            v.push((w[1].0, w[0].1));
        }
    }
    if let Some(&last) = l.last() {
        v.push(last);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grow_fills_everything() {
        let mut w = Work::new(20, 30);
        let cl = climate(Climate::Classic, &[0; 16], Terrain::Water, false);
        let mut rng = Rng::new(4);
        place_seeds(&mut w, &cl, &mut rng, 12, &[]);
        grow(&mut w, &cl, &mut rng);
        assert_eq!(w.count_none(), 0);
    }

    #[test]
    fn thick_lines_are_four_connected() {
        let l = thick_line((0, 0), (7, 3));
        for p in l.windows(2) {
            assert_eq!((p[0].0 - p[1].0).abs() + (p[0].1 - p[1].1).abs(), 1);
        }
    }

    #[test]
    fn regions_split_by_walls() {
        let mut w = Work::new(5, 5);
        for r in 0..5 {
            for c in 0..5 {
                w.set(
                    r,
                    c,
                    if c == 2 {
                        Terrain::Mountain
                    } else {
                        Terrain::Grass
                    },
                );
            }
        }
        let (_, n) = regions(&w, Terrain::is_open);
        assert_eq!(n, 2);
    }
}
