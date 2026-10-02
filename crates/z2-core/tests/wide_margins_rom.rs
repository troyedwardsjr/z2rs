//! ROM-gated widescreen checks (skip unless `Z2_ROM` is a file and the any%
//! movie is found via `Z2_ANY_PERCENT_MOVIE`, `Z2_MOVIES/anypct.bk2` or
//! `Z2_CORPUS/movies/anypct.bk2`).
//!
//! * scroll consistency: margin tiles predicted while a world column was
//!   outside the window match what the NES later draws there;
//! * `compose_wide` centre is byte-identical to `Game::frame`;
//! * the record never changes the frame or RAM.
//!
//! Optional: `Z2_WIDE_PNG_DIR` dumps a few wide frames as PNGs there (never
//! into the repository — they contain ROM graphics).

#![cfg(feature = "interp")]

mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use z2_core::game::Game;
use z2_core::wide_margins::{
    build_margins, margin_policy, overworld_row, overworld_world_left, sideview_world_left,
    MarginPolicy, ADDR_AREA_PRG_BANK, ADDR_GAME_MODE, ADDR_GROUND_TYPE, ADDR_MENU, MODE_OVERWORLD,
    MODE_SIDEVIEW,
};
use z2_ppu::{edge_fill, wide_bg_tile, FrameRecord, MarginFill, Margins, WideFrame, WIDTH};

fn register_all_groups(g: &mut Game) {
    z2_core::bank7_traps::register_bank7_traps(g);
    z2_core::sideview_traps::register_sideview_traps(g);
    z2_core::sideview_traps::register_overworld_traps(g);
    z2_core::player_traps::register_player_traps(g);
    z2_core::enemy_traps::register_enemy_traps(g);
    z2_core::town_traps::register_town_traps(g);
    z2_core::palace_traps::register_palace_traps(g);
    z2_core::title_traps::register_title_traps(g);
    z2_core::boot_traps::register_boot_traps(g);
}

fn rom_game() -> Option<Game> {
    let raw = common::rom_bytes("wide_margins_rom")?;
    let mut g = Game::from_ines(&raw).ok()?;
    register_all_groups(&mut g);
    g.reset();
    Some(g)
}

fn any_percent_pads() -> Option<Vec<u8>> {
    let candidates = [
        std::env::var("Z2_ANY_PERCENT_MOVIE").ok(),
        std::env::var("Z2_MOVIES")
            .ok()
            .map(|m| format!("{m}/anypct.bk2")),
        std::env::var("Z2_CORPUS")
            .ok()
            .map(|c| format!("{c}/movies/anypct.bk2")),
    ];
    let path = candidates
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.is_file())?;
    let bytes = std::fs::read(&path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&bytes).ok()?;
    Some(movie.pad1_track())
}

fn setup() -> Option<(Game, Vec<u8>)> {
    let Some(g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset or not a file");
        return None;
    };
    let Some(pads) = any_percent_pads() else {
        eprintln!("SKIP: any% movie not found (Z2_ANY_PERCENT_MOVIE / Z2_MOVIES / Z2_CORPUS)");
        return None;
    };
    Some((g, pads))
}

#[derive(Default)]
struct Tally {
    compared: u64,
    matched: u64,
    first_miss: Vec<String>,
}

impl Tally {
    fn rate(&self) -> f64 {
        if self.compared == 0 {
            0.0
        } else {
            self.matched as f64 / self.compared as f64
        }
    }
}

/// Prediction key: (policy tag, CHR page, world tile column, row key a, row key b).
type Key = (u8, u8, i32, i32, u8);

#[derive(Clone, Copy)]
struct Pred {
    frame: usize,
    epoch: u64,
    tile: u8,
    pal: u8,
}

const MAX_AGE: usize = 64;

