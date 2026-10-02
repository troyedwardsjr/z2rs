//! ROM-gated tests for the enemy and boss enhancements
//! (`z2_core::enh::enemies`). Each test stages a synthetic enemy slot in RAM,
//! maps the right PRG bank and runs the **real** ROM routine (through the
//! enhancement hook when the option is on, interpreted as-is when it is
//! off), then asserts the field the option changes. Skips when `Z2_ROM` is
//! unset.

mod common;

use z2_core::enh::enemies::{self, reduced_hp, IK_ABOVE_RANGE, TELEPORT_PAD};
use z2_core::enh::{call_original, hook, EnemyOpts, Enhancements};
use z2_core::game::Game;

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

/// A ROM game with the default trap groups and `opts` (when given).
fn rom_game(rom: &[u8], opts: Option<EnemyOpts>) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    if let Some(o) = opts {
        let e = Enhancements {
            enemies: o,
            ..Enhancements::default()
        };
        g.set_enhancements(e);
    }
    g
}

/// Map `bank` at `$8000` and set the world byte `$0707`.
fn stage(g: &mut Game, bank: u8, world: u8) {
    g.mmc1.ctrl = (g.mmc1.ctrl & 0x03) | 0x0C;
    g.mmc1.prg = bank;
    g.ram[0x0707] = world;
    // Area loader: copy the bank's enemy tables (HP, routine and display
    // vectors, attributes) to WRAM `$6D00`, as on entering a palace.
    g.call_asm(0xCE77);
}

/// Put enemy `code` into `slot` at absolute X `x`, Y `y`, facing `facing`
/// (1 = right, 2 = left), and make it the current slot (`$10`, `X`).
fn spawn(g: &mut Game, slot: usize, code: u8, x: u16, y: u8, facing: u8) {
    let [page, lo] = x.to_be_bytes();
    g.ram[0x00A1 + slot] = code;
    g.ram[0x00B6 + slot] = 1;
    g.ram[0x001A + slot] = 1;
    g.ram[0x003C + slot] = page;
    g.ram[0x004E + slot] = lo;
    g.ram[0x002A + slot] = y;
    g.ram[0x0060 + slot] = facing;
    g.ram[0x00C2 + slot] = 0x10;
    g.ram[0x0010] = slot as u8;
    g.cpu.x = slot as u8;
}

fn place_link(g: &mut Game, x: u16, y: u8) {
    let [page, lo] = x.to_be_bytes();
    g.ram[0x003B] = page;
    g.ram[0x004D] = lo;
    g.ram[0x0029] = y;
}

/// Run the enemy routine at `addr` the way the dispatcher does: through
/// its trap when one is registered (the enhancement hook), else as ROM.
fn run(g: &mut Game, addr: u16) {
    match g.traps.get(addr).map(|t| t.func) {
        Some(f) => f(g),
        None => g.call_asm(addr),
    }
}

#[test]
fn ra_hp_is_halved_in_the_wram_copy() {
    let Some(rom) = common::rom_bytes("ra_hp_is_halved_in_the_wram_copy") else {
        return;
    };
    let on = EnemyOpts {
        ra_hp_reduced: true,
        ..EnemyOpts::default()
    };
    for (world, bank, code) in [
        (3u8, 4u8, enemies::RA_CODE_PALACE),
        (5, 5, enemies::RA_CODE_GREAT_PALACE),
    ] {
        let hp = |opts| {
            let mut g = rom_game(&rom, opts);
            // `stage` runs bank7_Transfer_2A1_bytes_... (the WRAM copy).
            stage(&mut g, bank, world);
            let w = g.wram();
            (
                w[0x0D21 + usize::from(code)],
                w[0x0D21 + usize::from(code) + 1],
            )
        };
        let (og, og_next) = hp(None);
        let (en, en_next) = hp(Some(on));
        assert!(og > 1 && og != 0xFF, "world {world}: Ra HP {og:#x}");
        assert_eq!(en, reduced_hp(og), "world {world}");
        assert_eq!(en_next, og_next, "world {world}: neighbour untouched");
    }
}

