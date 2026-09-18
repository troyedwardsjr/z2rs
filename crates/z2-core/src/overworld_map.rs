//! Overworld map layer: RLE decode, region select, terrain, movement.
//!
//! Self-contained (no intra-crate imports; shared addresses are repeated
//! here on purpose) so `rustc --edition 2021 --test` compiles this file
//! standalone. `main` wires it with `pub mod overworld_map;`.
//!
//! # RLE format (verified vs `bank1_overworld_limit_check_jmp_from_bank7`,
//! # bank 1 `$83CF`, and `LE001`, bank 7 `$E001`)
//!
//! Each blob byte packs one run: high nibble = run length − 1 (so `$0` → 1
//! tile … `$F` → 16 tiles), low nibble = terrain 0-15. The walker keeps an
//! accumulator (`SEC : ADC`, i.e. `acc += hi + 1` per byte) and stops at the
//! first byte whose accumulated length exceeds the target column — the low
//! nibble of that byte is the tile (`prg1.asm $83EB-$8408`).
//!
//! Rows are 64 tiles wide (`MAP_W = 64`; east-boundary `$40` check at bank 1
//! `$83D1`, south-boundary `$4B` = 75 rows at bank 1 `$83DC`). Verified
//! against the ROM (read-only probe, `Z2_ROM`): West/East decode to exactly
//! 4800 = 75×64 tiles, Death Mountain/Maze Island to 3840 = 60×64, every
//! row summing to exactly 64 with whole-byte boundaries — so the region
//! heights are West/East 75, DM/Maze 60 (see `REGION_HEIGHT`).
//!
//! The `$6000` row-pointer build (`L8C30`, bank 0 `$8C30-$8C33`) walks each
//! row with `JSR LE001 : CMP #$41 : BPL done`: bytes accumulate `hi + 1`
//! until the total reaches `$41` = 65 — but the `BPL` exit skips the loop's
//! `INY`, so the threshold byte is only *peeked*, never consumed. Net
//! effect on well-formed (64-exact) blobs: each row consumes exactly its
//! own bytes and the next row starts at the next byte. `build_row_offsets`
//! replicates the peek rule exactly (it also governs short/ragged rows the
//! way the hardware does). `tile_at` walks columns from those starts just
//! like the east-boundary walker, and was spot-checked against West
//! key-area coordinates: sensible (walkable or hammer/flute-transformable)
//! terrain under this rule, mountains/water under the naive consume-the-
//! threshold misreading. Blobs live in the ROM file at (see `z2-assets`
//! `extract_tables.rs`):
//!
//! | region (`$0706`) | file offset | len | disassembly label |
//! |---|---|---|
//! | 0 West Hyrule | `$506C` | 801 | `bank1_West_Hyrule_Overworld_Map_Data` |
//! | 1 Death Mountain | `$665C` | 743 | `bank1_Death_Mountain_Overworld_Map_Data` |
//! | 2 East Hyrule | `$9056` | 794 | `bank2_East_Hyrule_Overworld_Map_Data` |
//! | 1 Maze Island (`$070A != 0`) | `$A65C` | 743 | `bank2_Maze_Island_Overworld_Map_Data` |
//!
//! KNOWN (cross-checked vs `z2-assets/tests/roundtrip.rs`): West blob starts
//! with `0xBB` (12 tiles of terrain `$B` mountain); Death Mountain and Maze
//! Island blobs are byte-identical.
//!
//! # WRAM layout
//!
//! `bank7_code18` (bank 7 `$CDCF-$CDF3`) copies the raw blob (up to
//! 256+256+256+128 = 896 bytes) to WRAM `$7C00-$7FFF`; `overworld4` (bank 0
//! `$87F3`) then builds the `$6000-$6095` row-pointer table (75 LE words,
//! `X/2 != $4B` loop) so the per-frame walker (`L8C48`, bank 0 `$8C48` /
//! `bank1_code8`, bank 1 `$93AC`) can fetch a row start in O(1).
//!
//! # Terrain codes (`Overworld_Tile_Mappings…`, bank 0 `$87A3`, 16×4 tiles)
//!
//! 0 town · 1 grotto/cave · 2 palace · 3 bridge · 4 desert · 5 grass ·
//! 6 forest · 7 swamp · 8 graveyard · 9 road · A lava · B mountain ·
//! C water · D walkable water · E rock · F spider.
//!
//! # Movement (bank 0 `$8601-$86AF`)
//!
//! `L8601` reads held buttons `$F7` low nibble as bit0 right, bit1 left,
//! bit2 down (`$73+1`), bit3 up (`$73−1`) — first set bit wins — stores the
//! direction in `$70`, sets facing `$562` (1/2/4/8), probes the target tile
//! with `Blocked_by_Tile_or_Not_Routine` (`$870F`) and reverts `$73`/`$74`
//! when blocked. On success the per-direction epilogue (`L861B`/`L864C`/
//! `L867D`/`L869F`) recomputes pixel coords `$75`/`$76` and calls `LDF01`
//! (bank 7 `$DF01`), which anchors scroll and sets `$7D = $10` (16 px).
//! Each later frame `overworld3` runs `L86AF`: swamp halves speed (skip on
//! odd frames), else `DEC $7D; INC $26` plus scroll shifting, until `$7D`
//! hits 0 — i.e. ~1 tile per 16 frames.
//!
//! # Sideview ↔ map coordinates (documented, no arithmetic identity)
//!
//! Overworld tiles (`$73`/`$74`, square units) and sideview pixels (`$29` Y,
//! `$4D` X low, `$3B` page/high) are separate spaces. The bridge between
//! them is per-step: `$76 = $74 ± 7`, `$75 = $73 − 11` (right/left, `L861B`/
//! `L864C`) or `$75 = $73 + 11` (down, `L867D`), from which `LDF01` derives
//! nametable state `$79`/`$7A` and tile word `$77`. On area transitions the
//! sideview is re-initialised independently (`bank7_code33` ground-finds
//! `$29 = $AF` down); on return `bank7_go_outside` restores `$73`/`$74`
//! from the `$6A00` area bytes (see `overworld_transition.rs`).

