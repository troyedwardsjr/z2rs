//! Palace generators.
//!
//! Two families:
//!
//! * **Vanilla-based** ([`vanilla`], [`shuffle`], [`shorten`]): the game's
//!   own palace graph, optionally with rooms swapped between positions
//!   that offer the same connections, and with straight rooms spliced out
//!   to shorten it.
//! * **Grown** ([`grow`]): a palace built room by room from a pool of
//!   rooms. A [`Policy`] decides which open exit to extend next, whether
//!   rooms sit on a grid (so neighbours must agree, like a map) or form a
//!   free graph, how loose ends are tied up, and a few style rules (towers
//!   prefer elevators, mirror palaces are symmetric, chaos palaces get
//!   one-way passages). Every style ends with the same checks
//!   ([`super::layout::validate`]).

use std::collections::{BTreeMap, BTreeSet};

use super::layout::{Dir, Ix, Layout, Placed};
use super::rooms::{Role, Room, RoomKey, Shape, VanillaPool};
use crate::rng::Rng;

/// The vanilla layout of palace `p`, with vanilla map numbers.
#[must_use]
pub fn vanilla(pool: &VanillaPool, p: u8) -> Layout {
    let keys = &pool.palace_rooms[usize::from(p - 1)];
    let mut order: Vec<RoomKey> = keys.clone();
    // Entrance first.
    order.sort_by_key(|k| (pool.rooms[k].role != Role::Entrance, k.map));
    let index: BTreeMap<u8, Ix> = order.iter().enumerate().map(|(i, k)| (k.map, i)).collect();
    let mut l = Layout {
        palace: p,
        rooms: order
            .iter()
            .map(|k| {
                let mut pl = Placed::new(pool.rooms[k].clone());
                pl.map = Some(k.map);
                pl
            })
            .collect(),
        style: "Vanilla",
        boss_continues: false,
    };
    for (i, k) in order.iter().enumerate() {
        let v = pool.links[k];
        let r = &mut l.rooms[i];
        r.left = v.left.and_then(|m| index.get(&m).copied());
        r.right = v.right.and_then(|m| index.get(&m).copied());
        r.up = v.up.and_then(|m| index.get(&m).copied());
        r.down = v.down.and_then(|m| index.get(&m).copied());
        r.drop = v.drop.and_then(|(_, m, _)| index.get(&m).copied());
    }
    l
}

/// Swap room contents between positions that offer the same connections
/// (entrance, boss and Thunderbird stay put). The positions keep their links and map
/// numbers; the bytes a position does not link keep the position's vanilla
/// values.
pub fn shuffle(l: &mut Layout, rng: &mut Rng) {
    type Class = (bool, bool, bool, bool, bool, bool);
    let mut buckets: BTreeMap<Class, Vec<Ix>> = BTreeMap::new();
    for (i, p) in l.rooms.iter().enumerate() {
        if matches!(p.room.role, Role::Entrance | Role::Boss | Role::Thunderbird)
            || p.sideview.is_some()
        {
            continue;
        }
        buckets.entry(p.room.shape.class()).or_default().push(i);
    }
    for (_, ixs) in buckets {
        let mut contents: Vec<Room> = ixs.iter().map(|&i| l.rooms[i].room.clone()).collect();
        rng.shuffle(&mut contents);
        for (&i, mut c) in ixs.iter().zip(contents) {
            c.conn = l.rooms[i].room.conn;
            l.rooms[i].room = c;
        }
    }
    l.style = "Vanilla shuffle";
}

/// Remove room `i`, fixing every index.
fn remove_room(l: &mut Layout, i: Ix) {
    l.rooms.remove(i);
    for p in &mut l.rooms {
        for d in Dir::ALL {
            match p.get(d) {
                Some(n) if n == i => p.set(d, None),
                Some(n) if n > i => p.set(d, Some(n - 1)),
                _ => {}
            }
        }
    }
}

