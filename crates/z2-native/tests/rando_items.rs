//! ROM-gated gameplay checks for the randomizer's start and item options
//! (`z2_rando::start`, `z2_rando::items`).
//!
//! Each test builds the randomized game through the app's seam
//! (`emu_from_rom_body_with` + `RandoSpec`), registers and loads a new file,
//! and reads the RAM the game filled from the save defaults:
//!
//! * `$0777-$0779` attack, magic and life levels;
//! * `$077B-$0782` spells known; `$0783`/`$0784` magic/heart containers;
//! * `$0785-$078C` candle .. magic key; `$0796` techniques; `$0700` lives.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --test
//! rando_items -- --ignored`.

use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{Flags, StartingLives, StartingTechs, Tri};

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;

fn rom_body() -> Vec<u8> {
    z2_assets::rom::open().expect("Z2_ROM must name the verified ROM")
}

fn build(body: &[u8], seed: &str, flags: &Flags) -> Emu {
    let fs = flags.to_flag_string();
    let spec = RandoSpec::from_cli(Some(seed), Some(&fs), None, None)
        .unwrap()
        .unwrap()
        .leak();
    let feats = Features {
        rando: Some(spec),
        ..Features::default()
    };
    app::emu_from_rom_body_with(body, 44_100, feats).unwrap()
}

fn step(emu: &mut Emu, frames: usize, input: u8) {
    app::step_frames(emu, &vec![input; frames], None);
}

/// Title -> register a name -> load it -> settled in the first room.
fn start_game(emu: &mut Emu) {
    step(emu, 30, 0);
    step(emu, 5, START);
    step(emu, 20, 0);
    step(emu, 5, START);
    step(emu, 20, 0);
    for _ in 0..8 {
        step(emu, 2, A);
        step(emu, 8, 0);
    }
    for _ in 0..3 {
        step(emu, 2, SELECT);
        step(emu, 8, 0);
    }
    step(emu, 5, START);
    step(emu, 30, 0);
    step(emu, 5, START);
    step(emu, 170, 0);
    assert_eq!(emu.game.exec_errors, 0);
    assert_eq!(emu.game.ram[0x0736], 0x0B, "side-view gameplay");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn starting_values_reach_a_new_file() {
    let body = rom_body();
    let mut f = Flags::default();
    f.start.start_with_boots = true;
    f.start.start_with_magic_key = true;
    f.start.start_with_fairy = true;
    f.start.starting_techs = StartingTechs::Both;
    f.start.starting_lives = StartingLives::L5;
    f.start.heart_containers_min = 6;
    f.start.heart_containers_max = 6;
    f.start.magic_containers_min = 5;
    f.start.magic_containers_max = 5;
    f.start.attack_level = 3;
    f.start.life_level = 2;
    let mut emu = build(&body, "start", &f);
    start_game(&mut emu);
    let ram = &emu.game.ram;
    assert_eq!(ram[0x0700], 5, "lives");
    assert_eq!(&ram[0x0777..0x077A], &[3, 1, 2], "levels");
    assert_eq!(&ram[0x077B..0x0783], &[0, 0, 0, 1, 0, 0, 0, 0], "spells");
    assert_eq!(ram[0x0783], 5, "magic containers");
    assert_eq!(ram[0x0784], 6, "heart containers");
    assert_eq!(&ram[0x0785..0x078D], &[0, 0, 0, 1, 0, 0, 0, 1], "tools");
    assert_eq!(ram[0x0796], 0x14, "techniques");
    step(&mut emu, 600, 0);
    assert_eq!(emu.game.exec_errors, 0);
}

#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_new_file_is_unchanged() {
    let body = rom_body();
    let mut emu = build(&body, "x", &Flags::default());
    start_game(&mut emu);
    let ram = &emu.game.ram;
    assert_eq!(ram[0x0700], 3);
    assert_eq!(ram[0x0783], 4);
    assert_eq!(ram[0x0784], 4);
    assert_eq!(&ram[0x0785..0x078D], &[0; 8]);
}

#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_items_boot_and_play() {
    let body = rom_body();
    let mut f = Flags::default();
    f.items.shuffle_palace_items = Tri::On;
    f.items.shuffle_overworld_items = Tri::On;
    f.items.mix_overworld_and_palace_items = Tri::On;
    f.items.include_pbag_caves = Tri::On;
    f.items.shuffle_small_items = true;
    f.items.palaces_contain_extra_keys = Tri::On;
    f.items.shuffle_pbag_amounts = Tri::On;
    f.items.allow_important_item_duplicates = true;
    f.start.start_with_raft = true;
    for seed in ["items-a", "items-b", "items-c"] {
        let out = z2_rando::randomize(&body, seed, &f).unwrap();
        assert!(out.spoiler.contains("Sphere 1"), "{seed}");
        let mut emu = build(&body, seed, &f);
        start_game(&mut emu);
        assert_eq!(emu.game.ram[0x0787], 1, "{seed}: raft owned at start");
        // Walk around the first room for a while.
        for _ in 0..4 {
            step(&mut emu, 120, 0x80);
            step(&mut emu, 120, 0x40);
        }
        assert_eq!(emu.game.exec_errors, 0, "{seed}");
    }
}