#[test]
fn stalfos_upthrust_no_longer_hides_the_sword() {
    let Some(rom) = common::rom_bytes("stalfos_upthrust_no_longer_hides_the_sword") else {
        return;
    };
    let on = EnemyOpts {
        stalfos_upthrust_fix: true,
        ..EnemyOpts::default()
    };
    let sword_after = |opts, link_anim| {
        let mut g = rom_game(&rom, opts);
        stage(&mut g, 4, 3);
        spawn(&mut g, 1, 0x1F, 0x80, 0x90, 2);
        g.ram[0x00AF + 1] = 1; // active (not waiting to drop)
        place_link(&mut g, 0x60, 0x90);
        g.ram[0x0080] = link_anim;
        g.ram[0x0480] = 0x20;
        run(&mut g, enemies::ADDR_STALFOS);
        g.ram[0x0480]
    };
    // Up-thrust frame (Link animation 8): the ROM leaves "no sword" behind.
    assert_eq!(sword_after(None, 8), 0xF8);
    assert_eq!(sword_after(Some(on), 8), 0x20);
    // Any other frame: unchanged either way.
    assert_eq!(sword_after(None, 0), 0x20);
    assert_eq!(sword_after(Some(on), 0), 0x20);
}

/// Run one Iron Knuckle frame; `true` = it went to the idle branch
/// (`L9E0F` loads its idle wait `$50` into `$0504`).
fn ik_idles(
    rom: &[u8],
    opts: Option<EnemyOpts>,
    wide: bool,
    ik_x: u16,
    facing: u8,
    link_y: u8,
) -> bool {
    let mut g = rom_game(rom, opts);
    if wide {
        g.set_wide_gameplay(Some(8));
    }
    stage(&mut g, 4, 3);
    spawn(&mut g, 0, 0x18, ik_x, 0x90, facing);
    place_link(&mut g, 0x80, link_y);
    g.call_asm(0x9C8C);
    g.ram[0x0504] == 0x50
}

#[test]
fn iron_knuckle_aggro_range_and_margin_rules() {
    let Some(rom) = common::rom_bytes("iron_knuckle_aggro_range_and_margin_rules") else {
        return;
    };
    let on = Some(EnemyOpts {
        ironknuckle_aggro: true,
        ..EnemyOpts::default()
    });
    // Level with Link, on screen: aggro either way.
    assert!(!ik_idles(&rom, None, false, 0xB0, 2, 0x90));
    assert!(!ik_idles(&rom, on, false, 0xB0, 2, 0x90));
    // Link $2F px above: the ROM gives up, the enhancement keeps aggro.
    assert!(ik_idles(&rom, None, false, 0xB0, 2, 0x90 - 0x2F));
    assert!(!ik_idles(&rom, on, false, 0xB0, 2, 0x90 - 0x2F));
    // Well above (past a full jump): idle either way.
    assert!(ik_idles(
        &rom,
        on,
        false,
        0xB0,
        2,
        0x90 - (IK_ABOVE_RANGE + 2)
    ));
    // Link below: idle either way.
    assert!(ik_idles(&rom, None, false, 0xB0, 2, 0x98));
    assert!(ik_idles(&rom, on, false, 0xB0, 2, 0x98));
    // In the right widescreen margin (screen X 264): the ROM idles; the
    // enhancement aggros only when the Iron Knuckle faces Link (left).
    assert!(ik_idles(&rom, None, true, 0x108, 2, 0x90));
    assert!(!ik_idles(&rom, on, true, 0x108, 2, 0x90));
    assert!(ik_idles(&rom, on, true, 0x108, 1, 0x90));
    // Same spot without widescreen: idle (never visible there).
    assert!(ik_idles(&rom, on, false, 0x108, 2, 0x90));
}