// ---------------------------------------------------------------------------
// Addresses (duplicated per overworld*.rs file; see overworld.rs).
// ---------------------------------------------------------------------------

/// Overworld region index (`$0706`).
pub const ADDR_OVERWORLD_INDEX: u16 = 0x0706;
/// Previous region (`$070A`).
pub const ADDR_PREV_REGION: u16 = 0x070A;
/// Overworld tile Y (`$0073`).
pub const ADDR_TILE_Y: u16 = 0x0073;
/// Overworld tile X (`$0074`).
pub const ADDR_TILE_X: u16 = 0x0074;
/// Facing (`$0562`): 1 right, 2 left, 4 down, 8 up.
pub const ADDR_FACING: u16 = 0x0562;
/// Terrain (`$0563`).
pub const ADDR_TERRAIN: u16 = 0x0563;
/// Boots flag (`$0788`): nonzero walks terrain `$0D`.
pub const ADDR_BOOTS: u16 = 0x0788;
/// WRAM RLE base (`$7C00`).
pub const WRAM_RLE_BASE: usize = 0x7C00;
/// WRAM RLE end (exclusive, `$8000`); capacity 1024, loader copies ≤896.
pub const WRAM_RLE_END: usize = 0x8000;
/// WRAM row-pointer table base (`$6000`, 75 LE words through `$6095`).
pub const WRAM_ROWPTR_BASE: usize = 0x6000;

// ---------------------------------------------------------------------------
// Map geometry + ROM catalog.
// ---------------------------------------------------------------------------

/// Tiles per playable overworld row (east-boundary `$40`, bank 1 `$83D1`).
pub const MAP_W: usize = 64;
/// `L8C30` stop threshold (`CMP #$41`, bank 0 `$8C33`): accumulation stops
/// when the total reaches 65, peeking — not consuming — that byte.
pub const ROW_PEEK_ACC: usize = 65;
/// Max overworld rows (south-boundary `$4B`, bank 1 `$83DC`).
pub const MAP_H: usize = 75;
/// Row-pointer entries built by `overworld4` (`$4B` rows, bank 0 `$8824`).
pub const ROW_COUNT: usize = 75;
/// Raw bytes `bank7_code18` copies to `$7C00-$7FFF` (256+256+256+128).
pub const RLE_COPY_LEN: usize = 896;
/// West Hyrule blob: ROM file offset (disassembly `$905C`); len 801.
pub const WEST_FILE_OFF: u32 = 0x506C;
/// West Hyrule blob length (asserted by `z2-assets` roundtrip test).
pub const WEST_LEN: usize = 801;
/// KNOWN first blob byte of West Hyrule: 12 × terrain `$B` (mountain).
pub const WEST_FIRST_BYTE: u8 = 0xBB;
/// Death Mountain blob: ROM file offset; len 743.
pub const DM_FILE_OFF: u32 = 0x665C;
/// Death Mountain blob length.
pub const DM_LEN: usize = 743;
/// East Hyrule blob: ROM file offset; len 794.
pub const EAST_FILE_OFF: u32 = 0x9056;
/// East Hyrule blob length.
pub const EAST_LEN: usize = 794;
/// Maze Island blob: ROM file offset; len 743, byte-identical to DM.
pub const MAZE_FILE_OFF: u32 = 0xA65C;
/// Maze Island blob length.
pub const MAZE_LEN: usize = 743;

