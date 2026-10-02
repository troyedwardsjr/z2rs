//! All enhancement groups together (`z2_core::enh`): ROM-gated checks that
//! the groups' PRG patches and trap hooks do not collide, that the game runs
//! with every gameplay option on at once, and that turning everything off
//! again restores the original PRG image and trap table.
//!
//! Self-skips unless `Z2_ROM` names a file; the replay also needs the any%
//! movie in `Z2_CORPUS_MOVIES` (default the out-of-tree corpus). No ROM
//! bytes or derived data are stored in the tree.

#![cfg(feature = "interp")]

mod common;

use std::collections::BTreeMap;

use z2_core::enh::{
    AbilityOpts, CheatOpts, ContinueFrom, EnemyOpts, Enhancements, FixesOpts, QolOpts, RandoOpts,
    TextHudOpts,
};
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

/// Every gameplay group, each with all of its options on (cheats left out:
/// they hold HP and lives, which would hide a crash in the replay).
fn groups() -> Vec<(&'static str, Enhancements)> {
    let off = Enhancements::default;
    vec![
        (
            "text",
            Enhancements {
                text: TextHudOpts {
                    dialogue_speed: 5,
                    protect_spell_name: true,
                },
                ..off()
            },
        ),
        (
            "qol",
            Enhancements {
                qol: QolOpts {
                    continue_from: ContinueFrom::LastTown,
                    gameover_keep_xp_pct: 25,
                    lives_from_dolls: true,
                    enter_town_from_side: true,
                    wise_men_restore_mp: true,
                    no_mp_requirement_for_spells: true,
                    overworld_softlock_warp: true,
                },
                ..off()
            },
        ),
        (
            "fixes",
            Enhancements {
                fixes: FixesOpts {
                    levelup_softlocks: true,
                    iframe_update_skip: true,
                    jump_direction_balance: true,
                    shield_hitbox_symmetry: true,
                    crumble_both_feet: true,
                    xp_drain_fix: true,
                },
                ..off()
            },
        ),
        (
            "enemies",
            Enhancements {
                enemies: EnemyOpts {
                    ironknuckle_aggro: true,
                    ra_hp_reduced: true,
                    stalfos_upthrust_fix: true,
                    wizard_teleport_wide: true,
                    mago_balance: true,
                    boss_first_attack_delay: true,
                    helmethead_fix: true,
                    carock_longer_vuln: true,
                    barba_aim: true,
                    p5_horsehead: true,
                },
                ..off()
            },
        ),
        (
            "abilities",
            Enhancements {
                abilities: AbilityOpts {
                    double_jump: true,
                    stab_frenzy: true,
                    sword_reach_px: 3,
                    damage_reduction_pct: 50,
                    mp_regen: true,
                    reflect_more: true,
                    dash_speed: true,
                    rescue_fairy: true,
                    flute_warp: true,
                },
                ..off()
            },
        ),
        (
            "rando",
            Enhancements {
                rando: RandoOpts {
                    seed: 0x5A11_A000_2026,
                    start_attack: 3,
                    start_magic: 3,
                    start_life: 3,
                    start_containers_heart: 5,
                    start_containers_magic: 5,
                    enemy_hp_pct: 25,
                    enemy_dmg_pct: -25,
                    xp_pct: 25,
                    level_cost_pct: -25,
                    spell_cost_pct: -25,
                    palette_rando: 2,
                    item_shuffle: true,
                    ..RandoOpts::default()
                },
                ..off()
            },
        ),
        (
            "cheats",
            Enhancements {
                cheats: CheatOpts {
                    invincible: true,
                    infinite_magic: true,
                    infinite_lives: true,
                    max_stats: true,
                },
                ..off()
            },
        ),
    ]
}

fn all_on() -> Enhancements {
    let g = groups();
    let pick = |name: &str| g.iter().find(|(n, _)| *n == name).unwrap().1;
    Enhancements {
        text: pick("text").text,
        qol: pick("qol").qol,
        fixes: pick("fixes").fixes,
        enemies: pick("enemies").enemies,
        abilities: pick("abilities").abilities,
        rando: pick("rando").rando,
        ..Enhancements::default()
    }
}