/// Mago fires (its `$AF` reaches `$D0`); returns the fireball's X.
fn mago_shot_x(rom: &[u8], opts: Option<EnemyOpts>, link_x: u16) -> u16 {
    let mut g = rom_game(rom, opts);
    stage(&mut g, 4, 3);
    spawn(&mut g, 2, 0x1D, 0x80, 0x90, 1);
    g.ram[0x00AF + 2] = 0xD1;
    place_link(&mut g, link_x, 0x90);
    let before: Vec<u8> = g.ram[0x87..0x8D].to_vec();
    run(&mut g, enemies::ADDR_MAGO);
    let p = (0..6)
        .find(|&p| before[p] == 0 && g.ram[0x87 + p] != 0)
        .expect("Mago fired");
    u16::from_be_bytes([g.ram[0x42 + p], g.ram[0x54 + p]])
}

#[test]
fn mago_fireball_spawns_mirrored() {
    let Some(rom) = common::rom_bytes("mago_fireball_spawns_mirrored") else {
        return;
    };
    let on = Some(EnemyOpts {
        mago_balance: true,
        ..EnemyOpts::default()
    });
    // Facing right (Link to the right): +8 either way.
    assert_eq!(mago_shot_x(&rom, None, 0xE0), 0x88);
    assert_eq!(mago_shot_x(&rom, on, 0xE0), 0x88);
    // Facing left: the ROM spawns 8 px outside (-8), balanced spawns at +0
    // (the mirror of +8 for an 8-px shot from a 16-px Mago).
    assert_eq!(mago_shot_x(&rom, None, 0x20), 0x78);
    assert_eq!(mago_shot_x(&rom, on, 0x20), 0x80);
}

#[test]
fn lone_mago_teleports_near_link() {
    let Some(rom) = common::rom_bytes("lone_mago_teleports_near_link") else {
        return;
    };
    let on = Some(EnemyOpts {
        mago_balance: true,
        ..EnemyOpts::default()
    });
    let landing = |opts, rng: u8, other_mago: bool| {
        let mut g = rom_game(&rom, opts);
        stage(&mut g, 4, 3);
        if other_mago {
            spawn(&mut g, 4, 0x1D, 0x30, 0x90, 1);
        }
        spawn(&mut g, 2, 0x1D, 0x80, 0x90, 1);
        g.ram[0x00AF + 2] = 0x50; // invisible, relocating
        g.ram[0x051B + 2] = rng;
        place_link(&mut g, 0x180, 0x90);
        g.ram[0x072A] = 0x01;
        g.ram[0x072C] = 0x00;
        run(&mut g, enemies::ADDR_MAGO);
        u16::from_be_bytes([g.ram[0x3C + 2], g.ram[0x4E + 2]])
    };
    let mut far = false;
    for rng in (0..=255u8).step_by(5) {
        let og = landing(None, rng, false);
        far |= og.abs_diff(0x180) > 0x70;
        let en = landing(on, rng, false);
        assert!(
            en.abs_diff(0x180) >= 20 && en.abs_diff(0x180) < 104,
            "{en:#x}"
        );
        assert!((0x108..=0x1E8).contains(&en), "{en:#x}");
        // With a second Mago around, the original placement stays.
        assert_eq!(landing(on, rng, true), landing(None, rng, true));
    }
    assert!(far, "the ROM places Magos anywhere on screen");
}

#[test]
fn wizard_teleports_over_every_page() {
    let Some(rom) = common::rom_bytes("wizard_teleports_over_every_page") else {
        return;
    };
    let on = Some(EnemyOpts {
        wizard_teleport_wide: true,
        ..EnemyOpts::default()
    });
    let teleport = |opts, rng: u8, frame: u8| {
        let mut g = rom_game(&rom, opts);
        stage(&mut g, 4, 4);
        spawn(&mut g, 3, 0x1D, 0x180, 0x90, 2);
        g.ram[0x00AF + 3] = 0x20; // next DEC drops below $40: teleport
        g.ram[0x051B + 3] = rng;
        g.ram[0x0012] = frame;
        g.ram[0x00D1] = 3;
        place_link(&mut g, 0x1A0, 0x90);
        run(&mut g, enemies::ADDR_WIZARD);
        assert_eq!(g.ram[0x00AF + 3], 0, "teleported");
        (g.ram[0x3C + 3], g.ram[0x4E + 3])
    };
    let mut pages = [0u32; 4];
    for i in 0..64u8 {
        let (og_page, _) = teleport(None, i.wrapping_mul(37), i);
        assert_eq!(og_page, 1, "ROM: page 1 only");
        let (page, x) = teleport(on, i.wrapping_mul(37), i);
        assert!(page <= 3);
        assert!(x >= TELEPORT_PAD && u16::from(x) + 16 <= 256 - u16::from(TELEPORT_PAD));
        pages[usize::from(page)] += 1;
    }
    assert!(pages.iter().all(|&n| n > 0), "{pages:?}");
}

