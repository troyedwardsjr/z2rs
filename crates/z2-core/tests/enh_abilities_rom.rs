//! ROM-gated end-to-end tests for the Abilities enhancement group
//! (`z2_core::enh::abilities`): each option is switched on over a real run
//! of the cartridge (full hybrid trap set, the frontends' order) and its
//! effect is measured against the same run with everything off.
//!
//! Skips (never fails) without `$Z2_ROM`; the movie-based damage test also
//! needs the out-of-tree corpus (`$Z2_CORPUS`, default
//! `/Volumes/HolyDrive/dev/z2-corpus`).

#![cfg(feature = "interp")]

mod common;

use z2_core::enh::abilities::{self, AbilityOpts, DASH_CAP, MP_REGEN_PERIOD, WALK_CAP};
use z2_core::enh::Enhancements;
use z2_core::game::Game;
use z2_core::state::GameState;

// Input bytes on the shared contract (bit0 = A … bit7 = Right).
const A: u8 = 0x01;
const B: u8 = 0x02;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const LEFT: u8 = 0x40;
const RIGHT: u8 = 0x80;

const MODE: usize = 0x0736;
const LINK_Y: usize = 0x29;
const LINK_X: usize = 0x4D;
const LINK_PAGE: usize = 0x3B;
const HSPEED: usize = 0x70;
const MIDAIR: usize = 0x0479;
const SLASH: usize = 0x0400;
const HP: usize = 0x0774;
const MP: usize = 0x0773;
const SUBSTATE: usize = 0xB5;
const KILL: usize = 0x0494;

