//! Randomizer (`enh::rando`) and cheats (`enh::cheats`) in real runs.
//!
//! ROM-gated tests self-skip unless `Z2_ROM` points at a file; the any%
//! movie comes from `Z2_CORPUS_MOVIES` (directory holding `anypct.bk2`),
//! else `$Z2_CORPUS/movies`, else the out-of-tree corpus, and also
//! self-skips when absent. The movie starts a new file within its first
//! 25 frames and stands in the North Palace (side view, mode `$0B`) from
//! about frame 185 to 447. No ROM bytes are stored in the tree: every
//! expectation is derived from the ROM image at run time.

#![cfg(feature = "interp")]
// One group set at a time reads clearer than a nested struct literal.
#![allow(clippy::field_reassign_with_default)]

mod common;

use z2_core::enh::rando::{self, RandoOpts};
use z2_core::enh::{CheatOpts, Enhancements};
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

fn rom_game(rom: &[u8], e: Enhancements) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    g.set_enhancements(e);
    g.reset();
    g
}

fn anypct_track(test: &str) -> Option<Vec<u8>> {
    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .or_else(|| common::var_present("Z2_CORPUS").map(|c| format!("{c}/movies")))
        .unwrap_or_else(|| "/Volumes/HolyDrive/dev/z2-corpus/movies".to_string());
    let path = common::file_present(&std::path::Path::new(&dir).join("anypct.bk2"), test)?;
    let zip = std::fs::read(&path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&zip).expect("parse anypct.bk2");
    Some(movie.pad1_track())
}

fn fixtures(test: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let rom = common::rom_bytes(test)?;
    let track = anypct_track(test)?;
    Some((rom, track))
}

fn facts(g: &Game) -> z2_core::facts::GameFacts {
    let ram = z2_core::ram::Ram::from_slice(g.ram()).expect("2 KiB");
    z2_core::facts::Game::new(ram).facts()
}

fn prg_at(g: &Game, bank: u8, addr: u16, len: usize) -> Vec<u8> {
    let off = rando::prg_offset(bank, addr);
    g.prg[off..off + len].to_vec()
}

const MODE: usize = 0x0736;
const GAME_STATE: usize = 0x076C;
const LIVES: usize = 0x0700;

// ------------------------------------------------------------ start loadout

#[test]
fn new_game_starts_with_the_loadout() {
    let Some((rom, track)) = fixtures("new_game_starts_with_the_loadout") else {
        return;
    };
    let mut e = Enhancements::default();
    e.rando = RandoOpts {
        start_attack: 5,
        start_magic: 3,
        start_life: 7,
        start_spells: 0b0000_0011,    // shield + jump
        start_items: 0x0001 | 0x0100, // candle + downward thrust
        start_containers_heart: 6,
        start_containers_magic: 5,
        ..RandoOpts::default()
    };
    let mut g = rom_game(&rom, e);
    for &p in track.iter().take(300) {
        g.step(p);
    }
    assert_eq!(g.ram[MODE], 0x0B, "in the North Palace side view");
    let f = facts(&g);
    let json = f.to_json();
    assert_eq!(
        (f.link.attack, f.link.magic, f.link.life),
        (5, 3, 7),
        "{json}"
    );
    assert!(json.contains("\"attack\":5"), "{json}");
    assert!(f.spells.shield && f.spells.jump && !f.spells.life);
    assert!(f.items.candle && !f.items.glove);
    assert_eq!(g.ram[0x0796] & 0x10, 0x10, "downward thrust");
    assert_eq!((g.ram[0x0783], g.ram[0x0784]), (5, 6));
    // Meters refilled for the new container counts (`LCB18`).
    assert_eq!(f.link.hp, 6 * 32 - 1);
    assert_eq!(f.link.mp, 5 * 32 - 1);
    // "Next level" is the cheapest of the three level-ups (`$A057`).
    let cost = |stat: usize, level: u8| {
        let i = stat * 8 + usize::from(level) - 1;
        u16::from(prg_at(&g, 0, 0x9659, 24)[i]) << 8 | u16::from(prg_at(&g, 0, 0x9671, 24)[i])
    };
    let want = cost(0, 5).min(cost(1, 3)).min(cost(2, 7));
    assert_eq!(
        u16::from(g.ram[0x0770]) << 8 | u16::from(g.ram[0x0771]),
        want
    );

    // The same run without the loadout keeps the original start.
    let mut og = rom_game(&rom, Enhancements::default());
    for &p in track.iter().take(300) {
        og.step(p);
    }
    let f0 = facts(&og);
    assert_eq!((f0.link.attack, f0.link.magic, f0.link.life), (1, 1, 1));
    assert_eq!(f0.link.hp, 4 * 32 - 1);
}

// ------------------------------------------------------------ scalers