#[test]
fn scroll_consistency_sideview_and_overworld() {
    let Some((mut g, pads)) = setup() else {
        return;
    };
    g.set_record(true);
    let frames = pads.len().min(6000);
    let mut margins = Margins::new(0);
    let mut preds: HashMap<Key, Pred> = HashMap::new();
    let mut epoch = 0u64;
    let mut prev_sig: Vec<u8> = Vec::new();
    let mut sv = Tally::default();
    let mut ow = Tally::default();
    let mut sv_edge = Tally::default();
    let mut ow_edge = Tally::default();
    for (t, &pad) in pads.iter().enumerate().take(frames) {
        g.step(pad);
        let mode = g.ram[usize::from(ADDR_GAME_MODE)];
        if mode != MODE_SIDEVIEW && mode != MODE_OVERWORLD {
            preds.clear();
            prev_sig.clear();
            continue;
        }
        // Epoch: any change of the level data invalidates older predictions
        // (breakable blocks, area/world changes, mode switches).
        let mut sig = vec![
            mode,
            g.ram[usize::from(ADDR_AREA_PRG_BANK)],
            g.ram[usize::from(ADDR_GROUND_TYPE)],
        ];
        if mode == MODE_SIDEVIEW {
            sig.extend_from_slice(&g.wram[..0x340]);
        } else {
            sig.extend_from_slice(&g.wram[0x1C00..]);
        }
        if sig != prev_sig {
            epoch += 1;
            prev_sig = sig;
        }
        let ram = g.ram;
        let menu = ram[usize::from(ADDR_MENU)];
        let rec: FrameRecord = g.frame_record().expect("record on").clone();
        build_margins(&g.ram, &g.wram, &g.prg, &rec, 11, &mut margins);

        for y in 0..240 {
            let line = rec.line(y);
            let policy = margin_policy(mode, menu, line);
            let (tag, wt0, ra, rb) = match policy {
                MarginPolicy::Sideview => (
                    0u8,
                    sideview_world_left(&ram, line) >> 3,
                    i32::from(FrameRecord::coarse_y(line)),
                    0u8,
                ),
                MarginPolicy::Overworld => {
                    let (r, parity) = overworld_row(&ram, line, y);
                    (1u8, overworld_world_left(&ram, line) >> 3, r, parity)
                }
                MarginPolicy::Backdrop => continue,
            };
            let page = FrameRecord::bg_page(line);
            // 1. Compare what the NES drew against earlier margin predictions.
            //    Slots 2..=30 are fully inside the window and settled; slots
            //    0/32 are partial edge tiles and 1/31 are the columns the
            //    ROM is still streaming (tiles and attributes arrive in
            //    different stages), so those are tallied separately.
            if !line.split {
                for k in 1..=31usize {
                    let edge = k == 1 || k == 31;
                    let tally = match (tag, edge) {
                        (0, false) => &mut sv,
                        (0, true) => &mut sv_edge,
                        (_, false) => &mut ow,
                        (_, true) => &mut ow_edge,
                    };
                    let actual = line.tiles[k];
                    if !actual.fetched {
                        continue;
                    }
                    let key = (tag, page, wt0 + k as i32, ra, rb);
                    let Some(p) = preds.get(&key) else { continue };
                    if p.epoch != epoch || p.frame + MAX_AGE < t || p.frame == t {
                        continue;
                    }
                    tally.compared += 1;
                    if (p.tile, p.pal) == (actual.tile, actual.pal) {
                        tally.matched += 1;
                    } else {
                        if std::env::var_os("Z2_WIDE_DIAG").is_some() {
                            eprintln!(
                                "DIAG t {t} age {} y {y} k {k} tile_eq {} pal {}->{}",
                                t - p.frame,
                                p.tile == actual.tile,
                                p.pal,
                                actual.pal
                            );
                        }
                    }
                    if (p.tile, p.pal) != (actual.tile, actual.pal) && tally.first_miss.len() < 8 {
                        tally.first_miss.push(format!(
                            "frame {t} (pred {}) y {y} k {k} wt {} row ({ra},{rb}) pred {:02X}/{} actual {:02X}/{}",
                            p.frame,
                            wt0 + k as i32,
                            p.tile,
                            p.pal,
                            actual.tile,
                            actual.pal
                        ));
                    }
                }
            }
            // 2. Remember this frame's margin predictions.
            let ml = &margins.lines[y];
            if ml.fill != MarginFill::Tiles {
                continue;
            }
            for j in 0..=usize::from(margins.tiles) {
                for (wt, id) in [
                    (wt0 - 1 - j as i32, ml.left[j]),
                    (wt0 + 32 + j as i32, ml.right[j]),
                ] {
                    preds.insert(
                        (tag, page, wt, ra, rb),
                        Pred {
                            frame: t,
                            epoch,
                            tile: id.tile,
                            pal: id.pal,
                        },
                    );
                }
            }
        }
    }
    eprintln!(
        "edge columns (k = 1 / 31, still streaming on the NES): sideview {}/{} = {:.4}, overworld {}/{} = {:.4}",
        sv_edge.matched,
        sv_edge.compared,
        sv_edge.rate(),
        ow_edge.matched,
        ow_edge.compared,
        ow_edge.rate()
    );
    eprintln!(
        "sideview: {}/{} = {:.4}; overworld: {}/{} = {:.4}",
        sv.matched,
        sv.compared,
        sv.rate(),
        ow.matched,
        ow.compared,
        ow.rate()
    );
    for m in sv.first_miss.iter().chain(ow.first_miss.iter()) {
        eprintln!("  miss: {m}");
    }
    assert!(sv.compared > 10_000, "too few sideview comparisons");
    assert!(ow.compared > 10_000, "too few overworld comparisons");
    assert!(sv.rate() >= 0.99, "sideview match rate {:.4}", sv.rate());
    assert!(ow.rate() >= 0.99, "overworld match rate {:.4}", ow.rate());
}

