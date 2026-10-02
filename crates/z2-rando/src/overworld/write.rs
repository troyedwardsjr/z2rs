//! Writing the finished overworld back into the ROM.
//!
//! * Maps that changed are re-encoded and stored in free space of their own
//!   bank (`$B470` / `$B9F0`, see [`Cont::new_map_addr`]); the map pointer
//!   is updated. If any map is bigger than the vanilla 896-byte copy, the
//!   loader is switched to the 1408-byte copy ([`map::apply_size_patch`]).
//! * Location tables are rewritten for the continents that changed.
//! * The palace-to-stone tables (banks 1 and 2, `$878F` pointers into the
//!   battery-RAM map copy, `$8797` palace numbers) are recomputed from the
//!   encoded maps, so completed palaces still turn to stone wherever they
//!   are.
//! * The north/south encounter split rows (`$CB32-$CB34`).
//! * East Hyrule's reveal data ([`super::hidden::write`]).

use super::continent::Continent;
use super::hidden;
use super::map::{self, Cont};
use crate::rom::Rom;
use crate::RandoError;

/// Per-continent encoded map.
struct MapOut {
    /// The bytes the loader copies from (new or vanilla).
    bytes: Vec<u8>,
    /// Byte offset of each slot's one-tile run (palaces, hidden spots).
    single_of: Vec<(usize, usize)>,
    /// Rewritten (needs the new pointer).
    rewritten: bool,
}

fn encode_cont(rom: &Rom, c: &Continent) -> Result<MapOut, RandoError> {
    let singles = c.single_tiles();
    if c.map_changed || c.table_changed {
        let pts: Vec<(usize, usize)> = singles.iter().map(|&(_, r, col)| (r, col)).collect();
        let e = map::encode(&c.grid, &pts);
        Ok(MapOut {
            single_of: singles
                .iter()
                .zip(e.singles.iter())
                .map(|(&(s, _, _), &o)| (s, o))
                .collect(),
            bytes: e.bytes,
            rewritten: true,
        })
    } else {
        let off = rom.cpu_offset(c.cont.bank(), c.vanilla_ptr)?;
        let avail = (rom.prg().len() - off).min(map::BIG_BUDGET);
        let raw = rom.read_slice(off, avail)?.to_vec();
        let d = map::decode(&raw, c.cont.vanilla_rows(), super::terrain::Terrain::Water)?;
        let single_of = singles
            .iter()
            .map(|&(s, r, col)| (s, d.run_of[r * map::MAP_W + col]))
            .collect();
        Ok(MapOut {
            bytes: raw[..d.len].to_vec(),
            single_of,
            rewritten: false,
        })
    }
}

/// Write every continent. `conts` is indexed by [`Cont::index`].
pub fn write_all(
    rom: &mut Rom,
    conts: &[Continent],
    legacy: bool,
    east_reveals_changed: bool,
) -> Result<(), RandoError> {
    let outs: Vec<MapOut> = conts
        .iter()
        .map(|c| encode_cont(rom, c))
        .collect::<Result<_, _>>()?;
    for (c, o) in conts.iter().zip(&outs) {
        if o.rewritten && o.bytes.len() > map::BIG_BUDGET {
            return Err(RandoError::Retry(format!(
                "{} map is {} bytes (max {})",
                c.cont.name(),
                o.bytes.len(),
                map::BIG_BUDGET
            )));
        }
    }
    let big = outs
        .iter()
        .any(|o| o.rewritten && o.bytes.len() > map::VANILLA_BUDGET);
    if big {
        map::apply_size_patch(rom)?;
    }
    let window = if big {
        map::BIG_WINDOW
    } else {
        map::VANILLA_WINDOW
    };
    let any_change = outs.iter().any(|o| o.rewritten);

    for (c, o) in conts.iter().zip(&outs) {
        if !o.rewritten {
            continue;
        }
        let addr = c.cont.new_map_addr();
        let mut bytes = o.bytes.clone();
        // Pad to the full copy length with one-tile mountains so the copy
        // never picks up stray bytes as map data.
        bytes.resize(map::BIG_BUDGET, 0x0B);
        rom.write_cpu(c.cont.bank(), addr, &bytes)?;
        rom.write_cpu_word(c.cont.bank(), c.cont.pointer_addr(), addr)?;
    }

    for (c, o) in conts.iter().zip(&outs) {
        if c.table_changed || o.rewritten {
            // Death Mountain and Maze Island share a table layout; a
            // rewritten one must not show the other's entries.
            let clear = matches!(c.cont, Cont::DeathMountain | Cont::Maze) && o.rewritten;
            c.write_table(rom, clear)?;
        }
        if let Some(sep) = c.separator {
            if let Some(i) = [Cont::West, Cont::DeathMountain, Cont::East]
                .iter()
                .position(|&k| k == c.cont)
            {
                rom.write_cpu(7, 0xCB32 + i as u16, &[sep])?;
            }
        }
    }

    if any_change || big || conts.iter().any(|c| c.table_changed) {
        write_stone_tables(rom, conts, &outs, window)?;
    }
    if conts.iter().any(|c| c.table_changed) {
        write_dock_spots(rom, conts)?;
    }
    let east = &conts[Cont::East.index()];
    if east_reveals_changed {
        hidden::write(rom, east, legacy)?;
    }
    Ok(())
}