#[test]
fn enemy_hp_tables_scale_within_the_percentage_and_reach_wram() {
    let Some((rom, track)) = fixtures("enemy_hp_tables_scale_within_the_percentage") else {
        return;
    };
    let pristine = Game::from_ines(&rom).expect("ROM");
    for pct in [20i8, -25] {
        let mut e = Enhancements::default();
        e.rando = RandoOpts {
            seed: 0xC0FFEE,
            enemy_hp_pct: pct,
            xp_pct: pct,
            spell_cost_pct: pct,
            ..RandoOpts::default()
        };
        let g = rom_game(&rom, e);
        let mut changed = 0;
        for bank in rando::ENEMY_BANKS {
            let orig = prg_at(&pristine, bank, rando::ENEMY_HP_ADDR, rando::ENEMY_COUNT);
            let new = prg_at(&g, bank, rando::ENEMY_HP_ADDR, rando::ENEMY_COUNT);
            for (&o, &n) in orig.iter().zip(&new) {
                if o == 0 || o >= 0xF0 {
                    assert_eq!(n, o, "markers stay");
                    continue;
                }
                let (o32, n32) = (i32::from(o), i32::from(n));
                let p = i32::from(pct);
                // Within [half, full] of the request (+ rounding), never 0.
                let lo = (o32 * (100 + p.min(p / 2)) - 50).div_euclid(100);
                let hi = (o32 * (100 + p.max(p / 2)) + 50).div_euclid(100);
                assert!(
                    n32 >= lo.max(1) && n32 <= hi.min(0xEF),
                    "{o} -> {n} at {pct}%"
                );
                if n != o {
                    changed += 1;
                }
            }
        }
        assert!(changed > 20, "most HP entries move at {pct}%: {changed}");
        // XP entry 0 stays 0; others move the right way.
        let xp = |g: &Game, i: usize| {
            u16::from(prg_at(g, 7, rando::XP_HI_ADDR, 16)[i]) << 8
                | u16::from(prg_at(g, 7, rando::XP_LO_ADDR, 16)[i])
        };
        assert_eq!(xp(&g, 0), 0);
        for i in 1..16 {
            if pct > 0 {
                assert!(xp(&g, i) >= xp(&pristine, i));
            } else {
                assert!(xp(&g, i) <= xp(&pristine, i));
            }
        }
    }

    // The game copies the (patched) table into WRAM `$6D21` itself.
    let mut e = Enhancements::default();
    e.rando = RandoOpts {
        seed: 7,
        enemy_hp_pct: 25,
        ..RandoOpts::default()
    };
    let mut g = rom_game(&rom, e);
    for &p in track.iter().take(300) {
        g.step(p);
    }
    let wram = g.wram()[0xD21..0xD21 + rando::ENEMY_COUNT].to_vec();
    let patched: Vec<Vec<u8>> = rando::ENEMY_BANKS
        .iter()
        .map(|&b| prg_at(&g, b, rando::ENEMY_HP_ADDR, rando::ENEMY_COUNT))
        .collect();
    let originals: Vec<Vec<u8>> = rando::ENEMY_BANKS
        .iter()
        .map(|&b| prg_at(&pristine, b, rando::ENEMY_HP_ADDR, rando::ENEMY_COUNT))
        .collect();
    assert!(patched.contains(&wram), "WRAM holds a patched HP table");
    assert!(!originals.contains(&wram), "and not an original one");
}

#[test]
fn same_seed_same_rom_patches_and_off_is_pristine() {
    let Some(rom) = common::rom_bytes("same_seed_same_rom_patches") else {
        return;
    };
    let mut e = Enhancements::default();
    e.rando = RandoOpts {
        seed: 12345,
        enemy_hp_pct: -10,
        enemy_dmg_pct: 15,
        xp_pct: 25,
        level_cost_pct: -25,
        spell_cost_pct: 10,
        palette_rando: 2,
        item_shuffle: true,
        ..RandoOpts::default()
    };
    let a = rom_game(&rom, e);
    let b = rom_game(&rom, e);
    assert_eq!(a.prg, b.prg);
    let pristine = rom_game(&rom, Enhancements::default());
    assert_ne!(a.prg, pristine.prg);
    // Link's palette moved everywhere it lives, consistently.
    let link = prg_at(&a, 7, 0xC454, 3);
    assert_ne!(link, rando::LINK_PALETTE.to_vec());
    assert_eq!(prg_at(&a, 1, 0x809F, 3), link);
    assert_eq!(prg_at(&a, 5, 0x80CF, 3), link);
    // A different seed differs; turning everything off restores the ROM.
    let mut e2 = e;
    e2.rando.seed = 54321;
    assert_ne!(rom_game(&rom, e2).prg, a.prg);
    let mut c = rom_game(&rom, e);
    c.set_enhancements(Enhancements::default());
    assert_eq!(c.prg, pristine.prg);
}