fn register_all(g: &mut Game) {
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

fn step_n(g: &mut Game, n: usize, input: u8) {
    for _ in 0..n {
        g.step(input);
    }
}

fn abilities(f: impl FnOnce(&mut AbilityOpts)) -> Enhancements {
    let mut e = Enhancements::default();
    f(&mut e.abilities);
    e
}

/// Fresh cartridge → named slot 0 → side-view gameplay in North Castle
/// (the `player_magic_rom` route).
fn to_gameplay(test: &str) -> Option<Game> {
    let raw = common::rom_bytes(test)?;
    let mut g = Game::from_ines(&raw).ok()?;
    register_all(&mut g);
    g.reset();
    step_n(&mut g, 30, 0);
    step_n(&mut g, 5, START);
    step_n(&mut g, 20, 0);
    step_n(&mut g, 5, START);
    step_n(&mut g, 20, 0);
    for _ in 0..8 {
        step_n(&mut g, 2, A);
        step_n(&mut g, 8, 0);
    }
    for _ in 0..3 {
        step_n(&mut g, 2, SELECT);
        step_n(&mut g, 8, 0);
    }
    step_n(&mut g, 5, START);
    step_n(&mut g, 30, 0);
    step_n(&mut g, 5, START);
    step_n(&mut g, 60, 0);
    step_n(&mut g, 1100, 0);
    assert_eq!(g.ram[MODE], 0x0B, "side-view gameplay mode");
    Some(g)
}

/// Base gameplay snapshot plus a closure running a variant from it.
fn from_base(g: &mut Game, base: &GameState, e: Enhancements) {
    g.load_state(base);
    g.set_enhancements(e);
}

// ------------------------------------------------------------ double jump

/// Jump, release A, press it again mid-air, then hold. Returns the apex
/// (smallest `$29`) and whether Link is back on the ground at the end.
fn jump_twice(g: &mut Game, presses: usize) -> (u8, bool) {
    let mut apex = g.ram[LINK_Y];
    let mut track = |g: &mut Game, input: u8, n: usize| {
        for _ in 0..n {
            g.step(input);
            apex = apex.min(g.ram[LINK_Y]);
        }
    };
    for _ in 0..presses {
        track(g, A, 14);
        track(g, 0, 1);
    }
    track(g, 0, 150);
    (apex, g.ram[MIDAIR] == 0)
}

#[test]
fn double_jump_reaches_higher_once_per_airtime() {
    let Some(mut g) = to_gameplay("double_jump") else {
        return;
    };
    let base = g.save_state();
    let ground = g.ram[LINK_Y];

    from_base(&mut g, &base, Enhancements::default());
    let (apex_og, landed_og) = jump_twice(&mut g, 2);

    from_base(&mut g, &base, abilities(|a| a.double_jump = true));
    let (apex_dj, landed_dj) = jump_twice(&mut g, 2);
    eprintln!("ground {ground:#04x} apex og {apex_og:#04x} dj {apex_dj:#04x}");
    assert!(landed_og && landed_dj, "both land again");
    assert!(apex_og < ground, "the plain jump rises");
    assert!(
        apex_dj + 16 < apex_og,
        "the mid-air jump climbs well past the plain apex ({apex_dj:#04x} vs {apex_og:#04x})"
    );
    assert_eq!(g.ram[LINK_Y], ground, "back on the floor");
    assert_eq!(g.enh_state.flags & 1, 0, "token reset on landing");

    // One token per airtime: a third press adds nothing.
    from_base(&mut g, &base, abilities(|a| a.double_jump = true));
    let (apex3, _) = jump_twice(&mut g, 3);
    assert_eq!(apex3, apex_dj, "the third press gave no extra lift");

    // The token comes back after landing: the same double jump again.
    let (again, _) = jump_twice(&mut g, 2);
    assert_eq!(again, apex_dj, "token reset on landing");
}

// ------------------------------------------------------------ dash

/// Hold `input` for `n` frames; returns the largest `|$70|` seen.
fn max_speed(g: &mut Game, input: u8, n: usize) -> u8 {
    let mut m = 0u8;
    for _ in 0..n {
        g.step(input);
        m = m.max((g.ram[HSPEED] as i8).unsigned_abs());
    }
    m
}

fn link_x(g: &Game) -> i32 {
    i32::from(g.ram[LINK_PAGE] as i8) * 256 + i32::from(g.ram[LINK_X])
}

#[test]
fn dash_raises_the_walk_cap_while_b_is_held() {
    let Some(mut g) = to_gameplay("dash") else {
        return;
    };
    let base = g.save_state();
    let x0 = link_x(&g);

    from_base(&mut g, &base, Enhancements::default());
    let og = max_speed(&mut g, LEFT | B, 110);
    let og_dx = x0 - link_x(&g);

    from_base(&mut g, &base, abilities(|a| a.dash_speed = true));
    let dash = max_speed(&mut g, LEFT | B, 110);
    let dash_dx = x0 - link_x(&g);
    eprintln!("og cap {og:#04x} dx {og_dx}; dash cap {dash:#04x} dx {dash_dx}");
    assert_eq!(og, WALK_CAP, "original cap");
    assert_eq!(dash, DASH_CAP, "dash cap");
    assert!(dash_dx > og_dx + 40, "dashing covers more ground");
    assert_eq!(g.ram[MODE], 0x0B);

    // Let go of B: back to the walk cap, never above the dash cap.
    let settled = max_speed(&mut g, LEFT, 30);
    assert!(settled <= DASH_CAP);
    assert_eq!(
        (g.ram[HSPEED] as i8).unsigned_abs(),
        WALK_CAP,
        "decayed back to the walk cap"
    );

    // Plain walking with the option on is the original walk.
    from_base(&mut g, &base, abilities(|a| a.dash_speed = true));
    assert_eq!(max_speed(&mut g, LEFT, 60), WALK_CAP);
}

// ------------------------------------------------------------ stab frenzy

/// Hold B for `n` frames; count the slashes (`$0400` entering state 2).
fn slashes(g: &mut Game, n: usize) -> usize {
    let mut count = 0;
    let mut prev = g.ram[SLASH];
    for _ in 0..n {
        g.step(B);
        let s = g.ram[SLASH];
        if s == 2 && prev != 2 {
            count += 1;
        }
        prev = s;
    }
    count
}

#[test]
fn stab_frenzy_repeats_stabs_while_b_is_held() {
    let Some(mut g) = to_gameplay("stab_frenzy") else {
        return;
    };
    let base = g.save_state();
    from_base(&mut g, &base, Enhancements::default());
    let og = slashes(&mut g, 150);
    from_base(&mut g, &base, abilities(|a| a.stab_frenzy = true));
    let frenzy = slashes(&mut g, 150);
    eprintln!("slashes in 150 frames: og {og}, frenzy {frenzy}");
    assert_eq!(og, 1, "holding B stabs once in the original");
    assert!(frenzy >= 8, "frenzy keeps stabbing ({frenzy})");
}

// ------------------------------------------------------------ MP regen

#[test]
fn mp_regen_adds_two_every_128_frames() {
    let Some(mut g) = to_gameplay("mp_regen") else {
        return;
    };
    let base = g.save_state();
    let frames = usize::from(MP_REGEN_PERIOD) * 4;
    for on in [false, true] {
        from_base(&mut g, &base, abilities(|a| a.mp_regen = on));
        g.ram[MP] = 0;
        step_n(&mut g, frames, 0);
        let want = if on { 8 } else { 0 };
        assert_eq!(g.ram[MP], want, "mp_regen {on}");
    }
    // Capped at the container cap.
    from_base(&mut g, &base, abilities(|a| a.mp_regen = true));
    let cap = g.ram[MP];
    g.ram[MP] = cap - 1;
    step_n(&mut g, frames, 0);
    assert_eq!(g.ram[MP], cap);
}

// ------------------------------------------------------------ sword reach

/// Sword box through the trapped port and the interpreted ROM bytes.
fn sword_box(g: &mut Game, anchor: u8) -> [(u8, u8); 2] {
    g.ram[0x047E] = anchor;
    g.ram[0xCC] = 0x80;
    g.ram[0x80] = 5; // standing stab
    g.ram[0x0480] = 0xA0;
    z2_core::player_traps::pl_sword_box(g);
    let port = (g.ram[0x00], g.ram[0x02]);
    g.ram[0x00] = 0;
    g.ram[0x02] = 0;
    g.set_untrapped(0xE9A2, true);
    g.call_asm(0xE9A2);
    g.set_untrapped(0xE9A2, false);
    [port, (g.ram[0x00], g.ram[0x02])]
}

#[test]
fn sword_reach_extends_the_box_in_the_facing_direction() {
    let Some(mut g) = to_gameplay("sword_reach") else {
        return;
    };
    let base = g.save_state();
    from_base(&mut g, &base, Enhancements::default());
    let right_og = sword_box(&mut g, 0x90);
    let left_og = sword_box(&mut g, 0x70);
    assert_eq!(right_og, [(0x88, 0x0E); 2], "ROM sword box, facing right");
    assert_eq!(left_og, [(0x72, 0x0E); 2], "ROM sword box, facing left");
    for n in 1..=3u8 {
        from_base(&mut g, &base, abilities(|a| a.sword_reach_px = n));
        let right = sword_box(&mut g, 0x90);
        let left = sword_box(&mut g, 0x70);
        // Right: same left edge, wider. Left: left edge moved out by n.
        assert_eq!(right, [(0x88, 0x0E + n); 2], "reach {n} right");
        assert_eq!(left, [(0x72 - n, 0x0E + n); 2], "reach {n} left");
    }
    // Back to OG restores the ROM bytes.
    g.set_enhancements(Enhancements::default());
    assert_eq!(sword_box(&mut g, 0x90), right_og);
}

// ------------------------------------------------------------ damage

fn anypct_track() -> Option<Vec<u8>> {
    let dir = common::var_present("Z2_CORPUS")
        .map(|d| format!("{d}/movies"))
        .or_else(|| common::var_present("Z2_CORPUS_MOVIES"))
        .unwrap_or_else(|| "/Volumes/HolyDrive/dev/z2-corpus/movies".to_string());
    let path = common::file_present(
        &std::path::Path::new(&dir).join("anypct.bk2"),
        "enh_abilities_rom any% movie",
    )?;
    let zip = std::fs::read(&path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&zip).expect("parse anypct.bk2");
    Some(movie.pad1_track())
}

fn movie_game(rom: &[u8], e: Enhancements) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_all(&mut g);
    g.set_enhancements(e);
    g.reset();
    g
}