/// Region selector (`$0706` + `$070A` disambiguation, `bank7_code18`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// West Hyrule (bank 1, `$506C` blob).
    West,
    /// Death Mountain (bank 1, `$665C` blob; `$0706 == 1`, `$070A == 0`).
    DeathMountain,
    /// East Hyrule (bank 2, `$9056` blob).
    East,
    /// Maze Island (bank 2, `$A65C` blob; `$0706 == 1`, `$070A != 0`).
    MazeIsland,
}

/// Select the region blob (`bank7_code18`, bank 7 `$CD4A-$CD57`).
pub const fn region_select(overworld_index: u8, prev_region: u8) -> Region {
    match overworld_index {
        0 => Region::West,
        2 => Region::East,
        _ => {
            if prev_region == 0 {
                Region::DeathMountain
            } else {
                Region::MazeIsland
            }
        }
    }
}

/// Decoded map heights in rows, inferred from blob decode lengths
/// (West/East 4800 = 75×64, DM/Maze 3840 = 60×64) and pinned by the
/// ROM-gated test. The `$6000` build always emits [`ROW_COUNT`] starts;
/// rows past the region height point at/past the blob end (clamped) and
/// are unreachable in normal play — `tile_at` maps off-blob walks to
/// water, matching the boundary fill (the hardware would read WRAM
/// residue there instead).
pub const fn region_height(region: Region) -> usize {
    match region {
        Region::West | Region::East => 75,
        Region::DeathMountain | Region::MazeIsland => 60,
    }
}
/// (file offset, length) of a region's RLE blob.
pub const fn region_blob(region: Region) -> (u32, usize) {
    match region {
        Region::West => (WEST_FILE_OFF, WEST_LEN),
        Region::DeathMountain => (DM_FILE_OFF, DM_LEN),
        Region::East => (EAST_FILE_OFF, EAST_LEN),
        Region::MazeIsland => (MAZE_FILE_OFF, MAZE_LEN),
    }
}

// ---------------------------------------------------------------------------
// Terrain.
// ---------------------------------------------------------------------------

/// Terrain codes (low nibble; `Overworld_Tile_Mappings…`, bank 0 `$87A3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Terrain {
    /// `$0` town.
    Town = 0x0,
    /// `$1` grotto / cave.
    Cave = 0x1,
    /// `$2` palace.
    Palace = 0x2,
    /// `$3` bridge.
    Bridge = 0x3,
    /// `$4` desert.
    Desert = 0x4,
    /// `$5` grass.
    Grass = 0x5,
    /// `$6` forest.
    Forest = 0x6,
    /// `$7` swamp (half movement speed, `L86AF` bank 0 `$86AF`).
    Swamp = 0x7,
    /// `$8` graveyard.
    Graveyard = 0x8,
    /// `$9` road.
    Road = 0x9,
    /// `$A` lava (walkable per `$870F`; damage via bank-7 lava routine).
    Lava = 0xA,
    /// `$B` mountain (blocked).
    Mountain = 0xB,
    /// `$C` water (blocked).
    Water = 0xC,
    /// `$D` walkable water (passable with boots `$0788`).
    WalkWater = 0xD,
    /// `$E` rock (blocked; hammer-cleared via `L850C`).
    Rock = 0xE,
    /// `$F` spider (blocked; cleared via flute/chop path).
    Spider = 0xF,
}

