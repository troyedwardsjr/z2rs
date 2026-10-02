//! Hidden locations in East Hyrule: the one the flute reveals (vanilla: the
//! sixth palace under the three-eyed rock) and the one the hammer reveals
//! (vanilla: New Kasuto under a forest tile).
//!
//! The vanilla reveal code is kept; only its data changes. All of it is in
//! the fixed bank except the flute's call-spot compare:
//!
//! | CPU | what |
//! |---|---|
//! | bank 2 `$8372` / `$8378` | raw row / column Link must stand on to play the flute (the location is two rows below) |
//! | `$DF60` / `$DF61` | the one-tile run byte under the flute / hammer location before the reveal |
//! | `$DF64` / `$DF65` | the run byte written by the reveal |
//! | `$DF66` / `$DF67` | the location's slot in the East table |
//! | `$DF68` / `$DF69` | the row byte the reveal writes into the RAM table |
//! | `$DF6A-$DF77` | PPU commands that draw the flute reveal: two rows of two tiles, then one attribute byte |
//! | `$DF9C` / `$DFA2` | hammer location row / column + 1 |
//! | `$DFA6`, `$DFAB`, `$DFB0`, `$DFB5` | the hammer reveal's four tiles |
//! | `$CCB0` | row Link comes back out at after a flute location whose table row is still 0 |
//! | `$CCC4` / `$CCCB` | column byte / row of the hammer location for the same fallback |
//!
//! Tiles and palettes come from the game's own terrain tables in bank 0
//! (`$87A3`: four tiles per terrain, left column then right column;
//! `$87E3`: palette per terrain).

use super::continent::{Continent, Reveal};
use super::map::RAW_ROW_BASE;
use super::terrain::Terrain;
use crate::rom::Rom;
use crate::RandoError;

/// The four tiles of a terrain metatile: top-left, bottom-left, top-right,
/// bottom-right.
pub fn metatile(rom: &Rom, t: Terrain) -> Result<[u8; 4], RandoError> {
    let mut m = [0u8; 4];
    for (k, b) in m.iter_mut().enumerate() {
        *b = rom.read_cpu(0, 0x87A3 + u16::from(t.code()) * 4 + k as u16)?;
    }
    Ok(m)
}

/// Background palette of a terrain.
pub fn palette(rom: &Rom, t: Terrain) -> Result<u8, RandoError> {
    Ok(rom.read_cpu(0, 0x87E3 + u16::from(t.code()))? & 3)
}

/// Nametable address of the top-left tile of the metatile at raw row
/// `raw_y`, column `x` (the overworld uses two vertically stacked
/// nametables of 15 metatile rows, 16 metatile columns wide).
#[must_use]
pub fn nametable_addr(raw_y: u8, x: u8) -> u16 {
    let mr = u16::from(raw_y % 15);
    let mc = u16::from(x % 16);
    let page = u16::from((raw_y % 30) / 15);
    0x2000 + page * 0x800 + mr * 0x40 + mc * 2
}

/// Attribute address and value for the 2x2-metatile block holding
/// (raw_y, x), given the terrain of each map tile (`tile(row, col)` in
/// internal rows; out-of-map tiles are asked for too).
pub fn attribute(
    rom: &Rom,
    raw_y: u8,
    x: u8,
    tile: impl Fn(i32, i32) -> Terrain,
) -> Result<(u16, u8), RandoError> {
    let mr = i32::from(raw_y % 15);
    let mc = i32::from(x % 16);
    let page = u16::from((raw_y % 30) / 15);
    let addr = 0x2000 + page * 0x800 + 0x3C0 + (mr as u16 / 2) * 8 + (mc as u16 / 2);
    let mut v = 0u8;
    for (q, (dr, dc)) in [(0, 0), (0, 1), (1, 0), (1, 1)].iter().enumerate() {
        let br = (mr & !1) + dr;
        let bc = (mc & !1) + dc;
        let pal = if br > 14 {
            0
        } else {
            let row = i32::from(raw_y) - mr + br - i32::from(RAW_ROW_BASE);
            let col = i32::from(x) - mc + bc;
            palette(rom, tile(row, col))?
        };
        v |= pal << (2 * q);
    }
    Ok((addr, v))
}

