//! ROM-gated: the hints and towns options boot and show what they promise.
//!
//! * The seed hash replaces the file-select heading: the randomized and
//!   vanilla file-select frames differ only in the heading rows.
//! * Every hint, text and town option together boots, reaches the file
//!   select and keeps running without wedging.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --test
//! rando_hints -- --ignored`.

use z2_native::app::{self, Emu, Features, FRAME_W};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{Flags, HelpfulHints, Tri};

/// Start button (pad bits: A, B, Select, Start, Up, Down, Left, Right).
const START: u8 = 0x08;

fn emu(flags: Option<&Flags>) -> Emu {
    let body = z2_assets::rom::open().expect("Z2_ROM must name the verified ROM");
    let Some(f) = flags else {
        return app::emu_from_rom_body(&body, 44_100).unwrap();
    };
    let spec = RandoSpec::from_cli(Some("hints"), Some(&f.to_flag_string()), None, None)
        .unwrap()
        .unwrap()
        .leak();
    let feats = Features {
        rando: Some(spec),
        ..Features::default()
    };
    app::emu_from_rom_body_with(&body, 44_100, feats).unwrap()
}

/// Title for 120 frames, one Start tap, then the file-select screen.
fn to_file_select(e: &mut Emu) {
    let mut input = vec![0u8; 120];
    input.extend([START; 6]);
    input.extend([0u8; 150]);
    app::step_frames(e, &input, None);
}

fn all_options() -> Flags {
    let mut f = Flags::default();
    f.hints.helpful_hints = HelpfulHints::TownsSeparate;
    f.hints.spell_item_hints = Tri::On;
    f.hints.town_name_hints = Tri::On;
    f.hints.reveal_walkthrough_walls = true;
    f.hints.reveal_hidden_jars = true;
    f.towns.shorten_wizards = true;
    f.towns.randomize_new_kasuto_jar_requirements = true;
    f.cosmetic.community_text = true;
    f
}

#[test]
#[ignore = "needs Z2_ROM"]
fn hash_shows_on_the_file_select_heading_only() {
    let mut plain = emu(None);
    let mut rando = emu(Some(&all_options()));
    to_file_select(&mut plain);
    to_file_select(&mut rando);
    assert_eq!(plain.game.ram[0x736], rando.game.ram[0x736], "same screen");
    let a = plain.game.frame_indexed();
    let b = rando.game.frame_indexed();
    let rows: Vec<usize> = (0..a.len())
        .filter(|&i| a[i] != b[i])
        .map(|i| i / FRAME_W)
        .collect();
    assert!(!rows.is_empty(), "the heading must change");
    // The heading is nametable row 3 (pixel rows 24-31).
    assert!(
        rows.iter().all(|r| (24..32).contains(r)),
        "only the heading row may differ: {:?}",
        (rows.first(), rows.last())
    );
}

#[test]
#[ignore = "needs Z2_ROM"]
fn every_hint_and_town_option_boots_and_runs() {
    let mut e = emu(Some(&all_options()));
    to_file_select(&mut e);
    // Walk through the menus with Start taps and keep running.
    let mut input = Vec::new();
    for _ in 0..4 {
        input.extend([START; 5]);
        input.extend([0u8; 40]);
    }
    input.extend([0u8; 1200]);
    app::step_frames(&mut e, &input, None);
    assert_eq!(e.game.exec_errors, 0, "interpreter faults");
    let f = e.game.frame_indexed();
    assert!(f.iter().any(|&p| p != f[0]), "the screen is not blank");
}