// ------------------------------------------------------------ item shuffle

#[test]
fn shuffled_pickup_grants_the_placed_item() {
    let Some(rom) = common::rom_bytes("shuffled_pickup_grants_the_placed_item") else {
        return;
    };
    // A seed whose candle location holds something else.
    let opts = (1..)
        .map(|seed| RandoOpts {
            seed,
            item_shuffle: true,
            ..RandoOpts::default()
        })
        .find(|o| rando::item_placement(o)[rando::CANDLE] != rando::CANDLE as u8)
        .unwrap();
    let placed = usize::from(rando::item_placement(&opts)[rando::CANDLE]);
    for (shuffle, want) in [(false, rando::CANDLE), (true, placed)] {
        let mut e = Enhancements::default();
        e.rando = if shuffle {
            opts
        } else {
            // Something else on so the group is live but items are vanilla.
            RandoOpts {
                xp_pct: 1,
                ..RandoOpts::default()
            }
        };
        let mut g = rom_game(&rom, e);
        for _ in 0..30 {
            g.step(0);
        }
        g.ram[0x0785..0x078D].fill(0);
        // Item object in slot 0: candle code, bit 7 = presence already
        // recorded (skips the `$C295` presence write).
        g.ram[0x0010] = 0;
        g.cpu.x = 0;
        g.ram[0x00AF] = 0x80;
        // Enter `bank7_get_item` the way a `JSR` does: through its trap
        // (the shuffle hook) when there is one, else interpreted.
        if g.traps.is_trapped(rando::GET_ITEM_ADDR) {
            let _ = g.fire_trap(rando::GET_ITEM_ADDR);
        } else {
            g.call_asm(rando::GET_ITEM_ADDR);
        }
        assert_eq!(g.traps.is_trapped(rando::GET_ITEM_ADDR), shuffle);
        assert_eq!(g.ram[0x00AF], 0x80, "object code restored");
        let owned: Vec<usize> = (0..8).filter(|&i| g.ram[0x0785 + i] != 0).collect();
        assert_eq!(owned, vec![want], "shuffle {shuffle}");
    }
}

// ------------------------------------------------------------ cheats

#[test]
fn cheats_hold_hp_mp_and_survive_a_lethal_hit() {
    let Some((rom, track)) = fixtures("cheats_hold_hp_mp_and_survive_a_lethal_hit") else {
        return;
    };
    let run = |cheats: CheatOpts| {
        let mut e = Enhancements::default();
        e.cheats = cheats;
        let mut g = rom_game(&rom, e);
        for &p in track.iter().take(250) {
            g.step(p);
        }
        assert_eq!(g.ram[MODE], 0x0B);
        // The state a lethal hit leaves (`$E33A-$E353`): meter emptied,
        // kill flag raised, injured timer `$20`; MP drained as well.
        g.ram[0x0774] = 0;
        g.ram[0x0494] = 1;
        g.ram[0x050C] = 0x20;
        g.ram[0x0773] = 0;
        let lives = g.ram[LIVES];
        let mut died = false;
        let mut meters_full = true;
        for &p in track.iter().skip(250).take(90) {
            g.step(p);
            died |= g.ram[GAME_STATE] == 2;
            meters_full &= g.ram[0x0774] == 4 * 32 - 1 && g.ram[0x0773] == 4 * 32 - 1;
        }
        (died, meters_full, lives, g.ram[LIVES])
    };
    let (died, _, lives0, lives1) = run(CheatOpts::default());
    assert!(died, "without cheats the hit kills");
    assert!(lives1 <= lives0);
    let (died, full, lives0, lives1) = run(CheatOpts {
        invincible: true,
        infinite_magic: true,
        infinite_lives: true,
        max_stats: false,
    });
    assert!(!died, "invincible survives the hit");
    assert!(full, "HP and MP held full");
    assert_eq!(lives0, lives1);
}

#[test]
fn max_stats_shows_in_facts() {
    let Some((rom, track)) = fixtures("max_stats_shows_in_facts") else {
        return;
    };
    let mut e = Enhancements::default();
    e.cheats.max_stats = true;
    let mut g = rom_game(&rom, e);
    for &p in track.iter().take(300) {
        g.step(p);
    }
    let f = facts(&g);
    assert_eq!((f.link.attack, f.link.magic, f.link.life), (8, 8, 8));
    assert_eq!((g.ram[0x0783], g.ram[0x0784]), (8, 8));
    // Next level at the cap: the level-8 entry of the table.
    let cap = u16::from(prg_at(&g, 0, 0x9659 + 7, 1)[0]) << 8
        | u16::from(prg_at(&g, 0, 0x9671 + 7, 1)[0]);
    assert_eq!(
        u16::from(g.ram[0x0770]) << 8 | u16::from(g.ram[0x0771]),
        cap
    );
}
