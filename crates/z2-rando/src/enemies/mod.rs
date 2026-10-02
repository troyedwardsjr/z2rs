//! `enemies` module: enemy shuffles and enemy stat tables.
//!
//! Options owned: [`crate::flags::EnemyFlags`] (`ctx.flags.enemies`).
//! Also reads [`crate::flags::PalaceFlags::aggressive_thunderbird`] (a pure
//! data change to the Thunderbird fight).
//!
//! Catalog: section 05 (sections 1-3, 4.5-4.7, 6.1, 6.3, 6.8).
//!
//! What it does, in order (each step only when its option is on):
//!
//! 1. scales regular enemy hit points (five tables, edited in place);
//! 2. scales boss hit points;
//! 3. reassigns sword immunity and experience stealing, shifts experience
//!    drops (attribute tables, edited in place), and rescales the two
//!    stolen-experience amounts (operands the ported hit routine reads from
//!    the ROM, see `TRAP_HONORED_BYTES`);
//! 4. picks the dripper's enemy (and optionally gives it full hit points);
//! 5. makes Thunderbird aggressive from the start;
//! 6. shuffles overworld and palace enemy placements ([`shuffle`]).
//!
//! Randomized knockback (`randomize_knockback`) is 6502 code and lives in
//! `asm_features`.
//!
//! Last, it finishes the boss values that depend on both the sword damage
//! (written earlier by `stats`) and the hit points: one-hit-kill hit points,
//! the Rebonack horse fix, the boss hit point bar divisors and the Big
//! Bubble split threshold.

pub mod data;
pub mod shuffle;

use crate::flags::{DripperEnemy, EnemyLife, SwordImmunity, XpEffectiveness};
use crate::rng::Rng;
use crate::rom::Rom;
use crate::spells::FireMode;
use crate::{Ctx, RandoError};
use data::{Boss, Group};

/// Bank 4 `LDA #` operand: the enemy a dripper's drop turns into.
pub const DRIPPER_ID: (u8, u16) = (4, 0x9917);
/// Bank 4 `LDY $10 : ASL $C2,X` right before the dripper sets the ID (the
/// full hit point hook replaces it).
pub const DRIPPER_HP_SITE: (u8, u16) = (4, 0x9912);
/// Fixed-bank `LDA #` operands of the two experience-drain amounts (small,
/// and the one for the big-drain enemy).
pub const STEAL_SMALL: u16 = 0xE2FE;
/// See [`STEAL_SMALL`].
pub const STEAL_BIG: u16 = 0xE304;

/// Experience ladder indexed by the attribute low nibble.
pub const EXP_LADDER: [u16; 16] = [
    0, 2, 3, 5, 10, 20, 30, 50, 70, 100, 150, 200, 300, 500, 700, 1000,
];

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let f = ctx.flags.enemies.clone();
    scale_enemy_hp(ctx, f.enemy_hp)?;
    scale_boss_hp(ctx, f.boss_hp)?;
    // Fire cannot hurt anything when it became Dash or is tied to Fairy.
    let fire_unusable = matches!(ctx.state.spells.fire, FireMode::Dash | FireMode::Linked(3));
    let immunity = match f.sword_immunity {
        SwordImmunity::ShuffleConditional if fire_unusable => SwordImmunity::None,
        SwordImmunity::ShuffleConditional => SwordImmunity::Shuffle,
        other => other,
    };
    for g in Group::ALL {
        randomize_attributes(
            &mut ctx.rom,
            &mut ctx.rng,
            g,
            immunity,
            f.shuffle_xp_stealers,
            f.xp_drops,
        )?;
    }
    if f.xp_drops != XpEffectiveness::Vanilla {
        shift_boss_exp(&mut ctx.rom, &mut ctx.rng, f.xp_drops)?;
    }
    if f.sword_immunity != SwordImmunity::Vanilla
        || f.shuffle_xp_stealers
        || f.xp_drops != XpEffectiveness::Vanilla
    {
        spoil_attributes(ctx)?;
    }
    if f.shuffle_xp_stolen_amount {
        rescale_stolen_exp(ctx)?;
    }
    dripper(ctx, f.dripper_enemy)?;
    if ctx.flags.palaces.aggressive_thunderbird {
        aggressive_thunderbird(&mut ctx.rom)?;
        ctx.spoiler
            .line("Enemies", "Thunderbird starts in its aggressive phase");
    }
    shuffle::apply(ctx)?;
    finish_bosses(ctx)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Values that depend on both the sword damage (`stats`, earlier) and the