#[test]
fn compose_wide_centre_is_the_frame() {
    let Some((mut g, pads)) = setup() else {
        return;
    };
    g.set_record(true);
    let mut margins = Margins::new(0);
    let mut wide = WideFrame::new(0);
    let mut tile_frames = 0;
    for (t, &pad) in pads.iter().enumerate().take(2000) {
        g.step(pad);
        let tiles = [11u8, 8, 16, 1][t % 4];
        assert!(g.compose_wide(tiles, &mut margins, &mut wide));
        let mp = wide.margin_px();
        assert_eq!(mp, 8 * usize::from(tiles));
        for y in 0..240 {
            assert_eq!(
                &wide.row(y)[mp..mp + WIDTH],
                &g.frame[y * WIDTH..(y + 1) * WIDTH],
                "frame {t} row {y}"
            );
        }
        if margins.lines.iter().any(|l| l.fill == MarginFill::Tiles) {
            tile_frames += 1;
        }
    }
    eprintln!("frames with tile margins: {tile_frames}/2000");
    assert!(tile_frames > 500, "expected level frames in the first 2000");
    // Record off: false, centre still copied, margins backdrop.
    g.set_record(false);
    let mut m2 = Margins::new(0);
    assert!(!g.wide_margins(11, &mut m2));
    assert!(!g.compose_wide(11, &mut margins, &mut wide));
    assert_eq!(wide.width, 432);
    assert!(margins.lines.iter().all(|l| l.fill == MarginFill::Backdrop));
    assert_eq!(
        &wide.row(120)[88..88 + WIDTH],
        &g.frame[120 * WIDTH..121 * WIDTH]
    );
}

#[test]
fn record_does_not_change_frame_or_ram() {
    let Some((mut on, pads)) = setup() else {
        return;
    };
    let mut off = rom_game().expect("ROM loaded once already");
    on.set_record(true);
    assert!(on.record_enabled() && !off.record_enabled());
    let mut margins = Margins::new(11);
    for (t, &pad) in pads.iter().enumerate().take(2000) {
        on.step(pad);
        off.step(pad);
        // Consuming the record must not perturb anything either.
        assert!(on.wide_margins(11, &mut margins));
        assert!(on.frame[..] == off.frame[..], "frame {t}: pixels differ");
        assert!(on.ram[..] == off.ram[..], "frame {t}: RAM differs");
        assert!(on.wram[..] == off.wram[..], "frame {t}: WRAM differs");
    }
}

