//! West and East Hyrule: generated land masses.
//!
//! One attempt:
//!
//! 1. Border and biome scaffolding (ridges, rivers, water or mountain grid
//!    lines, a canyon, a central mountain with a crater or volcano), plus an
//!    ocean strip on every edge a raft or bridge leaves from.
//! 2. East only: the Great Palace in a lava basin walled by mountains, with
//!    a winding lava path out (the lava trap tiles sit on it), and the
//!    flute-revealed spot: a 7x7 clearing with three boulders around the
//!    tile Link plays from.
//! 3. Locations on 3x3 pads: caves walled by mountains with an open mouth,
//!    everything else ringed by ground of a different terrain so the tile
//!    stands out. Linked caves (and Saria's two doors) are placed facing
//!    away from each other with a mountain wall (water for Saria) between
//!    them.
//! 4. Random climate seeds, then every open tile takes the terrain of the
//!    nearest seed (weighted by each terrain's growth rate).
//! 5. Separate land masses are joined with bridges over water and roads
//!    through mountains (the encounter tiles go on those crossings), until
//!    every location can be walked to (ignoring items).
//! 6. Raft and bridge connectors on their coasts; boulders and the river
//!    devil; minor tiles blended into matching terrain.
//!
//! Anything that does not work out fails the attempt; the caller retries.

use super::core::{self, ClimateDef, Dsu, Work, DIRS};
use crate::flags::{Biome, LessImportantLocations, RiverDevilBlocker};
use crate::overworld::continent::{Continent, Reveal};
use crate::overworld::loc::{self, Class};
use crate::overworld::map::{Cont, RAW_ROW_BASE};
use crate::overworld::terrain::Terrain;
use crate::overworld::Resolved;
use crate::rng::Rng;
use crate::RandoError;

fn retry(m: impl Into<String>) -> RandoError {
    RandoError::Retry(m.into())
}

/// Facing of a cave mouth (row, col step from the cave to its mouth).
type Dir = (i32, i32);

/// A candidate crossing: (sort key, tiles, region at the start, region at
/// the end, terrain crossed).
type Crossing = (u64, Vec<(i32, i32)>, usize, usize, Terrain);

/// Building state for one attempt.
pub struct Builder<'a> {
    /// Work grid.
    pub w: Work,
    /// The continent being built (positions are written here).
    pub c: Continent,
    rng: &'a mut Rng,
    cl: ClimateDef,
    o: &'a Resolved,
    biome: Biome,
    horizontal: bool,
    water: Terrain,
    /// Encounter slots still waiting for a crossing.
    enc_pool: Vec<usize>,
    /// Cave mouth tile per placed cave slot.
    mouths: Vec<(usize, (i32, i32))>,
    /// Linked placements (slot pairs).
    links: Vec<(usize, usize)>,
    /// Slots placed so far.
    placed: Vec<bool>,
}