// hit points (this module).
// ---------------------------------------------------------------------------

/// One-hit-kill hit points, Rebonack horse fix, boss bar divisors and the
/// Big Bubble split threshold. Only acts on values that differ from vanilla,
/// so it is a no-op when neither the damage nor the hit points changed.
fn finish_bosses(ctx: &mut Ctx) -> Result<(), RandoError> {
    let off = ctx.rom.cpu_offset(7, crate::stats::ATTACK_TABLE)?;
    let attack: Vec<u8> = ctx.rom.read_slice(off, 8)?.to_vec();
    let vanilla_attack: Vec<u8> = ctx.vanilla.read_slice(off, 8)?.to_vec();
    if attack.iter().all(|&v| v == crate::stats::OHKO_DAMAGE) {
        ohko_hit_points(&mut ctx.rom)?;
    }
    let rebo = data::boss_hp(&ctx.rom, Boss::Rebonack)?;
    if attack != vanilla_attack || rebo != data::boss_hp(&ctx.vanilla, Boss::Rebonack)? {
        // Damage that divides Rebonack's hit points exactly kills the horse
        // during the de-horsing and drops an extra key (softlock risk).
        let mut hp = rebo;
        while hp < 255
            && attack
                .iter()
                .any(|&v| u16::from(v) * 2 == u16::from(hp) || v == hp)
        {
            hp += 1;
        }
        if hp != rebo {
            data::set_boss_hp(&mut ctx.rom, Boss::Rebonack, hp)?;
            ctx.log(format!("enemies: Rebonack HP {rebo} -> {hp} (horse fix)"));
        }
    }
    data::update_boss_bar_divisors(&mut ctx.rom, &ctx.vanilla)?;
    data::update_big_bubble_threshold(&mut ctx.rom, &ctx.vanilla)?;
    Ok(())
}

/// One-hit-kill companions: objects that must survive a 192 hit get at
/// least 193, bubbles that must still die get 192, Rebonack gets 227.
fn ohko_hit_points(rom: &mut Rom) -> Result<(), RandoError> {
    let survive = [
        (Group::West, 0x01),
        (Group::East, 0x01),
        (Group::Palace125, 0x05),
        (Group::Palace346, 0x05),
        (Group::GreatPalace, 0x05),
        (Group::GreatPalace, 0x22),
    ];
    for (g, id) in survive {
        let v = data::hp(rom, g, id)?.max(193);
        data::write_hp(rom, g, id, v)?;
    }
    let die = [
        (Group::Palace125, 0x06),
        (Group::Palace125, 0x0E),
        (Group::Palace346, 0x06),
        (Group::Palace346, 0x0E),
        (Group::GreatPalace, 0x14),
        (Group::GreatPalace, 0x15),
        (Group::GreatPalace, 0x17),
    ];
    for (g, id) in die {
        data::write_hp(rom, g, id, 192)?;
    }
    data::write_hp(rom, Group::Palace346, data::REBONACK, 227)
}

/// `(numerator, denominator)` pairs for the low and high multiplier of an
/// [`EnemyLife`] option (`None` for vanilla).
#[must_use]
pub fn life_range(o: EnemyLife) -> Option<((u32, u32), (u32, u32))> {
    match o {
        EnemyLife::Vanilla => None,
        EnemyLife::Narrow => Some(((3, 4), (5, 4))),
        EnemyLife::Medium => Some(((1, 2), (3, 2))),
        EnemyLife::Wide => Some(((1, 4), (3, 1))),
        EnemyLife::MediumHigh => Some(((1, 2), (2, 1))),
        EnemyLife::High => Some(((1, 1), (2, 1))),
    }
}