/// Splice room `i` out if it is a straight two-way passage (left-right or
/// elevator up-down); returns whether it did.
pub fn splice(l: &mut Layout, i: Ix) -> bool {
    let p = &l.rooms[i];
    let s = p.room.shape;
    let horizontal = s.left && s.right && !s.up && !s.down && s.drop.is_none() && s.zone.is_none();
    let vertical = s.up && s.down && !s.left && !s.right && s.drop.is_none() && s.zone.is_none();
    let (a_dir, b_dir) = if horizontal {
        (Dir::Left, Dir::Right)
    } else if vertical {
        (Dir::Up, Dir::Down)
    } else {
        return false;
    };
    let (Some(a), Some(b)) = (p.get(a_dir), p.get(b_dir)) else {
        return false;
    };
    // Both neighbours must point back (two-way passages only).
    if l.rooms[a].get(b_dir) != Some(i) || l.rooms[b].get(a_dir) != Some(i) || a == b {
        return false;
    }
    // Nothing else may lead into this room.
    let others = l
        .rooms
        .iter()
        .enumerate()
        .any(|(j, q)| j != a && j != b && q.neighbours().any(|(_, n)| n == i));
    if others {
        return false;
    }
    l.rooms[a].set(b_dir, Some(b));
    l.rooms[b].set(a_dir, Some(a));
    remove_room(l, i);
    true
}

/// Splice out random straight normal rooms until `l` has `target` rooms
/// (or nothing more can go).
pub fn shorten(l: &mut Layout, target: usize, rng: &mut Rng) {
    let mut fails = 0;
    while l.rooms.len() > target && fails < 200 {
        let cands: Vec<Ix> = (0..l.rooms.len())
            .filter(|&i| l.rooms[i].room.role == Role::Normal)
            .collect();
        let Some(&i) = rng.pick(&cands) else { break };
        if splice(l, i) {
            fails = 0;
        } else {
            fails += 1;
        }
    }
}

/// Room choices for a grown palace.
#[derive(Debug, Clone)]
pub struct Parts {
    /// The palace's entrance.
    pub entrance: Room,
    /// The boss room.
    pub boss: Room,
    /// Item rooms.
    pub items: Vec<Room>,
    /// Thunderbird room (Great Palace).
    pub thunderbird: Option<Room>,
    /// Rooms to fill the rest with.
    pub pool: Vec<Room>,
}

/// Generator styles that grow palaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Free graph, open exits extended in random order, dead ends kept in
    /// balance, loose ends paired up.
    Reconstructed,
    /// Like Reconstructed without dead ends or drops: loose ends are
    /// joined into loops.
    Loopy,
    /// Reconstructed, then some passages are re-pointed one-way.
    Chaos,
    /// Grid: grow from a random open exit.
    RandomWalk,
    /// Grid: grow with per-palace direction weights (wide palaces stay
    /// wide, deep ones deep).
    VanillaWeighted,
    /// Grid: always fill the empty cell that touches the most open exits.
    Sequential,
    /// Grid: strongly prefers going up; floors are linked by elevators.
    Tower,
    /// Grid: built on the right half and mirrored onto the left.
    Mirror,
}

impl Style {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Style::Reconstructed => "Reconstructed",
            Style::Loopy => "Loopy",
            Style::Chaos => "Chaos",
            Style::RandomWalk => "Random walk",
            Style::VanillaWeighted => "Vanilla-weighted",
            Style::Sequential => "Sequential",
            Style::Tower => "Tower",
            Style::Mirror => "Mirror",
        }
    }

    fn grid(self) -> bool {
        !matches!(self, Style::Reconstructed | Style::Loopy | Style::Chaos)
    }

    /// Weight of extending an open exit in direction `d`.
    fn weight(self, d: Dir, palace: u8) -> u32 {
        let vertical = matches!(d, Dir::Up | Dir::Down | Dir::Drop);
        match self {
            Style::Tower => {
                if vertical {
                    8
                } else {
                    1
                }
            }
            Style::VanillaWeighted => {
                // Wide palaces (1, 2, 5) mostly grow sideways; 3, 4 and 6
                // and the Great Palace go deeper.
                let (h, v) = match palace {
                    1 | 2 | 5 => (4, 1),
                    3 | 4 => (3, 2),
                    _ => (2, 2),
                };
                if vertical {
                    v
                } else {
                    h
                }
            }
            Style::Mirror => {
                if vertical {
                    1
                } else {
                    3
                }
            }
            _ => 1,
        }
    }
}

/// Grid offset of moving in `d`.
fn delta(d: Dir) -> (i32, i32) {
    match d {
        Dir::Left => (-1, 0),
        Dir::Right => (1, 0),
        Dir::Up => (0, 1),
        Dir::Down | Dir::Drop => (0, -1),
    }
}