/// Convert a raw nibble to terrain (`None` is unreachable for 4 bits;
/// kept total for fuzz harnesses).
pub const fn terrain_from_nibble(n: u8) -> Option<Terrain> {
    match n & 0x0F {
        0x0 => Some(Terrain::Town),
        0x1 => Some(Terrain::Cave),
        0x2 => Some(Terrain::Palace),
        0x3 => Some(Terrain::Bridge),
        0x4 => Some(Terrain::Desert),
        0x5 => Some(Terrain::Grass),
        0x6 => Some(Terrain::Forest),
        0x7 => Some(Terrain::Swamp),
        0x8 => Some(Terrain::Graveyard),
        0x9 => Some(Terrain::Road),
        0xA => Some(Terrain::Lava),
        0xB => Some(Terrain::Mountain),
        0xC => Some(Terrain::Water),
        0xD => Some(Terrain::WalkWater),
        0xE => Some(Terrain::Rock),
        0xF => Some(Terrain::Spider),
        _ => None,
    }
}

/// First tile word of each terrain's 2×2 mapping
/// (`Overworld_Tile_Mappings…`, bank 0 `$87A3`).
pub const TILE_MAPPINGS: [[u8; 4]; 16] = [
    [0x5C, 0x5D, 0x5E, 0x5F], // town
    [0xF4, 0xF4, 0xF4, 0xF4], // cave
    [0x60, 0x61, 0x62, 0x63], // palace
    [0x5A, 0x5A, 0x5B, 0x5B], // bridge
    [0x6C, 0x6C, 0x6C, 0x6C], // desert
    [0x6D, 0x6D, 0x6D, 0x6D], // grass
    [0x68, 0x69, 0x6A, 0x6B], // forest
    [0x6F, 0x6F, 0x6F, 0x6F], // swamp
    [0x70, 0x71, 0xFE, 0xFE], // graveyard
    [0xFE, 0xFE, 0xFE, 0xFE], // road
    [0x6E, 0x6E, 0x6E, 0x6E], // lava
    [0x64, 0x65, 0x66, 0x67], // mountain
    [0x6E, 0x6E, 0x6E, 0x6E], // water
    [0x6E, 0x6E, 0x6E, 0x6E], // walkable water
    [0x56, 0x57, 0x58, 0x59], // rock
    [0x40, 0x41, 0x42, 0x43], // spider
];

/// Palette codes per terrain (`bank0_Overworld_Palette_Codes_0_3`, `$87E3`).
pub const PALETTE_CODES: [u8; 16] = [
    0x02, 0x01, 0x02, 0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x03, 0x03, 0x01, 0x01,
];

// ---------------------------------------------------------------------------
// RLE decode.
// ---------------------------------------------------------------------------

/// Decode one RLE byte: `(run_len, terrain)` with `run_len = hi + 1`.
pub const fn decode_byte(b: u8) -> (usize, u8) {
    (((b >> 4) as usize) + 1, b & 0x0F)
}

/// Pure RLE decode: expand `blob` to terrain bytes (low nibbles).
///
/// Entry `bank1_overworld_limit_check_jmp_from_bank7` (bank 1 `$83CF`)
/// walks this encoding in place; this helper materialises it for tests and
/// tooling. A full map decodes to `MAP_W × height` tiles (West/East 4800,
/// DM/Maze 3840); trailing partial runs are kept whole (callers index
/// playable columns `0..MAP_W` of each row).
pub fn decode_rle(blob: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in blob {
        let (len, terrain) = decode_byte(b);
        out.extend(std::iter::repeat_n(terrain, len));
    }
    out
}

/// Build the `$6000` row-pointer table: byte offset (from the blob start)
/// of the first RLE byte of each of the 75 rows.
///
/// Entry `overworld4` (bank 0 `$87F3` via `L8C30`): per row, bytes
/// accumulate `hi + 1` until the total would reach [`ROW_PEEK_ACC`] (65);
/// that threshold byte is peeked, not consumed (the `BPL` exit skips
/// `INY`), so on well-formed 64-exact blobs each row consumes exactly its
/// own bytes. Short blobs terminate the table early (remaining entries
/// repeat the end offset, mirroring the loop's `BNE` exit with `X`
/// stalled); rows past a short region's height (DM/Maze: 60) therefore
/// point at/past the blob end.
pub fn build_row_offsets(blob: &[u8]) -> [u16; ROW_COUNT] {
    let mut rows = [0u16; ROW_COUNT];
    let mut off = 0usize;
    for r in 0..ROW_COUNT {
        rows[r] = off.min(0xFFFF) as u16;
        let mut consumed = 0usize;
        while off < blob.len() {
            let (len, _) = decode_byte(blob[off]);
            // Peek rule: stop before the byte that reaches the threshold.
            if consumed + len >= ROW_PEEK_ACC {
                break;
            }
            consumed += len;
            off += 1;
        }
        if off >= blob.len() {
            for rest in rows.iter_mut().skip(r + 1) {
                *rest = off.min(0xFFFF) as u16;
            }
            break;
        }
    }
    rows
}