#[test]
fn carock_stays_solid_longer() {
    let Some(rom) = common::rom_bytes("carock_stays_solid_longer") else {
        return;
    };
    let on = Some(EnemyOpts {
        carock_longer_vuln: true,
        ..EnemyOpts::default()
    });
    // `$81` ends at 1 only on the solid branch (`LAEB2: INC $81`).
    let solid = |opts, aux: u8| {
        let mut g = rom_game(&rom, opts);
        stage(&mut g, 4, 4);
        spawn(&mut g, 0, 0x22, 0x180, 0x90, 2);
        g.ram[0x00AF] = aux;
        g.ram[0x0728] = 1; // arena locked: the boss is active
        g.ram[0x072A] = 1;
        place_link(&mut g, 0x120, 0x90);
        run(&mut g, enemies::ADDR_CAROCK);
        g.ram[0x0081] == 1
    };
    let count = |opts| (0..=0xFFu8).filter(|&a| solid(opts, a)).count();
    assert_eq!(count(None), 16);
    assert_eq!(count(on), 32);
    assert!(!solid(None, 0xD8));
    assert!(solid(on, 0xD8));
}

/// One Helmethead frame on a projectile frame (`$12 & $7F == 0`); returns
/// whether a projectile was spawned and the delay timer after the frame.
fn helmethead_fires(g: &mut Game) -> bool {
    stage(g, 4, 3);
    g.ram[0x0706] = 0;
    spawn(g, 0, 0x21, 0x180, 0x80, 2);
    place_link(g, 0x140, 0x90);
    g.ram[0x072A] = 1;
    g.ram[0x0728] = 1;
    g.ram[0x0012] = 0;
    g.ram[0x87..0x8D].fill(0);
    run(g, enemies::ADDR_HELMET_GOOMA);
    g.ram[0x87..0x8D].iter().any(|&t| t != 0)
}

#[test]
fn boss_holds_its_first_attack() {
    let Some(rom) = common::rom_bytes("boss_holds_its_first_attack") else {
        return;
    };
    let on = EnemyOpts {
        boss_first_attack_delay: true,
        ..EnemyOpts::default()
    };
    let mut og = rom_game(&rom, None);
    assert!(helmethead_fires(&mut og), "ROM fires on frame 0");
    let mut g = rom_game(&rom, Some(on));
    assert!(!helmethead_fires(&mut g), "held back");
    assert_eq!(
        g.enh_state.timers[3],
        0x8000 | enemies::BOSS_DELAY_FRAMES,
        "armed"
    );
    // Still delayed on the next projectile frame.
    g.enh_state.timers[3] = 0x8000 | 1;
    assert!(!helmethead_fires(&mut g));
    // Delay over (armed, 0 frames left; the countdown itself is covered by
    // the unit tests): the boss attacks, and is not re-armed.
    g.enh_state.timers[3] = 0x8000;
    assert!(helmethead_fires(&mut g), "attacks after the delay");
    assert_eq!(g.enh_state.timers[3], 0x8000);
}

/// `$E677` spy: remember the sword Y the sword-hit check saw.
fn sword_spy(g: &mut Game) {
    g.ram[0x07FF] = g.ram[0x0480];
    call_original(g, 0xE677);
}