/// What a room must offer to be entered through an exit in `d`.
fn accepts(r: &Room, d: Dir) -> bool {
    match d {
        Dir::Left => r.shape.right,
        Dir::Right => r.shape.left,
        Dir::Up => r.shape.down,
        Dir::Down => r.shape.up,
        Dir::Drop => r.shape.zone.is_some(),
    }
}

/// The exit of `r` used when it is entered through `d` (none for drops).
fn entry_exit(d: Dir) -> Option<Dir> {
    d.opposite()
}

fn exits_of(s: &Shape) -> Vec<Dir> {
    let mut v = Vec::new();
    if s.left {
        v.push(Dir::Left);
    }
    if s.right {
        v.push(Dir::Right);
    }
    if s.up {
        v.push(Dir::Up);
    }
    if s.down {
        v.push(Dir::Down);
    }
    if s.drop.is_some() {
        v.push(Dir::Drop);
    }
    v
}

/// Settings for [`grow`].
#[derive(Debug, Clone)]
pub struct GrowParams {
    /// Palace number.
    pub palace: u8,
    /// Style.
    pub style: Style,
    /// Rooms wanted (including the special rooms).
    pub target: usize,
    /// The boss room continues through its right side.
    pub boss_continues: bool,
    /// Put the Thunderbird room between the entrance and the boss.
    pub thunderbird_gate: bool,
    /// No two rooms with the same layout.
    pub no_dup_layout: bool,
    /// No two rooms with the same layout and enemies.
    pub no_dup_content: bool,
}

struct Builder<'a> {
    p: &'a GrowParams,
    parts: &'a Parts,
    rng: &'a mut Rng,
    l: Layout,
    open: Vec<(Ix, Dir)>,
    grid: BTreeMap<(i32, i32), Ix>,
    used_layouts: BTreeSet<u16>,
    used_content: BTreeSet<(u16, u16)>,
    closing: bool,
}