/// `v` scaled by a draw from `[v*lo, v*hi)`, capped at 255. Zero stays zero.
fn scaled_hp(rng: &mut Rng, v: u8, range: ((u32, u32), (u32, u32))) -> u8 {
    let ((ln, ld), (hn, hd)) = range;
    let lo = u32::from(v) * ln / ld;
    let hi = u32::from(v) * hn / hd;
    let n = if hi <= lo {
        lo
    } else {
        rng.range(i64::from(lo), i64::from(hi) - 1) as u32
    };
    n.min(255) as u8
}

fn scale_enemy_hp(ctx: &mut Ctx, o: EnemyLife) -> Result<(), RandoError> {
    let Some(range) = life_range(o) else {
        return Ok(());
    };
    for g in Group::ALL {
        for id in g.hp_ids() {
            let v = data::hp(&ctx.rom, g, id)?;
            let n = scaled_hp(&mut ctx.rng, v, range);
            data::write_hp(&mut ctx.rom, g, id, n)?;
        }
    }
    ctx.spoiler
        .line("Enemies", format!("Enemy hit points: {}", o.label()));
    Ok(())
}

fn scale_boss_hp(ctx: &mut Ctx, o: EnemyLife) -> Result<(), RandoError> {
    let Some(range) = life_range(o) else {
        return Ok(());
    };
    for b in Boss::ALL {
        let v = data::boss_hp(&ctx.rom, b)?;
        let n = scaled_hp(&mut ctx.rng, v, range).max(1);
        data::set_boss_hp(&mut ctx.rom, b, n)?;
    }
    for b in Boss::ALL {
        let n = data::boss_hp(&ctx.rom, b)?;
        ctx.spoiler
            .line("Enemies", format!("{} hit points: {n}", b.name()));
    }
    Ok(())
}

/// Experience nibble shift range (inclusive) of an [`XpEffectiveness`].
#[must_use]
pub fn xp_shift(o: XpEffectiveness) -> Option<(i64, i64)> {
    match o {
        XpEffectiveness::Vanilla => None,
        XpEffectiveness::RandomLow => Some((-3, 1)),
        XpEffectiveness::Random => Some((-2, 2)),
        XpEffectiveness::LowVariance => Some((-1, 1)),
        XpEffectiveness::SlightlyHigh => Some((0, 1)),
        XpEffectiveness::RandomHigh => Some((-1, 3)),
        XpEffectiveness::Wide => Some((-4, 4)),
        XpEffectiveness::None => Some((-15, -15)),
    }
}

fn shift_nibble(rng: &mut Rng, byte: u8, (lo, hi): (i64, i64)) -> u8 {
    let n = i64::from(byte & 0x0F) + rng.range(lo, hi);
    (byte & 0xF0) | n.clamp(0, 15) as u8
}

/// Move bit `mask` of row 1 among the listed IDs, keeping how many have it.
fn permute_bit(
    rom: &mut Rom,
    rng: &mut Rng,
    g: Group,
    ids: &[u8],
    mask: u8,
) -> Result<Vec<u8>, RandoError> {
    let mut bits = Vec::with_capacity(ids.len());
    for &id in ids {
        bits.push(data::attr1(rom, g, id)? & mask != 0);
    }
    rng.shuffle(&mut bits);
    let mut gained = Vec::new();
    for (&id, &on) in ids.iter().zip(&bits) {
        let old = data::attr1(rom, g, id)?;
        let new = if on { old | mask } else { old & !mask };
        if new & mask != 0 && old & mask == 0 {
            gained.push(id);
        }
        data::set_attr1(rom, g, id, new)?;
    }
    Ok(gained)
}