impl<'a> Builder<'a> {
    fn new(
        base: &Continent,
        o: &'a Resolved,
        rng: &'a mut Rng,
        cl: ClimateDef,
        rows: usize,
        cols: usize,
    ) -> Builder<'a> {
        let mut c = base.clone();
        for s in 0..loc::SLOTS {
            c.on_map[s] = false;
            c.hidden[s] = None;
        }
        c.rows = rows;
        c.cols = cols;
        let ci = match base.cont {
            Cont::West => 0,
            Cont::DeathMountain => 1,
            _ => 2,
        };
        Builder {
            w: Work::new(rows, cols),
            c,
            rng,
            cl,
            o,
            biome: o.biome[base.cont.index()],
            // Rafts sail east-west, so a canyon on a raft continent runs
            // east-west too (its river mouths are where the raft can be).
            horizontal: o.horizontal[ci]
                || (matches!(o.biome[base.cont.index()], Biome::Canyon | Biome::DryCanyon)
                    && base.cont != Cont::DeathMountain),
            water: if o.good_boots {
                Terrain::WalkableWater
            } else {
                Terrain::Water
            },
            enc_pool: Vec::new(),
            mouths: Vec::new(),
            links: Vec::new(),
            placed: vec![false; loc::SLOTS],
        }
    }

    fn mark(&mut self, slot: usize, r: i32, col: i32) {
        self.c.locs[slot].set_pos(r as usize, col as usize);
        self.c.on_map[slot] = true;
        self.placed[slot] = true;
    }

    fn rand_pos(&mut self, margin: i32) -> (i32, i32) {
        let r = margin
            + self
                .rng
                .index((self.w.rows as i32 - 2 * margin).max(1) as usize) as i32;
        let c = margin
            + self
                .rng
                .index((self.w.cols as i32 - 2 * margin).max(1) as usize) as i32;
        (r, c)
    }

    /// A random spot whose (2k+1)-square is free.
    fn free_spot(&mut self, k: i32, margin: i32, tries: usize) -> Option<(i32, i32)> {
        for _ in 0..tries {
            let (r, c) = self.rand_pos(margin.max(k + 1));
            if self.w.free_square(r, c, k) {
                return Some((r, c));
            }
        }
        None
    }

    /// Location tile plus a ring of one ground terrain (different from the
    /// location's own), all locked.
    fn pad_open(&mut self, slot: usize, r: i32, c: i32, center: Terrain) {
        let ring = self.cl.pick_ground(self.rng, &[center]);
        for dr in -1..=1 {
            for dc in -1..=1 {
                let t = if dr == 0 && dc == 0 { center } else { ring };
                self.w.set(r + dr, c + dc, t);
                self.w.lock(r + dr, c + dc);
            }
        }
        self.mark(slot, r, c);
    }

    /// Cave tile walled by mountain with a three-tile mouth on `face`.
    fn pad_cave(&mut self, slot: usize, r: i32, c: i32, face: Dir) {
        let mouth = if self.biome == Biome::Vanillalike {
            Terrain::Road
        } else {
            self.cl.pick_ground(self.rng, &[])
        };
        for dr in -1..=1 {
            for dc in -1..=1 {
                let t = if dr == 0 && dc == 0 {
                    Terrain::Cave
                } else if (face.0 != 0 && dr == face.0) || (face.1 != 0 && dc == face.1) {
                    mouth
                } else {
                    Terrain::Mountain
                };
                self.w.set(r + dr, c + dc, t);
                self.w.lock(r + dr, c + dc);
            }
        }
        // One more row of the mouth terrain in front (not locked), so the
        // mouth is not sealed by whatever grows there.
        for k in -1..=1 {
            let (rr, cc) = if face.0 != 0 {
                (r + 2 * face.0, c + k)
            } else {
                (r + k, c + 2 * face.1)
            };
            if self.w.inside(rr, cc) && self.w.get(rr, cc).is_none() {
                self.w.set_pre(rr, cc, mouth);
            }
        }
        self.mouths.push((slot, (r + face.0, c + face.1)));
        self.mark(slot, r, c);
    }

    fn random_dir(&mut self) -> Dir {
        DIRS[self.rng.index(4)]
    }

    /// Place a single cave with a random facing.
    fn place_cave(&mut self, slot: usize) -> Result<(), RandoError> {
        let (r, c) = self
            .free_spot(1, 2, 5000)
            .ok_or_else(|| retry("no room for a cave"))?;
        let face = self.random_dir();
        self.pad_cave(slot, r, c, face);
        Ok(())
    }

    /// Place an open (town, palace, tile) location.
    fn place_open(&mut self, slot: usize, icon: Terrain) -> Result<(), RandoError> {
        let (r, c) = self
            .free_spot(1, 2, 5000)
            .ok_or_else(|| retry("no room for a location"))?;
        self.pad_open(slot, r, c, icon);
        Ok(())
    }

    /// Two linked ends facing away from each other with a wall of `wall`
    /// between them. `open` lists ends that are open pads (Saria's doors,
    /// the fairy hole) instead of caves.
    fn place_pair(
        &mut self,
        a: usize,
        b: usize,
        wall: Terrain,
        open_icon: [Option<Terrain>; 2],
    ) -> Result<(), RandoError> {
        let (lo, hi) = if self.biome == Biome::Islands || self.biome == Biome::Mountainous {
            (8, 16)
        } else {
            (5, 14)
        };
        for _ in 0..400 {
            let Some((ra, ca)) = self.free_spot(1, 4, 50) else {
                continue;
            };
            let d = self.random_dir();
            let k = self.rng.range(lo, hi) as i32;
            let side = self.rng.range(-3, 3) as i32;
            let (rb, cb) = if d.0 != 0 {
                (ra + d.0 * k, ca + side)
            } else {
                (ra + side, ca + d.1 * k)
            };
            if !self.w.interior(rb, cb, 3) || !self.w.free_square(rb, cb, 1) {
                continue;
            }
            // Wall between them (skips decided tiles).
            let path = core::thick_line((ra + d.0 * 2, ca + d.1 * 2), (rb - d.0 * 2, cb - d.1 * 2));
            let blocked = path
                .iter()
                .any(|&(r, c)| self.w.is_locked(r, c) || !self.w.inside(r, c));
            if blocked {
                continue;
            }
            let back = (-d.0, -d.1);
            match open_icon[0] {
                Some(t) => self.pad_open(a, ra, ca, t),
                None => self.pad_cave(a, ra, ca, back),
            }
            match open_icon[1] {
                Some(t) => self.pad_open(b, rb, cb, t),
                None => self.pad_cave(b, rb, cb, d),
            }
            for (r, c) in path {
                if self.w.get(r, c).is_none() {
                    self.w.set_pre(r, c, wall);
                }
            }
            self.links.push((a, b));
            return Ok(());
        }
        Err(retry("no room for a linked pair"))
    }

    // -- scaffolding --------------------------------------------------

    fn border(&mut self, t: Terrain) {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        for r in 0..rows {
            for c in 0..cols {
                if r == 0 || c == 0 || r == rows - 1 || c == cols - 1 {
                    self.w.set_pre(r, c, t);
                }
            }
        }
    }

    /// Ocean strip along an edge (`dir` = which edge: (0,1) east, (0,-1)
    /// west, (1,0) south, (-1,0) north).
    fn ocean(&mut self, edge: Dir, depth: i32) {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        for r in 0..rows {
            for c in 0..cols {
                let d = match edge {
                    (0, 1) => cols - 1 - c,
                    (0, -1) => c,
                    (1, 0) => rows - 1 - r,
                    _ => r,
                };
                if d < depth && !self.w.is_locked(r, c) {
                    self.w.set_pre(r, c, self.water);
                }
            }
        }
    }

    fn paint_path(&mut self, pts: &[(i32, i32)], t: Terrain, width: i32) {
        for &(r, c) in pts {
            for k in 0..width {
                for (rr, cc) in [(r + k, c), (r, c + k)] {
                    if self.w.inside(rr, cc) && !self.w.is_locked(rr, cc) {
                        self.w.set_pre(rr, cc, t);
                    }
                }
            }
        }
    }

    /// A wandering line across the map along one axis.
    fn wander(&mut self, horizontal: bool, at: i32, wobble: i32) -> Vec<(i32, i32)> {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        let mut v = Vec::new();
        let mut x = at;
        let (len, span) = if horizontal {
            (cols, rows)
        } else {
            (rows, cols)
        };
        let mut prev: Option<(i32, i32)> = None;
        for i in 0..len {
            if self.rng.chance(1, 3) {
                x += self.rng.range(-wobble as i64, wobble as i64) as i32;
            }
            x = x.clamp(3, span - 4);
            let p = if horizontal { (x, i) } else { (i, x) };
            if let Some(q) = prev {
                v.extend(core::thick_line(q, p));
            } else {
                v.push(p);
            }
            prev = Some(p);
        }
        v
    }

    fn scaffold(&mut self) {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        match self.biome {
            Biome::Mountainous => self.border(Terrain::Mountain),
            _ => self.border(self.water),
        }
        match self.biome {
            Biome::Vanillalike => {
                let ridges = if self.c.cont == Cont::East { 2 } else { 1 };
                for k in 0..ridges {
                    if self.c.cont == Cont::East {
                        let at = cols * (k + 1) / 3 + self.rng.range(-3, 3) as i32;
                        let line = self.wander(false, at, 1);
                        self.paint_path(&line, Terrain::Mountain, 2);
                    } else {
                        let a = (
                            rows * 3 / 10 + self.rng.index((rows / 4) as usize) as i32,
                            0,
                        );
                        let b = (
                            rows * 3 / 10 + self.rng.index((rows / 4) as usize) as i32,
                            cols - 2 - self.rng.range(2, 7) as i32,
                        );
                        let line = core::thick_line(a, b);
                        self.paint_path(&line, Terrain::Mountain, 2);
                    }
                }
                // A river from one edge inward, then to another edge.
                let start = match self.rng.index(4) {
                    0 => (0, cols / 3 + self.rng.index((cols / 3) as usize) as i32),
                    1 => (
                        rows - 1,
                        cols / 3 + self.rng.index((cols / 3) as usize) as i32,
                    ),
                    2 => (rows / 3 + self.rng.index((rows / 3) as usize) as i32, 0),
                    _ => (
                        rows / 3 + self.rng.index((rows / 3) as usize) as i32,
                        cols - 1,
                    ),
                };
                let mid = (
                    rows / 4 + self.rng.index((rows / 2) as usize) as i32,
                    cols / 4 + self.rng.index((cols / 2) as usize) as i32,
                );
                let end = match self.rng.index(4) {
                    0 => (0, self.rng.index(cols as usize) as i32),
                    1 => (rows - 1, self.rng.index(cols as usize) as i32),
                    2 => (self.rng.index(rows as usize) as i32, 0),
                    _ => (self.rng.index(rows as usize) as i32, cols - 1),
                };
                let mut river = core::thick_line(start, mid);
                river.extend(core::thick_line(mid, end));
                let w = self.water;
                self.paint_path(&river, w, 1);
            }
            Biome::Islands | Biome::Mountainous => {
                let t = if self.biome == Biome::Islands {
                    self.water
                } else {
                    Terrain::Mountain
                };
                let n_rows = self.rng.range(2, 3);
                let n_cols = self.rng.range(2, 3);
                for k in 0..n_rows {
                    let r = (rows * (k as i32 + 1) / (n_rows as i32 + 1))
                        + self.rng.range(-3, 3) as i32;
                    let line = self.wander(true, r, 1);
                    self.paint_path(&line, t, 1);
                }
                for k in 0..n_cols {
                    let c = (cols * (k as i32 + 1) / (n_cols as i32 + 1))
                        + self.rng.range(-3, 3) as i32;
                    let line = self.wander(false, c, 1);
                    self.paint_path(&line, t, 1);
                }
            }
            Biome::Canyon | Biome::DryCanyon => {
                let river_t = if self.biome == Biome::DryCanyon {
                    Terrain::Desert
                } else {
                    self.water
                };
                let h = self.horizontal;
                let span = if h { rows } else { cols };
                let mid = span / 2 + self.rng.range(-3, 3) as i32;
                let river = self.wander(h, mid, 1);
                // Mountains far from the river on both sides.
                let reach = (span / 2 - 4).clamp(8, 15);
                let len = if h { cols } else { rows };
                for i in 0..len {
                    let centre = river
                        .iter()
                        .filter(|p| if h { p.1 == i } else { p.0 == i })
                        .map(|p| if h { p.0 } else { p.1 })
                        .sum::<i32>()
                        / river
                            .iter()
                            .filter(|p| if h { p.1 == i } else { p.0 == i })
                            .count()
                            .max(1) as i32;
                    for x in 0..span {
                        if (x - centre).abs() > reach {
                            let (r, c) = if h { (x, i) } else { (i, x) };
                            self.w.set_pre(r, c, Terrain::Mountain);
                        }
                    }
                }
                self.paint_path(&river, river_t, 2);
            }
            Biome::Caldera | Biome::Volcano => {
                self.centre_mountain();
            }
            _ => {}
        }
    }

    /// Half-axes (rows, cols) of the central mountain: about a sixth of the
    /// map across the short way, a quarter the long way, and never so
    /// small that a crater with a palace cannot fit.
    fn mountain_axes(&self) -> (i32, i32) {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        let (ar, ac) = if self.horizontal {
            (rows / 6, cols / 4)
        } else {
            (rows / 4, cols / 6)
        };
        (ar.max(9), ac.max(9))
    }

    /// A big mountain mass in the middle (caldera and volcano).
    fn centre_mountain(&mut self) {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        let (cr, cc) = (rows / 2, cols / 2);
        let (ar, ac) = self.mountain_axes();
        for r in 0..rows {
            for c in 0..cols {
                let dy = f64::from(r - cr) / f64::from(ar.max(1));
                let dx = f64::from(c - cc) / f64::from(ac.max(1));
                if dy * dy + dx * dx <= 1.0 {
                    self.w.set_pre(r, c, Terrain::Mountain);
                }
            }
        }
    }

    /// Carve the caldera's crater (an open ring around a lake) and return
    /// its centre and radius.
    fn crater(&mut self) -> (i32, i32, i32) {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        let (cr, cc) = (rows / 2, cols / 2);
        let (ar, ac) = self.mountain_axes();
        let rad = (ar.min(ac) - 3).clamp(5, 7);
        for r in cr - rad..=cr + rad {
            for c in cc - rad..=cc + rad {
                let d2 = (r - cr) * (r - cr) + (c - cc) * (c - cc);
                if d2 <= rad * rad {
                    self.w.clear(r, c);
                }
                if d2 <= (rad / 3) * (rad / 3) {
                    self.w.set_pre(r, c, self.water);
                }
            }
        }
        (cr, cc, rad)
    }

    // -- East: the Great Palace ------------------------------------------

    fn great_palace(&mut self) -> Result<(), RandoError> {
        let caldera_like = self.biome == Biome::Volcano;
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        let mut centre = None;
        for _ in 0..2000 {
            let (r, c) = if caldera_like {
                (
                    rows / 2 + self.rng.range(-6, 6) as i32,
                    cols / 2 + self.rng.range(-6, 6) as i32,
                )
            } else {
                self.rand_pos(6)
            };
            if !self.w.interior(r, c, 6) {
                continue;
            }
            let ok = (-4..=4).all(|dr: i32| {
                (-4..=4).all(|dc: i32| {
                    let (rr, cc) = (r + dr, c + dc);
                    !self.w.is_locked(rr, cc)
                        && match self.w.get(rr, cc) {
                            None => !caldera_like,
                            Some(Terrain::Mountain) => true,
                            Some(_) => false,
                        }
                })
            });
            if ok {
                centre = Some((r, c));
                break;
            }
        }
        let (r, c) = centre.ok_or_else(|| retry("no room for the Great Palace"))?;
        for dr in -4..=4i32 {
            for dc in -4..=4i32 {
                let ring = dr.abs() == 4 || dc.abs() == 4;
                let corner = dr.abs() == 3 && dc.abs() == 3;
                let t = if ring || corner {
                    Terrain::Mountain
                } else {
                    Terrain::Lava
                };
                self.w.set_pre(r + dr, c + dc, t);
                self.w.lock(r + dr, c + dc);
            }
        }
        self.w.set_force(r, c, Terrain::Palace);
        self.mark(54, r, c);
        // Lava path out.
        let mut d = DIRS[self.rng.index(4)];
        let (mut pr, mut pc) = (r + d.0 * 4, c + d.1 * 4);
        let len = self.rng.range(8, 16) as i32;
        let mut traps: Vec<usize> = [30usize, 31, 32]
            .into_iter()
            .filter(|&s| self.c.used[s])
            .collect();
        let mut path = Vec::new();
        for step in 0..len {
            if !self.w.interior(pr, pc, 2) || path.contains(&(pr, pc)) {
                break;
            }
            path.push((pr, pc));
            self.w.set_force(pr, pc, Terrain::Lava);
            self.w.lock(pr, pc);
            // Walls on both sides.
            for side in [(d.1, d.0), (-d.1, -d.0)] {
                let (sr, sc) = (pr + side.0, pc + side.1);
                if (self.w.get(sr, sc).is_none() || caldera_like) && !self.w.is_locked(sr, sc) {
                    self.w.set_pre(sr, sc, Terrain::Mountain);
                }
            }
            if step >= 2 && step % 3 == 1 && !traps.is_empty() {
                let s = traps.remove(0);
                self.w.set(pr, pc, Terrain::Lava);
                self.mark(s, pr, pc);
                self.c.locs[s].pass = true;
            }
            if step > 2 && self.rng.chance(1, 4) {
                let turn = if self.rng.coin() {
                    (d.1, d.0)
                } else {
                    (-d.1, -d.0)
                };
                d = turn;
            }
            pr += d.0;
            pc += d.1;
            if caldera_like && step > 4 && self.w.get(pr, pc) != Some(Terrain::Mountain) {
                break;
            }
        }
        // Open the mouth: the next tile in the path's direction.
        if self.w.inside(pr, pc) && !self.w.is_locked(pr, pc) {
            self.w.set_pre(pr, pc, Terrain::Lava);
        }
        for s in traps {
            self.c.on_map[s] = false;
        }
        Ok(())
    }

    // -- placement ---------------------------------------------------------

    /// The flute-revealed spot: a 7x7 clearing of one ground terrain, three
    /// boulders around the call tile, the location two rows below it.
    fn hidden_palace(&mut self, slot: usize) -> Result<(), RandoError> {
        let cover = self.cl.pick_ground(self.rng, &[Terrain::Forest]);
        for _ in 0..3000 {
            let (r, c) = self.rand_pos(5);
            if !self.w.free_square(r, c, 3) {
                continue;
            }
            for dr in -3..=3 {
                for dc in -3..=3 {
                    self.w.set(r + dr, c + dc, cover);
                    self.w.lock(r + dr, c + dc);
                }
            }
            // The game draws the eye rocks as plain mountain tiles.
            for (dr, dc) in [(-2, 0), (0, -2), (0, 2)] {
                self.w.set_force(r + dr, c + dc, Terrain::Mountain);
            }
            let (pr, pc) = (r + 2, c);
            self.w.set(pr, pc, cover);
            self.mark(slot, pr, pc);
            self.c.hidden[slot] = Some(Reveal::Flute);
            self.c.cover[slot] = cover;
            return Ok(());
        }
        Err(retry("no room for the flute spot"))
    }

    fn hidden_town(&mut self, slot: usize, avoid_col: Option<i32>) -> Result<(), RandoError> {
        for _ in 0..3000 {
            let Some((r, c)) = self.free_spot(1, 2, 20) else {
                continue;
            };
            if avoid_col == Some(c) {
                continue;
            }
            self.pad_open(slot, r, c, Terrain::Forest);
            // The ring must not be forest so the tile stays a one-tile run.
            self.c.hidden[slot] = Some(Reveal::Hammer);
            self.c.cover[slot] = Terrain::Forest;
            return Ok(());
        }
        Err(retry("no room for the hammer spot"))
    }

    fn bagu_woods(&mut self) -> Result<(), RandoError> {
        let woods: Vec<usize> = [6usize, 26, 27, 28, 29]
            .into_iter()
            .filter(|&s| self.c.used[s])
            .collect();
        let Some((r, c)) = self.free_spot(1, 6, 3000) else {
            return Err(retry("no room for Bagu's woods"));
        };
        self.pad_open(50, r, c, Terrain::Forest);
        let mut n = 0;
        for s in woods {
            for _ in 0..60 {
                let (rr, cc) = (
                    r + self.rng.range(-3, 3) as i32,
                    c + self.rng.range(-3, 3) as i32,
                );
                if self.w.interior(rr, cc, 1)
                    && self.w.get(rr, cc).is_none()
                    && !self.w.is_locked(rr, cc)
                {
                    self.w.set(rr, cc, Terrain::Forest);
                    self.w.lock(rr, cc);
                    self.mark(s, rr, cc);
                    n += 1;
                    break;
                }
            }
        }
        if n < 3 {
            return Err(retry("Bagu's woods too crowded"));
        }
        // Forest around them.
        for dr in -4..=4 {
            for dc in -4..=4 {
                if self.w.get(r + dr, c + dc).is_none() && self.rng.chance(2, 3) {
                    self.w.set_pre(r + dr, c + dc, Terrain::Forest);
                }
            }
        }
        Ok(())
    }

    /// Caldera: a linked cave pair through the crater wall and the third
    /// palace inside.
    fn caldera_contents(&mut self, crater: (i32, i32, i32)) -> Result<(), RandoError> {
        let (cr, cc, rad) = crater;
        // Third palace (or whatever palace sits in slot 54) inside.
        // Between the lake and the rim, with room for the 3x3 pad.
        let inner = rad - 1;
        let lake = rad / 3 + 2;
        let mut ok = false;
        for _ in 0..500 {
            let (r, c) = (
                cr + self.rng.range(-(inner as i64), inner as i64) as i32,
                cc + self.rng.range(-(inner as i64), inner as i64) as i32,
            );
            let d2 = (r - cr) * (r - cr) + (c - cc) * (c - cc);
            if d2 > inner * inner || d2 < lake * lake {
                continue;
            }
            if self.w.free_square(r, c, 1) {
                self.pad_open(54, r, c, Terrain::Palace);
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(retry("no room for the crater palace"));
        }
        // A passthrough pair from inside the crater to outside the wall.
        let pairs: Vec<(usize, usize)> = [(10usize, 11usize), (12, 13)]
            .into_iter()
            .filter(|&(a, b)| self.c.used[a] && self.c.used[b] && !self.placed[a])
            .collect();
        let Some(&(a, b)) = pairs.get(self.rng.index(pairs.len().max(1))) else {
            return Err(retry("no cave pair for the crater"));
        };
        for _ in 0..300 {
            let d = self.random_dir();
            // Inner end: just inside the crater rim, mouth toward the centre.
            let (ra, ca) = (cr + d.0 * (rad - 2), cc + d.1 * (rad - 2));
            for dr in -1..=1 {
                for dc in -1..=1 {
                    if self.w.get(ra + dr, ca + dc) == Some(Terrain::Mountain) {
                        self.w.clear(ra + dr, ca + dc);
                    }
                }
            }
            if !self.w.free_square(ra, ca, 1) {
                continue;
            }
            // Outer end: walk out until past the mountain.
            let mut k = rad + 2;
            let mut found = None;
            while k < rad + 20 {
                let (rb, cb) = (cr + d.0 * k, cc + d.1 * k);
                if !self.w.interior(rb, cb, 2) {
                    break;
                }
                let past = self.w.get(rb + d.0 * 2, cb + d.1 * 2) != Some(Terrain::Mountain);
                if past {
                    found = Some((rb, cb));
                    break;
                }
                k += 1;
            }
            let Some((rb, cb)) = found else { continue };
            // Clear the outer pad area (it is mountain scaffolding).
            for dr in -1..=1 {
                for dc in -1..=1 {
                    self.w.clear(rb + dr, cb + dc);
                }
            }
            if !self.w.free_square(rb, cb, 1) {
                continue;
            }
            self.pad_cave(a, ra, ca, (-d.0, -d.1));
            self.pad_cave(b, rb, cb, d);
            self.links.push((a, b));
            return Ok(());
        }
        Err(retry("no way through the crater wall"))
    }

    fn icon(&self, slot: usize) -> Terrain {
        self.c.icon[slot]
    }

    /// Everything that sits on a pad, in a random order (big things first).
    fn place_locations(&mut self) -> Result<(), RandoError> {
        let cont = self.c.cont;
        let o = self.o;
        // Hidden choices (East).
        let mut flute_slot = None;
        let mut hammer_slot = None;
        if cont == Cont::East {
            let eligible: Vec<usize> = (0..loc::SLOTS)
                .filter(|&s| {
                    self.c.used[s]
                        && !self.placed[s]
                        && matches!(
                            self.c.info(s).map(|i| i.class),
                            Some(Class::Palace(_) | Class::Town | Class::Item | Class::Minor)
                        )
                        && s != 54
                        && s != 22
                })
                .collect();
            if o.hide_palace {
                flute_slot = Some(if o.shuffle_hidden && !eligible.is_empty() {
                    eligible[self.rng.index(eligible.len())]
                } else {
                    53
                });
            }
            if o.hide_kasuto {
                let rest: Vec<usize> = eligible
                    .iter()
                    .copied()
                    .filter(|&s| Some(s) != flute_slot)
                    .collect();
                hammer_slot = Some(if o.shuffle_hidden && !rest.is_empty() {
                    rest[self.rng.index(rest.len())]
                } else {
                    49
                });
                if hammer_slot == flute_slot {
                    hammer_slot = None;
                }
            }
            if let Some(s) = flute_slot {
                self.hidden_palace(s)?;
            }
            if let Some(s) = hammer_slot {
                let avoid = self
                    .c
                    .locs
                    .get(flute_slot.unwrap_or(0))
                    .map(|l| i32::from(l.x));
                self.hidden_town(s, if flute_slot.is_some() { avoid } else { None })?;
            }
        }
        if cont == Cont::West && o.bagu_woods {
            self.bagu_woods()?;
        }
        // Linked groups.
        let mut groups: Vec<&'static [u8]> = Vec::new();
        for s in loc::slots(cont) {
            if !s.group.is_empty()
                && !groups.contains(&s.group)
                && s.group.iter().all(|&g| self.c.used[usize::from(g)])
            {
                groups.push(s.group);
            }
        }
        self.rng.shuffle(&mut groups);
        for g in groups {
            let (a, b) = (usize::from(g[0]), usize::from(g[1]));
            if self.placed[a] || self.placed[b] {
                continue;
            }
            let saria = cont == Cont::West && a == 48;
            let fairy = cont == Cont::West && a == 17;
            if o.sane_caves || saria || fairy {
                let wall = if saria { self.water } else { Terrain::Mountain };
                let icons = if saria {
                    [Some(Terrain::Town), Some(Terrain::Town)]
                } else if fairy {
                    [Some(self.icon(17)), None]
                } else {
                    [None, None]
                };
                self.place_pair(a, b, wall, icons)?;
            } else {
                self.place_cave(a)?;
                self.place_cave(b)?;
                self.links.push((a, b));
            }
        }
        // Singles.
        let mut singles: Vec<usize> = (0..loc::SLOTS)
            .filter(|&s| {
                self.c.used[s]
                    && !self.placed[s]
                    && match self.c.info(s).map(|i| i.class) {
                        Some(Class::Start | Class::Palace(_) | Class::Town | Class::Item) => true,
                        Some(Class::Special) => true,
                        Some(Class::Connector) => s == loc::CAVE1 || s == loc::CAVE2,
                        Some(Class::Minor) => o.less == LessImportantLocations::Isolate,
                        _ => false,
                    }
                    && s != 22
            })
            .collect();
        self.rng.shuffle(&mut singles);
        for s in singles {
            let icon = self.icon(s);
            if icon == Terrain::Cave {
                self.place_cave(s)?;
            } else {
                self.place_open(s, icon)?;
            }
        }
        Ok(())
    }

    // -- crossings ---------------------------------------------------------

    /// A placed location on this tile or next to it.
    fn near_location(&self, r: i32, c: i32) -> bool {
        self.near_placed(r, c)
    }

    /// Join land masses that hold locations with straight crossings
    /// (bridges over water, roads through mountains). Every candidate
    /// crossing is listed once, then they are added in random order
    /// (shorter first) whenever they join two separate masses, until every
    /// location can be walked to.
    fn connect(&mut self) -> Result<(), RandoError> {
        let max_len = {
            let base = if self.biome == Biome::Mountainous {
                15
            } else {
                10
            };
            if self.o.good_boots {
                base
            } else {
                base * 3 / 2
            }
        };
        // Shallow water needs the boots, so it counts as something to cross
        // (bridged like deep water); no boulders or river devils exist yet.
        let pass = |t: Terrain| t.is_open();
        let (lab, n) = core::regions(&self.w, pass);
        let cols = self.w.cols;
        let at = |r: i32, c: i32| lab[r as usize * cols + c as usize];
        let mut dsu = Dsu::new(n);
        for &(a, b) in &self.links {
            if let (Some(pa), Some(pb)) = (self.c.locs[a].pos(), self.c.locs[b].pos()) {
                let (la, lb) = (at(pa.0 as i32, pa.1 as i32), at(pb.0 as i32, pb.1 as i32));
                if la != usize::MAX && lb != usize::MAX {
                    dsu.union(la, lb);
                }
            }
        }
        let mut need: Vec<usize> = Vec::new();
        for s in 0..loc::SLOTS {
            if self.placed[s] {
                if let Some((r, c)) = self.c.locs[s].pos() {
                    let l = at(r as i32, c as i32);
                    if l != usize::MAX {
                        need.push(l);
                    }
                }
            }
        }
        let joined = |dsu: &mut Dsu, need: &[usize]| {
            let roots: std::collections::BTreeSet<usize> =
                need.iter().map(|&l| dsu.find(l)).collect();
            roots.len() <= 1
        };
        if joined(&mut dsu, &need) {
            return Ok(());
        }
        // Candidate crossings.
        let mut cands: Vec<Crossing> = Vec::new();
        for r in 1..self.w.rows as i32 - 1 {
            for c in 1..self.w.cols as i32 - 1 {
                let l0 = at(r, c);
                if l0 == usize::MAX {
                    continue;
                }
                // Only east and south: the reverse crossings are the same.
                for d in [(0, 1), (1, 0)] {
                    let cross = match self.w.get(r + d.0, c + d.1) {
                        Some(t @ (Terrain::Water | Terrain::Mountain | Terrain::WalkableWater)) => {
                            t
                        }
                        _ => continue,
                    };
                    let mut span = Vec::new();
                    let (mut rr, mut cc) = (r + d.0, c + d.1);
                    let mut end = None;
                    while span.len() <= max_len && self.w.interior(rr, cc, 1) {
                        let t = self.w.get(rr, cc);
                        if t == Some(cross) && !self.w.is_locked(rr, cc) {
                            span.push((rr, cc));
                            rr += d.0;
                            cc += d.1;
                            continue;
                        }
                        if t.is_some_and(pass) {
                            end = Some((rr, cc));
                        }
                        break;
                    }
                    let Some((er, ec)) = end else { continue };
                    let l1 = at(er, ec);
                    if span.len() < 2 || l1 == usize::MAX || l1 == l0 {
                        continue;
                    }
                    let side = (d.1, d.0);
                    let clean = span.iter().all(|&(sr, sc)| {
                        [side, (-side.0, -side.1)].iter().all(|&(a, b)| {
                            matches!(
                                self.w.get(sr + a, sc + b),
                                Some(Terrain::Water | Terrain::Mountain | Terrain::WalkableWater)
                            )
                        }) && !self.near_location(sr, sc)
                    });
                    if !clean {
                        continue;
                    }
                    // Random order, shorter crossings first on the whole.
                    let key = (span.len() as u64) * 1000 + self.rng.below(4000);
                    cands.push((key, span, l0, l1, cross));
                }
            }
        }
        cands.sort_by_key(|c| c.0);
        let mut used = vec![false; self.w.rows * self.w.cols];
        let mut style = 0u32;
        for (_, span, l0, l1, cross) in cands {
            if joined(&mut dsu, &need) {
                break;
            }
            if dsu.find(l0) == dsu.find(l1) {
                continue;
            }
            // Keep crossings apart (no two side by side).
            let busy = span.iter().any(|&(sr, sc)| {
                std::iter::once((0, 0)).chain(DIRS).any(|(a, b)| {
                    let (rr, cc) = (sr + a, sc + b);
                    self.w.inside(rr, cc) && used[rr as usize * cols + cc as usize]
                })
            });
            if busy {
                continue;
            }
            dsu.union(l0, l1);
            let paint = if cross == Terrain::Mountain {
                Terrain::Road
            } else if !self.o.good_boots && style % 3 == 2 && span.len() >= 3 {
                Terrain::WalkableWater
            } else {
                Terrain::Bridge
            };
            style += 1;
            for (i, &(sr, sc)) in span.iter().enumerate() {
                let t = if paint == Terrain::WalkableWater && (i == 0 || i == span.len() - 1) {
                    Terrain::Road
                } else {
                    paint
                };
                self.w.set(sr, sc, t);
                used[sr as usize * cols + sc as usize] = true;
            }
            if !self.enc_pool.is_empty() && span.len() >= 3 && paint != Terrain::WalkableWater {
                let (mr, mc) = span[span.len() / 2];
                let s = self.enc_pool.remove(self.rng.index(self.enc_pool.len()));
                let icon = match self.icon(s) {
                    Terrain::Bridge | Terrain::Road => paint,
                    other if other.is_open() => other,
                    _ => paint,
                };
                self.w.set(mr, mc, icon);
                self.mark(s, mr, mc);
                self.c.locs[s].pass = true;
                self.c.icon[s] = icon;
            }
            for &(sr, sc) in &span {
                self.w.lock(sr, sc);
            }
        }
        if joined(&mut dsu, &need) {
            Ok(())
        } else {
            if std::env::var_os("OW_DEBUG").is_some() {
                let mut by_root: std::collections::BTreeMap<usize, Vec<usize>> =
                    std::collections::BTreeMap::new();
                for s in 0..loc::SLOTS {
                    if let (true, Some((r, c))) = (self.placed[s], self.c.locs[s].pos()) {
                        let l = at(r as i32, c as i32);
                        if l != usize::MAX {
                            by_root.entry(dsu.find(l)).or_default().push(s);
                        }
                    }
                }
                eprintln!("{:?}\n{}", by_root, self.w.dump());
            }
            Err(retry("could not join the land masses"))
        }
    }

    // -- connectors ----------------------------------------------------------

    /// Raft (`bridge == false`) or bridge connector on the coast of `edge`.
    fn coast_connector(&mut self, slot: usize, edge: Dir, bridge: bool) -> Result<(), RandoError> {
        let (rows, cols) = (self.w.rows as i32, self.w.cols as i32);
        for _ in 0..300 {
            let (mut r, mut c) = match edge {
                (0, 1) => (2 + self.rng.index((rows - 4) as usize) as i32, cols - 1),
                (0, -1) => (2 + self.rng.index((rows - 4) as usize) as i32, 0),
                (1, 0) => (rows - 1, 2 + self.rng.index((cols - 4) as usize) as i32),
                _ => (0, 2 + self.rng.index((cols - 4) as usize) as i32),
            };
            let inward = (-edge.0, -edge.1);
            let mut water = Vec::new();
            while self.w.inside(r, c)
                && matches!(
                    self.w.get(r, c),
                    Some(Terrain::Water | Terrain::WalkableWater)
                )
                && !self.w.is_locked(r, c)
            {
                water.push((r, c));
                r += inward.0;
                c += inward.1;
            }
            if water.len() < 2 || water.len() > 12 || !self.w.inside(r, c) {
                continue;
            }
            // Through a little mountain if need be (canyons, mountain
            // borders): a road up to open ground.
            let mut cut = Vec::new();
            while cut.len() < 10
                && self.w.interior(r, c, 1)
                && self.w.get(r, c) == Some(Terrain::Mountain)
                && !self.w.is_locked(r, c)
            {
                cut.push((r, c));
                r += inward.0;
                c += inward.1;
            }
            let land = self.w.get(r, c);
            if !land.is_some_and(Terrain::is_open) || self.w.is_locked(r, c) {
                continue;
            }
            for &(cr, cc) in &cut {
                self.w.set(cr, cc, Terrain::Road);
            }
            let Some(&(lr, lc)) = water.last() else {
                continue;
            };
            if self.near_location(lr, lc) {
                continue;
            }
            self.w.set(lr, lc, Terrain::Bridge);
            self.w.lock(lr, lc);
            if bridge {
                for &(wr, wc) in &water {
                    self.w.set(wr, wc, Terrain::Bridge);
                    self.w.lock(wr, wc);
                }
            }
            self.mark(slot, lr, lc);
            self.c.icon[slot] = Terrain::Bridge;
            return Ok(());
        }
        Err(retry("no coast for a connector"))
    }

    // -- blockers ------------------------------------------------------------

    /// Put `t` on the mouth of a random cave (one of `slots`).
    fn block_cave(&mut self, t: Terrain, allowed: &dyn Fn(usize) -> bool) -> bool {
        let cands: Vec<(usize, (i32, i32))> = self
            .mouths
            .iter()
            .copied()
            .filter(|&(s, _)| allowed(s))
            .collect();
        if cands.is_empty() {
            return false;
        }
        let (s, (r, c)) = cands[self.rng.index(cands.len())];
        self.w.set_force(r, c, t);
        self.mouths.retain(|&(x, _)| x != s);
        true
    }

    fn blockers(&mut self) -> Result<(), RandoError> {
        let cont = self.c.cont;
        let conn_caves = |s: usize, cont: Cont| -> bool {
            s == loc::CAVE1
                || s == loc::CAVE2
                || loc::info(cont, s).is_some_and(|i| i.class == Class::Link)
        };
        let blockable = self.o.caves_blockable;
        match cont {
            Cont::West => {
                let n = self.rng.index(3);
                for _ in 0..n {
                    self.block_cave(Terrain::Rock, &|s| !conn_caves(s, cont) || blockable);
                }
            }
            Cont::East => {
                let devil = self.o.devil;
                match devil {
                    RiverDevilBlocker::Cave => {
                        if !self
                            .block_cave(Terrain::RiverDevil, &|s| ![17, 18, 19, 20].contains(&s))
                        {
                            return Err(retry("no cave for the river devil"));
                        }
                    }
                    RiverDevilBlocker::Siege => self.siege()?,
                    _ => self.devil_on_path(Terrain::RiverDevil)?,
                }
                if self.o.east_rocks {
                    if self.o.east_rock_is_path {
                        let _ = self.devil_on_path(Terrain::Rock);
                    } else {
                        self.block_cave(Terrain::Rock, &|s| {
                            ![17, 18, 19, 20].contains(&s) && (!conn_caves(s, cont) || blockable)
                        });
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// A blocker on a crossing tile (bridge or road in a one-wide channel).
    fn devil_on_path(&mut self, t: Terrain) -> Result<(), RandoError> {
        let mut cands = Vec::new();
        for r in 1..self.w.rows as i32 - 1 {
            for c in 1..self.w.cols as i32 - 1 {
                if !matches!(self.w.get(r, c), Some(Terrain::Bridge | Terrain::Road)) {
                    continue;
                }
                if self.c.slot_at(r as usize, c as usize).is_some() || self.near_placed(r, c) {
                    continue;
                }
                let blocked = |a: Option<Terrain>| {
                    matches!(
                        a,
                        Some(Terrain::Water | Terrain::Mountain | Terrain::WalkableWater)
                    )
                };
                let horiz = blocked(self.w.get(r - 1, c)) && blocked(self.w.get(r + 1, c));
                let vert = blocked(self.w.get(r, c - 1)) && blocked(self.w.get(r, c + 1));
                if horiz || vert {
                    cands.push((r, c));
                }
            }
        }
        if cands.is_empty() {
            return Err(retry("no crossing for a blocker"));
        }
        let (r, c) = cands[self.rng.index(cands.len())];
        self.w.set_force(r, c, t);
        Ok(())
    }

    fn near_placed(&self, r: i32, c: i32) -> bool {
        (0..loc::SLOTS).any(|s| {
            self.placed[s]
                && self.c.locs[s]
                    .pos()
                    .is_some_and(|(pr, pc)| (pr as i32 - r).abs() + (pc as i32 - c).abs() <= 1)
        })
    }

    /// River devils on the four sides of a town.
    fn siege(&mut self) -> Result<(), RandoError> {
        let towns: Vec<usize> = [45usize, 47, 51, 49]
            .into_iter()
            .filter(|&s| self.placed[s] && self.c.hidden[s].is_none())
            .collect();
        for &s in &towns {
            let Some((r, c)) = self.c.locs[s].pos() else {
                continue;
            };
            let (r, c) = (r as i32, c as i32);
            if DIRS
                .iter()
                .all(|&(dr, dc)| self.w.get(r + dr, c + dc).is_some_and(Terrain::is_open))
            {
                for (dr, dc) in DIRS {
                    self.w.set_force(r + dr, c + dc, Terrain::RiverDevil);
                }
                return Ok(());
            }
        }
        Err(retry("no town to besiege"))
    }

    /// Bytes the map would take (one per run, runs end at terrain changes,
    /// every 16 tiles and at row ends; locations count as their own runs).
    fn run_bytes(&self) -> usize {
        let mut n = 0;
        for r in 0..self.w.rows as i32 {
            let mut prev: Option<Terrain> = None;
            let mut len = 0;
            for c in 0..self.w.cols as i32 {
                let t = self.w.get(r, c);
                if t != prev || len == 16 || self.w.is_locked(r, c) {
                    n += 1;
                    len = 0;
                }
                prev = t;
                len += 1;
            }
            // Filler columns past the playable area.
            n += (64 - self.w.cols).div_ceil(16);
        }
        n + (75 - self.w.rows) * 4
    }

    /// Too many short runs for the map budget (busy climates): merge lone
    /// ground tiles into the ground on both sides of them. Only ground
    /// changes into other ground, so nothing that was reachable stops being
    /// reachable.
    fn simplify(&mut self) {
        let ground = |t: Option<Terrain>| {
            matches!(
                t,
                Some(
                    Terrain::Desert
                        | Terrain::Grass
                        | Terrain::Forest
                        | Terrain::Swamp
                        | Terrain::Grave
                        | Terrain::Road
                )
            )
        };
        for pass in 0..6 {
            if self.run_bytes() <= crate::overworld::map::BIG_BUDGET - 32 {
                return;
            }
            // Later passes also fill lone mountain, water and lava tiles
            // (that only opens ways, never closes one).
            let loose = pass >= 2;
            for r in 0..self.w.rows as i32 {
                for c in 1..self.w.cols as i32 - 1 {
                    let here = self.w.get(r, c);
                    let fillable = ground(here)
                        || (loose
                            && matches!(
                                here,
                                Some(Terrain::Mountain | Terrain::Water | Terrain::Lava)
                            ));
                    if self.w.is_locked(r, c) || self.w.is_pre(r, c) || !fillable {
                        continue;
                    }
                    let (a, b) = (self.w.get(r, c - 1), self.w.get(r, c + 1));
                    if a == b && ground(a) && a != self.w.get(r, c) {
                        if let Some(t) = a {
                            self.w.set(r, c, t);
                        }
                    }
                }
            }
        }
    }

    /// Minor tiles blended into terrain of their own kind.
    fn blend_minor(&mut self) {
        let minors: Vec<usize> = (0..loc::SLOTS)
            .filter(|&s| {
                self.c.used[s]
                    && !self.placed[s]
                    && self.c.info(s).map(|i| i.class) == Some(Class::Minor)
                    && s != 22
            })
            .collect();
        for s in minors {
            let want = self.icon(s);
            let mut done = false;
            for _ in 0..2000 {
                let (r, c) = self.rand_pos(1);
                if self.w.get(r, c) == Some(want)
                    && !self.w.is_locked(r, c)
                    && !self.near_placed(r, c)
                {
                    self.w.lock(r, c);
                    self.mark(s, r, c);
                    done = true;
                    break;
                }
            }
            if !done {
                self.c.on_map[s] = false;
            }
        }
    }
}

/// Generate West or East Hyrule into `base` (positions, grid, flags).
pub fn generate(
    base: &Continent,
    o: &Resolved,
    rng: &mut Rng,
    cl: ClimateDef,
    rows: usize,
    cols: usize,
) -> Result<Continent, RandoError> {
    let mut b = Builder::new(base, o, rng, cl, rows, cols);
    let cont = b.c.cont;
    // Encounter pool.
    b.enc_pool = (0..loc::SLOTS)
        .filter(|&s| {
            b.c.used[s]
                && b.c.info(s).map(|i| i.class) == Some(Class::Encounter)
                && !(cont == Cont::East && (30..=32).contains(&s))
        })
        .collect();
    b.scaffold();
    // Ocean strips where the connectors leave.
    // The raft sails east from dock 0 and west from dock 1, so the dock
    // sits on that coast; a bridge leaves from the other side.
    let raft_edge: Dir = if b.c.dock == Some(1) { (0, -1) } else { (0, 1) };
    let uses_raft = b.c.used[loc::RAFT];
    let uses_bridge = b.c.used[loc::BRIDGE];
    let bridge_edge: Dir = if uses_raft {
        (-raft_edge.0, -raft_edge.1)
    } else {
        (0, 1)
    };
    if uses_raft {
        b.ocean(raft_edge, 3);
    }
    if uses_bridge {
        b.ocean(bridge_edge, 3);
    }
    if cont == Cont::East {
        b.great_palace()?;
    }
    if b.biome == Biome::Caldera {
        let crater = b.crater();
        b.caldera_contents(crater)?;
    }
    b.place_locations()?;
    let seeds = b.cl.seeds;
    let walls: Vec<Terrain> = Vec::new();
    core::place_seeds(&mut b.w, &b.cl, b.rng, seeds, &walls);
    core::grow(&mut b.w, &b.cl, b.rng);
    if uses_raft {
        b.coast_connector(loc::RAFT, raft_edge, false)?;
    }
    if uses_bridge {
        b.coast_connector(loc::BRIDGE, bridge_edge, true)?;
    }
    b.connect()?;
    b.blockers()?;
    if o.less == LessImportantLocations::BlendIn {
        b.blend_minor();
    }
    // Encounters that found no crossing are removed.
    for s in b.enc_pool.clone() {
        b.c.on_map[s] = false;
    }
    b.simplify();
    let filler = if b.biome == Biome::Mountainous {
        Terrain::Mountain
    } else {
        Terrain::Water
    };
    let grid = b.w.to_grid(filler);
    let mut c = b.c;
    c.grid = grid;
    c.map_changed = true;
    c.table_changed = true;
    c.separator = Some(RAW_ROW_BASE + (rows / 2) as u8);
    for s in 0..loc::SLOTS {
        if c.used[s] && c.on_map[s] && c.info(s).map(|i| i.class) != Some(Class::Encounter) {
            let is_lava_trap = cont == Cont::East && (30..=32).contains(&s);
            if !is_lava_trap {
                c.locs[s].pass = false;
            }
        }
    }
    // The twin of East slot 28 is never shown.
    if cont == Cont::East {
        c.on_map[22] = false;
    }
    Ok(c)
}
