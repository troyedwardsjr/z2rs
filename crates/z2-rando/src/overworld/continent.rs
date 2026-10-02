//! One continent as the generator works on it: its terrain grid, its
//! location table and what changed, plus loading from and writing back to
//! the ROM.

use super::loc::{self, Class, Loc, SLOTS};
use super::map::{self, Cont, Grid, MAP_ROWS, MAP_W};
use super::terrain::Terrain;
use crate::rom::Rom;
use crate::RandoError;

/// What reveals a hidden location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// Play the flute two tiles above it (the vanilla sixth palace).
    Flute,
    /// Hit it with the hammer (vanilla New Kasuto).
    Hammer,
}

/// Connector slots each continent uses in the vanilla game.
#[must_use]
pub fn vanilla_connectors(cont: Cont) -> &'static [usize] {
    match cont {
        Cont::West => &[loc::RAFT, loc::CAVE1, loc::CAVE2],
        Cont::DeathMountain => &[loc::CAVE1, loc::CAVE2],
        Cont::East => &[loc::BRIDGE, loc::RAFT],
        Cont::Maze => &[loc::BRIDGE],
    }
}

/// A continent being built.
#[derive(Debug, Clone)]
pub struct Continent {
    /// Which one.
    pub cont: Cont,
    /// Terrain (all 75 rows; rows and columns outside the playable area
    /// hold impassable filler).
    pub grid: Grid,
    /// Playable rows (vanilla: 75 or 60).
    pub rows: usize,
    /// Playable columns.
    pub cols: usize,
    /// The table as it will be written.
    pub locs: Vec<Loc>,
    /// The slot belongs to this continent (from the slot catalog, or a
    /// connector this continent uses).
    pub used: Vec<bool>,
    /// The slot is on the map (a used slot that is off the map is written
    /// with a zero row).
    pub on_map: Vec<bool>,
    /// Hidden until revealed.
    pub hidden: Vec<Option<Reveal>>,
    /// The tile a location shows once visible.
    pub icon: Vec<Terrain>,
    /// What the tile of a hidden location shows before the reveal.
    pub cover: Vec<Terrain>,
    /// The terrain changed (the map must be rewritten).
    pub map_changed: bool,
    /// The location table changed.
    pub table_changed: bool,
    /// Raw row where the "south" encounter tables start (`None` keeps the
    /// ROM value).
    pub separator: Option<u8>,
    /// Vanilla map pointer (CPU address in the continent's bank).
    pub vanilla_ptr: u16,
    /// Palace number (1-7) entered at a palace slot.
    pub palace: Vec<Option<u8>>,
    /// Raft dock index when this continent has the raft: 0 sails east
    /// (dock on the east coast), 1 sails west.
    pub dock: Option<u8>,
}

impl Continent {
    /// Load the vanilla continent from `rom`.
    pub fn vanilla(rom: &Rom, cont: Cont) -> Result<Continent, RandoError> {
        let bank = cont.bank();
        let ptr = rom.read_cpu_word(bank, cont.pointer_addr())?;
        let off = rom.cpu_offset(bank, ptr)?;
        let avail = rom.prg().len() - off;
        let bytes = rom.read_slice(off, avail.min(map::BIG_BUDGET))?;
        let filler = if cont == Cont::West || cont == Cont::DeathMountain {
            Terrain::Mountain
        } else {
            Terrain::Water
        };
        let dec = map::decode(bytes, cont.vanilla_rows(), filler)?;
        let locs = loc::read_table(rom, cont)?;
        let mut c = Continent {
            cont,
            grid: dec.grid,
            rows: cont.vanilla_rows(),
            cols: MAP_W,
            locs,
            used: vec![false; SLOTS],
            on_map: vec![false; SLOTS],
            hidden: vec![None; SLOTS],
            icon: vec![Terrain::Grass; SLOTS],
            cover: vec![Terrain::Grass; SLOTS],
            map_changed: false,
            table_changed: false,
            separator: None,
            vanilla_ptr: ptr,
            palace: vec![None; SLOTS],
            dock: match cont {
                Cont::West => Some(0),
                Cont::East => Some(1),
                _ => None,
            },
        };
        for s in loc::slots(cont) {
            let i = usize::from(s.slot);
            if let Class::Palace(n) = s.class {
                c.palace[i] = Some(n);
            }
            if s.class == Class::Connector {
                // Only the connectors the vanilla links use (Death Mountain
                // and Maze Island share one table, so each must ignore the
                // other's).
                c.used[i] = vanilla_connectors(cont).contains(&i);
                c.on_map[i] = c.used[i] && c.locs[i].pos().is_some();
            } else {
                c.used[i] = true;
                c.on_map[i] = c.locs[i].pos().is_some();
            }
            if let Some((r, col)) = c.locs[i].pos() {
                c.icon[i] = c.grid.get(r, col);
                c.cover[i] = c.icon[i];
            }
        }
        if cont == Cont::East {
            // The two hidden locations: their rows come from the reveal
            // data in the fixed bank (`$DF66`/`$DF68`: slot and row).
            for k in 0..2u16 {
                let slot = usize::from(rom.read_cpu(7, 0xDF66 + k)?);
                let y = rom.read_cpu(7, 0xDF68 + k)? & 0x7F;
                if slot >= SLOTS || !c.used[slot] || c.on_map[slot] {
                    continue;
                }
                let mut l = c.locs[slot];
                l.raw_y = y;
                if let Some((r, col)) = l.pos() {
                    c.locs[slot] = l;
                    c.locs[slot].external = rom.read_cpu(7, 0xDF68 + k)? & 0x80 != 0;
                    c.on_map[slot] = true;
                    c.hidden[slot] = Some(if k == 0 {
                        Reveal::Flute
                    } else {
                        Reveal::Hammer
                    });
                    c.cover[slot] = c.grid.get(r, col);
                    c.icon[slot] = Terrain::from_code(rom.read_cpu(7, 0xDF64 + k)?);
                }
            }
        }
        Ok(c)
    }

