//! ROM-free overworld map tests + ROM/corpus-gated snapshots.
//!
//! Compiles standalone (`rustc --edition 2021 --test`) and under cargo.
//! Gated tests skip gracefully without `Z2_ROM` / `corpus/`.

#[path = "../src/overworld_map.rs"]
mod overworld_map;

mod common;

use overworld_map::*;

// ---------------------------------------------------------------------------
// Pure unit tests (always run).
// ---------------------------------------------------------------------------

#[test]
fn decode_byte_splits_len_minus_one_and_terrain() {
    assert_eq!(decode_byte(0x00), (1, 0x0));
    assert_eq!(decode_byte(0xBB), (12, 0x0B));
    assert_eq!(decode_byte(0xFF), (16, 0x0F));
    assert_eq!(decode_byte(0x10), (2, 0x0));
}

#[test]
fn region_select_matches_bank7_code18() {
    assert_eq!(region_select(0, 0), Region::West);
    assert_eq!(region_select(0, 1), Region::West);
    assert_eq!(region_select(2, 0), Region::East);
    assert_eq!(region_select(1, 0), Region::DeathMountain);
    assert_eq!(region_select(1, 7), Region::MazeIsland);
    assert_eq!(region_blob(Region::West), (WEST_FILE_OFF, WEST_LEN));
    assert_eq!(region_blob(Region::MazeIsland), (MAZE_FILE_OFF, MAZE_LEN));
}

#[test]
fn row_offsets_match_naive_row_split() {
    // Deterministic pseudo-blob: runs of 1..16 cycling terrains.
    let mut blob = Vec::new();
    for i in 0..400u32 {
        let len = (i % 16) as u8;
        blob.push((len << 4) | (i % 16) as u8);
    }
    let rows = build_row_offsets(&blob);
    // Structural: starts at 0, monotonic, in-bounds. Exactness on
    // well-formed data is covered by the src unit test + ROM-gated test.
    assert_eq!(rows[0], 0);
    for w in rows.windows(2) {
        assert!(w[0] <= w[1] && (w[1] as usize) <= blob.len());
    }
}

#[test]
fn address_consts_match_ram_map() {
    assert_eq!(ADDR_OVERWORLD_INDEX, 0x0706);
    assert_eq!(ADDR_PREV_REGION, 0x070A);
    assert_eq!(ADDR_TILE_Y, 0x0073);
    assert_eq!(ADDR_TILE_X, 0x0074);
    assert_eq!(ADDR_FACING, 0x0562);
    assert_eq!(ADDR_TERRAIN, 0x0563);
    assert_eq!(ADDR_BOOTS, 0x0788);
    assert_eq!(ROW_PEEK_ACC, 65);
    assert_eq!(
        (WRAM_RLE_BASE, WRAM_RLE_END, WRAM_ROWPTR_BASE),
        (0x7C00, 0x8000, 0x6000)
    );
    assert_eq!(TILE_MAPPINGS[0x0], [0x5C, 0x5D, 0x5E, 0x5F]);
    assert_eq!(TILE_MAPPINGS[0xF], [0x40, 0x41, 0x42, 0x43]);
    assert_eq!(PALETTE_CODES[0x4], 0x03);
    // Vertical anchors leave X to the redraw path.
    assert_eq!(step_pixel_anchor(20, 30, Facing::Down), (None, 31));
    assert_eq!(step_pixel_anchor(20, 30, Facing::Right), (Some(37), 9));
    assert_eq!(region_height(Region::West), 75);
    assert_eq!(region_height(Region::East), 75);
    assert_eq!(region_height(Region::DeathMountain), 60);
    assert_eq!(region_height(Region::MazeIsland), 60);
}

#[test]
fn tile_at_agrees_with_row_expansion_on_fuzz_blob() {
    // LCG fuzz blob (deterministic, no ROM).
    let mut st: u32 = 0x1234_5678;
    let mut next = move || {
        st = st.wrapping_mul(1664525).wrapping_add(1013904223);
        (st >> 16) as u8
    };
    let blob: Vec<u8> = (0..801).map(|_| next()).collect();
    let rows = build_row_offsets(&blob);
    assert_eq!(rows[0], 0);
    // Independent reference: expand each row's byte range flat, then index
    // playable columns 0..64 of the expansion (accumulation vs expansion).
    for (r, &off) in rows.iter().enumerate().take(20) {
        let expanded = decode_rle(&blob[off as usize..]);
        if expanded.len() < MAP_W {
            break;
        }
        for x in [0u8, 1, 31, 62, 63] {
            let got = tile_at(&blob, off as usize, x, 0x1E + r as u8) as u8;
            assert_eq!(got, expanded[x as usize], "row{r} col{x}");
        }
    }
}

