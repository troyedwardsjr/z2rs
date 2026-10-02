//! Overworld map storage: the 64-column grid, the run-length codec the game
//! reads, and where each continent's map lives in the ROM.
//!
//! # Format
//!
//! A map is a list of rows, top to bottom. Each row is exactly 64 tiles and
//! is stored as runs: one byte per run, high nibble = run length - 1 (1-16
//! tiles), low nibble = terrain code. Runs never cross a row boundary.
//!
//! # Loading
//!
//! When an overworld loads, the fixed-bank loader (`$CDBE`) reads the
//! region's map pointer from the switched bank (`$8508 + {0,2}`) and copies
//! 896 bytes (`$7C00-$7F7F`) into battery RAM; bank 0 (`$87F3`) then builds
//! a table of 75 row start pointers at `$6000`. Every later reader (the
//! movement, boundary, hammer and flute code, and the palace-to-stone
//! routine) goes through that pointer table, so moving the map elsewhere in
//! the switched bank only needs the pointer changed. A map bigger than 896
//! bytes needs the copy enlarged; [`apply_size_patch`] moves the RAM window
//! to `$7A00` and copies 1408 bytes (our own loader code, see there).

use super::terrain::Terrain;
use crate::rom::Rom;
use crate::RandoError;

/// Tiles per row.
pub const MAP_W: usize = 64;
/// Rows the game addresses (row pointers built, boundary check).
pub const MAP_ROWS: usize = 75;
/// Raw Y of internal row 0 (location tables store raw rows).
pub const RAW_ROW_BASE: u8 = 30;

/// Battery-RAM map window in the vanilla loader.
pub const VANILLA_WINDOW: u16 = 0x7C00;
/// Bytes the vanilla loader copies.
pub const VANILLA_BUDGET: usize = 896;
/// Battery-RAM map window after [`apply_size_patch`].
pub const BIG_WINDOW: u16 = 0x7A00;
/// Bytes copied after [`apply_size_patch`] (`$7A00-$7F7F`, the same end as
/// the vanilla window).
pub const BIG_BUDGET: usize = 1408;

/// The four overworld maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Cont {
    /// West Hyrule (region 0, bank 1).
    West = 0,
    /// Death Mountain (region 1, bank 1).
    DeathMountain = 1,
    /// East Hyrule (region 2, bank 2).
    East = 2,
    /// Maze Island (region 1 in bank 2).
    Maze = 3,
}

impl Cont {
    /// All four, in generation order.
    pub const ALL: [Cont; 4] = [Cont::West, Cont::DeathMountain, Cont::East, Cont::Maze];

    /// Index 0-3.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// From an index 0-3.
    #[must_use]
    pub fn from_index(i: usize) -> Cont {
        Cont::ALL[i & 3]
    }

    /// The switched bank holding this map and its location table.
    #[must_use]
    pub fn bank(self) -> u8 {
        match self {
            Cont::West | Cont::DeathMountain => 1,
            Cont::East | Cont::Maze => 2,
        }
    }

    /// CPU address of this map's pointer in [`Cont::bank`].
    #[must_use]
    pub fn pointer_addr(self) -> u16 {
        match self {
            Cont::West | Cont::East => 0x8508,
            Cont::DeathMountain | Cont::Maze => 0x850A,
        }
    }

    /// CPU address of the location table (4 parallel 63-byte arrays).
    #[must_use]
    pub fn table_addr(self) -> u16 {
        match self {
            Cont::West | Cont::East => 0x861F,
            Cont::DeathMountain | Cont::Maze => 0xA0FC,
        }
    }

    /// Where a rewritten map goes (free space in the vanilla bank; 1408
    /// bytes each, `$B470-$BF6F`).
    #[must_use]
    pub fn new_map_addr(self) -> u16 {
        match self {
            Cont::West | Cont::East => 0xB470,
            Cont::DeathMountain | Cont::Maze => 0xB9F0,
        }
    }

    /// Rows the vanilla map has.
    #[must_use]
    pub fn vanilla_rows(self) -> usize {
        match self {
            Cont::West | Cont::East => 75,
            Cont::DeathMountain | Cont::Maze => 60,
        }
    }

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Cont::West => "West Hyrule",
            Cont::DeathMountain => "Death Mountain",
            Cont::East => "East Hyrule",
            Cont::Maze => "Maze Island",
        }
    }
}

/// A full 64 x 75 terrain grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    cells: Vec<Terrain>,
}