/// Sword immunity, experience stealing and experience drops for one group.
pub fn randomize_attributes(
    rom: &mut Rom,
    rng: &mut Rng,
    g: Group,
    immunity: SwordImmunity,
    stealers: bool,
    xp: XpEffectiveness,
) -> Result<(), RandoError> {
    const SWORD_IMMUNE: u8 = 0x20;
    const STEALS: u8 = 0x10;
    const FIRE_IMMUNE: u8 = 0x20;
    let ids = g.sets().all();
    match immunity {
        SwordImmunity::Vanilla | SwordImmunity::ShuffleConditional => {}
        SwordImmunity::Shuffle => {
            // An enemy that newly resists the sword must still take Fire.
            for id in permute_bit(rom, rng, g, &ids, SWORD_IMMUNE)? {
                let a2 = data::attr2(rom, g, id)?;
                data::set_attr2(rom, g, id, a2 & !FIRE_IMMUNE)?;
            }
        }
        SwordImmunity::None => {
            for &id in &ids {
                let a = data::attr1(rom, g, id)?;
                data::set_attr1(rom, g, id, a & !SWORD_IMMUNE)?;
            }
        }
    }
    if stealers {
        permute_bit(rom, rng, g, &ids, STEALS)?;
    }
    if let Some(range) = xp_shift(xp) {
        for &id in &ids {
            let a = data::attr1(rom, g, id)?;
            data::set_attr1(rom, g, id, shift_nibble(rng, a, range))?;
        }
    }
    Ok(())
}

fn shift_boss_exp(rom: &mut Rom, rng: &mut Rng, xp: XpEffectiveness) -> Result<(), RandoError> {
    let Some(range) = xp_shift(xp) else {
        return Ok(());
    };
    let mut sites: Vec<(u8, u16)> = Boss::ALL.iter().map(|b| (4, b.exp_addr())).collect();
    sites.push(data::THUNDERBIRD_EXP);
    for (bank, addr) in sites {
        let v = rom.read_cpu(bank, addr)?;
        rom.write_cpu(bank, addr, &[shift_nibble(rng, v, range)])?;
    }
    Ok(())
}

fn spoil_attributes(ctx: &mut Ctx) -> Result<(), RandoError> {
    for g in Group::ALL {
        let mut immune = Vec::new();
        let mut steal = Vec::new();
        for id in g.sets().all() {
            let a = data::attr1(&ctx.rom, g, id)?;
            if a & 0x20 != 0 {
                immune.push(format!("{id:02X}"));
            }
            if a & 0x10 != 0 {
                steal.push(format!("{id:02X}"));
            }
        }
        ctx.spoiler.line(
            "Enemies",
            format!(
                "{}: sword immune [{}], steal experience [{}]",
                g.name(),
                immune.join(" "),
                steal.join(" ")
            ),
        );
    }
    Ok(())
}

/// Stolen experience becomes 50-150% of vanilla (both amounts).
fn rescale_stolen_exp(ctx: &mut Ctx) -> Result<(), RandoError> {
    let mut out = Vec::new();
    for addr in [STEAL_SMALL, STEAL_BIG] {
        // Only touch the operand when it still is `LDA #imm`.
        if ctx.rom.read_cpu(7, addr - 1)? != 0xA9 {
            continue;
        }
        let v = i64::from(ctx.rom.read_cpu(7, addr)?);
        let n = ctx.rng.range(v / 2, v * 3 / 2).clamp(1, 255) as u8;
        ctx.rom.write_cpu(7, addr, &[n])?;
        out.push(n.to_string());
    }
    ctx.spoiler.line(
        "Enemies",
        format!("Experience stolen (small, big): {}", out.join(", ")),
    );
    Ok(())
}

/// Candidates for the dripper's enemy (palace 1/2/5 IDs).
#[must_use]
pub fn dripper_candidates(o: DripperEnemy) -> Vec<u8> {
    match o {
        DripperEnemy::OnlyBots => Vec::new(),
        DripperEnemy::AnyGroundEnemy => data::PALACE125.ground(),
        DripperEnemy::EasierGroundEnemies | DripperEnemy::EasierGroundEnemiesFullHp => {
            let mut v = data::PALACE125.small.to_vec();
            v.extend_from_slice(&[0x0C, 0x18, 0x1F, 0x23]);
            v
        }
    }
}