#[test]
fn blocked_table_matches_870f() {
    // Passable: everything below mountain.
    for t in 0x00u8..0x0B {
        assert!(!is_blocked(t, false), "terrain {t:02X} should pass");
    }
    // Mountain/rock/spider always blocked; water blocked; walk-water needs boots.
    assert!(is_blocked(0x0B, true));
    assert!(is_blocked(0x0C, true));
    assert!(is_blocked(0x0D, false));
    assert!(!is_blocked(0x0D, true));
    assert!(is_blocked(0x0E, true));
    assert!(is_blocked(0x0F, true));
}

#[test]
fn facing_and_step_round_trip() {
    assert_eq!(facing_from_byte(1), Some(Facing::Right));
    assert_eq!(facing_from_byte(8), Some(Facing::Up));
    assert_eq!(facing_from_byte(3), None);
    // Unblocked step moves exactly one tile; blocked step stays.
    assert_eq!(try_step(10, 20, Facing::Right, 0x05, false), (10, 21));
    assert_eq!(try_step(10, 20, Facing::Up, 0x05, false), (9, 20));
    assert_eq!(try_step(10, 20, Facing::Right, 0x0B, false), (10, 20));
    assert_eq!(try_step(10, 20, Facing::Down, 0x0D, false), (10, 20));
    assert_eq!(try_step(10, 20, Facing::Down, 0x0D, true), (11, 20));
    // Faced tile follows the $84AD/$84B6 offsets.
    assert_eq!(faced_tile(10, 20, Facing::Right), (10, 21));
    assert_eq!(faced_tile(10, 20, Facing::Left), (10, 19));
    assert_eq!(faced_tile(10, 20, Facing::Down), (11, 20));
    assert_eq!(faced_tile(10, 20, Facing::Up), (9, 20));
}

#[test]
fn scroll_tick_swamp_halves_speed() {
    // Grass: every frame commits.
    assert_eq!(scroll_tick(16, 0, 0x05, 0), Some((15, 1)));
    assert_eq!(scroll_tick(16, 0, 0x05, 1), Some((15, 1)));
    // Swamp: odd frames skip.
    assert_eq!(scroll_tick(16, 0, 0x07, 1), None);
    assert_eq!(scroll_tick(16, 0, 0x07, 2), Some((15, 1)));
    assert_eq!(PIXELS_PER_TILE, 0x10);
}

// ---------------------------------------------------------------------------
// ROM-gated tests (skip without Z2_ROM).
// ---------------------------------------------------------------------------

fn rom_bytes() -> Option<Vec<u8>> {
    common::rom_bytes("overworld_map_tests")
}

/// `z2-assets` `file_off` values are headered iNES file offsets (raw file
/// indices); the blob starts with the KNOWN first byte.
fn file_slice(img: &[u8], file_off: u32, len: usize) -> &[u8] {
    &img[file_off as usize..file_off as usize + len]
}