/// First frame (of the first `cap`) whose step lowers Link's HP in side
/// view, with the HP before and after.
fn first_hit(g: &mut Game, track: &[u8], cap: usize) -> Option<(usize, u8, u8)> {
    for (f, &pad) in track.iter().take(cap).enumerate() {
        let before = g.ram[HP];
        let mode = g.ram[MODE];
        g.step(pad);
        if mode == 0x0B && g.ram[MODE] == 0x0B && g.ram[HP] < before {
            return Some((f, before, g.ram[HP]));
        }
    }
    None
}

#[test]
fn damage_reduction_shrinks_the_first_hit_of_the_any_pct_run() {
    let Some(rom) = common::rom_bytes("damage_reduction") else {
        return;
    };
    let Some(track) = anypct_track() else {
        return;
    };
    let mut og = movie_game(&rom, Enhancements::default());
    let (f, hp0, hp1) = first_hit(&mut og, &track, 40_000).expect("the run takes a hit");
    let loss_og = hp0 - hp1;
    for pct in [50u8, 100] {
        let mut g = movie_game(&rom, abilities(|a| a.damage_reduction_pct = pct));
        // Identical up to the hit: the patch only changes the damage read.
        let (f2, h0, h1) = first_hit(&mut g, &track, f + 1).expect("same hit");
        let loss = h0 - h1;
        eprintln!("frame {f}: og loss {loss_og}, {pct}% loss {loss}");
        assert_eq!((f2, h0), (f, hp0), "same frame, same HP before");
        assert_eq!(loss, abilities::reduced_damage(loss_og, pct), "{pct}%");
        assert!(loss >= 1 && loss < loss_og);
    }
}

