//! Wide gameplay (`z2_core::wide_gameplay`): pure helper pins, default-off
//! and registration pins (synthetic), plus ROM-gated runs over corpus movies:
//!
//! * `M = 0` equivalence: with the mode on at a zero margin the blob port
//!   must reproduce the ROM exactly — RAM (stack page included), WRAM, OAM,
//!   CPU registers and the cycle clock, every frame.
//! * `M > 0` behaviour: blobs are pushed into the margins, stay alive there,
//!   show up as margin sprites and come back into the window.
//!
//! ROM-gated tests self-skip unless `Z2_ROM` points at a file; movies come
//! from `Z2_CORPUS_MOVIES` (default the out-of-tree corpus) and self-skip
//! when absent. `Z2_WIDE_FRAMES` caps the frames replayed per movie.

#![cfg(feature = "interp")]

mod common;

use z2_core::game::Game;
use z2_core::wide_gameplay::{
    self, follow, life_bonus_ticks, out_of_bounds, shift_column, sideview_spawn_shift, spawn_push,
    town_offsets, ADDR_BLOBS, ADDR_SPAWN, TOWN_OFFSETS_VANILLA,
};

// ------------------------------------------------------------ helpers

/// Default trap groups in the frontends' order.
fn register_default_groups(game: &mut Game) {
    z2_core::bank7_traps::register_bank7_traps(game);
    z2_core::sideview_traps::register_sideview_traps(game);
    z2_core::sideview_traps::register_overworld_traps(game);
    z2_core::player_traps::register_player_traps(game);
    z2_core::enemy_traps::register_enemy_traps(game);
    z2_core::town_traps::register_town_traps(game);
    z2_core::palace_traps::register_palace_traps(game);
    z2_core::title_traps::register_title_traps(game);
    z2_core::boot_traps::register_boot_traps(game);
}

fn rom_game(rom: &[u8], wide: Option<u8>) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    g.set_wide_gameplay(wide);
    g.reset();
    g
}

fn movie_track(name: &str) -> Option<Vec<u8>> {
    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .unwrap_or_else(|| "/Volumes/Holy Drive/dev/z2-corpus/movies".to_string());
    let path = common::file_present(
        &std::path::Path::new(&dir).join(name),
        "wide_gameplay_tests movie",
    )?;
    let bytes = std::fs::read(&path).ok()?;
    if name.ends_with(".fm2") {
        let movie = z2_verify::movie_fm2::parse_fm2_bytes(&bytes).expect("parse fm2");
        Some(movie.pad1_track())
    } else {
        let movie = z2_verify::movie_bk2::parse_bk2_zip(&bytes).expect("parse bk2");
        Some(movie.pad1_track())
    }
}

