//! Writing palace layouts back into the game's tables.
//!
//! Per group: map numbers are assigned (each palace keeps its vanilla
//! entrance and boss map, so the overworld and any code that knows those
//! numbers stay right), then every map slot gets its sideview pointer, enemy
//! pointer, four connection bytes and "item still here" nibble. Rooms keep
//! pointing at their vanilla room data; only changed or duplicated layouts
//! (an item room used twice, a boss room that continues) get new bytes,
//! placed in unused space of the group's bank.

use std::collections::BTreeMap;

use super::layout::{Dir, Layout};
use super::rooms::{Group, Role, ITEM_BITS_BANK, MAPS};
use crate::rom::Rom;
use crate::sideview::{self, Sideview};
use crate::RandoError;

/// Unused, `$FF`-filled ranges `[start, end)` in the palace banks that only
/// this module uses (the `palaces` entries of
/// [`crate::rom::FREE_SPACE_REGISTRY`]). Bank 4 holds palaces 1-6, bank 5
/// the Great Palace.
#[must_use]
pub fn free_ranges() -> Vec<(u8, u16, u16)> {
    crate::rom::claimed_ranges(crate::rom::Owner::Module("palaces"))
        .into_iter()
        .map(|(b, s, e)| (b, s, e as u16))
        .collect()
}

/// The palace banks' free space (first fit).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Space {
    ranges: Vec<(u8, u16, u16)>,
    /// Every range handed out, for the record.
    pub used: Vec<(u8, u16, u16)>,
}

impl Space {
    /// The ranges of [`free_ranges`] that are still `$FF`-filled in `rom`
    /// (a range another change already wrote into is left alone).
    #[must_use]
    pub fn from_rom(rom: &Rom) -> Space {
        let mut ranges = Vec::new();
        for (b, s, e) in free_ranges() {
            let clean = (s..e).all(|a| rom.read_cpu(b, a).ok() == Some(0xFF));
            if clean {
                ranges.push((b, s, e));
            }
        }
        Space {
            ranges,
            used: Vec::new(),
        }
    }

    /// Free bytes left in `bank`.
    #[must_use]
    pub fn free_in(&self, bank: u8) -> usize {
        self.ranges
            .iter()
            .filter(|r| r.0 == bank)
            .map(|r| usize::from(r.2 - r.1))
            .sum()
    }

    /// Allocate `len` bytes in `bank`.
    pub fn alloc(&mut self, bank: u8, len: usize) -> Option<u16> {
        let len = u16::try_from(len).ok()?;
        let i = self
            .ranges
            .iter()
            .position(|&(b, s, e)| b == bank && e - s >= len)?;
        let (b, s, e) = self.ranges[i];
        if e - s == len {
            self.ranges.remove(i);
        } else {
            self.ranges[i].1 = s + len;
        }
        self.used.push((b, s, s + len));
        Some(s)
    }
}

/// What the writer reports per placed room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenRoom {
    /// Palace (1-7).
    pub palace: u8,
    /// Index in the palace layout.
    pub index: usize,
    /// Map number.
    pub map: u8,
    /// Final sideview pointer.
    pub sideview: u16,
    /// Headerless PRG offset of the first collectable's item byte, if the
    /// room has one (item rooms: the item the player gets).
    pub item_offset: Option<usize>,
}

/// Assign map numbers to every room of `layouts` (all in `group`). Rooms
/// that already have a map keep it; each palace's entrance and boss get
/// their vanilla numbers; everything else takes the lowest free number.
pub fn assign_maps(group: Group, layouts: &mut [&mut Layout]) -> Result<(), RandoError> {
    let mut taken = [false; MAPS];
    for &m in group.reserved_maps() {
        taken[usize::from(m)] = true;
    }
    let claim = |m: u8, taken: &mut [bool; MAPS]| -> Result<(), RandoError> {
        let i = usize::from(m);
        if i >= MAPS || taken[i] {
            return Err(RandoError::Retry(format!("{group:?} map {m} used twice")));
        }
        taken[i] = true;
        Ok(())
    };
    for l in layouts.iter_mut() {
        let p = l.palace;
        for r in &mut l.rooms {
            if r.map.is_none() {
                r.map = match r.room.role {
                    Role::Entrance => Some(super::rooms::entrance_map(p)),
                    Role::Boss => Some(super::rooms::boss_map(p)),
                    _ => None,
                };
            }
        }
        for r in &l.rooms {
            if let Some(m) = r.map {
                claim(m, &mut taken)?;
            }
        }
    }
    let mut next = 0usize;
    for l in layouts.iter_mut() {
        for r in &mut l.rooms {
            if r.map.is_some() {
                continue;
            }
            while next < MAPS && taken[next] {
                next += 1;
            }
            if next >= MAPS {
                return Err(RandoError::Retry(format!(
                    "{group:?}: more than {MAPS} rooms"
                )));
            }
            taken[next] = true;
            r.map = Some(next as u8);
        }
    }
    Ok(())
}