/// Write the reveal data for East Hyrule's hidden locations. `single_at`
/// gives the encoded-map byte offset of a slot's one-tile run (unused here
/// beyond checking it exists), `legacy` keeps the vanilla revealed terrain.
pub fn write(rom: &mut Rom, east: &Continent, legacy: bool) -> Result<(), RandoError> {
    let flute = (0..east.locs.len())
        .find(|&s| east.hidden[s] == Some(Reveal::Flute) && east.pos(s).is_some());
    let hammer = (0..east.locs.len())
        .find(|&s| east.hidden[s] == Some(Reveal::Hammer) && east.pos(s).is_some());

    match flute {
        Some(s) => {
            let l = east.locs[s];
            let (r, c) = l.pos().unwrap_or((0, 0));
            let call_raw = l.raw_y - 2;
            rom.write_cpu(2, 0x8372, &[call_raw])?;
            rom.write_cpu(2, 0x8378, &[l.x])?;
            rom.write_cpu(7, 0xDF60, &[east.cover[s].code()])?;
            let shown = if legacy {
                Terrain::Palace
            } else {
                east.icon[s]
            };
            rom.write_cpu(7, 0xDF64, &[shown.code()])?;
            rom.write_cpu(7, 0xDF66, &[s as u8])?;
            rom.write_cpu(7, 0xDF68, &[l.raw_y | if l.external { 0x80 } else { 0 }])?;
            rom.write_cpu(7, 0xCCB0, &[l.raw_y])?;
            let m = metatile(rom, shown)?;
            let nt = nametable_addr(l.raw_y, l.x);
            let (aaddr, aval) = attribute(rom, l.raw_y, l.x, |rr, cc| {
                if rr == r as i32 && cc == c as i32 {
                    shown
                } else {
                    east.grid.get_i(rr, cc)
                }
            })?;
            let cmds = [
                (nt >> 8) as u8,
                nt as u8,
                2,
                m[0],
                m[2],
                ((nt + 0x20) >> 8) as u8,
                (nt + 0x20) as u8,
                2,
                m[1],
                m[3],
                (aaddr >> 8) as u8,
                aaddr as u8,
                1,
                aval,
                0xFF,
            ];
            rom.write_cpu(7, 0xDF6A, &cmds)?;
        }
        None => {
            // No flute location: the call spot can never match.
            rom.write_cpu(2, 0x8372, &[0xFF])?;
        }
    }

    match hammer {
        Some(s) => {
            let l = east.locs[s];
            let (r, _) = l.pos().unwrap_or((0, 0));
            rom.write_cpu(7, 0xDF61, &[east.cover[s].code()])?;
            let shown = if legacy { Terrain::Town } else { east.icon[s] };
            rom.write_cpu(7, 0xDF65, &[shown.code()])?;
            rom.write_cpu(7, 0xDF67, &[s as u8])?;
            rom.write_cpu(7, 0xDF69, &[l.raw_y | if l.external { 0x80 } else { 0 }])?;
            rom.write_cpu(7, 0xDF9C, &[r as u8])?;
            rom.write_cpu(7, 0xDFA2, &[l.x + 1])?;
            let m = metatile(rom, shown)?;
            for (k, addr) in [0xDFA6u16, 0xDFAB, 0xDFB0, 0xDFB5].iter().enumerate() {
                rom.write_cpu(7, *addr, &[m[k]])?;
            }
            rom.write_cpu(7, 0xCCC4, &[l.x | (l.entrance << 6)])?;
            rom.write_cpu(7, 0xCCCB, &[l.raw_y])?;
        }
        None => {
            // No hammer location: the row compare can never match.
            rom.write_cpu(7, 0xDF9C, &[0xFF])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nametable_math() {
        // The vanilla sixth palace: raw row 102, column 45.
        assert_eq!(nametable_addr(102, 45), 0x231A);
        assert_eq!(nametable_addr(30, 0), 0x2000);
        assert_eq!(nametable_addr(45, 0), 0x2800);
    }
}