#[test]
fn dump_wide_pngs_when_requested() {
    let Some(dir) = common::var_present("Z2_WIDE_PNG_DIR").map(PathBuf::from) else {
        eprintln!("skipping dump_wide_pngs_when_requested: Z2_WIDE_PNG_DIR is not set");
        return;
    };
    let Some((mut g, pads)) = setup() else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("create png dir");
    g.set_record(true);
    let wanted: Vec<usize> = std::env::var("Z2_WIDE_PNG_FRAMES")
        .ok()
        .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![300, 560, 1400]);
    let last = wanted.iter().copied().max().unwrap_or(0);
    let mut margins = Margins::new(11);
    let mut wide = WideFrame::new(11);
    for (t, &pad) in pads.iter().enumerate().take(last + 1) {
        g.step(pad);
        if wanted.contains(&t) {
            margins.fill_left_clip = std::env::var_os("Z2_WIDE_FILL_CLIP").is_some();
            g.compose_wide(11, &mut margins, &mut wide);
            let png = z2_ppu::encode_indexed_png_wh(wide.width, wide.height, &wide.pixels);
            let mode = g.ram[usize::from(ADDR_GAME_MODE)];
            let path = dir.join(format!("wide_{t:05}_mode{mode:02X}.png"));
            std::fs::write(&path, png).expect("write png");
            eprintln!("wrote {}", path.display());
        }
    }
}