// ------------------------------------------------------------ rescue fairy

/// Jump, then touch "lava" mid-air (`$B5 = 2`, what
/// `bank7_Link_touched_Lava_Water` does) and play on for `n` frames.
/// Returns (takeoff X, takeoff Y, died).
fn lava_jump(g: &mut Game, n: usize) -> (i32, u8, bool) {
    step_n(g, 2, 0);
    let (x, y) = (link_x(g), g.ram[LINK_Y]);
    step_n(g, 10, A | LEFT);
    assert_ne!(g.ram[MIDAIR], 0, "airborne");
    g.ram[SUBSTATE] = 2;
    g.ram[0x050C] = 0x10;
    let mut died = false;
    for _ in 0..n {
        g.step(0);
        died |= g.ram[KILL] != 0 || g.ram[MODE] != 0x0B;
    }
    (x, y, died)
}

#[test]
fn rescue_fairy_returns_link_to_safe_ground_once_per_room() {
    let Some(mut g) = to_gameplay("rescue_fairy") else {
        return;
    };
    let base = g.save_state();
    from_base(&mut g, &base, Enhancements::default());
    let (_, _, died) = lava_jump(&mut g, 200);
    assert!(died, "the original sinks and dies");

    from_base(&mut g, &base, abilities(|a| a.rescue_fairy = true));
    let (x, y, died) = lava_jump(&mut g, 1);
    assert!(!died);
    assert_eq!((link_x(&g), g.ram[LINK_Y]), (x, y), "back at the takeoff");
    assert_eq!(g.ram[SUBSTATE], 1);
    step_n(&mut g, 200, 0);
    assert_eq!(g.ram[MODE], 0x0B, "still alive in the room");
    assert_eq!(g.ram[KILL], 0);
    // Once per room: the second dip is the original.
    let (_, _, died) = lava_jump(&mut g, 200);
    assert!(died, "second dip in the same room is fatal");
}

// ------------------------------------------------------------ reflect

/// Run `LE3B9` for projectile slot 0 through a `JSR` from RAM (so the trap
/// table sees the call), with a shot of attribute `attr` flying left into
/// Link's front while REFLECT is on. Returns (type, X velocity) after.
fn shield_check(g: &mut Game, attr: u8) -> (u8, u8) {
    let ty = 0x05u8;
    g.ram[0x12] = 1; // slot 0 is checked on odd frames
    g.ram[0x0710] = 1; // REFLECT
    g.ram[0x13] = 0;
    g.ram[0x17] = 1; // standing
    g.ram[0x9F] = 1; // facing right
    g.ram[0xCC] = 0x80; // Link screen X
    g.ram[LINK_Y] = 0xB0;
    g.ram[0x87] = ty;
    g.ram[0x77] = 0xF0; // flying left
    g.ram[0x66] = 2;
    g.ram[0x30] = 0xB0;
    g.ram[0xCE] = 0x94;
    g.wram[0x0D17 + usize::from(ty)] = attr;
    let code = [0xA2, 0x00, 0x20, 0xB9, 0xE3, 0x60]; // LDX #0 : JSR $E3B9 : RTS
    let saved: Vec<u8> = g.ram[0x110..0x116].to_vec();
    g.ram[0x110..0x116].copy_from_slice(&code);
    g.call_asm(0x0110);
    g.ram[0x110..0x116].copy_from_slice(&saved);
    (g.ram[0x87], g.ram[0x77])
}

#[test]
fn reflect_more_reflects_shots_the_rom_only_absorbs() {
    let Some(mut g) = to_gameplay("reflect_more") else {
        return;
    };
    let base = g.save_state();
    // A plain shield-blocked shot (class $00).
    from_base(&mut g, &base, Enhancements::default());
    let og = shield_check(&mut g, 0x01);
    from_base(&mut g, &base, abilities(|a| a.reflect_more = true));
    let more = shield_check(&mut g, 0x01);
    // A reflectable shot (class $C0) is reflected either way.
    from_base(&mut g, &base, Enhancements::default());
    let og_c0 = shield_check(&mut g, 0xC1);
    eprintln!("og {og:02X?} more {more:02X?} og C0 {og_c0:02X?}");
    assert_eq!(og.0, 0xF2, "the original absorbs a class-$00 shot");
    assert_eq!(
        more, og_c0,
        "reflect_more sends it back like a reflectable one"
    );
    assert_eq!(more.0, 0x05);
    assert!((more.1 as i8) > 0, "now flying right");
    assert_eq!(g.wram[0x0D17 + 5], 0xC1);
    from_base(&mut g, &base, abilities(|a| a.reflect_more = true));
    shield_check(&mut g, 0x01);
    assert_eq!(g.wram[0x0D17 + 5], 0x01, "attribute table restored");
}