#[test]
fn helmethead_ignores_the_sword_while_a_head_regrows() {
    let Some(rom) = common::rom_bytes("helmethead_ignores_the_sword_while_a_head_regrows") else {
        return;
    };
    let on = EnemyOpts {
        helmethead_fix: true,
        ..EnemyOpts::default()
    };
    let seen = |opts, heads_lost: u8, regrow: u8| {
        let mut g = rom_game(&rom, opts);
        hook(
            &mut g,
            "test_sword_spy",
            Some(7),
            0xE677,
            sword_spy,
            Some(0),
        );
        stage(&mut g, 4, 3);
        g.ram[0x0706] = 0;
        spawn(&mut g, 0, 0x21, 0x180, 0x80, 2);
        g.ram[0x0081] = heads_lost;
        g.ram[0x05DE] = regrow;
        place_link(&mut g, 0x168, 0x80);
        g.ram[0x072A] = 1;
        g.ram[0x0728] = 1;
        g.ram[0x0012] = 1;
        g.ram[0x0480] = 0x10;
        g.ram[0x07FF] = 0;
        run(&mut g, enemies::ADDR_HELMET_GOOMA);
        (g.ram[0x07FF], g.ram[0x0480])
    };
    // Regrowing: the boss's sword check sees "no sword", restored after.
    assert_eq!(seen(None, 1, 0x10), (0x10, 0x10));
    assert_eq!(seen(Some(on), 1, 0x10), (0xF8, 0x10));
    // Head grown (cooldown over) or no head lost yet: untouched.
    assert_eq!(seen(Some(on), 1, 0), (0x10, 0x10));
    assert_eq!(seen(Some(on), 0, 0x10), (0x10, 0x10));
}

#[test]
fn barba_fireballs_aim_at_link() {
    let Some(rom) = common::rom_bytes("barba_fireballs_aim_at_link") else {
        return;
    };
    let on = Some(EnemyOpts {
        barba_aim: true,
        ..EnemyOpts::default()
    });
    let shot = |opts| {
        let mut g = rom_game(&rom, opts);
        stage(&mut g, 4, 4);
        spawn(&mut g, 0, 0x21, 0x180, 0x80, 2);
        g.ram[0x0499] = 2; // firing phase
        g.ram[0x050D] = 0;
        g.ram[0x0504] = 0x41;
        g.ram[0x049A] = 0x80;
        g.ram[0x0728] = 1; // arena locked: the boss is active
        g.ram[0x072A] = 1;
        g.ram[0x0012] = 1;
        place_link(&mut g, 0x120, 0xB0);
        g.ram[0x87..0x8D].fill(0);
        run(&mut g, enemies::ADDR_BARBA);
        let p = (0..6).find(|&p| g.ram[0x87 + p] != 0)?;
        Some((
            g.ram[0x77 + p] as i8,
            g.ram[0x584 + p] as i8,
            i32::from(g.ram[0x30 + p]),
        ))
    };
    let (ovx, ovy, _) = shot(None).expect("Barba fires");
    let (vx, vy, py) = shot(on).expect("Barba fires");
    // Link is to the left and below the mouth.
    assert!(vx < 0, "{vx}");
    assert!(py + 4 < 0xB0 + 16 && vy > 0, "{vy} from y {py:#x}");
    let s0 = i32::from(ovx).pow(2) + i32::from(ovy).pow(2);
    let s1 = i32::from(vx).pow(2) + i32::from(vy).pow(2);
    let tol = 2 * (f64::from(s0).sqrt() as i32) + 2;
    assert!((s1 - s0).abs() <= tol, "speed {s0} -> {s1}");
}

#[test]
fn horsehead_holds_its_first_swing() {
    let Some(rom) = common::rom_bytes("horsehead_holds_its_first_swing") else {
        return;
    };
    let on = Some(EnemyOpts {
        boss_first_attack_delay: true,
        ..EnemyOpts::default()
    });
    // Link right in front of Horsehead: the ROM starts a swing (`$81` 0 → 1).
    let swing = |opts| {
        let mut g = rom_game(&rom, opts);
        stage(&mut g, 4, 3);
        spawn(&mut g, 0, 0x20, 0x180, 0x90, 2);
        place_link(&mut g, 0x170, 0x90);
        g.ram[0x072A] = 1;
        g.ram[0x0728] = 1;
        g.ram[0x0012] = 1;
        g.ram[0x051B] = 0;
        run(&mut g, enemies::ADDR_HORSEHEAD);
        g.ram[0x0081]
    };
    assert_eq!(swing(None), 1);
    assert_eq!(swing(on), 0);
}