/// The overworld's edge strips (window x 0-7 and 248-255) are what the ROM
/// hides, and for good reason: its nametable is a single 32-column ring, so
/// window slots 0 and 32 share one nametable column, and the column being
/// streamed sits half-written in slot 1 or 31. Painting the strips from the
/// recorded tiles showed those stale columns as seams. With the fills on,
/// the strips must show the map instead: every strip pixel's tile identity
/// is compared with what the NES draws for the same world cell once it has
/// scrolled into the settled interior (slots 2..=30) — the same scroll
/// consistency check as the margins — and the recorded identities are
/// tallied alongside to pin the root cause.
#[test]
fn overworld_edge_strips_follow_the_map() {
    let Some((mut g, pads)) = setup() else {
        return;
    };
    g.set_record(true);
    let chr = g.chr.clone();
    let mut margins = Margins::new(11);
    margins.fill_left_clip = true;
    margins.fill_right_clip = true;
    // (world tile column, map row, parity) -> every (frame, strip id, record
    // id) shown there that has not been checked yet.
    type Shown = (usize, u8, u8, u8, u8);
    let mut preds: HashMap<(i32, i32, u8), Vec<Shown>> = HashMap::new();
    let mut prev_map: Vec<u8> = Vec::new();
    let (mut strip_lines, mut compared, mut fixed_ok, mut record_ok) = (0u64, 0u64, 0u64, 0u64);
    let mut first_miss: Vec<String> = Vec::new();
    for (t, &pad) in pads.iter().enumerate().take(pads.len().min(6000)) {
        g.step(pad);
        if g.ram[usize::from(ADDR_GAME_MODE)] != MODE_OVERWORLD {
            preds.clear();
            continue;
        }
        if g.wram[0x1C00..] != prev_map[..] {
            preds.clear();
            prev_map = g.wram[0x1C00..].to_vec();
        }
        let rec: FrameRecord = g.frame_record().expect("record on").clone();
        build_margins(&g.ram, &g.wram, &g.prg, &rec, 11, &mut margins);
        for y in 0..240 {
            let line = rec.line(y);
            if margin_policy(MODE_OVERWORLD, 0, line) != MarginPolicy::Overworld || line.split {
                continue;
            }
            let wt0 = overworld_world_left(&g.ram, line) >> 3;
            let (r, parity) = overworld_row(&g.ram, line, y);
            // Settled interior: check earlier strip predictions.
            for k in 2..=30usize {
                let actual = line.tiles[k];
                let key = (wt0 + k as i32, r, parity);
                if !actual.fetched {
                    continue;
                }
                let Some(shown) = preds.get_mut(&key) else {
                    continue;
                };
                for &(f, tile, pal, rtile, rpal) in shown.iter() {
                    if f == t || f + MAX_AGE < t {
                        continue;
                    }
                    compared += 1;
                    if (tile, pal) == (actual.tile, actual.pal) {
                        fixed_ok += 1;
                    } else if first_miss.len() < 8 {
                        first_miss.push(format!(
                            "frame {t} (strip at {f}) y {y} wt {} pred {tile:02X}/{pal} actual {:02X}/{}",
                            key.0, actual.tile, actual.pal
                        ));
                    }
                    if (rtile, rpal) == (actual.tile, actual.pal) {
                        record_ok += 1;
                    }
                }
                shown.retain(|&(f, ..)| f == t);
            }
            // Strips shown this frame.
            let fill = edge_fill(&rec, &margins, y, &chr);
            assert!(
                fill.left,
                "frame {t} line {y}: the overworld clips the left 8 columns"
            );
            let ml = &margins.lines[y];
            let mut any = false;
            let strips = (0..8).chain(if fill.right { 248..256 } else { 0..0 });
            for wx in strips {
                let (id, sub_x) = wide_bg_tile(line, Some(ml), fill, wx);
                if sub_x != 0 && wx != 0 && wx != 248 {
                    continue; // one sample per slot
                }
                let k = (wx + i32::from(line.fine_x & 7)) >> 3;
                let rid = line.tiles[k as usize];
                assert!(id.fetched, "frame {t} line {y} x {wx}: strip has a tile");
                preds
                    .entry((wt0 + k, r, parity))
                    .or_default()
                    .push((t, id.tile, id.pal, rid.tile, rid.pal));
                any = true;
            }
            strip_lines += u64::from(any);
        }
    }
    let rate = |ok: u64| ok as f64 / compared.max(1) as f64;
    eprintln!(
        "overworld strips: {strip_lines} lines; vs later interior: map {fixed_ok}/{compared} = {:.4}, record {record_ok}/{compared} = {:.4}",
        rate(fixed_ok),
        rate(record_ok)
    );
    for m in &first_miss {
        eprintln!("  miss: {m}");
    }
    assert!(compared > 10_000, "too few strip comparisons ({compared})");
    assert!(
        rate(fixed_ok) >= 0.99,
        "strip/map match rate {:.4}",
        rate(fixed_ok)
    );
    assert!(
        rate(record_ok) < 0.95,
        "the recorded strip tiles were expected to be the stale ones"
    );
}

/// Overworld frames of the any% movie replayed by the seam tests.
const SEAM_FRAMES: usize = 20_000;

/// Window-x boxes `[a, b)` of every sprite row on line `y`, recorded or
/// margin.
fn sprite_boxes(rec: &FrameRecord, margins: &Margins, y: usize) -> Vec<(i32, i32)> {
    let line = rec.line(y);
    let mut boxes: Vec<(i32, i32)> = rec
        .sprites_on(y)
        .iter()
        .map(|s| (i32::from(s.x), i32::from(s.x) + 8))
        .collect();
    boxes.extend(
        margins
            .sprites
            .iter()
            .filter(|s| z2_ppu::margin_sprite_on_line(line, y, s).is_some())
            .map(|s| (i32::from(s.x), i32::from(s.x) + 8)),
    );
    boxes
}

