//! Mirror the generated overworld into the shared logic model
//! ([`crate::world::World`]): maps, spot positions, hidden spots, palace
//! identities and the requirements that depend on where things ended up.

use super::continent::{Continent, Reveal};
use super::map::{Cont, MAP_ROWS, MAP_W};
use crate::world::{
    Continent as WCont, ItemId, OverworldMap, Requirement, SpotKind, Terrain as WTerrain, Token,
    World,
};

fn wcont(c: Cont) -> WCont {
    WCont::ALL[c.index()]
}

/// Copy `conts` into `w`.
pub fn sync(w: &mut World, conts: &[Continent]) {
    if !w.built {
        return;
    }
    // Maps.
    while w.maps.len() < 4 {
        w.maps.push(OverworldMap::default());
    }
    for c in conts {
        let mut tiles = Vec::with_capacity(MAP_W * MAP_ROWS);
        for r in 0..MAP_ROWS {
            for &t in c.grid.row(r) {
                tiles.push(WTerrain::from_code(t.code()));
            }
        }
        w.maps[c.cont.index()] = OverworldMap {
            cols: MAP_W,
            rows: MAP_ROWS,
            tiles,
        };
    }
    // Spots.
    for c in conts {
        let wc = wcont(c.cont);
        for i in 0..w.spots.len() {
            if w.spots[i].continent != wc {
                continue;
            }
            let slot = usize::from(w.spots[i].slot);
            if slot >= c.locs.len() {
                continue;
            }
            let sp = &mut w.spots[i];
            let pos = c.pos(slot);
            sp.on_map = pos.is_some();
            if let Some((r, col)) = pos {
                sp.x = col as u8;
                sp.row = r as u8;
            }
            sp.raw = c.locs[slot].to_bytes();
            // Hidden spots: swap the reveal requirement.
            if sp.hidden {
                sp.access = sp.access.without(&[ItemId::Flute, ItemId::Hammer]);
            }
            sp.hidden = c.hidden[slot].is_some();
            match c.hidden[slot] {
                Some(Reveal::Flute) => {
                    sp.access = sp.access.with_hard(Token::Item(ItemId::Flute));
                }
                Some(Reveal::Hammer) => {
                    sp.access = sp.access.with_hard(Token::Item(ItemId::Hammer));
                }
                None => {}
            }
        }
        if c.cont == Cont::East {
            // The swamp and desert tiles' scenes need a spell to cross when
            // they pass through.
            for slot in [21u8, 23] {
                if let Some(i) = w.spot_index(wc, slot) {
                    let base = w.spots[i].access.without(&[ItemId::Jump, ItemId::Fairy]);
                    w.spots[i].access = if c.locs[usize::from(slot)].pass {
                        base.and(&Requirement::any(&[ItemId::Jump, ItemId::Fairy]))
                    } else {
                        base
                    };
                }
            }
        }
    }
    // Palace identities: permute with swaps until every palace spot shows
    // the palace our tables say it enters.
    let mut want: Vec<(usize, u8)> = Vec::new();
    for c in conts {
        for slot in 0..c.locs.len() {
            if let Some(n) = c.palace[slot] {
                if let Some(i) = w.spot_index(wcont(c.cont), slot as u8) {
                    want.push((i, n));
                }
            }
        }
    }
    for &(i, n) in &want {
        if w.spots[i].kind == SpotKind::Palace(n) {
            continue;
        }
        if let Some(j) = w.palace_spot(n) {
            w.swap_palace_spots(i, j);
        }
    }
}