/// Writer half of `bank7_code18` (bank 7 `$CDCF-$CDF3`): copy the raw blob
/// into the `$7C00-$7FFF` window (`wram7c`: the 1024-byte window slice).
/// Copies at most [`RLE_COPY_LEN`] (896) bytes; returns bytes written.
pub fn write_wram_rle(wram7c: &mut [u8], blob: &[u8]) -> usize {
    let n = blob.len().min(RLE_COPY_LEN).min(wram7c.len());
    wram7c[..n].copy_from_slice(&blob[..n]);
    n
}

/// Terrain at tile (`tile_x`, `tile_y`) by walking one RLE row.
///
/// Entry `bank1_overworld_limit_check_jmp_from_bank7` (bank 1 `$83CF`;
/// bank 2 identical): rejects `tile_x >= $40` (east) and
/// `tile_y - $1E >= $4B` (south) as water `$0C`, else fetches the row start
/// via the `$6000` table (`bank1_code8`, bank 1 `$93AC`) and accumulates
/// `hi + 1` until the column is covered (`BCS L8408`).
///
/// `row_start` is the row's byte offset (from [`build_row_offsets`]).
/// Out-of-range targets return `Terrain::Water` (the boundary fill).
pub fn tile_at(blob: &[u8], row_start: usize, tile_x: u8, tile_y: u8) -> Terrain {
    if tile_x as usize >= MAP_W {
        return Terrain::Water;
    }
    let rel = tile_y.wrapping_sub(0x1E);
    if rel as usize >= MAP_H {
        return Terrain::Water;
    }
    let mut acc = 0usize;
    let mut off = row_start;
    while let Some(&b) = blob.get(off) {
        let (len, terrain) = decode_byte(b);
        acc += len;
        // `CMP $00 : BCS L8408` — carry set means acc >= target+1, i.e.
        // the target column lies inside this run. The listing compares
        // against the incremented column, equivalent to `acc > tile_x`.
        if acc > tile_x as usize {
            return terrain_from_nibble(terrain).unwrap_or(Terrain::Water);
        }
        off += 1;
    }
    Terrain::Water
}

// ---------------------------------------------------------------------------
// Movement: facing, blocked check, step commit.
// ---------------------------------------------------------------------------

/// Facing values (`$0562`): 1 right, 2 left, 4 down(+Y), 8 up(−Y).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Facing {
    /// Right (`INC $74`).
    Right = 1,
    /// Left (`DEC $74`).
    Left = 2,
    /// Down/south (`INC $73`).
    Down = 4,
    /// Up/north (`DEC $73`).
    Up = 8,
}

/// Decode `$0562` to a facing (`None` = no/invalid facing; code paths that
/// `AND #$03` / shifts treat other values as vertical by default).
pub const fn facing_from_byte(b: u8) -> Option<Facing> {
    match b {
        1 => Some(Facing::Right),
        2 => Some(Facing::Left),
        4 => Some(Facing::Down),
        8 => Some(Facing::Up),
        _ => None,
    }
}

/// Hammer-tile XY offsets (`Table_for_Hammer_tile_XY_offset`, `$84AD`, +
/// `L84B6`): indexed by facing value; X `[0,1,-1,0,…]`, Y `[0,0,0,0,1,…−1]`.
pub const HAMMER_X_OFF: [i8; 9] = [0, 1, -1, 0, 0, 0, 0, 0, 0];
/// Y half of the hammer-tile offset table (`L84B6`, bank 0 `$84B6`).
pub const HAMMER_Y_OFF: [i8; 9] = [0, 0, 0, 0, 1, 0, 0, 0, -1];

/// Tile faced by Link (hammer/flute probe, `overworld2` bank 0 `$84BF`):
/// `(tile_y + Y_OFF[facing], tile_x + X_OFF[facing])`, wrapping.
pub const fn faced_tile(tile_y: u8, tile_x: u8, facing: Facing) -> (u8, u8) {
    let i = facing as usize;
    let dx = HAMMER_X_OFF[i];
    let dy = HAMMER_Y_OFF[i];
    (tile_y.wrapping_add(dy as u8), tile_x.wrapping_add(dx as u8))
}