/// The raft docks and the Maze Island bridge are also known to bank 0 by
/// position: stepping on a raft dock starts the raft ride only on the
/// tiles listed at `$8528` (columns: West, East) / `$852A` (raw rows), and
/// the bridge tiles at `$8554`/`$8555` (raw rows: Maze Island, East) /
/// `$8556`/`$8557` (columns) keep the overworld music playing. Point them
/// at wherever those connectors are now.
fn write_dock_spots(rom: &mut Rom, conts: &[Continent]) -> Result<(), RandoError> {
    for c in conts {
        let raft = super::loc::RAFT;
        if let (Some(dock), Some(_)) = (c.dock, c.pos(raft)) {
            let l = c.locs[raft];
            rom.write_cpu(0, 0x8528 + u16::from(dock), &[l.x])?;
            rom.write_cpu(0, 0x852A + u16::from(dock), &[l.raw_y])?;
        }
        let bridge = super::loc::BRIDGE;
        let region = match c.cont {
            Cont::DeathMountain | Cont::Maze => 0,
            Cont::East => 1,
            Cont::West => continue,
        };
        if c.pos(bridge).is_some() {
            let l = c.locs[bridge];
            rom.write_cpu(0, 0x8554 + region, &[l.raw_y])?;
            rom.write_cpu(0, 0x8556 + region, &[l.x])?;
        }
    }
    Ok(())
}

/// Palace-to-stone tables. Bank 1: entries 0-2 serve West Hyrule. Bank 2:
/// entries 0-1 (and 2, which we enable when a normal palace sits on the
/// Great Palace's spot) serve East Hyrule, entry 3 Maze Island.
fn write_stone_tables(
    rom: &mut Rom,
    conts: &[Continent],
    outs: &[MapOut],
    window: u16,
) -> Result<(), RandoError> {
    let entry = |cont: Cont, slot: usize| -> (u16, u8) {
        let c = &conts[cont.index()];
        let o = &outs[cont.index()];
        let id = match c.palace[slot] {
            Some(n) if (1..=6).contains(&n) && c.pos(slot).is_some() => n - 1,
            _ => 0xFF,
        };
        let ptr = o
            .single_of
            .iter()
            .find(|(s, _)| *s == slot)
            .map_or(0, |(_, off)| window + *off as u16);
        if id == 0xFF || ptr == 0 {
            (0, 0xFF)
        } else {
            (ptr, id)
        }
    };
    let set = |rom: &mut Rom, bank: u8, k: u16, (ptr, id): (u16, u8)| -> Result<(), RandoError> {
        rom.write_cpu_word(bank, 0x878F + 2 * k, ptr)?;
        rom.write_cpu(bank, 0x8797 + k, &[id])
    };
    for (k, slot) in [52usize, 53, 54].iter().enumerate() {
        set(rom, 1, k as u16, entry(Cont::West, *slot))?;
    }
    set(rom, 2, 0, entry(Cont::East, 52))?;
    set(rom, 2, 1, entry(Cont::East, 53))?;
    let gp_slot = entry(Cont::East, 54);
    set(rom, 2, 2, gp_slot)?;
    if gp_slot.1 != 0xFF {
        // East loop starts at entry 1 (`LDX #$01` at bank 2 `$87A4`); make
        // it start at entry 2.
        if rom.read_cpu(2, 0x87A4)? == 0xA2 {
            rom.write_cpu(2, 0x87A5, &[0x02])?;
        }
    }
    set(rom, 2, 3, entry(Cont::Maze, 52))?;
    Ok(())
}