impl Grid {
    /// A grid of one terrain.
    #[must_use]
    pub fn filled(t: Terrain) -> Grid {
        Grid {
            cells: vec![t; MAP_W * MAP_ROWS],
        }
    }

    /// Tile at (row, col); out of range reads as water.
    #[must_use]
    pub fn get(&self, r: usize, c: usize) -> Terrain {
        if r < MAP_ROWS && c < MAP_W {
            self.cells[r * MAP_W + c]
        } else {
            Terrain::Water
        }
    }

    /// Tile at signed (row, col); out of range reads as water.
    #[must_use]
    pub fn get_i(&self, r: i32, c: i32) -> Terrain {
        if r < 0 || c < 0 {
            Terrain::Water
        } else {
            self.get(r as usize, c as usize)
        }
    }

    /// Set (row, col); out of range is ignored.
    pub fn set(&mut self, r: usize, c: usize, t: Terrain) {
        if r < MAP_ROWS && c < MAP_W {
            self.cells[r * MAP_W + c] = t;
        }
    }

    /// One row.
    #[must_use]
    pub fn row(&self, r: usize) -> &[Terrain] {
        &self.cells[r * MAP_W..(r + 1) * MAP_W]
    }

    /// Debug text dump (one glyph per tile).
    #[must_use]
    pub fn dump(&self, rows: usize) -> String {
        let mut s = String::new();
        for r in 0..rows.min(MAP_ROWS) {
            s.extend(self.row(r).iter().map(|t| t.glyph()));
            s.push('\n');
        }
        s
    }
}

/// A decoded map plus, for every tile, the index of the run byte that
/// covers it.
#[derive(Debug, Clone)]
pub struct Decoded {
    /// Terrain.
    pub grid: Grid,
    /// Byte index per tile (`row * 64 + col`), for the decoded rows.
    pub run_of: Vec<usize>,
    /// Bytes consumed.
    pub len: usize,
}

/// Decode `rows` rows from `bytes`. Rows past `rows` are filled with
/// `filler`. Fails if the data ends early or a run crosses a row boundary.
pub fn decode(bytes: &[u8], rows: usize, filler: Terrain) -> Result<Decoded, RandoError> {
    let mut grid = Grid::filled(filler);
    let mut run_of = vec![usize::MAX; MAP_W * MAP_ROWS];
    let mut i = 0;
    for r in 0..rows.min(MAP_ROWS) {
        let mut c = 0;
        while c < MAP_W {
            let b = *bytes
                .get(i)
                .ok_or_else(|| RandoError::Rom(format!("overworld map ends in row {r}")))?;
            let n = usize::from(b >> 4) + 1;
            if c + n > MAP_W {
                return Err(RandoError::Rom(format!(
                    "overworld map run crosses the end of row {r}"
                )));
            }
            let t = Terrain::from_code(b);
            for k in 0..n {
                grid.set(r, c + k, t);
                run_of[r * MAP_W + c + k] = i;
            }
            c += n;
            i += 1;
        }
    }
    Ok(Decoded {
        grid,
        run_of,
        len: i,
    })
}

/// Result of [`encode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// The run bytes.
    pub bytes: Vec<u8>,
    /// For each requested single tile, the index of its one-tile run.
    pub singles: Vec<usize>,
}

/// Encode all [`MAP_ROWS`] rows. Every tile in `singles` (row, col) gets a
/// run of its own, so the game can rewrite that one byte later (hidden
/// palace and town reveals, palaces turning to stone).
#[must_use]
pub fn encode(grid: &Grid, singles: &[(usize, usize)]) -> Encoded {
    let mut bytes = Vec::with_capacity(1024);
    let mut single_at = vec![usize::MAX; singles.len()];
    let is_single = |r: usize, c: usize| singles.iter().position(|&s| s == (r, c));
    for r in 0..MAP_ROWS {
        let row = grid.row(r);
        let mut c = 0;
        while c < MAP_W {
            if let Some(k) = is_single(r, c) {
                single_at[k] = bytes.len();
                bytes.push(row[c].code());
                c += 1;
                continue;
            }
            let t = row[c];
            let mut n = 1;
            while c + n < MAP_W && n < 16 && row[c + n] == t && is_single(r, c + n).is_none() {
                n += 1;
            }
            bytes.push(((n as u8 - 1) << 4) | t.code());
            c += n;
        }
    }
    Encoded {
        bytes,
        singles: single_at,
    }
}

