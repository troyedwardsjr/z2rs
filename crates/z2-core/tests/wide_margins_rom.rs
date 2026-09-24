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