/// Connection bytes of room `i` of `l` (maps must be assigned).
#[must_use]
pub fn connection_bytes(l: &Layout, i: usize) -> [u8; 4] {
    let p = &l.rooms[i];
    let mut c = p.room.conn;
    let map = |j: usize| l.rooms[j].map.unwrap_or(0);
    if let Some(j) = p.left {
        let pages = l.rooms[j].room.pages.max(1);
        c[0] = (map(j) << 2) | (pages - 1);
    }
    if let Some(j) = p.right {
        let pages = p.room.pages.clamp(1, 4);
        c[usize::from(pages - 1)] = map(j) << 2;
        if pages < 4 {
            c[3] = 0xFF;
        }
    }
    if let Some(j) = p.down {
        c[1] = (map(j) << 2) | l.rooms[j].room.elevator.unwrap_or(0);
    }
    if let Some(j) = p.up {
        c[2] = (map(j) << 2) | l.rooms[j].room.elevator.unwrap_or(0);
    }
    if let Some(j) = p.drop {
        let land = l.rooms[j].room.shape.zone.unwrap_or(0);
        let v = (map(j) << 2) | land;
        for (b, byte) in c.iter_mut().enumerate() {
            if p.room.drop_bytes & (1 << b) != 0 {
                *byte = v;
            }
        }
    }
    c
}

/// Write every layout of `group` into `rom`. Map numbers must be assigned.
pub fn write_group(
    rom: &mut Rom,
    group: Group,
    layouts: &[&Layout],
    space: &mut Space,
) -> Result<Vec<WrittenRoom>, RandoError> {
    let bank = group.bank();
    let mut out = Vec::new();
    // Sideview blobs: dedupe new layouts by content (never for item rooms,
    // whose item byte must stay theirs).
    let mut blobs: BTreeMap<Vec<u8>, u16> = BTreeMap::new();
    let mut item_ptrs: Vec<u16> = Vec::new();
    for l in layouts {
        for (i, p) in l.rooms.iter().enumerate() {
            let map = p
                .map
                .ok_or_else(|| RandoError::Other("map not assigned".into()))?;
            let is_item = p.room.role == Role::Item;
            let mut bytes = p.sideview.clone();
            if is_item && bytes.is_none() && item_ptrs.contains(&p.room.sideview) {
                // A second room with the same item layout needs its own copy.
                let (_, raw) = sideview::read_at(rom, bank, p.room.sideview)?;
                bytes = Some(raw);
            }
            let ptr = match bytes {
                None => p.room.sideview,
                Some(b) => {
                    if let Some(&a) = blobs.get(&b).filter(|_| !is_item) {
                        a
                    } else {
                        let a = space.alloc(bank, b.len()).ok_or_else(|| {
                            RandoError::Retry(format!(
                                "no room for {} more bytes of palace layout in bank {bank}",
                                b.len()
                            ))
                        })?;
                        rom.write_cpu(bank, a, &b)?;
                        if !is_item {
                            blobs.insert(b, a);
                        }
                        a
                    }
                }
            };
            if is_item {
                item_ptrs.push(ptr);
            }
            let m16 = u16::from(map);
            rom.write_cpu_word(bank, group.sideview_table() + 2 * m16, ptr)?;
            rom.write_cpu_word(bank, group.enemy_table() + 2 * m16, p.room.enemies)?;
            rom.write_cpu(
                bank,
                group.connection_table() + 4 * m16,
                &connection_bytes(l, i),
            )?;
            let a = group.item_bits() + m16 / 2;
            let old = rom.read_cpu(ITEM_BITS_BANK, a)?;
            let nib = p.room.nibble & 0x0F;
            let new = if map % 2 == 0 {
                (old & 0x0F) | (nib << 4)
            } else {
                (old & 0xF0) | nib
            };
            rom.write_cpu(ITEM_BITS_BANK, a, &[new])?;
            let item_offset = first_item_offset(rom, bank, ptr)?;
            out.push(WrittenRoom {
                palace: l.palace,
                index: i,
                map,
                sideview: ptr,
                item_offset,
            });
        }
    }
    Ok(out)
}

/// Headerless offset of the item byte of the first collectable in the
/// sideview at `(bank, ptr)`.
pub fn first_item_offset(rom: &Rom, bank: u8, ptr: u16) -> Result<Option<usize>, RandoError> {
    let b = if ptr >= 0xC000 { 7 } else { bank };
    let start = rom.cpu_offset(b, ptr)?;
    let len = usize::from(rom.read(start)?);
    let raw = rom.read_slice(start, len.max(1))?;
    let (sv, offs) = Sideview::parse_with_offsets(raw)?;
    let first = sv.collectables().next().map(|(i, _)| start + offs[i] + 2);
    Ok(first)
}

/// Does `d` use a connection byte in a room of `pages` pages (for
/// validating room-pack rooms)?
#[must_use]
pub fn byte_for(d: Dir, pages: u8) -> usize {
    match d {
        Dir::Left => 0,
        Dir::Right => usize::from(pages.clamp(1, 4) - 1),
        Dir::Down => 1,
        Dir::Up => 2,
        Dir::Drop => 0,
    }
}
