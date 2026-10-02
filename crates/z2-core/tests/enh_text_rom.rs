//! ROM-gated tests for the Text & HUD enhancements (`enh::text`).
//!
//! * `dialogue_speed`: the Rauru wise man's talk (reached by the scene warp
//!   `player_magic_rom.rs` uses) finishes typing in fewer frames at speed 5
//!   than at speed 0, and the final nametables are identical, so every
//!   glyph is drawn where the original draws it.
//! * `protect_spell_name`: the pause pane shows PROTECT where it showed
//!   SHIELD.
//!
//! Skips (never fails) without `$Z2_ROM`.

#![cfg(feature = "interp")]

mod common;

use z2_core::enh::Enhancements;
use z2_core::game::Game;

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const RIGHT: u8 = 0x80;

/// Register the same trap groups the native app does.
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

/// Fresh cartridge, named slot 0, side-view gameplay in North Castle
/// (same script as `player_magic_rom.rs`).
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
    assert_eq!(g.ram[0x0736], 0x0B, "side-view gameplay mode");
    Some(g)
}

/// Both nametables (the cartridge mirrors two).
fn nametables(g: &Game) -> Vec<u8> {
    let m = g.ppu.model();
    (0x2000u16..0x2800).map(|a| m.nt_read(a)).collect()
}

/// Warp into the Rauru wise man's basement, walk into him and let the talk
/// type itself out. Returns (frames from the text pointer load, `$0524 =
/// 6`, to the end of typing, `$0524 = 8`; the nametables at that frame).
fn run_talk(base: &z2_core::state::GameState, g: &mut Game, speed: u8) -> (usize, Vec<u8>) {
    let mut e = Enhancements::default();
    e.text.dialogue_speed = speed;
    g.set_enhancements(e);
    g.load_state(base);
    g.ram[0x0706] = 0; // West Hyrule
    g.ram[0x0707] = 1; // town world
    g.ram[0x0748] = 0x2C; // town key area
    g.ram[0x056B] = 0; // Rauru
    g.ram[0x0561] = 0x24; // wise man's basement
    g.ram[0x075C] = 0;
    g.ram[0x0701] = 0;
    g.ram[0x0736] = 0;
    step_n(g, 160, 0);
    assert_eq!(g.ram[0x0736], 0x0B, "warp lands in side view");
    let mut start = None;
    for f in 0..4000usize {
        // Walk right until the talk starts; never press B (it skips).
        let input = if start.is_none() && g.ram[0x0524] == 0 {
            RIGHT
        } else {
            0
        };
        g.step(input);
        if start.is_none() && g.ram[0x0524] == 0x06 {
            start = Some(f);
        }
        if let Some(s) = start {
            if g.ram[0x0524] == 0x08 {
                return (f - s, nametables(g));
            }
        }
    }
    panic!("speed {speed}: the talk never finished typing (start {start:?})");
}

#[test]
fn dialogue_speed_types_faster_with_the_same_glyphs() {
    let Some(mut g) = to_gameplay("dialogue_speed_types_faster") else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    let base = g.save_state();
    let (og_frames, og_nt) = run_talk(&base, &mut g, 0);
    let (fast_frames, fast_nt) = run_talk(&base, &mut g, 5);
    let (mid_frames, mid_nt) = run_talk(&base, &mut g, 2);
    eprintln!("talk typing frames: og {og_frames}, speed 2 {mid_frames}, speed 5 {fast_frames}");
    // Text glyphs really are there: the box holds letter tiles.
    let letters = og_nt
        .iter()
        .filter(|&&t| (0xDA..=0xF3).contains(&t))
        .count();
    assert!(
        letters > 20,
        "the original talk drew text ({letters} letters)"
    );
    assert!(fast_frames < og_frames, "speed 5 must be faster");
    assert!(mid_frames < og_frames && fast_frames < mid_frames);
    // One letter a frame at speed 5: at least three times faster.
    assert!(fast_frames * 3 < og_frames, "{fast_frames} vs {og_frames}");
    assert_eq!(fast_nt, og_nt, "speed 5 draws exactly the original glyphs");
    assert_eq!(mid_nt, og_nt, "speed 2 draws exactly the original glyphs");
    // Back to the original: original timing again (patches undone).
    let (again, _) = run_talk(&base, &mut g, 0);
    assert_eq!(again, og_frames);
}

/// Open the pause pane with every spell learned; return the nametables.
fn pause_pane(base: &z2_core::state::GameState, g: &mut Game, protect: bool) -> Vec<u8> {
    let mut e = Enhancements::default();
    e.text.protect_spell_name = protect;
    g.set_enhancements(e);
    g.load_state(base);
    for a in 0x077B..0x0783 {
        g.ram[a] = 1;
    }
    g.ram[0x0783] = 8;
    g.ram[0x0773] = 0xFF;
    g.step(START);
    step_n(g, 40, 0);
    nametables(g)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn protect_spell_name_shows_in_the_pause_pane() {
    let Some(mut g) = to_gameplay("protect_spell_name_shows") else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    let base = g.save_state();
    let shield = z2_core::enh::text::encode_text("SHIELD");
    let protect = z2_core::enh::text::encode_text("PROTECT");
    let og = pause_pane(&base, &mut g, false);
    assert!(contains(&og, &shield), "original pane lists SHIELD");
    assert!(!contains(&og, &protect));
    let on = pause_pane(&base, &mut g, true);
    let mut row = protect.clone();
    // The pane overwrites the last three dots with the spell's MP cost,
    // so seven letters leave one dot before the digits (like THUNDER).
    row.push(0xCF);
    assert!(contains(&on, &row), "pane lists PROTECT.");
    assert!(!contains(&on, &shield), "and no SHIELD");
    // Every other tile is unchanged.
    let diff = og.iter().zip(&on).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 7, "only the seven name tiles change");
}