/// The flashing "sticks" of the Discord report: with `fine_x != 0`, part of
/// the half-written edge slot 31 (or slot 1) lies *outside* the hidden 8
/// columns, and the NES draws it there in the wrong tiles or palette. On a
/// TV that is a sliver beside the black bar; next to a painted margin it is
/// a column of wrong scenery flashing in the middle of the picture.
///
/// Every background pixel the wide image shows in the bands around both
/// seams (window x -16..24 and 232..272) is compared with the colour the
/// NES draws at the same world pixel once it has scrolled into the settled
/// interior (window x 24..232). The wide image must agree; the raw frame in
/// the in-window part of those bands is tallied alongside and is expected
/// to disagree measurably (the root cause).
#[test]
fn overworld_seam_columns_are_stable_across_a_scroll() {
    let Some((mut g, pads)) = setup() else {
        return;
    };
    g.set_record(true);
    let mut margins = Margins::new(11);
    margins.fill_left_clip = true;
    margins.fill_right_clip = true;
    let mut wide = WideFrame::new(11);
    let mp = 88i32;
    // (world x, map row, parity, fine y) -> [(frame, wide colour, raw colour
    // or 0xFF where the raw frame has no pixel)]
    type Shown = (usize, u8, u8);
    let mut preds: HashMap<(i32, i32, u8, u8), Vec<Shown>> = HashMap::new();
    let mut prev_map: Vec<u8> = Vec::new();
    let (mut compared, mut wide_ok, mut raw_compared, mut raw_ok) = (0u64, 0u64, 0u64, 0u64);
    let mut first_miss: Vec<String> = Vec::new();
    for (t, &pad) in pads.iter().enumerate().take(pads.len().min(SEAM_FRAMES)) {
        g.step(pad);
        if g.ram[usize::from(ADDR_GAME_MODE)] != MODE_OVERWORLD {
            preds.clear();
            continue;
        }
        if g.wram[0x1C00..] != prev_map[..] {
            preds.clear();
            prev_map = g.wram[0x1C00..].to_vec();
        }
        if !g.compose_wide(11, &mut margins, &mut wide) {
            continue;
        }
        let rec: FrameRecord = g.frame_record().expect("record on").clone();
        let frame = g.frame_indexed();
        for y in 0..240 {
            let line = rec.line(y);
            if margins.lines[y].fill != MarginFill::Tiles || line.split {
                continue;
            }
            let wl = overworld_world_left(&g.ram, line);
            let (r, parity) = overworld_row(&g.ram, line, y);
            let fy = FrameRecord::fine_y(line);
            let boxes = sprite_boxes(&rec, &margins, y);
            let covered = |wx: i32| boxes.iter().any(|&(a, b)| (a..b).contains(&wx));
            // Settled interior: check what the seams showed earlier.
            for wx in 24..232 {
                if covered(wx) {
                    continue;
                }
                let Some(shown) = preds.get_mut(&(wl + wx, r, parity, fy)) else {
                    continue;
                };
                let actual = frame[y * WIDTH + wx as usize];
                for &(f, wc, rc) in shown.iter() {
                    if f == t || f + MAX_AGE < t {
                        continue;
                    }
                    compared += 1;
                    if wc == actual {
                        wide_ok += 1;
                    } else if first_miss.len() < 8 {
                        first_miss.push(format!(
                            "frame {t} (seam at {f}) y {y} world x {} wide {wc:02X} settled {actual:02X}",
                            wl + wx
                        ));
                    }
                    if rc != 0xFF {
                        raw_compared += 1;
                        raw_ok += u64::from(rc == actual);
                    }
                }
                shown.retain(|&(f, ..)| f == t);
            }
            // Seam bands shown this frame.
            for wx in (-16..24).chain(232..272) {
                if covered(wx) {
                    continue;
                }
                let wc = wide.pixels[y * wide.width + (wx + mp) as usize];
                let rc = if (8..24).contains(&wx) || (232..248).contains(&wx) {
                    frame[y * WIDTH + wx as usize]
                } else {
                    0xFF
                };
                preds
                    .entry((wl + wx, r, parity, fy))
                    .or_default()
                    .push((t, wc, rc));
            }
        }
    }
    let rate = |ok: u64, n: u64| ok as f64 / n.max(1) as f64;
    eprintln!(
        "overworld seams vs settled interior: wide {wide_ok}/{compared} = {:.5}, raw frame (in-window bands) {raw_ok}/{raw_compared} = {:.5}",
        rate(wide_ok, compared),
        rate(raw_ok, raw_compared)
    );
    for m in &first_miss {
        eprintln!("  miss: {m}");
    }
    assert!(compared > 100_000, "too few seam comparisons ({compared})");
    assert!(
        rate(wide_ok, compared) >= 0.9995,
        "seam/world match rate {:.5}",
        rate(wide_ok, compared)
    );
    assert!(
        raw_compared - raw_ok > 1_000,
        "the raw frame was expected to show stale edge-slot columns ({} misses)",
        raw_compared - raw_ok
    );
}