// ------------------------------------------------------------ real run

/// `anypct.bk2` pad-1 track from `$Z2_CORPUS` (the corpus movies never
/// enter a palace; they only bring the game into normal play).
fn anypct_track(test: &str) -> Option<Vec<u8>> {
    let dir = common::env_dir("Z2_CORPUS", "/Volumes/HolyDrive/dev/z2-corpus", test)?;
    let path = common::file_present(&dir.join("movies/anypct.bk2"), test)?;
    let bytes = std::fs::read(path).ok()?;
    Some(
        z2_verify::movie_bk2::parse_bk2_zip(&bytes)
            .expect("parse bk2")
            .pad1_track(),
    )
}

/// Play `anypct` into normal play, then load Helmethead's room (palace 2,
/// world 3 scene 34) through the game's own loader and walk right into the
/// arena. Returns the game and the frame (since the load) the arena locked.
fn helmethead_arena(rom: &[u8], track: &[u8], opts: Option<EnemyOpts>) -> (Game, usize) {
    let mut g = rom_game(rom, opts);
    g.reset();
    for &pad in &track[..3000] {
        g.step(pad);
    }
    // Scene registers, then game mode 0 (README.md).
    g.ram[0x0706] = 0;
    g.ram[0x0707] = 3;
    g.ram[0x0748] = 0x35;
    g.ram[0x056C] = 0x01;
    g.ram[0x0561] = 34;
    g.ram[0x075C] = 0;
    g.ram[0x0701] = 0;
    g.ram[0x0774] = 0x80;
    g.ram[0x0736] = 0;
    for f in 0..600 {
        g.step(if f < 150 { 0 } else { 0x80 });
        if g.ram[0x0728] != 0 {
            return (g, f);
        }
    }
    panic!("never reached Helmethead's arena");
}

fn any_projectile(g: &Game) -> bool {
    g.ram[0x87..0x8D].iter().any(|&t| t != 0)
}

#[test]
fn real_run_helmethead_arena_with_every_enemy_option() {
    const TEST: &str = "real_run_helmethead_arena_with_every_enemy_option";
    let Some(rom) = common::rom_bytes(TEST) else {
        return;
    };
    let Some(track) = anypct_track(TEST) else {
        return;
    };
    let all = Enhancements::zalia_preset().enemies;

    // The ROM: the arena locks and Helmethead fires on that very frame.
    let (og, og_lock) = helmethead_arena(&rom, &track, None);
    assert!(any_projectile(&og), "ROM fires on the lock frame");
    let og_ra = og.wram()[0x0D21 + usize::from(enemies::RA_CODE_PALACE)];

    // Every enemy option on: same inputs reach the same arena.
    let (mut g, lock) = helmethead_arena(&rom, &track, Some(all));
    assert_eq!(lock, og_lock, "nothing differs before the fight");
    // The loader copied the halved Ra HP for palaces 1/2/5.
    assert_eq!(
        g.wram()[0x0D21 + usize::from(enemies::RA_CODE_PALACE)],
        reduced_hp(og_ra)
    );
    // The first attack is held back for the whole delay ...
    assert_eq!(g.enh_state.timers[3] & 0x8000, 0x8000, "armed on lock");
    let delay = usize::from(enemies::BOSS_DELAY_FRAMES);
    for f in 0..delay - 2 {
        assert!(!any_projectile(&g), "projectile {f} frames into the fight");
        g.step(0);
    }
    // ... and the boss fights normally afterwards (it fires every 128).
    let fired = (0..200).any(|_| {
        g.step(0);
        any_projectile(&g)
    });
    assert!(fired, "Helmethead fires after the delay");
    assert_eq!(g.exec_errors, 0);
    assert_eq!(og.exec_errors, 0);
}