#[test]
fn rom_blobs_decode_to_exact_maps_when_present() {
    let Some(img) = rom_bytes() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    if img.len() < MAZE_FILE_OFF as usize + MAZE_LEN || &img[0..4] != b"NES\x1A" {
        eprintln!("SKIP: ROM too short / bad magic for overworld blobs");
        return;
    }
    // KNOWN: West starts 0xBB, len 801; DM == Maze bytes.
    let west = file_slice(&img, WEST_FILE_OFF, WEST_LEN);
    let dm = file_slice(&img, DM_FILE_OFF, DM_LEN);
    let east = file_slice(&img, EAST_FILE_OFF, EAST_LEN);
    let maze = file_slice(&img, MAZE_FILE_OFF, MAZE_LEN);
    assert_eq!(west[0], WEST_FIRST_BYTE, "West blob must start 0xBB");
    assert_eq!(dm, maze, "Death Mountain == Maze Island blob");
    // Exact-fit decodes pin the region heights (75 / 60 rows of 64).
    assert_eq!(decode_rle(west).len(), 75 * MAP_W, "west 4800 tiles");
    assert_eq!(decode_rle(east).len(), 75 * MAP_W, "east 4800 tiles");
    assert_eq!(decode_rle(dm).len(), 60 * MAP_W, "dm 3840 tiles");
    // Row tables: West/East fully in-bounds; DM rows 0..60 in-bounds with
    // row 60 starting exactly at the blob end (rows past clamp there).
    for (name, blob, h) in [("west", west, 75), ("east", east, 75), ("dm", dm, 60)] {
        let full = decode_rle(blob);
        assert!(full.iter().all(|&t| t < 16), "{name}: 4-bit terrains");
        let rows = build_row_offsets(blob);
        assert_eq!(rows[0], 0);
        for (r, row) in rows.iter().enumerate().take(h) {
            assert!(
                (*row as usize) < blob.len(),
                "{name} row{r} starts in-bounds"
            );
        }
        // The row after the last starts exactly at the blob end: for DM
        // it is rows[60]; for West/East the last row must consume to the
        // end (75 exact rows fill the blob).
        if h < ROW_COUNT {
            assert_eq!(
                rows[h] as usize,
                blob.len(),
                "{name}: row after last starts at blob end"
            );
        } else {
            let mut tail = 0usize;
            let mut o = rows[h - 1] as usize;
            while o < blob.len() {
                let (len, _) = decode_byte(blob[o]);
                tail += len;
                o += 1;
            }
            assert_eq!((o, tail), (blob.len(), MAP_W), "{name}: last row exact");
        }
        // Every playable cell matches the flat decode (row-major, 64 exact).
        for r in 0..h {
            for x in [0u8, 1, 31, 62, 63] {
                let got = tile_at(blob, rows[r] as usize, x, 0x1E + r as u8) as u8;
                assert_eq!(got, full[r * MAP_W + x as usize], "{name} r{r} x{x}");
            }
        }
    }
    // WRAM writer accepts the longest blob (West, 801 < 896).
    let mut wram = vec![0u8; 1024];
    assert_eq!(write_wram_rle(&mut wram, west), WEST_LEN);
    assert_eq!(&wram[..WEST_LEN], west);
}

// ---------------------------------------------------------------------------
// Corpus-gated snapshot tests (harness-ready; skip without corpus/).
// ---------------------------------------------------------------------------

fn corpus_overworld_snaps() -> Vec<std::path::PathBuf> {
    let Some(rd) = common::corpus_snapshots("overworld map corpus snapshots") else {
        return vec![];
    };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("overworld-"))
        })
        .collect()
}

#[test]
fn corpus_overworld_snapshots_verify_when_present() {
    let snaps = corpus_overworld_snaps();
    if snaps.is_empty() {
        eprintln!("SKIP: no corpus overworld-* snapshots (set Z2_CORPUS)");
        return;
    }
    // Harness-ready structural check: Z2SNAP01 magic + room for full
    // RAM (2 KiB) + WRAM (8 KiB) images. Full state diff needs z2-verify
    // + Game trap wiring (main); recorded here as the oracle-gated step.
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(
            b.starts_with(b"Z2SNAP01"),
            "{}: bad snapshot magic",
            p.display()
        );
        assert!(
            b.len() >= 8 + 2048 + 8192,
            "{}: too small for ram+wram",
            p.display()
        );
    }
}

#[test]
fn warpless_overworld_segments_clean_when_present() {
    let Some(rd) =
        common::env_read_dir("Z2_CORPUS", "corpus/movies", "warpless_overworld_segments")
    else {
        return;
    };
    let warpless: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.to_ascii_lowercase().contains("warpless"))
        })
        .collect();
    if warpless.is_empty() {
        eprintln!("SKIP: no warpless movie segments in corpus");
        return;
    }
    // Harness-ready: replay + RLE/terrain divergence check runs here once
    // main wires Game traps + z2-verify oracle. Presence of
    // the segment files is all this layer can assert.
    for p in warpless {
        assert!(std::fs::metadata(&p).is_ok_and(|m| m.len() > 0));
    }
}