/// Blocked check (`Blocked_by_Tile_or_Not_Routine`, bank 0 `$870F`).
///
/// Passable (carry clear) iff `terrain < $0B`, or `terrain == $0D`
/// (walkable water) with boots (`$0788 != 0`). Everything `>= $0B`
/// (mountain, water, walkable-water-without-boots, rock, spider) is
/// blocked (carry set, and the original zeroes `$70` at `L8721`).
pub const fn is_blocked(terrain: u8, has_boots: bool) -> bool {
    if terrain == Terrain::WalkWater as u8 {
        return !has_boots;
    }
    terrain >= Terrain::Mountain as u8
}

/// Attempt one tile step: returns the new `(tile_y, tile_x)` or the input
/// when blocked (`L8601` reverts `$73`/`$74` on carry, bank 0 `$8618`…).
pub const fn try_step(
    tile_y: u8,
    tile_x: u8,
    facing: Facing,
    terrain_at_target: u8,
    has_boots: bool,
) -> (u8, u8) {
    if is_blocked(terrain_at_target, has_boots) {
        return (tile_y, tile_x);
    }
    match facing {
        Facing::Right => (tile_y, tile_x.wrapping_add(1)),
        Facing::Left => (tile_y, tile_x.wrapping_sub(1)),
        Facing::Down => (tile_y.wrapping_add(1), tile_x),
        Facing::Up => (tile_y.wrapping_sub(1), tile_x),
    }
}

/// `L861B`/`L864C` pixel anchors: right `($76 = $74 + 7, $75 = $73 − 11)`,
/// left `($76 = $74 − 7, $75 = $73 − 11)`, down `($75 = $73 + 11)`,
/// up `($75 = $73 − 11)`; `$76` for vertical steps is set by the redraw
/// path, so this returns `None` for its X half.
pub const fn step_pixel_anchor(tile_y: u8, tile_x: u8, facing: Facing) -> (Option<u8>, u8) {
    match facing {
        Facing::Right => (Some(tile_x.wrapping_add(7)), tile_y.wrapping_sub(11)),
        Facing::Left => (Some(tile_x.wrapping_sub(7)), tile_y.wrapping_sub(11)),
        Facing::Down => (None, tile_y.wrapping_add(11)),
        Facing::Up => (None, tile_y.wrapping_sub(11)),
    }
}

/// Scroll commit for one frame (`L86AF`, bank 0 `$86AF`).
///
/// * Swamp (`terrain == $07`) moves every other frame: returns `None` on
///   odd `frame_counter` (the `LSR : BCC L869E` skip).
/// * Else `pixels_left − 1`, `step_tally + 1` (wrapping), i.e. `DEC $7D;
///   INC $26`.
/// * Returns `Some((pixels_left, step_tally))`, or `None` = frame skipped.
pub const fn scroll_tick(
    pixels_left: u8,
    step_tally: u8,
    terrain: u8,
    frame_counter: u8,
) -> Option<(u8, u8)> {
    if terrain == Terrain::Swamp as u8 && frame_counter & 1 == 1 {
        return None;
    }
    Some((pixels_left.wrapping_sub(1), step_tally.wrapping_add(1)))
}

/// Pixels per tile step (`LDF01` sets `$7D = $10`, bank 7 `$DF13`).
pub const PIXELS_PER_TILE: u8 = 0x10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn west_known_first_byte_decodes_to_12_mountains() {
        let (len, t) = decode_byte(WEST_FIRST_BYTE);
        assert_eq!((len, t), (12, 0x0B));
        let v = decode_rle(&[WEST_FIRST_BYTE]);
        assert_eq!(v, vec![0x0B; 12]);
    }

    #[test]
    fn tile_at_matches_full_decode_spot_checks() {
        // Two exact-64 rows: row0 = 64×grass, row1 = 12×mountain + 52×water.
        let mut blob = vec![0xF5, 0xF5, 0xF5, 0xF5]; // 4×16 grass = 64
        blob.push(0xBB); // 12 mountain
        blob.extend([0xCC; 4]); // 4×13 water = 52 → 64
        let rows = build_row_offsets(&blob);
        assert_eq!(rows[0], 0);
        assert_eq!(rows[1], 4);
        let full = decode_rle(&blob);
        assert_eq!(full.len(), 2 * MAP_W);
        for (r, &off) in rows.iter().enumerate().take(2) {
            let base = r * MAP_W;
            for x in [0u8, 11, 12, 33, 63] {
                let got = tile_at(&blob, off as usize, x, 0x1E + r as u8) as u8;
                assert_eq!(got, full[base + x as usize], "row{r} col{x}");
            }
        }
    }
}
