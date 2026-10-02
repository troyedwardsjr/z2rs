//! A rough model of how Link moves through a room, used to tell which
//! rooms ask for nothing at all.
//!
//! Link stands on a cell when it and the cell above are free and the cell
//! below is solid. From there he walks (stepping down any height), jumps
//! (a few rows up and a few columns across), rides the elevator up and
//! down its column, or flies anywhere free (fairy). Breakable blocks are
//! solid unless the glove is allowed.
//!
//! The model only ever *lowers* what a room is assumed to need (see
//! [`super::rooms::VanillaPool::read`]): a room whose every exit reaches
//! every other exit without spells counts as needing nothing; anything else
//! keeps its palace's conservative assumption.

use std::collections::VecDeque;

use super::rooms::{Needs, Shape};
use crate::sideview::{Cell, Grid, PAGE_COLS, ROWS};

/// Rows Link clears with a plain jump, and with the jump spell.
const JUMP_ROWS: [usize; 2] = [2, 4];
/// Columns Link clears sideways with a plain jump, and with the spell.
const JUMP_COLS: [usize; 2] = [2, 4];

fn free(g: &Grid, glove: bool, x: usize, y: usize) -> bool {
    match g.cell(x, y) {
        Cell::Empty | Cell::FalseWall => true,
        Cell::Breakable => glove,
        _ => false,
    }
}

fn solid(g: &Grid, glove: bool, x: usize, y: usize) -> bool {
    if y >= ROWS {
        return false;
    }
    !free(g, glove, x, y) && g.cell(x, y) != Cell::Lava
}

/// Standing spots `(x, y)`: row `y` is where Link's feet are.
fn standable(g: &Grid, glove: bool, x: usize, y: usize) -> bool {
    y >= 1
        && y + 1 < ROWS
        && free(g, glove, x, y)
        && free(g, glove, x, y - 1)
        && solid(g, glove, x, y + 1)
}

/// Where Link lands falling from `(x, y)`; `None` = out of the bottom or
/// into lava.
fn fall(g: &Grid, glove: bool, x: usize, mut y: usize) -> Option<usize> {
    while y + 1 < ROWS {
        if solid(g, glove, x, y + 1) {
            return Some(y);
        }
        if g.cell(x, y + 1) == Cell::Lava {
            return None;
        }
        y += 1;
    }
    None
}

/// Everything Link can stand on from `starts`, with these abilities. Also
/// reports whether he can fall out of the bottom.
fn flood(g: &Grid, n: Needs, el: Option<usize>, starts: &[(usize, usize)]) -> (Vec<bool>, bool) {
    let w = g.width();
    let glove = n.has(Needs::GLOVE);
    let jump = usize::from(n.has(Needs::JUMP));
    let fairy = n.has(Needs::FAIRY);
    let mut seen = vec![false; w * ROWS];
    let mut q = VecDeque::new();
    let mut out_bottom = false;
    for &(x, y) in starts {
        if x < w && y < ROWS && !seen[y * w + x] {
            seen[y * w + x] = true;
            q.push_back((x, y));
        }
    }
    while let Some((x, y)) = q.pop_front() {
        let mut next: Vec<(usize, usize)> = Vec::new();
        if fairy {
            // Fly through any free cell (fairy Link is one cell tall).
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx as usize >= w {
                    continue;
                }
                if ny as usize >= ROWS {
                    out_bottom = true;
                    continue;
                }
                if free(g, glove, nx as usize, ny as usize) {
                    next.push((nx as usize, ny as usize));
                }
            }
        } else {
            // Walk one column, then fall.
            for dx in [-1i32, 1] {
                let nx = x as i32 + dx;
                if nx < 0 || nx as usize >= w {
                    continue;
                }
                let nx = nx as usize;
                if free(g, glove, nx, y) && free(g, glove, nx, y.saturating_sub(1)) {
                    match fall(g, glove, nx, y) {
                        Some(ly) => next.push((nx, ly)),
                        None => out_bottom |= g.cell(nx, ROWS - 1) == Cell::Empty,
                    }
                }
            }
            // Jump: up to H rows up, then across up to W columns at the top
            // of the arc, then fall.
            let h = JUMP_ROWS[jump];
            let wc = JUMP_COLS[jump];
            let mut top = y;
            for _ in 0..h {
                if top >= 2 && free(g, glove, x, top - 2) {
                    top -= 1;
                } else {
                    break;
                }
            }
            for dir in [-1i32, 1] {
                for step in 0..=wc as i32 {
                    let nx = x as i32 + dir * step;
                    if nx < 0 || nx as usize >= w {
                        break;
                    }
                    let nx = nx as usize;
                    if !(free(g, glove, nx, top) && free(g, glove, nx, top.saturating_sub(1))) {
                        break;
                    }
                    match fall(g, glove, nx, top) {
                        Some(ly) => next.push((nx, ly)),
                        None => out_bottom |= g.cell(nx, ROWS - 1) == Cell::Empty,
                    }
                }
            }
            // Elevator: anywhere along its two columns.
            if let Some(ex) = el {
                if x == ex || x == ex + 1 {
                    for yy in 1..ROWS - 1 {
                        if standable(g, glove, x, yy) {
                            next.push((x, yy));
                        }
                    }
                }
            }
        }
        for (nx, ny) in next {
            let i = ny * w + nx;
            if !seen[i] {
                seen[i] = true;
                q.push_back((nx, ny));
            }
        }
    }
    (seen, out_bottom)
}