/// Install the larger map copy: the loader copies [`BIG_BUDGET`] bytes into
/// `$7A00-$7F7F` instead of 896 into `$7C00-$7F7F`, and the row-pointer
/// builder starts at `$7A00`. Our own replacement for the copy loop in the
/// fixed bank (`$CDBE-$CDF4`, 55 bytes); it must leave `Y = 0` for the
/// area-table copy that follows at `$CDF5`.
pub fn apply_size_patch(rom: &mut Rom) -> Result<(), RandoError> {
    const START: u16 = 0xCDBE;
    const END: u16 = 0xCDF5;
    let src = "
        ; X = region; the map pointer pair sits at $8508 + {0, 2}.
        ldx $0706
        lda $CD27,x
        tax
        lda $8508,x
        sta $02
        lda $8509,x
        sta $03
        lda #$00
        sta $04
        tay
        lda #$7A
        sta $05
        ldx #$05
    page:
        lda ($02),y
        sta ($04),y
        iny
        bne page
        inc $03
        inc $05
        dex
        bne page
    half:
        lda ($02),y
        sta $7F00,y
        iny
        bpl half
        ldy #$00
        jmp $CDF5
    ";
    let out = crate::asm::Assembler::new().assemble(&format!(".org ${START:04X}\n{src}"))?;
    let bytes = out.bytes();
    let room = usize::from(END - START);
    if bytes.len() > room {
        return Err(RandoError::Other(format!(
            "map loader patch is {} bytes, room for {room}",
            bytes.len()
        )));
    }
    let mut padded = bytes;
    padded.resize(room, 0xEA);
    rom.write_cpu(7, START, &padded)?;
    // Row-pointer builder: LDA #$7C at bank 0 $87F7.
    if rom.read_cpu(0, 0x87F7)? != 0xA9 {
        return Err(RandoError::Rom("unexpected row-pointer builder".into()));
    }
    rom.write_cpu(0, 0x87F8, &[(BIG_WINDOW >> 8) as u8])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Grid {
        let mut g = Grid::filled(Terrain::Water);
        for r in 0..MAP_ROWS {
            for c in 0..MAP_W {
                let t = match (r * 7 + c * 3) % 11 {
                    0..=3 => Terrain::Grass,
                    4..=6 => Terrain::Mountain,
                    7 => Terrain::Road,
                    _ => Terrain::Water,
                };
                g.set(r, c, t);
            }
        }
        g
    }

    #[test]
    fn encode_decode_round_trip() {
        let g = sample();
        let e = encode(&g, &[]);
        let d = decode(&e.bytes, MAP_ROWS, Terrain::Water).unwrap();
        assert_eq!(d.grid, g);
        assert_eq!(d.len, e.bytes.len());
    }

    #[test]
    fn long_runs_split_at_sixteen() {
        let g = Grid::filled(Terrain::Mountain);
        let e = encode(&g, &[]);
        assert_eq!(e.bytes.len(), 4 * MAP_ROWS);
        assert!(e.bytes.iter().all(|&b| b == 0xFB));
    }

    #[test]
    fn singles_get_their_own_byte() {
        let g = Grid::filled(Terrain::Desert);
        let e = encode(&g, &[(3, 10), (3, 11), (70, 63)]);
        for (k, &(r, c)) in [(3usize, 10usize), (3, 11), (70, 63)].iter().enumerate() {
            let i = e.singles[k];
            assert_eq!(e.bytes[i], Terrain::Desert.code());
            let d = decode(&e.bytes, MAP_ROWS, Terrain::Water).unwrap();
            assert_eq!(d.run_of[r * MAP_W + c], i);
        }
    }

    #[test]
    fn decode_rejects_bad_rows() {
        assert!(decode(&[0xF5, 0xF5, 0xF5, 0xE5, 0x15], 1, Terrain::Water).is_err());
        assert!(decode(&[0xF5], 1, Terrain::Water).is_err());
    }

    #[test]
    fn size_patch_fits() {
        let body = vec![0u8; crate::rom::VANILLA_BODY_LEN];
        let mut rom = Rom::from_body(&body).unwrap();
        rom.write_cpu(0, 0x87F7, &[0xA9, 0x7C]).unwrap();
        apply_size_patch(&mut rom).unwrap();
        assert_eq!(rom.read_cpu(0, 0x87F8).unwrap(), 0x7A);
        assert_eq!(rom.read_cpu(7, 0xCDBE).unwrap(), 0xAE);
    }
}
