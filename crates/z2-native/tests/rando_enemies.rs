//! ROM-gated: a seed with every enemy, stat and drop option on boots, loads
//! palace and overworld scenes through the game's own loader, and survives
//! fights in them.
//!
//! * the game copies the randomized hit point tables and the shuffled enemy
//!   lists into WRAM (`$6D21`, `$7000`) exactly as the randomizer wrote
//!   them, so the ported routines see the new values;
//! * every scene settles back into side-view play and runs hundreds of
//!   frames of walking and sword swings without an interpreter fault.
//!
//! Run with `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-native --release
//! --test rando_enemies -- --ignored`.

use z2_native::app::{self, Emu, Features};
use z2_native::rando::RandoSpec;
use z2_rando::flags::{
    AttackEffectiveness, DripperEnemy, DropPool, EnemyLife, Flags, LifeEffectiveness,
    MagicEffectiveness, SwordImmunity, Tri, XpEffectiveness,
};

const A: u8 = 0x01;
const B: u8 = 0x02;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const LEFT: u8 = 0x40;
const RIGHT: u8 = 0x80;
const MODE: usize = 0x0736;
const MODE_SIDEVIEW: u8 = 0x0B;

fn flags(mix: bool) -> Flags {
    let mut f = Flags::default();
    let e = &mut f.enemies;
    e.shuffle_overworld_enemies = Tri::On;
    e.shuffle_palace_enemies = Tri::On;
    e.mix_large_and_small = if mix { Tri::On } else { Tri::Off };
    e.dripper_enemy = DripperEnemy::EasierGroundEnemiesFullHp;
    e.enemy_hp = EnemyLife::Wide;
    e.boss_hp = EnemyLife::High;
    e.shuffle_xp_stealers = true;
    e.shuffle_xp_stolen_amount = true;
    e.sword_immunity = SwordImmunity::Shuffle;
    e.xp_drops = XpEffectiveness::Wide;
    let s = &mut f.stats;
    s.shuffle_attack_exp = true;
    s.shuffle_magic_exp = true;
    s.shuffle_life_exp = true;
    s.attack_level_cap = 5;
    s.scale_level_requirements_to_cap = true;
    s.attack_effectiveness = AttackEffectiveness::AverageHigh;
    s.magic_effectiveness = MagicEffectiveness::AverageLowCost;
    s.life_effectiveness = LifeEffectiveness::AverageHigh;
    let d = &mut f.drops;
    d.shuffle_drop_frequency = true;
    d.standardize_drops = true;
    d.randomize_drops = true;
    d.large_pool = DropPool {
        key: true,
        ..DropPool::default()
    };
    f.items.shuffle_pbag_amounts = Tri::On;
    f.palaces.aggressive_thunderbird = true;
    f
}

fn rando_emu(seed: &str, flags: Flags) -> (Emu, z2_rando::Output) {
    let body = z2_assets::rom::open().expect("Z2_ROM must name the verified ROM");
    let spec = RandoSpec {
        seed: seed.to_string(),
        flags,
        spoiler_path: None,
        sprite_ips: None,
    };
    let out = spec.run(&body).expect("randomize");
    let feats = Features {
        rando: Some(spec.leak()),
        ..Features::default()
    };
    let emu = app::emu_from_rom_body_with(&body, 44_100, feats).expect("build rando emu");
    assert_eq!(
        emu.rom.body_crc32, out.crc32,
        "the emulator runs the patched body"
    );
    (emu, out)
}

fn step(emu: &mut Emu, frames: usize, pad: u8) {
    app::step_frames(emu, &vec![pad; frames], None);
}

/// Title -> file select -> name -> load -> side-view play (pad 1 only).
fn reach_gameplay(emu: &mut Emu) {
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
    assert_eq!(emu.game.exec_errors, 0, "menu/load path faulted");
    assert_eq!(emu.game.ram[MODE], MODE_SIDEVIEW, "reached side-view play");
}

/// Keep Link alive and strong enough to walk through a room.
fn buff_link(emu: &mut Emu) {
    let ram = &mut emu.game.ram;
    ram[0x0774] = 0xFF; // life
    ram[0x0773] = 0xFF; // magic
    ram[0x0784] = 8; // heart containers
    ram[0x0783] = 8; // magic containers
    ram[0x0785] = 1; // candle
}

/// Load a scene through the game's own loader (see
/// README.md).
fn load_scene(emu: &mut Emu, region: u8, world: u8, key_area: u8, scene: u8) {
    let ram = &mut emu.game.ram;
    ram[0x0706] = region;
    ram[0x0707] = world;
    ram[0x0748] = key_area;
    ram[0x056C] = key_area.wrapping_sub(0x34);
    ram[0x0561] = scene;
    ram[0x075C] = 0;
    ram[0x0701] = 0;
    ram[0x0709] = 0;
    ram[MODE] = 0;
    step(emu, if world == 0 { 300 } else { 150 }, 0);
}

/// Walk and swing for a while, topping Link up as he goes.
fn fight(emu: &mut Emu, frames: usize) {
    let mut f = 0;
    while f < frames {
        buff_link(emu);
        let dir = if (f / 240) % 2 == 0 { RIGHT } else { LEFT };
        for i in 0..60 {
            let pad = if i % 8 < 3 { dir | B } else { dir };
            step(emu, 1, pad);
        }
        f += 60;
        assert_eq!(
            emu.game.exec_errors, 0,
            "interpreter fault during the fight"
        );
    }
}

