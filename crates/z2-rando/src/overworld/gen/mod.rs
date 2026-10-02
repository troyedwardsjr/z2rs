//! Terrain generation for the generated biomes ([`land`] for West and East
//! Hyrule, [`dm`] for Death Mountain, [`maze`] for Maze Island; shared
//! pieces in [`core`]).

pub mod core;
pub mod dm;
pub mod land;
pub mod maze;

use super::continent::Continent;
use super::map::{self, Cont, MAP_ROWS, MAP_W};
use super::terrain::Terrain;
use super::Resolved;
use crate::flags::ContinentSize;
use crate::rng::Rng;
use crate::RandoError;

/// Attempts per continent before the caller starts over.
const TRIES: usize = 400;

/// Tile counts of a grid (vanilla-weighted climates).
fn counts(c: &Continent) -> [usize; 16] {
    let mut n = [0usize; 16];
    for r in 0..c.rows.min(MAP_ROWS) {
        for &t in &c.grid.row(r)[..MAP_W] {
            n[t as usize] += 1;
        }
    }
    n
}

/// Land size (rows, cols).
#[must_use]
pub fn land_dims(s: ContinentSize) -> (usize, usize) {
    match s {
        ContinentSize::Large => (75, 64),
        ContinentSize::Medium => (52, 52),
        ContinentSize::Small => (44, 44),
    }
}

/// Whether a generated continent's map fits the larger copy.
fn fits(c: &Continent) -> bool {
    let singles: Vec<(usize, usize)> = c
        .single_tiles()
        .iter()
        .map(|&(_, r, col)| (r, col))
        .collect();
    map::encode(&c.grid, &singles).bytes.len() <= map::BIG_BUDGET
}

/// Generate one continent in place.
pub fn generate(c: &mut Continent, o: &Resolved, rng: &mut Rng) -> Result<(), RandoError> {
    match c.cont {
        Cont::West | Cont::East => {
            let (ci, si) = if c.cont == Cont::West { (0, 0) } else { (2, 1) };
            let (rows, cols) = land_dims(o.size[si]);
            let water = if o.good_boots {
                Terrain::WalkableWater
            } else {
                Terrain::Water
            };
            let cl = core::climate(o.climate[ci], &counts(c), water, c.cont == Cont::East);
            let mut last = String::new();
            for _ in 0..TRIES {
                match land::generate(c, o, rng, cl.clone(), rows, cols) {
                    Ok(n) => {
                        let singles: Vec<(usize, usize)> =
                            n.single_tiles().iter().map(|&(_, r, c)| (r, c)).collect();
                        let len = map::encode(&n.grid, &singles).bytes.len();
                        if len > map::BIG_BUDGET {
                            last = format!("map too big ({len} bytes)");
                            continue;
                        }
                        *c = n;
                        return Ok(());
                    }
                    Err(RandoError::Retry(m)) => last = m,
                    Err(e) => return Err(e),
                }
            }
            Err(RandoError::Retry(format!("{}: {last}", c.cont.name())))
        }
        Cont::DeathMountain | Cont::Maze => {
            let cl = core::climate(o.climate[1], &counts(c), Terrain::Water, false);
            for _ in 0..20 {
                let n = if c.cont == Cont::Maze {
                    maze::generate(c, o, rng)?
                } else {
                    dm::generate(c, o, &cl, rng)?
                };
                if fits(&n) {
                    *c = n;
                    return Ok(());
                }
            }
            Err(RandoError::Retry(format!("{}: map too big", c.cont.name())))
        }
    }
}