/// One 8x16 blob half on screen: window x of its left column, its top line,
/// and whether the PPU drew it (`oam`) or it is a margin sprite.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Half {
    x: i32,
    top: usize,
    oam: bool,
}

/// Expected opaque pixels `(window x, colour)` of one sprite row whose left
/// column is at `x`.
fn row_pixels(
    chr: &[u8],
    line: &z2_ppu::LineRecord,
    s: &z2_ppu::SpriteRef,
    x: i32,
) -> Vec<(i32, u8)> {
    let id = z2_ppu::BgTileId {
        page: s.page,
        tile: s.tile,
        fine_y: s.fine_row & 7,
        fetched: true,
        ..z2_ppu::BgTileId::NONE
    };
    (0..8u8)
        .filter_map(|dx| {
            let col = if s.flip_h { 7 - dx } else { dx };
            z2_ppu::chr_sub(chr, id, col).filter(|&v| v != 0).map(|v| {
                let c = line.palette_entry(0x10 + usize::from(s.pal & 3) * 4 + usize::from(v));
                (x + i32::from(dx), c)
            })
        })
        .collect()
}

/// Overworld encounter blobs crossing the original picture edges must stay
/// whole in the wide image. Before the fix they vanished there: the edge
/// fills repainted x 0-7 and 248-255 with background only (a blob's OAM
/// half there was clipped or under the mask), wide gameplay hid a blob's
/// whole OAM entry once it passed the edge (so the half still inside the
/// window was drawn by nobody), and its margin half was latched one frame
/// ahead of its OAM half after lag frames (the blob split in two).
///
/// Every opaque pixel of every blob half the frame shows (PPU halves from
/// the record, OAM 32-63, and margin halves) must be painted in its sprite
/// colour, and every half must sit next to its partner. Runs with wide
/// gameplay off (the ROM's own despawn) and on (the interactive default).
#[test]
fn overworld_blobs_stay_whole_across_the_picture_edges() {
    for wide_gameplay in [None, Some(11u8)] {
        let Some((_, pads)) = setup() else {
            return;
        };
        let raw = common::rom_bytes("wide_margins_rom").expect("ROM checked by setup");
        let mut g = Game::from_ines(&raw).expect("ROM");
        register_all_groups(&mut g);
        g.set_wide_gameplay(wide_gameplay);
        g.reset();
        g.set_record(true);
        let chr = g.chr.clone();
        let mut margins = Margins::new(11);
        margins.fill_left_clip = true;
        margins.fill_right_clip = true;
        margins.fill_left_sprites = true;
        let mut wide = WideFrame::new(11);
        let mp = 88i32;
        let (mut edge_n, mut edge_ok, mut mid_n, mut mid_ok) = (0u64, 0u64, 0u64, 0u64);
        let (mut halves_n, mut unpaired) = (0u64, 0u64);
        let mut first_miss: Vec<String> = Vec::new();
        for (t, &pad) in pads.iter().enumerate().take(pads.len().min(SEAM_FRAMES)) {
            g.step(pad);
            if g.ram[usize::from(ADDR_GAME_MODE)] != MODE_OVERWORLD {
                continue;
            }
            if !g.compose_wide(11, &mut margins, &mut wide) {
                continue;
            }
            let rec = g.frame_record().expect("record on");
            let mut halves: Vec<Half> = Vec::new();
            for y in 0..240 {
                let line = rec.line(y);
                if margins.lines[y].fill != MarginFill::Tiles {
                    continue;
                }
                let fill = edge_fill(rec, &margins, y, &chr);
                // Expected blob pixels in priority order: PPU halves first.
                let mut want: Vec<(i32, u8)> = Vec::new();
                let line_sprites = rec.sprites_on(y);
                for s in line_sprites.iter().filter(|s| s.oam_index >= 32) {
                    if s.row_in_sprite == 0 {
                        halves.push(Half {
                            x: i32::from(s.x),
                            top: y,
                            oam: true,
                        });
                    }
                    if z2_ppu::edge_drops_window_sprite(fill, s, line_sprites, &chr) {
                        continue; // dropped by design (the ROM's wrapped despawn ghost)
                    }
                    // The PPU stops at x 255; a straddling half's margin
                    // part is the provider's margin sprite.
                    want.extend(
                        row_pixels(&chr, line, s, i32::from(s.x))
                            .into_iter()
                            .filter(|&(wx, _)| wx < WIDTH as i32),
                    );
                }
                for ms in g.overworld_margin_sprites() {
                    let Some(s) = z2_ppu::margin_sprite_on_line(line, y, ms) else {
                        continue;
                    };
                    if s.row_in_sprite == 0 {
                        halves.push(Half {
                            x: i32::from(ms.x),
                            top: y,
                            oam: false,
                        });
                    }
                    want.extend(row_pixels(&chr, line, &s, i32::from(ms.x)));
                }
                let mut seen: Vec<i32> = Vec::new();
                for (wx, c) in want {
                    if !(-mp..256 + mp).contains(&wx) || seen.contains(&wx) {
                        continue; // outside the image, or a lower-priority pixel
                    }
                    seen.push(wx);
                    let got = wide.pixels[y * wide.width + (wx + mp) as usize];
                    let edge = (-16..16).contains(&wx) || (240..272).contains(&wx);
                    let ok = got == c;
                    if edge {
                        edge_n += 1;
                        edge_ok += u64::from(ok);
                    } else {
                        mid_n += 1;
                        mid_ok += u64::from(ok);
                    }
                    if !ok && edge && first_miss.len() < 8 {
                        first_miss.push(format!(
                            "frame {t} y {y} x {wx}: want {c:02X} got {got:02X}"
                        ));
                    }
                }
            }
            // Pairing: each half has a partner 8 px to one side on the same
            // top line (PPU halves pair modulo 256: OAM X wraps).
            halves.dedup();
            for h in &halves {
                halves_n += 1;
                let paired = halves.iter().any(|o| {
                    let d = o.x - h.x;
                    o.top == h.top
                        && (d.abs() == 8
                            || (h.oam && o.oam && matches!(d.rem_euclid(256), 8 | 248)))
                });
                if !paired {
                    unpaired += 1;
                    if first_miss.len() < 16 {
                        first_miss.push(format!("frame {t}: unpaired blob half {h:?}"));
                    }
                }
            }
        }
        let rate = |ok: u64, n: u64| ok as f64 / n.max(1) as f64;
        eprintln!(
            "wide gameplay {wide_gameplay:?}: blob pixels at the edges {edge_ok}/{edge_n} = {:.5}, elsewhere {mid_ok}/{mid_n} = {:.5}; halves {halves_n}, unpaired {unpaired}",
            rate(edge_ok, edge_n),
            rate(mid_ok, mid_n)
        );
        for m in &first_miss {
            eprintln!("  {m}");
        }
        assert!(
            edge_n > 1_000,
            "too few blob pixels at the edges ({edge_n})"
        );
        assert!(
            rate(edge_ok, edge_n) >= 0.99,
            "blob pixels at the picture edges drawn {:.5}",
            rate(edge_ok, edge_n)
        );
        assert!(
            unpaired * 1000 <= halves_n,
            "{unpaired} of {halves_n} blob halves without a partner"
        );
    }
}