fn dripper(ctx: &mut Ctx, o: DripperEnemy) -> Result<(), RandoError> {
    let cands = dripper_candidates(o);
    let Some(&id) = ctx.rng.pick(&cands) else {
        return Ok(());
    };
    if ctx.rom.read_cpu(DRIPPER_ID.0, DRIPPER_ID.1 - 1)? != 0xA9 {
        ctx.log("enemies: dripper spawn code not found, dripper left alone");
        return Ok(());
    }
    ctx.rom.write_cpu(DRIPPER_ID.0, DRIPPER_ID.1, &[id])?;
    if o == DripperEnemy::EasierGroundEnemiesFullHp {
        let hp = data::hp(&ctx.rom, Group::Palace125, id)?;
        dripper_full_hp(&mut ctx.rom, hp)?;
    }
    ctx.spoiler
        .line("Enemies", format!("Drippers spawn enemy {id:02X}"));
    Ok(())
}

/// Give the dripper's spawn `hp` hit points: `LDY $10 : ASL $C2,X` becomes
/// `JSR stub`, and the stub (fixed-bank padding) stores the hit points and
/// does the `LDY $10`.
fn dripper_full_hp(rom: &mut Rom, hp: u8) -> Result<(), RandoError> {
    let site = rom.read_slice(rom.cpu_offset(DRIPPER_HP_SITE.0, DRIPPER_HP_SITE.1)?, 4)?;
    if site != [0xA4, 0x10, 0x16, 0xC2] {
        // Not the vanilla code (synthetic test images): leave it alone.
        return Ok(());
    }
    let stub = rom.alloc_vanilla(7, 7)?;
    let src = format!(
        "
        .org ${site:04X}
            JSR stub
            NOP
        .org ${stub:04X}
        stub:
            LDA #{hp}
            STA $C2,X
            LDY $10
            RTS
        ",
        site = DRIPPER_HP_SITE.1,
    );
    let out = crate::asm::assemble(&src)?;
    rom.apply_asm(DRIPPER_HP_SITE.0, &out)
}