    /// Slot catalog entry.
    #[must_use]
    pub fn info(&self, slot: usize) -> Option<&'static loc::SlotInfo> {
        loc::info(self.cont, slot)
    }

    /// Internal position of a slot that is on the map.
    #[must_use]
    pub fn pos(&self, slot: usize) -> Option<(usize, usize)> {
        if self.used[slot] && self.on_map[slot] {
            self.locs[slot].pos()
        } else {
            None
        }
    }

    /// Slots on the map.
    #[must_use]
    pub fn placed(&self) -> Vec<usize> {
        (0..SLOTS).filter(|&s| self.pos(s).is_some()).collect()
    }

    /// The slot sitting on (row, col), if any.
    #[must_use]
    pub fn slot_at(&self, r: usize, c: usize) -> Option<usize> {
        (0..SLOTS).find(|&s| self.pos(s) == Some((r, c)))
    }

    /// Take a slot off the map.
    pub fn remove(&mut self, slot: usize) {
        if self.on_map[slot] {
            self.on_map[slot] = false;
            self.hidden[slot] = None;
            self.table_changed = true;
        }
    }

    /// Paint every visible location's icon onto its tile, and every hidden
    /// one's cover.
    pub fn paint_icons(&mut self) {
        for s in self.placed() {
            let (r, c) = self.locs[s].pos().unwrap_or((0, 0));
            let t = if self.hidden[s].is_some() {
                self.cover[s]
            } else {
                self.icon[s]
            };
            if self.grid.get(r, c) != t {
                self.grid.set(r, c, t);
                self.map_changed = true;
            }
        }
    }

    /// Tiles that must be one-tile runs in the encoded map: hidden
    /// locations and palaces (the stone routine rewrites a palace's byte).
    #[must_use]
    pub fn single_tiles(&self) -> Vec<(usize, usize, usize)> {
        let mut v = Vec::new();
        for s in self.placed() {
            let is_palace = matches!(self.info(s).map(|i| i.class), Some(Class::Palace(_)));
            if self.hidden[s].is_some() || is_palace {
                if let Some((r, c)) = self.locs[s].pos() {
                    v.push((s, r, c));
                }
            }
        }
        v
    }

    /// The four table bytes to write for a slot.
    #[must_use]
    pub fn slot_bytes(&self, slot: usize) -> [u8; 4] {
        let mut b = self.locs[slot].to_bytes();
        if !self.on_map[slot] || self.hidden[slot].is_some() {
            b[0] = 0;
        }
        b
    }

    /// Write this continent's table (all slots it uses; with
    /// `clear_others`, every other slot gets a zero row so a shared table
    /// half from the other continent cannot appear on this map).
    pub fn write_table(&self, rom: &mut Rom, clear_others: bool) -> Result<(), RandoError> {
        let vanilla = loc::read_table(rom, self.cont)?;
        for (slot, v) in vanilla.iter().enumerate() {
            // A slot this continent no longer uses (a connector moved to
            // another type) must not stay on the map either.
            let stale = !self.used[slot]
                && (self.info(slot).is_some() || clear_others)
                && v.pos().is_some();
            let b = if self.used[slot] {
                self.slot_bytes(slot)
            } else if stale {
                let mut b = v.to_bytes();
                b[0] = 0;
                b
            } else {
                continue;
            };
            if b != v.to_bytes() {
                loc::write_slot(rom, self.cont, slot, b)?;
            }
        }
        Ok(())
    }

    /// Fill everything outside the playable rectangle with `t`.
    pub fn fill_outside(&mut self, t: Terrain) {
        for r in 0..MAP_ROWS {
            for c in 0..MAP_W {
                if r >= self.rows || c >= self.cols {
                    self.grid.set(r, c, t);
                }
            }
        }
    }
}