fn frame_cap(default: usize) -> usize {
    common::var_present("Z2_WIDE_FRAMES")
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

const MODE: usize = 0x0736;

// ------------------------------------------------------------ pure helpers

#[test]
fn spawn_push_moves_only_side_spawns() {
    // Spawn table screen X: $40/$60 left, $80 above/below, $A0/$C0 right.
    assert_eq!(spawn_push(0x40, 88), (0x40 - 88, true));
    assert_eq!(spawn_push(0x60, 88), (0x60 - 88, true));
    assert_eq!(spawn_push(0x80, 88), (0x80, false));
    assert_eq!(spawn_push(0xA0, 88), (0xA0 + 88, true));
    assert_eq!(spawn_push(0xC0, 88), (0xC0 + 88, true));
    // A zero margin is the ROM.
    for s8 in [0x40u8, 0x60, 0x80, 0xA0, 0xC0] {
        assert_eq!(spawn_push(s8, 0), (i16::from(s8), false));
    }
    // Pushed spawns are alive in the margin at every supported width.
    for tiles in 1..=16u8 {
        let m = tiles * 8;
        for s8 in [0x40u8, 0x60, 0xA0, 0xC0] {
            let (ex, _) = spawn_push(s8, m);
            assert!(!out_of_bounds(ex, m), "s8 {s8:#x} M {m}: ex {ex}");
        }
    }
}

#[test]
fn life_bonus_covers_the_walk_back() {
    assert_eq!(life_bonus_ticks(0), 0);
    assert_eq!(life_bonus_ticks(64), 4); // 64 frames = 3.05 ticks
    assert_eq!(life_bonus_ticks(88), 5); // 88 frames = 4.19 ticks
    assert_eq!(life_bonus_ticks(128), 7);
    for m in (0..=128u8).step_by(8) {
        assert!(u16::from(life_bonus_ticks(m)) * 21 >= u16::from(m));
    }
}

#[test]
fn despawn_bounds_match_the_rom_at_zero_margin() {
    // ROM: removed when the 8-bit screen X is >= $F8, i.e. at 248..255 or
    // after wrapping below 0.
    for ex in -8i16..256 {
        let rom = (ex as u8) >= 0xF8;
        assert_eq!(out_of_bounds(ex, 0), rom, "ex {ex}");
    }
    // With a margin: alive on [-M, 256 + M - 8).
    assert!(!out_of_bounds(-88, 88));
    assert!(out_of_bounds(-89, 88));
    assert!(!out_of_bounds(256 + 88 - 9, 88));
    assert!(out_of_bounds(256 + 88 - 8, 88));
}

#[test]
fn follow_tracks_small_moves_across_the_wrap() {
    assert_eq!(follow(-3, 0xFC), -4);
    assert_eq!(follow(-3, 0xFE), -2);
    assert_eq!(follow(300, 300u16 as u8), 300);
    assert_eq!(follow(300, (301u16) as u8), 301);
    assert_eq!(follow(255, 0), 256);
    assert_eq!(follow(0, 0xFF), -1);
}

#[test]
fn town_offsets_extend_the_vanilla_distances() {
    assert_eq!(town_offsets(0), TOWN_OFFSETS_VANILLA);
    // M = 88: -120 = $FF88, +352 = $0160.
    assert_eq!(town_offsets(88), [0x88, 0x60, 0xFF, 0x01]);
    // M = 64: -96 = $FFA0, +328 = $0148.
    assert_eq!(town_offsets(64), [0xA0, 0x48, 0xFF, 0x01]);
}

#[test]
fn sideview_shift_puts_spawns_outside_the_margin() {
    assert_eq!(sideview_spawn_shift(0), (0, 0));
    for tiles in 0..=16u8 {
        let m = i16::from(tiles) * 8;
        let (l, r) = sideview_spawn_shift(tiles * 8);
        // Worst case after the shift is still entirely outside the margin.
        let left_worst = wide_gameplay::SV_LEFT_SPAWN_MAX - 16 * i16::from(l);
        let right_worst = wide_gameplay::SV_RIGHT_SPAWN_MIN + 16 * i16::from(r);
        assert!(left_worst + 16 <= -m, "M {m}: left {left_worst}");
        assert!(right_worst >= 256 + m, "M {m}: right {right_worst}");
        // And minimal: one column less would not be.
        if l > 0 {
            assert!(left_worst + 16 + 16 > -m);
        }
        if r > 0 {
            assert!(right_worst - 16 < 256 + m);
        }
    }
}

#[test]
fn shift_column_is_linear_without_wrap() {
    assert_eq!(shift_column(2, 5, 1), (2, 6));
    assert_eq!(shift_column(2, 15, 1), (3, 0));
    assert_eq!(shift_column(2, 0, -1), (1, 15));
    assert_eq!(shift_column(0, 0, -1), (0xFF, 15));
    assert_eq!(shift_column(0xFF, 11, -2), (0xFF, 9));
    assert_eq!(shift_column(0xFF, 15, 1), (0, 0));
}

// ------------------------------------------------------------ synthetic pins

#[test]
fn off_by_default_and_toggles_its_traps() {
    let mut g = Game::new();
    assert_eq!(g.wide_gameplay_tiles(), None);
    assert!(!g.traps.is_trapped(ADDR_SPAWN));
    assert!(!g.traps.is_trapped(ADDR_BLOBS));
    assert!(g.overworld_margin_sprites().is_empty());
    g.set_wide_gameplay(Some(11));
    assert_eq!(g.wide_gameplay_tiles(), Some(11));
    assert!(g.traps.is_trapped(ADDR_SPAWN));
    assert!(g.traps.is_trapped(ADDR_BLOBS));
    g.set_wide_gameplay(Some(99));
    assert_eq!(
        g.wide_gameplay_tiles(),
        Some(16),
        "clamped to the widescreen max"
    );
    g.set_wide_gameplay(None);
    assert_eq!(g.wide_gameplay_tiles(), None);
    assert!(!g.traps.is_trapped(ADDR_SPAWN));
    assert!(!g.traps.is_trapped(ADDR_BLOBS));
}

#[test]
fn state_round_trips_the_wide_block() {
    let mut g = Game::new();
    g.set_wide_gameplay(Some(8));
    g.wide_game.ex = [-60, 1, 2, 3, 300, 5, 6, 7];
    g.wide_game.tracked = 0x11;
    let bytes = g.save_state().to_bytes();
    let st = z2_core::state::GameState::from_bytes(&bytes).expect("decode");
    let mut h = Game::new();
    h.load_state(&st);
    assert_eq!(h.wide_game, g.wide_game);
    assert!(
        h.traps.is_trapped(ADDR_BLOBS),
        "load re-registers the traps"
    );
    let mut off = Game::new();
    off.set_wide_gameplay(Some(8));
    off.load_state(&Game::new().save_state());
    assert!(
        !off.traps.is_trapped(ADDR_BLOBS),
        "load of an off state drops them"
    );
}

// ------------------------------------------------------------ ROM-gated

/// Lockstep the default trap set against the same set plus wide gameplay
/// at `M = 0`; returns (frames compared, overworld frames).
fn zero_margin_lockstep(rom: &[u8], track: &[u8], cap: usize) -> (usize, usize) {
    let mut a = rom_game(rom, None);
    let mut b = rom_game(rom, Some(0));
    let mut overworld = 0usize;
    let n = track.len().min(cap);
    for (f, &pad) in track.iter().take(n).enumerate() {
        a.step(pad);
        b.step(pad);
        if a.ram[MODE] == 0x05 {
            overworld += 1;
        }
        let same = a.ram() == b.ram()
            && a.wram() == b.wram()
            && a.oam() == b.oam()
            && a.cpu_state() == b.cpu_state()
            && a.cpu.cycles == b.cpu.cycles;
        if !same {
            let ram = (0..0x800).find(|&i| a.ram[i] != b.ram[i]);
            let wram = (0..0x2000).find(|&i| a.wram()[i] != b.wram()[i]);
            panic!(
                "M=0 diverged at frame {f} (mode {:#04x}): ram {:?} wram {:?} \
                 cpu {:?} vs {:?}, cycles {} vs {}",
                a.ram[MODE],
                ram.map(|i| (format!("${i:04X}"), a.ram[i], b.ram[i])),
                wram.map(|i| format!("${:04X}", 0x6000 + i)),
                a.cpu_state(),
                b.cpu_state(),
                a.cpu.cycles,
                b.cpu.cycles
            );
        }
    }
    (n, overworld)
}

#[test]
fn zero_margin_port_is_the_rom_over_corpus_movies() {
    let Some(rom) = common::rom_bytes("wide_gameplay_tests") else {
        return;
    };
    let cap = frame_cap(usize::MAX);
    let mut total_ow = 0;
    for name in [
        "anypct.bk2",
        "hundred-percent.bk2",
        "warpless.fm2",
        "warp-glitch.bk2",
    ] {
        let Some(track) = movie_track(name) else {
            continue;
        };
        let (n, ow) = zero_margin_lockstep(&rom, &track, cap);
        eprintln!("{name}: {n} frames identical at M=0 ({ow} overworld frames)");
        total_ow += ow;
    }
    if total_ow > 0 {
        eprintln!("overworld frames covered: {total_ow}");
    }
}

/// `M = 88`: replay `anypct.bk2` to an overworld stretch with vanilla
/// gameplay (frame 14530, Link walking east across open ground), turn wide
/// gameplay on and keep feeding the movie's input. A blob spawned to the
/// right is pushed out into the right margin, is drawn there as margin
/// sprites, and walks back into the window; blobs that leave the window on
/// the left stay alive in the left margin instead of despawning at x 0.
#[test]
fn blobs_live_in_the_margins_and_come_back() {
    const START: usize = 14_530;
    const FRAMES: usize = 600;
    const TILES: u8 = 11;
    let Some(rom) = common::rom_bytes("wide_gameplay_tests") else {
        return;
    };
    let Some(track) = movie_track("anypct.bk2") else {
        return;
    };
    let mut g = rom_game(&rom, None);
    for &pad in &track[..START] {
        g.step(pad);
    }
    assert_eq!(
        g.ram[MODE], 0x05,
        "frame {START} of anypct is on the overworld"
    );
    g.set_wide_gameplay(Some(TILES));
    let m = i16::from(TILES) * 8;

    // Per slot: was it seen alive out in the right margin, then back inside?
    let mut in_right = [false; 8];
    let mut came_back = false;
    let mut in_left = false;
    let mut sprites_seen = 0usize;
    for &pad in &track[START..START + FRAMES] {
        g.step(pad);
        if g.ram[MODE] != 0x05 {
            break;
        }
        for (s, right) in in_right.iter_mut().enumerate() {
            let alive = g.ram[0x82 + s] != 0 && g.ram[0x050E + s] != 0;
            if !alive {
                *right = false;
                continue;
            }
            let ex = g.wide_game.ex[s];
            assert!(
                (-m..256 + m - 8).contains(&ex),
                "slot {s} alive at ex {ex}, outside the margins"
            );
            // The ROM's 8-bit X tracks the extended one (to within the
            // scroll step Link took after the blob pass).
            let d = g.ram[0x4E + s]
                .wrapping_sub(g.ram[0xFD])
                .wrapping_sub(ex as u8) as i8;
            assert!(d.abs() <= 4, "slot {s}: 8-bit X off extended X by {d}");
            if ex >= 256 {
                *right = true;
            } else if (0..248).contains(&ex) && *right {
                came_back = true;
            }
            if ex < 0 {
                in_left = true;
            }
        }
        for sp in g.overworld_margin_sprites() {
            assert!(
                (-m..256 + m).contains(&sp.x),
                "margin sprite at x {} outside the margins",
                sp.x
            );
            if sp.x < 0 || sp.x >= 256 {
                sprites_seen += 1;
            }
        }
    }
    let st = &g.wide_game;
    eprintln!(
        "M={m}: pushed {} blobs, {} blob-frames in a margin, {sprites_seen} margin sprites shown",
        st.n_pushed, st.n_margin_frames
    );
    assert!(st.n_pushed >= 1, "a side spawn was pushed into a margin");
    assert!(
        came_back,
        "a blob spawned in the right margin walked back into the window"
    );
    assert!(in_left, "a blob stayed alive in the left margin");
    assert!(
        st.n_margin_frames > 0 && sprites_seen > 0,
        "margin blobs were drawn as margin sprites"
    );
}