/// Thunderbird starts (and stays) in its aggressive phase: the hard-mode
/// threshold is raised to its full hit points.
pub fn aggressive_thunderbird(rom: &mut Rom) -> Result<(), RandoError> {
    const FULL: u8 = 192;
    for (bank, addr) in [(5, 0x9443), (5, 0x9EC6), (5, 0xA3CF), (5, 0xA403)] {
        rom.write_cpu(bank, addr, &[FULL])?;
    }
    rom.write_cpu(5, 0xA3F6, &[24])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::VANILLA_BODY_LEN;

    fn rom() -> Rom {
        let mut r = Rom::from_body(&vec![0u8; VANILLA_BODY_LEN]).unwrap();
        for g in Group::ALL {
            for id in 0..0x24u8 {
                data::write_hp(&mut r, g, id, 0x10 + id).unwrap();
                // A few immune/stealing enemies, exp index 5.
                let a1 =
                    0x05 | if id % 5 == 0 { 0x20 } else { 0 } | if id % 7 == 0 { 0x10 } else { 0 };
                data::set_attr1(&mut r, g, id, a1).unwrap();
                data::set_attr2(&mut r, g, id, 0x23).unwrap();
            }
        }
        r
    }

    #[test]
    fn hp_ranges() {
        let mut rng = Rng::new(4);
        for v in [0u8, 1, 3, 16, 100, 200, 255] {
            for o in EnemyLife::ALL.iter().copied() {
                let Some(r) = life_range(o) else { continue };
                let n = scaled_hp(&mut rng, v, r);
                let lo = u32::from(v) * r.0 .0 / r.0 .1;
                assert!(u32::from(n) >= lo.min(255), "{o:?} {v} {n}");
                if v == 0 {
                    assert_eq!(n, 0);
                }
            }
        }
    }

    #[test]
    fn permuting_keeps_counts_and_new_immunes_take_fire() {
        for seed in 0..30 {
            let mut r = rom();
            let before = r.clone();
            let mut rng = Rng::new(seed);
            for g in Group::ALL {
                randomize_attributes(
                    &mut r,
                    &mut rng,
                    g,
                    SwordImmunity::Shuffle,
                    true,
                    XpEffectiveness::Vanilla,
                )
                .unwrap();
                let ids = g.sets().all();
                let count = |rom: &Rom, m: u8| {
                    ids.iter()
                        .filter(|&&id| data::attr1(rom, g, id).unwrap() & m != 0)
                        .count()
                };
                assert_eq!(count(&r, 0x20), count(&before, 0x20));
                assert_eq!(count(&r, 0x10), count(&before, 0x10));
                for &id in &ids {
                    let was = data::attr1(&before, g, id).unwrap() & 0x20 != 0;
                    let is = data::attr1(&r, g, id).unwrap() & 0x20 != 0;
                    if is && !was {
                        assert_eq!(data::attr2(&r, g, id).unwrap() & 0x20, 0);
                    }
                    // Low nibble untouched.
                    assert_eq!(data::attr1(&r, g, id).unwrap() & 0x0F, 5);
                }
            }
        }
    }

    #[test]
    fn xp_none_zeroes_and_wide_stays_in_ladder() {
        let mut r = rom();
        let mut rng = Rng::new(1);
        for g in Group::ALL {
            randomize_attributes(
                &mut r,
                &mut rng,
                g,
                SwordImmunity::Vanilla,
                false,
                XpEffectiveness::None,
            )
            .unwrap();
            for id in g.sets().all() {
                assert_eq!(data::attr1(&r, g, id).unwrap() & 0x0F, 0);
            }
        }
        let mut r = rom();
        for g in Group::ALL {
            randomize_attributes(
                &mut r,
                &mut rng,
                g,
                SwordImmunity::Vanilla,
                false,
                XpEffectiveness::Wide,
            )
            .unwrap();
            for id in g.sets().all() {
                let n = data::attr1(&r, g, id).unwrap() & 0x0F;
                assert!((1..=9).contains(&n), "{n}");
            }
        }
    }

    #[test]
    fn sword_immunity_none_clears_listed_only() {
        let mut r = rom();
        let mut rng = Rng::new(1);
        randomize_attributes(
            &mut r,
            &mut rng,
            Group::West,
            SwordImmunity::None,
            false,
            XpEffectiveness::Vanilla,
        )
        .unwrap();
        for id in Group::West.sets().all() {
            assert_eq!(data::attr1(&r, Group::West, id).unwrap() & 0x20, 0);
        }
        // Unlisted fairy (00) keeps its bit.
        assert_eq!(data::attr1(&r, Group::West, 0).unwrap() & 0x20, 0x20);
    }

    #[test]
    fn dripper_lists() {
        assert!(dripper_candidates(DripperEnemy::OnlyBots).is_empty());
        assert_eq!(dripper_candidates(DripperEnemy::AnyGroundEnemy).len(), 12);
        assert_eq!(
            dripper_candidates(DripperEnemy::EasierGroundEnemies).len(),
            8
        );
    }

    #[test]
    fn dripper_full_hp_hook() {
        let mut r = rom();
        r.write_cpu(4, 0x9912, &[0xA4, 0x10, 0x16, 0xC2, 0xA9, 0x04])
            .unwrap();
        dripper_full_hp(&mut r, 0x30).unwrap();
        assert_eq!(r.read_cpu(4, 0x9912).unwrap(), 0x20);
        let stub = r.read_cpu_word(4, 0x9913).unwrap();
        assert!(stub >= 0xC000);
        assert_eq!(r.read_cpu(4, 0x9915).unwrap(), 0xEA);
        let off = r.cpu_offset(7, stub).unwrap();
        assert_eq!(
            r.read_slice(off, 7).unwrap(),
            &[0xA9, 0x30, 0x95, 0xC2, 0xA4, 0x10, 0x60]
        );
    }
}
