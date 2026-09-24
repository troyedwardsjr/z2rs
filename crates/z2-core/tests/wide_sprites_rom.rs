//! ROM-gated checks of the widescreen margin-sprite observer
//! (`z2_core::wide_sprites`). Skips unless `Z2_ROM` is a file and the
//! warpless movie is found (`Z2_WARPLESS_MOVIE`, `Z2_MOVIES/warpless.fm2` or
//! `Z2_CORPUS/movies/warpless.fm2`).
//!
//! * lockstep: with the observer on, RAM, WRAM, CPU state and the indexed
//!   frame stay byte-identical to an unobserved run, every frame;
//! * the observer produces margin sprites in side view, and only there;
//! * the latched list agrees with PPU OAM for the part of an object still
//!   inside the window (right-edge columns the game did draw; at least 80%);
//! * `compose_wide` draws them and keeps the centre verbatim.
//!
//! `Z2_WIDE_SPRITES_FRAMES` overrides the frame count (default 4000).

#![cfg(feature = "interp")]

mod common;

use std::path::PathBuf;

use z2_core::game::Game;
use z2_core::wide_margins::{ADDR_GAME_MODE, MODE_SIDEVIEW};
use z2_ppu::{Margins, WideFrame, WIDTH};

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

fn warpless_pads(test: &str) -> Option<Vec<u8>> {
    let candidates = [
        common::var_present("Z2_WARPLESS_MOVIE"),
        common::var_present("Z2_MOVIES").map(|m| format!("{m}/warpless.fm2")),
        common::var_present("Z2_CORPUS").map(|c| format!("{c}/movies/warpless.fm2")),
    ];
    let Some(path) = candidates
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    else {
        eprintln!("skipping {test}: warpless movie not found");
        return None;
    };
    let text = std::fs::read_to_string(&path).ok()?;
    Some(z2_verify::movie_fm2::parse_fm2(&text).ok()?.pad1_track())
}

fn frames() -> usize {
    std::env::var("Z2_WIDE_SPRITES_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4000)
}

#[test]
fn observer_is_display_only_and_fills_the_margins() {
    const T: &str = "observer_is_display_only_and_fills_the_margins";
    let Some(raw) = common::rom_bytes(T) else {
        return;
    };
    let Some(pads) = warpless_pads(T) else {
        return;
    };
    let mk = |observe: bool| {
        let mut g = Game::from_ines(&raw).expect("rom loads");
        register_all_groups(&mut g);
        g.set_record(true);
        if observe {
            g.set_margin_sprites(true);
        }
        g.reset();
        g
    };
    let mut plain = mk(false);
    let mut seen = mk(true);
    assert!(!plain.margin_sprites_enabled());
    assert!(plain
        .traps
        .iter()
        .all(|t| !z2_core::wide_sprites::is_display_only_trap(t.name)));

    let (mut sv_frames, mut with_sprites, mut total, mut right_checked) = (0, 0, 0usize, 0);
    let mut first: Option<usize> = None;
    let mut edge_hits = 0usize;
    let mut margins = Margins::new(11);
    let mut wide = WideFrame::new(11);
    for (f, &p) in pads.iter().enumerate().take(frames()) {
        plain.step(p);
        seen.step(p);
        assert_eq!(plain.ram(), seen.ram(), "RAM diverged at frame {f}");
        assert_eq!(plain.wram(), seen.wram(), "WRAM diverged at frame {f}");
        assert_eq!(
            plain.cpu_state(),
            seen.cpu_state(),
            "CPU diverged at frame {f}"
        );
        assert_eq!(
            plain.cpu.cycles, seen.cpu.cycles,
            "cycle count diverged at frame {f}"
        );
        assert_eq!(
            plain.frame_indexed()[..],
            seen.frame_indexed()[..],
            "frame diverged at frame {f}"
        );
        assert!(plain.sideview_margin_sprites().is_empty());
        let list = seen.sideview_margin_sprites();
        let sideview = seen.ram()[usize::from(ADDR_GAME_MODE)] == MODE_SIDEVIEW;
        sv_frames += usize::from(sideview);
        if !list.is_empty() {
            with_sprites += 1;
            total += list.len();
            first.get_or_insert(f);
        }
        for s in list {
            assert!(
                s.x < 0 || i32::from(s.x) > WIDTH as i32 - 8,
                "frame {f}: {s:?} lies inside the window"
            );
            // A column straddling the right edge is normally also in OAM, at
            // the same Y/tile/attr and X = window x (the game hides a few of
            // them with its column mask).
            if (249..256).contains(&s.x) {
                let oam = seen.oam();
                let hit = oam.chunks_exact(4).any(|e| {
                    e[0] == s.y && e[1] == s.tile && e[2] == s.attr && i16::from(e[3]) == s.x
                });
                edge_hits += usize::from(hit);
                right_checked += 1;
            }
        }
        if !list.is_empty() && f % 7 == 0 {
            assert!(seen.compose_wide(11, &mut margins, &mut wide));
            assert_eq!(margins.sprites, list, "compose_wide carries the list");
            let mp = wide.margin_px();
            for y in 0..240 {
                assert_eq!(
                    &wide.row(y)[mp..mp + WIDTH],
                    &seen.frame_indexed()[y * WIDTH..(y + 1) * WIDTH],
                    "frame {f} line {y}: centre not verbatim"
                );
            }
        }
    }
    assert_eq!(seen.margin_sprite_errors(), 0, "shadow runs failed");
    println!(
        "{} frames: side view {sv_frames}, with margin sprites {with_sprites} (first {first:?}), \
         entries {total}, right-edge columns found in OAM {edge_hits}/{right_checked}",
        pads.len().min(frames())
    );
    // Whole warpless movie: 1030 of 1105 right-edge columns (93%) are in OAM.
    assert!(
        edge_hits * 10 >= right_checked * 8,
        "right-edge columns disagree with OAM: {edge_hits}/{right_checked}"
    );
    assert!(with_sprites > 0, "no margin sprites in {} frames", frames());
}