/// Places two groups share on purpose. The damage table `$E2AF` (bank 7) is
/// scaled by both the abilities damage reduction and the randomizer's damage
/// variance; the randomizer runs second and scales the reduced values.
/// The enemy HP tables `$9421` (banks 1-5) are halved for Ra by the enemies
/// group and then scaled by the randomizer's HP variance.
/// `bank7_get_item` `$E771` is hooked by the QoL doll counter and the item
/// shuffle; the hooks chain.
fn shared_on_purpose(offset: usize) -> bool {
    let damage = 7 * 0x4000 + 0x22AF;
    let hp = |bank: usize| bank * 0x4000 + 0x1421;
    (damage..damage + 7 * 8).contains(&offset)
        || (1..=5).any(|b| (hp(b)..hp(b) + 36).contains(&offset))
}

const SHARED_HOOKS: &[u16] = &[0xE771];

fn fresh(rom: &[u8]) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    g
}

/// Trap entries as (addr, name), for comparing tables.
fn trap_list(g: &Game) -> Vec<(u16, &'static str)> {
    let mut v: Vec<_> = g.traps.iter().map(|t| (t.addr, t.name)).collect();
    v.sort_unstable();
    v
}

#[test]
fn groups_patch_and_hook_disjoint_places() {
    let Some(rom) = common::rom_bytes("groups_patch_and_hook_disjoint_places") else {
        return;
    };
    let base = fresh(&rom);
    let base_traps = trap_list(&base);

    // PRG offset -> group, and trap (addr, name) -> group, over all groups.
    let mut patched: BTreeMap<usize, &str> = BTreeMap::new();
    let mut hooked: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
    let mut clashes = Vec::new();
    for (name, e) in groups() {
        let mut g = fresh(&rom);
        g.set_enhancements(e);
        for (i, (a, b)) in base.prg.iter().zip(&g.prg).enumerate() {
            if a != b {
                if let Some(other) = patched.insert(i, name).filter(|_| !shared_on_purpose(i)) {
                    clashes.push(format!("PRG offset {i:#07x}: {other} and {name}"));
                }
            }
        }
        for t in trap_list(&g) {
            if !base_traps.contains(&t) {
                hooked.entry(t.0).or_default().push(name);
            }
        }
    }
    for (addr, names) in &hooked {
        let mut owners = names.clone();
        owners.dedup();
        if owners.len() > 1 && !SHARED_HOOKS.contains(addr) {
            clashes.push(format!("trap {addr:#06x}: {owners:?}"));
        }
    }
    assert!(
        clashes.is_empty(),
        "groups collide:\n{}",
        clashes.join("\n")
    );
}

#[test]
fn everything_on_runs_and_everything_off_restores() {
    let test = "everything_on_runs_and_everything_off_restores";
    let Some(rom) = common::rom_bytes(test) else {
        return;
    };
    let base = fresh(&rom);

    let mut g = fresh(&rom);
    g.set_enhancements(all_on());
    g.reset();

    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .unwrap_or_else(|| "/Volumes/HolyDrive/dev/z2-corpus/movies".to_string());
    if let Some(path) = common::file_present(&std::path::Path::new(&dir).join("anypct.bk2"), test) {
        let zip = std::fs::read(path).expect("read anypct.bk2");
        let movie = z2_verify::movie_bk2::parse_bk2_zip(&zip).expect("parse anypct.bk2");
        // The movie desyncs once options change the game, but it still feeds
        // eight thousand frames of real input through every mode it reaches.
        for &p in movie.pad1_track().iter().take(8000) {
            g.step(p);
        }
        assert_eq!(g.exec_errors, 0, "interpreter errors with all options on");
    }

    g.set_enhancements(Enhancements::default());
    assert!(
        g.prg == base.prg,
        "PRG not restored after turning options off"
    );
    assert_eq!(trap_list(&g), trap_list(&base), "trap table not restored");
}