/// The ways in and out of a room, as standing spots.
fn ends(g: &Grid, s: &Shape, el: Option<usize>, fairy: bool) -> Vec<Vec<(usize, usize)>> {
    let w = g.width();
    let mut v: Vec<Vec<(usize, usize)>> = Vec::new();
    let side = |x: usize| -> Vec<(usize, usize)> {
        (1..ROWS - 1)
            .filter(|&y| {
                if fairy {
                    g.cell(x, y) == Cell::Empty
                } else {
                    standable(g, false, x, y)
                }
            })
            .map(|y| (x, y))
            .collect()
    };
    if s.left {
        v.push(side(0));
    }
    if s.right {
        v.push(side(w - 1));
    }
    if s.up || s.down {
        if let Some(ex) = el {
            v.push(
                (1..ROWS - 1)
                    .filter(|&y| standable(g, false, ex, y))
                    .map(|y| (ex, y))
                    .collect(),
            );
        }
    }
    if let Some(page) = s.zone {
        let x = usize::from(page) * PAGE_COLS + PAGE_COLS / 2;
        if x < w {
            if let Some(y) = fall(g, false, x, 1) {
                v.push(vec![(x, y)]);
            }
        }
    }
    v
}

/// Can every way into the room reach every way out with `n`? Drops count
/// as reached when Link can fall out of the bottom.
fn passable(g: &Grid, s: &Shape, el: Option<usize>, n: Needs) -> bool {
    let fairy = n.has(Needs::FAIRY);
    let es = ends(g, s, el, fairy);
    if es.iter().any(Vec::is_empty) {
        return false;
    }
    let w = g.width();
    for a in &es {
        let (seen, out) = flood(g, n, el, a);
        for b in &es {
            if !b.iter().any(|&(x, y)| seen[y * w + x]) {
                return false;
            }
        }
        if s.drop.is_some() && !out {
            return false;
        }
    }
    true
}

/// Whether the room can be crossed between all its exits with no spells
/// and no glove.
#[must_use]
pub fn needs_nothing(g: &Grid, s: &Shape, elevator_page: Option<u8>) -> bool {
    let el = elevator_page.and_then(|p| {
        // The elevator column is the left of its two columns, found on the
        // page; take the grid's own record when there is one.
        g.elevator
            .map(usize::from)
            .or(Some(usize::from(p) * PAGE_COLS + 7))
    });
    passable(g, s, el, Needs::NONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sideview::{Command, FloorTable, ObjectSet, Sideview, ROW_EXTENDED};

    fn floors() -> FloorTable {
        let mut m = [[0u8; 2]; 16];
        m[0] = [0x40, 0x06];
        m[15] = [0x7E, 0x7E];
        FloorTable::from_masks(m)
    }

    fn lr() -> Shape {
        Shape {
            left: true,
            right: true,
            ..Shape::default()
        }
    }

    #[test]
    fn flat_corridor_needs_nothing() {
        let sv = Sideview {
            header: [0, 0x60, 0, 0],
            commands: vec![],
        };
        let g = sv.grid(ObjectSet::Palace, &floors());
        assert!(needs_nothing(&g, &lr(), None));
    }

    #[test]
    fn high_wall_needs_more() {
        // Two solid columns from floor to ceiling.
        let sv = Sideview {
            header: [0, 0x60, 0, 0],
            commands: vec![Command::floor(30, 0x0F), Command::floor(32, 0x00)],
        };
        let g = sv.grid(ObjectSet::Palace, &floors());
        assert!(!needs_nothing(&g, &lr(), None));
        // A 2-high brick step is fine.
        let sv = Sideview {
            header: [0, 0x60, 0, 0],
            commands: vec![Command::new(30, 8, 0x61)],
        };
        let g = sv.grid(ObjectSet::Palace, &floors());
        assert!(g.is_solid(30, 8) && g.is_solid(31, 9));
        assert!(needs_nothing(&g, &lr(), None));
        // A 4-high one is not.
        let sv = Sideview {
            header: [0, 0x60, 0, 0],
            commands: vec![Command::new(30, 6, 0x61), Command::new(30, 8, 0x61)],
        };
        let g = sv.grid(ObjectSet::Palace, &floors());
        assert!(!needs_nothing(&g, &lr(), None));
    }

    #[test]
    fn wide_pit_needs_more() {
        let sv = Sideview {
            header: [0, 0x60, 0, 0],
            commands: vec![Command::new(20, ROW_EXTENDED, 0x17)],
        };
        let g = sv.grid(ObjectSet::Palace, &floors());
        assert!(!needs_nothing(&g, &lr(), None));
        let s = Shape {
            drop: Some(1),
            ..lr()
        };
        assert!(!needs_nothing(&g, &s, None));
    }
}
