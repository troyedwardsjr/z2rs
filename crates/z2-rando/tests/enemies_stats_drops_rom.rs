//! ROM-gated checks for the `enemies`, `stats` and `drops` modules: run the
//! whole randomizer with their options on over several seeds and read the
//! patched tables back.
//!
//! `Z2_ROM=/path/to/zelda2.nes cargo test -p z2-rando --test
//! enemies_stats_drops_rom -- --ignored`

use z2_rando::enemies::data::{self, Boss, Group};
use z2_rando::enemies::shuffle::{self, kind_of, Kind};
use z2_rando::flags::{
    AttackEffectiveness, DripperEnemy, DropPool, EnemyLife, Flags, LifeEffectiveness,
    MagicEffectiveness, SwordImmunity, Tri, XpEffectiveness,
};
use z2_rando::rom::Rom;
use z2_rando::{drops, randomize, stats};

const SEEDS: [&str; 6] = ["1", "2", "race", "hello", "z2rs", "999999"];

fn body() -> Vec<u8> {
    z2_assets::rom::open().expect("Z2_ROM must name the verified ROM")
}

fn all_on() -> Flags {
    let mut f = Flags::default();
    let e = &mut f.enemies;
    e.shuffle_overworld_enemies = Tri::On;
    e.shuffle_palace_enemies = Tri::On;
    e.mix_large_and_small = Tri::Off;
    e.dripper_enemy = DripperEnemy::EasierGroundEnemiesFullHp;
    e.enemy_hp = EnemyLife::Medium;
    e.boss_hp = EnemyLife::MediumHigh;
    e.shuffle_xp_stealers = true;
    e.shuffle_xp_stolen_amount = true;
    e.sword_immunity = SwordImmunity::Shuffle;
    e.xp_drops = XpEffectiveness::Random;
    let s = &mut f.stats;
    s.shuffle_attack_exp = true;
    s.shuffle_magic_exp = true;
    s.shuffle_life_exp = true;
    s.attack_level_cap = 6;
    s.magic_level_cap = 7;
    s.life_level_cap = 8;
    s.scale_level_requirements_to_cap = true;
    s.attack_effectiveness = AttackEffectiveness::Average;
    s.magic_effectiveness = MagicEffectiveness::Average;
    s.life_effectiveness = LifeEffectiveness::Average;
    let d = &mut f.drops;
    d.shuffle_drop_frequency = true;
    d.standardize_drops = true;
    d.small_pool = DropPool {
        blue_jar: true,
        small_bag: true,
        ..DropPool::default()
    };
    d.randomize_drops = true;
    f.items.shuffle_pbag_amounts = Tri::On;
    f.palaces.aggressive_thunderbird = true;
    f
}

fn rom_of(body: &[u8]) -> Rom {
    Rom::from_body(body).unwrap()
}