/// `(bank, addr)` of a group's hit point table.
type HpTable = (u8, u16);

/// `(region, world, key area, scene, hp table)`.
const PALACE_SCENES: &[(u8, u8, u8, u8, HpTable)] = &[
    (0, 3, 0x34, 1, (4, 0x9421)),
    (0, 3, 0x34, 4, (4, 0x9421)),
    (0, 3, 0x34, 13, (4, 0x9421)),
    (0, 3, 0x35, 18, (4, 0x9421)),
    (0, 3, 0x35, 34, (4, 0x9421)),
    (0, 4, 0x36, 2, (4, 0xA921)),
    (0, 4, 0x36, 14, (4, 0xA921)),
    (1, 4, 0x34, 20, (4, 0xA921)),
    (2, 3, 0x34, 38, (4, 0x9421)),
    (2, 4, 0x35, 45, (4, 0xA921)),
    (2, 5, 0x36, 3, (5, 0x9421)),
    (2, 5, 0x36, 20, (5, 0x9421)),
    (2, 5, 0x36, 40, (5, 0x9421)),
];

fn rom_bytes(out: &z2_rando::Output, bank: u8, addr: u16, len: usize) -> Vec<u8> {
    let rom = z2_rando::rom::Rom::from_body(&out.body).unwrap();
    let off = rom.cpu_offset(bank, addr).unwrap();
    rom.read_slice(off, len).unwrap().to_vec()
}

/// The WRAM copy of the enemy lists matches the patched ROM, apart from
/// the bit 7 marks the game sets on entries already used up (keys taken,
/// enemies that stay dead) while it is played.
fn assert_lists_copied(wram: &[u8], rom: &[u8], what: &str) {
    let marks = wram
        .iter()
        .zip(rom)
        .filter(|(w, r)| w != r)
        .inspect(|(w, r)| assert_eq!(**w & 0x7F, **r & 0x7F, "{what}: enemy lists"))
        .count();
    assert!(marks <= 16, "{what}: {marks} list bytes differ");
}

/// With `Z2_RANDO_SHOTS=DIR` (outside the work tree: the frames come from
/// the ROM), save a PNG of the screen for eyeballing placements.
fn shot(emu: &Emu, name: &str) {
    if let Some(dir) = std::env::var_os("Z2_RANDO_SHOTS") {
        let png = z2_ppu::encode_indexed_png(emu.game.frame_indexed());
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        std::fs::write(path, png).expect("write shot");
    }
}

fn run_seed(seed: &str, flags: Flags) {
    let patched = flags != Flags::default();
    let (mut emu, out) = rando_emu(seed, flags);
    assert_eq!(
        !emu.rom.untrapped.is_empty(),
        patched,
        "standardized drops patch a ported routine"
    );
    reach_gameplay(&mut emu);
    for &(region, world, key, scene, (bank, hp)) in PALACE_SCENES {
        load_scene(&mut emu, region, world, key, scene);
        let what = format!("{seed}: world {world} scene {scene}");
        assert_eq!(emu.game.exec_errors, 0, "{what}: load faulted");
        assert_eq!(emu.game.ram[MODE], MODE_SIDEVIEW, "{what}: settled");
        // The game copied this group's (randomized) tables into WRAM.
        let want = rom_bytes(&out, bank, hp, 0x24);
        assert_eq!(
            &emu.game.wram[0x0D21..0x0D45],
            &want[..],
            "{what}: hp table"
        );
        let attrs = rom_bytes(&out, bank, hp + 0xB4, 0x48);
        assert_eq!(
            &emu.game.wram[0x0DD5..0x0E1D],
            &attrs[..],
            "{what}: attributes"
        );
        let blob = rom_bytes(&out, bank, 0x88A0, 0x400);
        assert_lists_copied(&emu.game.wram[0x1000..0x1400], &blob, &what);
        step(&mut emu, 60, RIGHT);
        shot(&emu, &format!("{seed}-w{world}-s{scene}"));
        fight(&mut emu, 600);
    }
    // An overworld scene (world 0) loads the shuffled overworld lists.
    load_scene(&mut emu, 0, 0, 0x02, 0);
    assert_eq!(emu.game.exec_errors, 0);
    let blob = rom_bytes(&out, 1, 0x88A0, 0x400);
    assert_lists_copied(&emu.game.wram[0x1000..0x1400], &blob, seed);
    fight(&mut emu, 300);
}

fn tall_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(128 * 1024 * 1024)
        .spawn(body)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn shuffled_enemies_load_and_fight() {
    tall_stack(|| {
        for seed in ["enemies-1", "enemies-2"] {
            run_seed(seed, flags(false));
        }
    });
}

#[test]
#[ignore = "needs Z2_ROM"]
fn mixed_size_enemies_load_and_fight() {
    tall_stack(|| run_seed("mixed-1", flags(true)));
}

/// The same tour with vanilla flags (a baseline for the screenshots).
#[test]
#[ignore = "needs Z2_ROM"]
fn vanilla_tour_baseline() {
    tall_stack(|| run_seed("vanilla", Flags::default()));
}
