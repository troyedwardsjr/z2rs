//! Continent connections: which continents the four connector types (the
//! bridge, slot 40; the raft, 41; and the two caves, 42 and 43) join.
//!
//! # What the game can do
//!
//! Entering a connector loads the region in its location's world bits
//! (`$CB91`: region = byte 3 & 3) and puts Link on the same slot of the new
//! continent's table. Region 1 is Death Mountain when coming from West
//! Hyrule and Maze Island otherwise (`$070A`, the region left). So, without
//! changing that code, the possible links are West - Death Mountain,
//! West - East and East - Maze Island (Death Mountain to Maze Island would
//! work one way only, so it is not used).
//!
//! The raft starts its ride only on the two dock tiles bank 0 lists
//! (`$8528`/`$852A`); dock 0 sails east, dock 1 west. The dock coordinates
//! are rewritten wherever the raft ends up ([`super::write`]), and a raft
//! continent keeps its dock on the matching coast.
//!
//! # Modes
//!
//! * Normal: the vanilla links.
//! * Transportation shuffle: the same continent pairs, with the connector
//!   types dealt out at random.
//! * Anything goes: each connector type joins a random allowed pair; all
//!   three pairs appear at least once.
//!
//! A continent with a vanilla map (vanilla or vanilla-shuffled terrain)
//! keeps its vanilla connectors: links that touch it keep their type and
//! ends.

use super::continent::Continent;
use super::loc;
use super::map::Cont;
use super::terrain::Terrain;
use crate::flags::ContinentConnections;
use crate::rng::Rng;
use crate::world::{Continent as WCont, ItemId, Link, Requirement, Spot, SpotKind, World};

/// For connector slot `40 + k`: the continents it joins. For the raft the
/// first end is dock 0 (sails east).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Links {
    /// Ends per connector type.
    pub ends: [(Cont, Cont); 4],
}

/// The vanilla links.
pub const NORMAL: Links = Links {
    ends: [
        (Cont::Maze, Cont::East),
        (Cont::West, Cont::East),
        (Cont::West, Cont::DeathMountain),
        (Cont::West, Cont::DeathMountain),
    ],
};

/// Pairs the game's transition code supports (raft dock 0 first).
const PAIRS: [(Cont, Cont); 3] = [
    (Cont::West, Cont::DeathMountain),
    (Cont::West, Cont::East),
    (Cont::Maze, Cont::East),
];

fn joins_all(l: &Links) -> bool {
    let has = |p: (Cont, Cont)| l.ends.iter().any(|&e| e == p || (e.1, e.0) == p);
    PAIRS.iter().all(|&p| has(p))
}

/// Decide the links. `vanilla_map(c)` says whether a continent keeps the
/// vanilla terrain (its connectors cannot move).
pub fn assign(
    mode: ContinentConnections,
    rng: &mut Rng,
    vanilla_map: impl Fn(Cont) -> bool,
) -> Links {
    let pinned = |k: usize| {
        let (a, b) = NORMAL.ends[k];
        vanilla_map(a) || vanilla_map(b)
    };
    match mode {
        ContinentConnections::Normal => NORMAL,
        ContinentConnections::TransportationShuffle => {
            // The vanilla pair of each type; free types are dealt over the
            // free pairs.
            let free: Vec<usize> = (0..4).filter(|&k| !pinned(k)).collect();
            let mut types = free.clone();
            rng.shuffle(&mut types);
            let mut l = NORMAL;
            for (i, &k) in free.iter().enumerate() {
                l.ends[types[i]] = NORMAL.ends[k];
            }
            l
        }
        ContinentConnections::AnythingGoes => {
            for _ in 0..200 {
                let mut l = NORMAL;
                for k in 0..4 {
                    if pinned(k) {
                        continue;
                    }
                    let choices: Vec<(Cont, Cont)> = PAIRS
                        .iter()
                        .copied()
                        .filter(|&(a, b)| !vanilla_map(a) && !vanilla_map(b))
                        .collect();
                    if let Some(&p) = rng.pick(&choices) {
                        l.ends[k] = p;
                    }
                }
                if joins_all(&l) {
                    return l;
                }
            }
            NORMAL
        }
    }
}

/// Region number in a location's world bits.
fn region(c: Cont) -> u8 {
    match c {
        Cont::West => 0,
        Cont::DeathMountain | Cont::Maze => 1,
        Cont::East => 2,
    }
}