#[test]
#[ignore = "needs Z2_ROM"]
fn stats_tables_are_consistent() {
    let body = body();
    let vanilla = rom_of(&body);
    for seed in SEEDS {
        let out = randomize(&body, seed, &all_on()).unwrap();
        let rom = rom_of(&out.body);
        // Experience: tens, monotone, one shared 1-up above everything,
        // digits matching the values.
        let rows = stats::read_experience(&rom).unwrap();
        let caps = [6usize, 7, 8];
        let one_up = rows[0][caps[0] - 1];
        for (s, row) in rows.iter().enumerate() {
            for (i, &v) in row.iter().enumerate() {
                assert_eq!(v % 10, 0);
                if i + 1 < caps[s] {
                    assert!(v < one_up, "{seed}: stat {s} level {i}");
                    if i > 0 {
                        assert!(v >= row[i - 1]);
                    }
                } else {
                    assert_eq!(v, one_up, "{seed}: stat {s} slot {i}");
                }
                let k = s * 8 + i;
                let digits = stats::digit_tiles(v);
                let off = rom
                    .cpu_offset(stats::EXP_DIGITS.0, stats::EXP_DIGITS.1)
                    .unwrap();
                assert_eq!(rom.read(off + k).unwrap(), digits[0]);
                assert_eq!(rom.read(off + 24 + k).unwrap(), digits[1]);
                assert_eq!(rom.read(off + 48 + k).unwrap(), digits[2]);
            }
        }
        // The level-up check calls the cap stub.
        let site = stats::LEVEL_CAP_SITE;
        assert_eq!(rom.read_cpu(site.0, site.1).unwrap(), 0x20, "JSR");
        // Attack monotone, life and magic never rise with level.
        let atk_off = rom.cpu_offset(7, stats::ATTACK_TABLE).unwrap();
        let atk = rom.read_slice(atk_off, 8).unwrap();
        assert!(atk.windows(2).all(|w| w[0] <= w[1]), "{seed}: {atk:?}");
        let life_off = rom.cpu_offset(7, stats::LIFE_TABLE).unwrap();
        for row in rom.read_slice(life_off, 56).unwrap().chunks(8) {
            assert!(row.windows(2).all(|w| w[0] >= w[1]), "{seed}: {row:?}");
        }
        // The always-OHKO row after the table is untouched.
        assert_eq!(
            rom.read_slice(life_off + 56, 8).unwrap(),
            vanilla.read_slice(life_off + 56, 8).unwrap()
        );
        // Rebonack's hit points are not a multiple that kills the horse.
        let rebo = data::boss_hp(&rom, Boss::Rebonack).unwrap();
        assert!(atk
            .iter()
            .all(|&v| v != rebo && u16::from(v) * 2 != u16::from(rebo)));
        // Boss bar divisors give a full bar of at most 8 segments.
        for b in Boss::ALL {
            let hp = data::boss_hp(&rom, b).unwrap();
            for &a in b.bar_divisor_addrs() {
                let div = rom.read_cpu(4, a).unwrap();
                assert!(div >= 1);
                assert!(
                    u16::from(hp).div_ceil(u16::from(div)) <= 8,
                    "{b:?} {hp}/{div}"
                );
            }
        }
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn enemy_tables_respect_their_ranges() {
    let body = body();
    let vanilla = rom_of(&body);
    for seed in SEEDS {
        let out = randomize(&body, seed, &all_on()).unwrap();
        let rom = rom_of(&out.body);
        for g in Group::ALL {
            // Medium: x0.5-1.5 (OHKO is off here).
            for id in g.hp_ids() {
                let v = u32::from(data::hp(&vanilla, g, id).unwrap());
                let n = u32::from(data::hp(&rom, g, id).unwrap());
                if g == Group::GreatPalace && id == 0x22 {
                    continue;
                }
                assert!(
                    n >= v / 2 && n <= (v * 3 / 2).max(v / 2),
                    "{g:?} {id:#X}: {v} -> {n}"
                );
            }
            // Same number of sword-immune and stealing enemies; experience
            // within two ladder steps.
            let ids = g.sets().all();
            for (mask, what) in [(0x20u8, "immune"), (0x10, "steal")] {
                let count = |r: &Rom| {
                    ids.iter()
                        .filter(|&&id| data::attr1(r, g, id).unwrap() & mask != 0)
                        .count()
                };
                assert_eq!(count(&rom), count(&vanilla), "{seed} {g:?} {what}");
            }
            for &id in &ids {
                let v = i32::from(data::attr1(&vanilla, g, id).unwrap() & 0x0F);
                let n = i32::from(data::attr1(&rom, g, id).unwrap() & 0x0F);
                assert!((n - v).abs() <= 2 || n == 0 || n == 15, "{g:?} {id:#X}");
                // Unlisted bits are kept.
                assert_eq!(
                    data::attr1(&rom, g, id).unwrap() & 0xC0,
                    data::attr1(&vanilla, g, id).unwrap() & 0xC0
                );
            }
        }
        // The dripper spawns an easier palace enemy.
        let id = rom.read_cpu(4, 0x9917).unwrap();
        assert!([0x03, 0x04, 0x11, 0x12, 0x0C, 0x18, 0x1F, 0x23].contains(&id));
        assert_eq!(rom.read_cpu(4, 0x9912).unwrap(), 0x20, "full HP hook");
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn enemy_lists_keep_their_shape() {
    let body = body();
    let vanilla = rom_of(&body);
    let mut areas = shuffle::OVERWORLD.to_vec();
    areas.extend(shuffle::PALACES);
    let sites = shuffle::list_sites(&vanilla, &areas).unwrap();
    assert!(sites.len() > 300, "{}", sites.len());
    let mut total_changed = 0;
    for seed in SEEDS {
        let out = randomize(&body, seed, &all_on()).unwrap();
        let rom = rom_of(&out.body);
        for &(bank, addr, g) in &sites {
            assert_eq!(
                rom.read_cpu(bank, addr).unwrap(),
                vanilla.read_cpu(bank, addr).unwrap(),
                "list length at {bank}:{addr:04X}"
            );
            let a = shuffle::read_list(&vanilla, bank, addr).unwrap();
            let b = shuffle::read_list(&rom, bank, addr).unwrap();
            for (x, y) in a.iter().zip(&b) {
                match kind_of(g, x.id) {
                    None => assert_eq!(x, y, "{g:?} {bank}:{addr:04X} untouchable enemy"),
                    Some(k) => {
                        let ky = kind_of(g, y.id);
                        match k {
                            Kind::Small | Kind::Large => {
                                assert_eq!(ky, Some(k), "{g:?} size class kept (no mix)")
                            }
                            Kind::Flying => assert_eq!(ky, Some(Kind::Flying)),
                            Kind::Generator | Kind::DumbMoblinGenerator => assert!(matches!(
                                ky,
                                Some(Kind::Generator | Kind::DumbMoblinGenerator)
                            )),
                        }
                        if x != y {
                            total_changed += 1;
                        }
                    }
                }
            }
            // Generators always match within a list.
            let gens: Vec<u8> = b
                .iter()
                .filter(|e| kind_of(g, e.id) == Some(Kind::Generator))
                .map(|e| e.id)
                .collect();
            assert!(gens.windows(2).all(|w| w[0] == w[1]), "{g:?} {gens:?}");
        }
    }
    assert!(total_changed > 500, "{total_changed}");
}

#[test]
#[ignore = "needs Z2_ROM"]
fn mixed_sizes_stay_on_the_ground() {
    let body = body();
    let vanilla = rom_of(&body);
    let mut f = all_on();
    f.enemies.mix_large_and_small = Tri::On;
    let mut areas = shuffle::OVERWORLD.to_vec();
    areas.extend(shuffle::PALACES);
    let sites = shuffle::list_sites(&vanilla, &areas).unwrap();
    for seed in SEEDS {
        let out = randomize(&body, seed, &f).unwrap();
        let rom = rom_of(&out.body);
        for &(bank, addr, g) in &sites {
            let a = shuffle::read_list(&vanilla, bank, addr).unwrap();
            let b = shuffle::read_list(&rom, bank, addr).unwrap();
            for (x, y) in a.iter().zip(&b) {
                if matches!(kind_of(g, x.id), Some(Kind::Small | Kind::Large)) {
                    assert!(matches!(kind_of(g, y.id), Some(Kind::Small | Kind::Large)));
                }
            }
        }
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn drop_tables_and_fixed_bank_edits() {
    let body = body();
    let vanilla = rom_of(&body);
    for seed in SEEDS {
        let out = randomize(&body, seed, &all_on()).unwrap();
        let rom = rom_of(&out.body);
        let small = rom.read_slice(rom.cpu_offset(7, drops::SMALL_DROPS).unwrap(), 8);
        let small = small.unwrap();
        assert!(small.contains(&0x90) && small.contains(&0x8A), "{small:?}");
        assert!(small.iter().all(|c| drops::POOL_CODES.contains(c)));
        let large = rom
            .read_slice(rom.cpu_offset(7, drops::LARGE_DROPS).unwrap(), 8)
            .unwrap();
        assert!(large.iter().all(|c| drops::POOL_CODES.contains(c)));
        let freq = rom.read_cpu(7, drops::DROP_FREQUENCY).unwrap();
        assert!((4..=8).contains(&freq));
        assert_eq!(rom.read_cpu(7, drops::DROP_DRAW_SITE).unwrap(), 0x20);
        // Fixed-bank edits are only the known tables, the honoured
        // operands, and code in the reserved padding.
        let allowed = |a: u16| {
            (0xE870..0xE880).contains(&a)
                || (0xE7F0..0xE7F4).contains(&a)
                || (0xE66D..0xE675).contains(&a)
                || (0xE2AF..0xE2E7).contains(&a)
                || [0xE8A0, 0xE2FE, 0xE304].contains(&a)
                || (drops::DROP_DRAW_SITE..drops::DROP_DRAW_SITE + 3).contains(&a)
                || (0xD39A..0xD3CA).contains(&a)
                || (0xFEAA..0xFED0).contains(&a)
        };
        for a in &out.fixed_bank_changes {
            assert!(
                allowed(*a),
                "{seed}: unexpected fixed-bank edit at ${a:04X}"
            );
        }
        let _ = &vanilla;
    }
}

#[test]
#[ignore = "needs Z2_ROM"]
fn ohko_and_extremes_run() {
    let body = body();
    for (atk, life, magic, hp) in [
        (
            AttackEffectiveness::Ohko,
            LifeEffectiveness::Ohko,
            MagicEffectiveness::Free,
            EnemyLife::Wide,
        ),
        (
            AttackEffectiveness::Low,
            LifeEffectiveness::Invincible,
            MagicEffectiveness::HighCost,
            EnemyLife::High,
        ),
        (
            AttackEffectiveness::High,
            LifeEffectiveness::High,
            MagicEffectiveness::LowCost,
            EnemyLife::Narrow,
        ),
    ] {
        let mut f = all_on();
        f.stats.attack_effectiveness = atk;
        f.stats.life_effectiveness = life;
        f.stats.magic_effectiveness = magic;
        f.enemies.enemy_hp = hp;
        f.stats.attack_level_cap = 1;
        f.stats.life_level_cap = 3;
        f.enemies.mix_large_and_small = Tri::Random;
        f.enemies.xp_drops = XpEffectiveness::None;
        for seed in SEEDS {
            let a = randomize(&body, seed, &f).unwrap();
            let b = randomize(&body, seed, &f).unwrap();
            assert_eq!(a, b, "deterministic");
            let rom = rom_of(&a.body);
            if atk == AttackEffectiveness::Ohko {
                assert_eq!(data::boss_hp(&rom, Boss::Rebonack).unwrap(), 227);
            }
        }
    }
}