impl Builder<'_> {
    fn pos(&self, i: Ix) -> (i32, i32) {
        self.l.rooms[i].pos.unwrap_or((0, 0))
    }

    fn allowed(&self, r: &Room) -> bool {
        if self.p.no_dup_layout && self.used_layouts.contains(&r.sideview) {
            return false;
        }
        if self.p.no_dup_content && self.used_content.contains(&(r.sideview, r.enemies)) {
            return false;
        }
        if self.p.style == Style::Loopy
            && !self.closing
            && (r.shape.drop.is_some() || r.shape.exits() < 2)
        {
            return false;
        }
        true
    }

    /// Can `r` sit at `at` on the grid, entered from the cell in direction
    /// `from` (the exit it is entered through)? Every neighbour cell must
    /// agree: an occupied neighbour pointing here needs the matching exit,
    /// and an exit of `r` into an occupied cell needs the neighbour's open
    /// matching exit.
    fn fits(&self, r: &Room, at: (i32, i32)) -> bool {
        if self.grid.contains_key(&at) {
            return false;
        }
        if self.p.style == Style::Mirror && at.0 < 0 {
            return false;
        }
        if self.p.style == Style::Mirror && at.0 == 0 && r.shape.left != r.shape.right {
            return false;
        }
        for d in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
            let (dx, dy) = delta(d);
            let n = (at.0 + dx, at.1 + dy);
            let mine = match d {
                Dir::Left => r.shape.left,
                Dir::Right => r.shape.right,
                Dir::Up => r.shape.up,
                Dir::Down => r.shape.down || r.shape.drop.is_some(),
                Dir::Drop => false,
            };
            match self.grid.get(&n) {
                Some(&j) => {
                    let o = d.opposite().unwrap_or(Dir::Up);
                    let q = &self.l.rooms[j];
                    let theirs = q.has_exit(o) || (o == Dir::Down && q.has_exit(Dir::Drop));
                    if mine != theirs {
                        return false;
                    }
                    if mine {
                        // vertical pairs must match kinds: an elevator meets
                        // an elevator, a drop meets a landing.
                        if d == Dir::Down {
                            let ok = (r.shape.down && q.room.shape.up)
                                || (r.shape.drop.is_some() && q.room.shape.zone.is_some());
                            if !ok {
                                return false;
                            }
                        }
                        if d == Dir::Up {
                            let ok = (r.shape.up && q.room.shape.down)
                                || (r.shape.zone.is_some() && q.room.shape.drop.is_some());
                            if !ok {
                                return false;
                            }
                        }
                        if q.get(o).is_some() {
                            return false;
                        }
                    }
                }
                None => {
                    if self.p.style == Style::Mirror && mine && n.0 < 0 && at.0 != 0 {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Place `r`, entered through open exit `(from, d)`.
    fn attach(&mut self, from: Ix, d: Dir, r: Room) -> Ix {
        let i = self.l.rooms.len();
        self.used_layouts.insert(r.sideview);
        self.used_content.insert((r.sideview, r.enemies));
        let mut pl = Placed::new(r);
        if self.p.style.grid() {
            let (x, y) = self.pos(from);
            let (dx, dy) = delta(d);
            pl.pos = Some((x + dx, y + dy));
        }
        self.l.rooms.push(pl);
        self.l.link(from, d, i);
        self.open.retain(|&(a, b)| !(a == from && b == d));
        let entered = entry_exit(d);
        let shape = self.l.rooms[i].room.shape;
        let centre = self.p.style == Style::Mirror && self.l.rooms[i].pos.is_some_and(|p| p.0 == 0);
        for e in exits_of(&shape) {
            if Some(e) == entered {
                continue;
            }
            // Mirror: the centre column's left side is joined to the
            // mirrored half afterwards.
            if centre && e == Dir::Left {
                continue;
            }
            if self.l.rooms[i].room.role == Role::Boss && e == Dir::Right && !self.p.boss_continues
            {
                continue;
            }
            self.open.push((i, e));
        }
        if let Some(at) = self.l.rooms[i].pos {
            self.grid.insert(at, i);
            // Join with neighbours that already point here.
            for e in exits_of(&shape) {
                if Some(e) == entered {
                    continue;
                }
                let (dx, dy) = delta(e);
                if let Some(&j) = self.grid.get(&(at.0 + dx, at.1 + dy)) {
                    if e == Dir::Drop {
                        self.l.rooms[i].drop = Some(j);
                    } else {
                        self.l.link(i, e, j);
                        let o = e.opposite().unwrap_or(Dir::Up);
                        self.open.retain(|&(a, b)| !(a == j && b == o));
                    }
                    self.open.retain(|&(a, b)| !(a == i && b == e));
                }
            }
            // A drop from above lands here.
            if shape.zone.is_some() {
                if let Some(&j) = self.grid.get(&(at.0, at.1 + 1)) {
                    if self.l.rooms[j].has_exit(Dir::Drop) && self.l.rooms[j].drop.is_none() {
                        self.l.rooms[j].drop = Some(i);
                        self.open.retain(|&(a, b)| !(a == j && b == Dir::Drop));
                    }
                }
            }
        }
        i
    }

    fn target_cell(&self, from: Ix, d: Dir) -> (i32, i32) {
        let (x, y) = self.pos(from);
        let (dx, dy) = delta(d);
        (x + dx, y + dy)
    }

    /// Candidate rooms (indices into the pool) for open exit `(from, d)`.
    fn candidates(&self, from: Ix, d: Dir) -> Vec<usize> {
        let at = self.target_cell(from, d);
        (0..self.parts.pool.len())
            .filter(|&k| {
                let r = &self.parts.pool[k];
                accepts(r, d) && self.allowed(r) && (!self.p.style.grid() || self.fits(r, at))
            })
            .collect()
    }

    fn pick_open(&mut self) -> Option<(Ix, Dir)> {
        if self.open.is_empty() {
            return None;
        }
        if self.p.style == Style::Sequential {
            // The open exit whose target cell touches the most other open
            // exits (fill holes first); ties broken randomly.
            let mut best: Vec<usize> = Vec::new();
            let mut best_score = -1i32;
            for (k, &(i, d)) in self.open.iter().enumerate() {
                let at = self.target_cell(i, d);
                let score = self
                    .open
                    .iter()
                    .filter(|&&(j, e)| self.target_cell(j, e) == at)
                    .count() as i32;
                if score > best_score {
                    best_score = score;
                    best.clear();
                }
                if score == best_score {
                    best.push(k);
                }
            }
            let k = *self.rng.pick(&best)?;
            return Some(self.open[k]);
        }
        let weights: Vec<u32> = self
            .open
            .iter()
            .map(|&(_, d)| self.p.style.weight(d, self.p.palace))
            .collect();
        let k = self.rng.weighted_index(&weights)?;
        Some(self.open[k])
    }

    /// Grow `n` normal rooms off the open exits.
    fn grow_normals(&mut self, n: usize, keep_open: usize) -> bool {
        let mut placed = 0;
        let mut stuck = 0;
        while placed < n {
            if stuck > 60 {
                return false;
            }
            let Some((from, d)) = self.pick_open() else {
                return false;
            };
            let cands = self.candidates(from, d);
            if cands.is_empty() {
                stuck += 1;
                continue;
            }
            let remaining = n - placed;
            let open_after = self.open.len() - 1;
            // Prefer rooms that keep the number of loose ends sensible:
            // never close the last way on early, and taper off near the end.
            let weights: Vec<u32> = cands
                .iter()
                .map(|&k| {
                    let new = u32::from(self.parts.pool[k].shape.exits().saturating_sub(1));
                    let after = open_after + new as usize;
                    if after < keep_open.max(1) {
                        0
                    } else if after > remaining + keep_open + 2 {
                        1
                    } else if new == 1 {
                        6
                    } else {
                        4
                    }
                })
                .collect();
            let Some(w) = self.rng.weighted_index(&weights) else {
                stuck += 1;
                continue;
            };
            let room = self.parts.pool[cands[w]].clone();
            self.attach(from, d, room);
            placed += 1;
            stuck = 0;
        }
        true
    }

    /// Open exits a special room can hang off. Boss and Thunderbird rooms
    /// are always walked into from their left side.
    fn special_spots(&self, r: &Room) -> Vec<(Ix, Dir)> {
        let only_right = matches!(r.role, Role::Boss | Role::Thunderbird);
        self.open
            .iter()
            .copied()
            .filter(|&(i, d)| {
                (!only_right || d == Dir::Right)
                    && accepts(r, d)
                    && (!self.p.style.grid() || self.fits(r, self.target_cell(i, d)))
            })
            .collect()
    }

    /// Attach a special room to a compatible open exit, first growing a
    /// connector room or two if no open exit fits.
    fn place_special(&mut self, r: Room) -> Option<Ix> {
        for _ in 0..8 {
            let mut opts = self.special_spots(&r);
            if !opts.is_empty() {
                self.rng.shuffle(&mut opts);
                let (i, d) = opts[0];
                return Some(self.attach(i, d, r));
            }
            // Grow one room that offers a fitting exit.
            let mut tries: Vec<(Ix, Dir, usize)> = Vec::new();
            for &(i, d) in &self.open {
                for k in self.candidates(i, d) {
                    let c = &self.parts.pool[k];
                    let only_right = matches!(r.role, Role::Boss | Role::Thunderbird);
                    let offers = exits_of(&c.shape).into_iter().any(|e| {
                        Some(e) != entry_exit(d)
                            && (!only_right || e == Dir::Right)
                            && accepts(&r, e)
                    });
                    if offers {
                        tries.push((i, d, k));
                    }
                }
            }
            let &(i, d, k) = self.rng.pick(&tries)?;
            let room = self.parts.pool[k].clone();
            self.attach(i, d, room);
        }
        None
    }

    /// Tie up loose ends: pair opposite open exits (free graph), or cap
    /// them with the smallest fitting rooms; returns false if some stay
    /// open.
    fn close(&mut self, budget: usize) -> bool {
        self.closing = true;
        let ok = self.close_inner(budget);
        self.closing = false;
        ok
    }

    fn close_inner(&mut self, budget: usize) -> bool {
        let mut extra = 0;
        let mut guard = 0;
        while !self.open.is_empty() {
            guard += 1;
            if guard > 400 {
                return false;
            }
            if !self.p.style.grid() && self.pair_once() {
                continue;
            }
            // Cap one open exit with the fitting room that adds the fewest
            // new exits.
            let mut best: Option<((Ix, Dir), usize, u8)> = None;
            let mut opens = self.open.clone();
            self.rng.shuffle(&mut opens);
            for (i, d) in opens {
                for k in self.candidates(i, d) {
                    let new = self.parts.pool[k].shape.exits().saturating_sub(1);
                    if best.is_none_or(|b| new < b.2) {
                        best = Some(((i, d), k, new));
                    }
                }
                if best.is_some_and(|b| b.2 == 0) {
                    break;
                }
            }
            let Some(((i, d), k, _)) = best else {
                return false;
            };
            if extra >= budget {
                return false;
            }
            let room = self.parts.pool[k].clone();
            self.attach(i, d, room);
            extra += 1;
        }
        true
    }

    /// Pair two opposite open exits (free-graph styles). Loopy prefers the
    /// pair farthest apart.
    fn pair_once(&mut self) -> bool {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for a in 0..self.open.len() {
            for b in 0..self.open.len() {
                let (ia, da) = self.open[a];
                let (ib, db) = self.open[b];
                if ia == ib {
                    continue;
                }
                let ok = match da {
                    Dir::Right => db == Dir::Left,
                    Dir::Up => db == Dir::Down,
                    Dir::Drop => false,
                    _ => false,
                };
                if ok {
                    pairs.push((a, b));
                }
            }
        }
        // Drops: any landing room already placed.
        if pairs.is_empty() {
            if let Some(k) = self.open.iter().position(|&(_, d)| d == Dir::Drop) {
                let (i, _) = self.open[k];
                let zones: Vec<Ix> = (0..self.l.rooms.len())
                    .filter(|&j| j != i && self.l.rooms[j].room.shape.zone.is_some())
                    .collect();
                if let Some(&z) = self.rng.pick(&zones) {
                    self.l.rooms[i].drop = Some(z);
                    self.open.remove(k);
                    return true;
                }
            }
            return false;
        }
        let (a, b) = if self.p.style == Style::Loopy {
            let d = self.l.distances();
            let far = |&(a, b): &(usize, usize)| {
                let x = d[self.open[a].0];
                let y = d[self.open[b].0];
                x.abs_diff(y)
            };
            let best = pairs.iter().map(far).max().unwrap_or(0);
            let top: Vec<(usize, usize)> = pairs.into_iter().filter(|p| far(p) == best).collect();
            *self.rng.pick(&top).unwrap_or(&(0, 0))
        } else {
            *self.rng.pick(&pairs).unwrap_or(&(0, 0))
        };
        let (ia, da) = self.open[a];
        let (ib, _) = self.open[b];
        self.l.link(ia, da, ib);
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        self.open.remove(hi);
        self.open.remove(lo);
        true
    }
}

/// Grow one palace; `None` when this attempt got stuck (call again).
pub fn grow(params: &GrowParams, parts: &Parts, rng: &mut Rng) -> Option<Layout> {
    grow_why(params, parts, rng).ok()
}

/// [`grow`] with the reason it got stuck.
pub fn grow_why(params: &GrowParams, parts: &Parts, rng: &mut Rng) -> Result<Layout, &'static str> {
    let mut b = Builder {
        p: params,
        parts,
        rng,
        l: Layout {
            palace: params.palace,
            rooms: Vec::new(),
            style: params.style.name(),
            boss_continues: params.boss_continues,
        },
        open: Vec::new(),
        grid: BTreeMap::new(),
        used_layouts: BTreeSet::new(),
        used_content: BTreeSet::new(),
        closing: false,
    };
    let mut entrance = Placed::new(parts.entrance.clone());
    if params.style.grid() {
        entrance.pos = Some((0, 0));
        b.grid.insert((0, 0), 0);
    }
    b.l.rooms.push(entrance);
    for e in exits_of(&parts.entrance.shape) {
        if e != Dir::Left {
            b.open.push((0, e));
        }
    }
    let specials = 1 + parts.items.len() + usize::from(parts.thunderbird.is_some());
    let mut normals = params.target.saturating_sub(1 + specials).max(1);
    if params.style == Style::Mirror {
        // The right half is grown; the left half is its mirror image.
        normals = (normals / 2).max(1);
    }
    let gate = params.thunderbird_gate && parts.thunderbird.is_some();
    // The rooms past the boss (when it continues) or past Thunderbird form
    // their own section, so the gate can only be passed one way.
    let second = if gate || params.boss_continues {
        normals / 2
    } else {
        0
    };
    let first = normals - second;
    let items_first = if gate {
        parts.items.len()
    } else {
        parts.items.len().div_ceil(2)
    };
    let tb_here = usize::from(parts.thunderbird.is_some() && !gate);
    let keep_one = if gate {
        1 + items_first
    } else {
        1 + items_first + tb_here + usize::from(second == 0) * (parts.items.len() - items_first)
    };
    // Section one.
    if !b.grow_normals(first, keep_one) {
        return Err("grow section one");
    }
    for it in parts.items.iter().take(items_first) {
        b.place_special(it.clone()).ok_or("place item")?;
    }
    let gate_room = if gate {
        parts.thunderbird.clone()
    } else {
        None
    };
    if let Some(tb) = gate_room {
        let t = b.place_special(tb).ok_or("place thunderbird")?;
        // Close section one, keeping the gate's far side for later.
        let keep: Vec<(Ix, Dir)> = b.open.iter().copied().filter(|&(i, _)| i == t).collect();
        b.open.retain(|&(i, _)| i != t);
        if !b.close(8) {
            return Err("close section one");
        }
        b.open = keep;
        if !b.grow_normals(second, 1 + parts.items.len() - items_first) {
            return Err("grow section two");
        }
        for it in parts.items.iter().skip(items_first) {
            b.place_special(it.clone()).ok_or("place item")?;
        }
        b.place_special(parts.boss.clone()).ok_or("place boss")?;
        if !b.close(8) {
            return Err("close section two");
        }
    } else {
        if let Some(tb) = parts.thunderbird.clone() {
            b.place_special(tb).ok_or("place thunderbird")?;
        }
        let mut boss_room = parts.boss.clone();
        if params.boss_continues {
            boss_room.shape.right = true;
        }
        let boss = b.place_special(boss_room).ok_or("place boss")?;
        if params.boss_continues {
            let keep: Vec<(Ix, Dir)> = b.open.iter().copied().filter(|&(i, _)| i == boss).collect();
            b.open.retain(|&(i, _)| i != boss);
            if !b.close(8) {
                return Err("close before boss");
            }
            b.open = keep;
            if !b.grow_normals(second.max(1), 1 + parts.items.len() - items_first) {
                return Err("grow past boss");
            }
            for it in parts.items.iter().skip(items_first) {
                b.place_special(it.clone()).ok_or("place item")?;
            }
        } else {
            for it in parts.items.iter().skip(items_first) {
                b.place_special(it.clone()).ok_or("place item")?;
            }
        }
        if !b.close(8) {
            return Err("close");
        }
    }
    let mut l = b.l;
    if params.style == Style::Mirror {
        mirror(&mut l, parts);
    }
    if params.style == Style::Chaos {
        chaos(&mut l, &mut *b.rng);
    }
    Ok(l)
}

/// Mirror a palace grown on the right half (x >= 0) onto the left half:
/// every room right of the centre gets a twin whose connections are the
/// mirror image (a pool room with the swapped shape; special rooms get a
/// normal twin).
fn mirror(l: &mut Layout, parts: &Parts) {
    let n = l.rooms.len();
    let mut twin: BTreeMap<Ix, Ix> = BTreeMap::new();
    for i in 0..n {
        let Some((x, y)) = l.rooms[i].pos else {
            continue;
        };
        if x <= 0 {
            continue;
        }
        let want = l.rooms[i].room.shape.mirrored();
        let pick = parts
            .pool
            .iter()
            .find(|r| r.shape.class() == want.class())
            .cloned();
        let Some(r) = pick else { continue };
        let mut pl = Placed::new(r);
        pl.pos = Some((-x, y));
        twin.insert(i, l.rooms.len());
        l.rooms.push(pl);
    }
    for (&i, &t) in &twin {
        let src = l.rooms[i].clone();
        for (d, nb) in src.neighbours() {
            let md = match d {
                Dir::Left => Dir::Right,
                Dir::Right => Dir::Left,
                o => o,
            };
            let target = if let Some(&tn) = twin.get(&nb) {
                tn
            } else if l.rooms[nb].pos.is_some_and(|p| p.0 == 0) {
                nb
            } else {
                continue;
            };
            if md == Dir::Drop {
                l.rooms[t].drop = Some(target);
            } else if l.rooms[t].has_exit(md) && l.rooms[t].get(md).is_none() {
                l.link(t, md, target);
            }
        }
    }
}

/// Re-point some side passages one way: walking out of a room no longer
/// necessarily brings you back where you came from. A change that would
/// strand a room is undone.
fn chaos(l: &mut Layout, rng: &mut Rng) {
    let rights: Vec<Ix> = (0..l.rooms.len())
        .filter(|&i| l.rooms[i].right.is_some() && l.rooms[i].room.role == Role::Normal)
        .collect();
    let lefts: Vec<Ix> = (0..l.rooms.len())
        .filter(|&i| l.rooms[i].room.shape.left && l.rooms[i].room.role == Role::Normal)
        .collect();
    let swaps = rights.len() / 3;
    for _ in 0..swaps {
        let (Some(&a), Some(&b)) = (rng.pick(&rights), rng.pick(&lefts)) else {
            return;
        };
        if a == b {
            continue;
        }
        let old = l.rooms[a].right;
        l.rooms[a].right = Some(b);
        let all = l.reachable_from(l.entrance(), None).iter().all(|&x| x);
        if !all || !strongly_connected(l) {
            l.rooms[a].right = old;
        }
    }
}

/// Every room can get back to the entrance (no one-way trap), the check
/// Chaos palaces need on top of the usual ones.
#[must_use]
pub fn strongly_connected(l: &Layout) -> bool {
    let e = l.entrance();
    (0..l.rooms.len()).all(|i| l.reachable_from(i, None)[e])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palaces::layout::tests::{lr, only, room};
    use crate::palaces::layout::{validate, DropRule, Rules};

    fn parts() -> Parts {
        let mut pool = Vec::new();
        let shapes = [
            lr(),
            lr(),
            lr(),
            only(Dir::Left),
            only(Dir::Right),
            Shape {
                left: true,
                right: true,
                down: true,
                ..Shape::default()
            },
            Shape {
                left: true,
                right: true,
                up: true,
                ..Shape::default()
            },
            Shape {
                up: true,
                right: true,
                ..Shape::default()
            },
            Shape {
                up: true,
                left: true,
                ..Shape::default()
            },
            Shape {
                up: true,
                down: true,
                ..Shape::default()
            },
            Shape {
                down: true,
                left: true,
                ..Shape::default()
            },
            Shape {
                down: true,
                right: true,
                ..Shape::default()
            },
        ];
        for (i, s) in shapes.iter().enumerate() {
            pool.push(room(10 + i as u8, Role::Normal, *s));
        }
        Parts {
            entrance: room(0, Role::Entrance, only(Dir::Down)),
            boss: room(1, Role::Boss, only(Dir::Left)),
            items: vec![room(2, Role::Item, only(Dir::Left))],
            thunderbird: None,
            pool,
        }
    }

    fn rules() -> Rules {
        Rules {
            require_thunderbird: true,
            boss_min_distance: 0,
            drops: DropRule::EntranceOrBoss,
        }
    }

    #[test]
    fn every_style_grows_valid_palaces() {
        let parts = parts();
        for style in [
            Style::Reconstructed,
            Style::Loopy,
            Style::Chaos,
            Style::RandomWalk,
            Style::VanillaWeighted,
            Style::Sequential,
            Style::Tower,
            Style::Mirror,
        ] {
            let mut ok = 0;
            for seed in 0..40 {
                let mut rng = Rng::new(seed);
                let p = GrowParams {
                    palace: 3,
                    style,
                    target: 16,
                    boss_continues: seed % 2 == 0 && style != Style::Mirror,
                    thunderbird_gate: false,
                    no_dup_layout: false,
                    no_dup_content: false,
                };
                match grow_why(&p, &parts, &mut rng) {
                    Ok(l) => match validate(&l, &rules()) {
                        Ok(()) if style != Style::Chaos || strongly_connected(&l) => {
                            ok += 1;
                            assert!(l.find(Role::Item).is_some());
                            continue;
                        }
                        r => eprintln!("{style:?} seed {seed}: {r:?}"),
                    },
                    Err(why) => eprintln!("{style:?} seed {seed}: stuck {why}"),
                }
            }
            eprintln!("{style:?}: {ok}/40");
            assert!(ok > 0, "{style:?} never produced a valid palace");
        }
    }

    #[test]
    fn splice_straight_rooms() {
        let mut l = Layout {
            palace: 1,
            rooms: vec![
                Placed::new(room(0, Role::Entrance, only(Dir::Right))),
                Placed::new(room(1, Role::Normal, lr())),
                Placed::new(room(2, Role::Boss, only(Dir::Left))),
            ],
            style: "t",
            boss_continues: false,
        };
        l.rooms[0].room.shape.right = true;
        l.link(0, Dir::Right, 1);
        l.link(1, Dir::Right, 2);
        assert!(splice(&mut l, 1));
        assert_eq!(l.rooms.len(), 2);
        assert_eq!(l.rooms[0].right, Some(1));
        assert_eq!(l.rooms[1].left, Some(0));
    }
}