/// Set up the connector slots of every continent that does not keep its
/// vanilla map. Call before generating.
pub fn apply(conts: &mut [Continent], links: &Links, vanilla_map: impl Fn(Cont) -> bool) {
    for c in conts.iter_mut() {
        if vanilla_map(c.cont) {
            continue;
        }
        c.dock = None;
        for k in 0..4 {
            let slot = loc::BRIDGE + k;
            let (a, b) = links.ends[k];
            let dest = if a == c.cont {
                Some(b)
            } else if b == c.cont {
                Some(a)
            } else {
                None
            };
            c.used[slot] = dest.is_some();
            c.on_map[slot] = false;
            let Some(dest) = dest else { continue };
            let l = &mut c.locs[slot];
            l.external = true;
            l.entrance = 0;
            l.map = slot as u8;
            l.page = 0;
            l.world = region(dest);
            l.right = false;
            l.hole = false;
            l.pass = slot == loc::BRIDGE;
            c.icon[slot] = if slot >= loc::CAVE1 {
                Terrain::Cave
            } else {
                Terrain::Bridge
            };
            if slot == loc::RAFT {
                c.dock = Some(if a == c.cont { 0 } else { 1 });
            }
            c.table_changed = true;
        }
    }
}

fn wcont(c: Cont) -> WCont {
    WCont::ALL[c.index()]
}

/// Rebuild the logic's connector spots and links for `links`.
pub fn sync(w: &mut World, conts: &[Continent], links: &Links) {
    if !w.built {
        return;
    }
    let is_conn = |w: &World, i: usize| (40..=43).contains(&w.spots[i].slot);
    let old: Vec<Link> = std::mem::take(&mut w.links);
    w.links = old
        .into_iter()
        .filter(|l| !(is_conn(w, l.a) && is_conn(w, l.b)))
        .collect();
    for k in 0..4 {
        let slot = (loc::BRIDGE + k) as u8;
        let (a, b) = links.ends[k];
        let mut ends = [0usize; 2];
        for (j, (c, other)) in [(a, b), (b, a)].into_iter().enumerate() {
            let cc = &conts[c.index()];
            let i = match w.spot_index(wcont(c), slot) {
                Some(i) => i,
                None => {
                    w.spots.push(Spot {
                        continent: wcont(c),
                        slot,
                        name: format!(
                            "{} {}",
                            c.name(),
                            loc::info(c, usize::from(slot)).map_or("connector", |s| s.name)
                        ),
                        x: 0,
                        row: 0,
                        on_map: false,
                        hidden: false,
                        kind: SpotKind::Area,
                        access: Requirement::none(),
                        required: false,
                        raw: [0; 4],
                    });
                    w.spots.len() - 1
                }
            };
            let sp = &mut w.spots[i];
            sp.kind = SpotKind::Connector(wcont(other));
            if let Some((r, col)) = cc.pos(usize::from(slot)) {
                sp.x = col as u8;
                sp.row = r as u8;
                sp.on_map = true;
            } else {
                sp.on_map = false;
            }
            sp.raw = cc.locs[usize::from(slot)].to_bytes();
            ends[j] = i;
        }
        w.links.push(Link {
            a: ends[0],
            b: ends[1],
            req: if usize::from(slot) == loc::RAFT {
                Requirement::item(ItemId::Raft)
            } else {
                Requirement::none()
            },
            one_way: false,
        });
    }
    // Connector spots no link uses are off the map.
    for i in 0..w.spots.len() {
        let sp = &w.spots[i];
        if !(40..=43).contains(&sp.slot) {
            continue;
        }
        let c = Cont::from_index(sp.continent.index());
        let k = usize::from(sp.slot) - loc::BRIDGE;
        let (a, b) = links.ends[k];
        if a != c && b != c {
            w.spots[i].on_map = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_keep_every_continent_joined() {
        for seed in 0..50 {
            let mut rng = Rng::new(seed);
            for mode in [
                ContinentConnections::Normal,
                ContinentConnections::TransportationShuffle,
                ContinentConnections::AnythingGoes,
            ] {
                let l = assign(mode, &mut rng, |_| false);
                assert!(joins_all(&l), "{mode:?} {l:?}");
                for &(a, b) in &l.ends {
                    assert!(PAIRS.contains(&(a, b)), "{mode:?} {l:?}");
                }
            }
        }
    }

    #[test]
    fn vanilla_maps_pin_their_links() {
        let mut rng = Rng::new(1);
        for _ in 0..30 {
            let l = assign(ContinentConnections::AnythingGoes, &mut rng, |c| {
                c == Cont::West
            });
            assert_eq!(l.ends[1], NORMAL.ends[1]);
            assert_eq!(l.ends[2], NORMAL.ends[2]);
            assert_eq!(l.ends[3], NORMAL.ends[3]);
            let l = assign(ContinentConnections::TransportationShuffle, &mut rng, |c| {
                c == Cont::East
            });
            assert_eq!(l.ends[0], NORMAL.ends[0]);
            assert_eq!(l.ends[1], NORMAL.ends[1]);
        }
    }
}