// ------------------------------------------------------------ flute warp

/// Walk left out of North Castle onto the West overworld.
fn to_overworld(g: &mut Game) {
    for _ in 0..80 {
        step_n(g, 5, LEFT);
        if g.ram[MODE] == 0x05 {
            break;
        }
    }
    step_n(g, 30, 0);
    assert_eq!(g.ram[MODE], 0x05, "overworld main");
}

fn towns(g: &Game) -> Vec<usize> {
    (0..0x3E)
        .filter(|&s| matches!((g.wram[0x0ABD + s] >> 2) & 7, 1 | 2))
        .collect()
}

/// Step until side-view gameplay (`$0B`); `false` after `n` frames.
fn until_sideview(g: &mut Game, n: usize) -> bool {
    for _ in 0..n {
        g.step(0);
        if g.ram[MODE] == 0x0B {
            return true;
        }
    }
    false
}

#[test]
fn flute_warp_enters_a_visited_town() {
    let Some(mut g) = to_gameplay("flute_warp") else {
        return;
    };
    to_overworld(&mut g);
    let base = g.save_state();
    let town_slots = towns(&g);
    eprintln!("West town key areas: {town_slots:?}");
    assert!(town_slots.len() >= 2);

    // Off, or on with nothing visited: Select + B does nothing.
    for e in [Enhancements::default(), abilities(|a| a.flute_warp = true)] {
        from_base(&mut g, &base, e);
        g.step(SELECT);
        g.step(SELECT | B);
        step_n(&mut g, 10, 0);
        assert_eq!(g.ram[MODE], 0x05, "no warp");
    }

    // Visited town ordinal 1: the warp enters it.
    from_base(&mut g, &base, abilities(|a| a.flute_warp = true));
    g.enh_state.scratch[8] = 0b10;
    g.step(SELECT);
    g.step(SELECT | B);
    assert_eq!(
        usize::from(g.ram[0x0748]),
        town_slots[1],
        "entering town #1"
    );
    assert!(until_sideview(&mut g, 400), "town loaded");
    assert!(
        matches!(g.ram[0x0707], 1 | 2),
        "in a town (world {})",
        g.ram[0x0707]
    );
    step_n(&mut g, 5, 0);
    assert_eq!(abilities::visited_towns(&g, 0), 0b10, "visit recorded");
}

#[test]
fn walking_into_a_town_records_it() {
    let Some(mut g) = to_gameplay("flute_record") else {
        return;
    };
    to_overworld(&mut g);
    g.set_enhancements(abilities(|a| a.flute_warp = true));
    let base = g.save_state();
    let slot = towns(&g)[0];
    let ty = g.wram[0x0A00 + slot] & 0x7F;
    let tx = g.wram[0x0A3F + slot] & 0x3F;
    // Stand next to the town (logically; the screen is not redrawn) and
    // walk onto it: the ROM's own key-area check enters it.
    let mut entered = false;
    for (dy, dx, input) in [
        (0i8, 1i8, LEFT),
        (0, -1, RIGHT),
        (1, 0, 0x10),
        (-1, 0, 0x20),
    ] {
        g.load_state(&base);
        g.ram[0x73] = ty.wrapping_add(dy as u8);
        g.ram[0x74] = tx.wrapping_add(dx as u8);
        for _ in 0..60 {
            g.step(input);
            if g.ram[MODE] != 0x05 {
                break;
            }
        }
        if g.ram[MODE] != 0x05 && usize::from(g.ram[0x0748]) == slot {
            entered = true;
            break;
        }
    }
    assert!(entered, "walked into town slot {slot}");
    assert_eq!(
        abilities::visited_towns(&g, 0),
        0b01,
        "first West town recorded"
    );
    assert!(until_sideview(&mut g, 400), "town loaded");
    assert!(matches!(g.ram[0x0707], 1 | 2));
    // Back out on the overworld the warp now has a target.
    assert_eq!(abilities::visited_towns(&g, 0), 0b01);
}
