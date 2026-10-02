//! `stats` module: experience tables, level caps and attack/magic/life
//! effectiveness.
//!
//! Options owned: [`crate::flags::StatsFlags`] (`ctx.flags.stats`).
//!
//! Catalog: section 05 (stat tables, sections 4.1-4.4 and 6.4).
//!
//! Every table is edited **in place** at its vanilla address, because the
//! ported routines in `z2-core` read them there through the bus:
//!
//! | Table | Where | Reader |
//! |---|---|---|
//! | Experience to level (24 high bytes, then 24 low bytes) | bank 0 `$9659` | bank 0 level-up code (ROM) |
//! | Level-up menu digits (tens, hundreds, thousands rows) | bank 0 `$9E32` | bank 0 menu (ROM) |
//! | Sword damage per attack level | fixed `$E66D` | `player_traps` sword hit (`LE66C,y` via the bus) |
//! | Damage to Link (7 classes x 8 life levels, doubled) | fixed `$E2AF` | `player_traps::pl_link_hit` (`LE2AE,y` via the bus) |
//! | Spell cost (8 spells x 8 magic levels) | bank 0 `$8D7B` | bank 0 spell code (ROM) |
//!
//! Level caps below 8 need one small hook in bank 0 (the level-up code
//! compares a stat's level with a per-stat cap instead of 8), placed in
//! vanilla bank-0 padding.
//!
//! It runs after `spells` (the cost rows are then in menu order) and before
//! `items`, so the logic sees the new spell costs: a spell the logic
//! expects to be cast with `n` magic containers needs `n` scaled by how much
//! its cost at that magic level changed. The boss values that also depend
//! on the sword damage are finished by `enemies`, which runs later.
//!
//! With default options it does nothing.

use crate::flags::{AttackEffectiveness, LifeEffectiveness, MagicEffectiveness, StatsFlags};
use crate::rng::Rng;
use crate::rom::Rom;
use crate::{Ctx, RandoError};

/// Experience-to-level table: bank 0, 24 high bytes then 24 low bytes, in
/// the order attack L2-L9, magic L2-L9, life L2-L9.
pub const EXP_TABLE: (u8, u16) = (0, 0x9659);
/// Level-up menu digit tiles: three rows of 24 (tens, hundreds, thousands).
pub const EXP_DIGITS: (u8, u16) = (0, 0x9E32);
/// Sword damage per attack level (8 bytes, fixed bank).
pub const ATTACK_TABLE: u16 = 0xE66D;
/// Damage to Link: 7 damage classes x 8 life levels, stored doubled.
pub const LIFE_TABLE: u16 = 0xE2AF;
/// Spell costs: 8 spells x 8 magic levels (vanilla spell order).
pub const MAGIC_TABLE: (u8, u16) = (0, 0x8D7B);
/// Bank 0 level-up check `LDA $0777,X : CMP #$08` (5 bytes).
pub const LEVEL_CAP_SITE: (u8, u16) = (0, 0x9F7A);

/// Highest experience value the game can show (the menu has 4 digits and the
/// ones digit is always 0).
const EXP_MAX: u16 = 9990;
/// Tile for digit 0 in the level-up menu.
const DIGIT_TILE_0: u8 = 0xD0;
/// Blank tile for leading zeros in the level-up menu.
const BLANK_TILE: u8 = 0xF4;
/// Value one-hit-kill attack writes to every attack level.
pub const OHKO_DAMAGE: u8 = 192;

const STAT_NAMES: [&str; 3] = ["Attack", "Magic", "Life"];
const DAMAGE_CLASS_NAMES: [&str; 7] = ["A", "B", "C", "D", "E", "F", "G"];

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let f = ctx.flags.stats.clone();
    let exp_changed = randomize_experience(&mut ctx.rom, &mut ctx.rng, &f)?;
    if exp_changed {
        if f.attack_level_cap < 8 || f.magic_level_cap < 8 || f.life_level_cap < 8 {
            patch_level_caps(
                &mut ctx.rom,
                [f.attack_level_cap, f.magic_level_cap, f.life_level_cap],
            )?;
        }
        spoil_experience(ctx)?;
    }
    if randomize_attack(&mut ctx.rom, &mut ctx.rng, f.attack_effectiveness)? {
        let row = ctx.rom.read_slice(attack_off(&ctx.rom)?, 8)?.to_vec();
        ctx.spoiler
            .line("Stats", format!("Attack damage: {}", join(&row)));
    }
    if randomize_life(&mut ctx.rom, &mut ctx.rng, f.life_effectiveness)? {
        spoil_grid(
            ctx,
            "Damage taken (per life level)",
            life_off(&ctx.rom)?,
            &DAMAGE_CLASS_NAMES,
        )?;
    }
    let off = ctx.rom.cpu_offset(MAGIC_TABLE.0, MAGIC_TABLE.1)?;
    let before = ctx.rom.read_slice(off, 64)?.to_vec();
    if randomize_magic(&mut ctx.rom, &mut ctx.rng, f.magic_effectiveness)? {
        let after = ctx.rom.read_slice(off, 64)?.to_vec();
        if ctx.state.world.built {
            let old = ctx.state.world.spell_containers;
            let new = spell_containers(&old, &ctx.state.spells.menu, &before, &after);
            ctx.state.world.set_spell_containers(new);
        }
        // Rows are in menu order (after `spells`); label each row with the
        // spell now in that slot ("Dash" when Fire became Dash).
        let names: Vec<&str> = (0..8).map(|slot| ctx.state.spells.name(slot)).collect();
        spoil_grid(ctx, "Spell costs (per magic level)", off, &names)?;
    }
    Ok(())
}

fn join(v: &[u8]) -> String {
    v.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")
}

fn attack_off(rom: &Rom) -> Result<usize, RandoError> {
    rom.cpu_offset(7, ATTACK_TABLE)
}

fn life_off(rom: &Rom) -> Result<usize, RandoError> {
    rom.cpu_offset(7, LIFE_TABLE)
}

// ---------------------------------------------------------------------------
// Experience to level.
// ---------------------------------------------------------------------------

/// The three 8-entry experience rows (attack, magic, life) as stored.
pub fn read_experience(rom: &Rom) -> Result<[[u16; 8]; 3], RandoError> {
    let off = rom.cpu_offset(EXP_TABLE.0, EXP_TABLE.1)?;
    let raw = rom.read_slice(off, 48)?;
    let mut out = [[0u16; 8]; 3];
    for (s, row) in out.iter_mut().enumerate() {
        for (i, v) in row.iter_mut().enumerate() {
            let k = s * 8 + i;
            *v = u16::from_be_bytes([raw[k], raw[24 + k]]);
        }
    }
    Ok(out)
}

/// Write the experience rows and the matching menu digits.
pub fn write_experience(rom: &mut Rom, rows: &[[u16; 8]; 3]) -> Result<(), RandoError> {
    let mut raw = [0u8; 48];
    let mut digits = [0u8; 72];
    for (s, row) in rows.iter().enumerate() {
        for (i, &v) in row.iter().enumerate() {
            let k = s * 8 + i;
            let [hi, lo] = v.to_be_bytes();
            raw[k] = hi;
            raw[24 + k] = lo;
            let [tens, hundreds, thousands] = digit_tiles(v);
            digits[k] = tens;
            digits[24 + k] = hundreds;
            digits[48 + k] = thousands;
        }
    }
    rom.write_cpu(EXP_TABLE.0, EXP_TABLE.1, &raw)?;
    rom.write_cpu(EXP_DIGITS.0, EXP_DIGITS.1, &digits)
}

/// Menu tiles for the tens, hundreds and thousands digits of `v` (the ones
/// digit is always drawn as 0). Leading zeros are blank.
#[must_use]
pub fn digit_tiles(v: u16) -> [u8; 3] {
    let d = |n: u16| DIGIT_TILE_0 + (n % 10) as u8;
    let tens = d(v / 10);
    let hundreds = if v < 100 { BLANK_TILE } else { d(v / 100) };
    let thousands = if v < 1000 { BLANK_TILE } else { d(v / 1000) };
    [tens, hundreds, thousands]
}

/// Uniform integer in `[lo, hi)`; `lo` when the range is empty.
fn half_open(rng: &mut Rng, lo: i64, hi: i64) -> i64 {
    if hi <= lo {
        lo
    } else {
        rng.range(lo, hi - 1)
    }
}

/// Resample a vanilla row for a level cap below 8: the cap's `cap - 1`
/// regular thresholds are spread over the same overall curve (piecewise
/// linear through the vanilla thresholds, so the last regular level costs
/// what level 8 costs in vanilla). Entries from `cap - 1` on are filled with
/// [`EXP_MAX`] (the 1-up value overwrites them later).
#[must_use]
pub fn scale_row(vanilla: &[u16; 8], cap: u8) -> [u16; 8] {
    let regular = usize::from(cap.clamp(1, 8)) - 1;
    let mut out = [EXP_MAX; 8];
    if regular == 0 {
        return out;
    }
    // Curve points: x = 0 -> 0, x = k (1..=7) -> vanilla[k - 1].
    let at = |x: f64| -> f64 {
        let k = x.floor() as usize;
        if k >= 7 {
            return f64::from(vanilla[6]);
        }
        let y0 = if k == 0 {
            0.0
        } else {
            f64::from(vanilla[k - 1])
        };
        let y1 = f64::from(vanilla[k]);
        y0 + (y1 - y0) * (x - k as f64)
    };
    for (i, slot) in out.iter_mut().enumerate().take(regular) {
        let x = 7.0 * (i + 1) as f64 / regular as f64;
        *slot = (at(x).round() as u16).clamp(10, EXP_MAX);
    }
    out
}

/// Randomize the experience table. Returns whether anything was written.
pub fn randomize_experience(
    rom: &mut Rom,
    rng: &mut Rng,
    f: &StatsFlags,
) -> Result<bool, RandoError> {
    let shuffle = [
        f.shuffle_attack_exp,
        f.shuffle_magic_exp,
        f.shuffle_life_exp,
    ];
    let caps = [f.attack_level_cap, f.magic_level_cap, f.life_level_cap].map(|c| c.clamp(1, 8));
    if !shuffle.iter().any(|&s| s) && caps.iter().all(|&c| c == 8) {
        return Ok(false);
    }
    let vanilla = read_experience(rom)?;
    let mut rows = vanilla;
    for s in 0..3 {
        if f.scale_level_requirements_to_cap && caps[s] < 8 {
            rows[s] = scale_row(&vanilla[s], caps[s]);
        }
        if shuffle[s] {
            let mut prev = 10i64;
            for v in rows[s].iter_mut() {
                let base = i64::from(*v);
                let lo = prev.max(base * 3 / 4);
                let hi = (base * 5 / 4).min(i64::from(EXP_MAX));
                let n = half_open(rng, lo, hi);
                *v = n as u16;
                prev = n;
            }
        }
        for v in rows[s].iter_mut() {
            *v = (*v / 10) * 10;
        }
    }
    // One shared 1-up threshold above every regular threshold.
    let highest = (0..3)
        .filter(|&s| caps[s] >= 2)
        .map(|s| rows[s][usize::from(caps[s]) - 2])
        .max()
        .unwrap_or(0);
    let lo = (i64::from(highest) + 10).min(i64::from(EXP_MAX));
    let one_up = ((half_open(rng, lo, 9999) / 10) * 10) as u16;
    for s in 0..3 {
        for v in rows[s].iter_mut().skip(usize::from(caps[s]) - 1) {
            *v = one_up;
        }
    }
    write_experience(rom, &rows)?;
    Ok(true)
}

/// Make the bank 0 level-up check compare each stat's level with its own
/// cap: `LDA $0777,X : CMP #$08` becomes `JSR stub : NOP : NOP`, and the
/// stub (in bank 0 padding) does `LDA $0777,X : CMP caps,X : RTS`.
pub fn patch_level_caps(rom: &mut Rom, caps: [u8; 3]) -> Result<(), RandoError> {
    let site = rom.read_slice(rom.cpu_offset(LEVEL_CAP_SITE.0, LEVEL_CAP_SITE.1)?, 5)?;
    if site != [0xBD, 0x77, 0x07, 0xC9, 0x08] {
        // Not the vanilla code (synthetic test images): leave it alone.
        return Ok(());
    }
    let stub = rom.alloc_vanilla(0, 10)?;
    let src = format!(
        "
        .org ${site:04X}
            JSR stub
            NOP
            NOP
        .org ${stub:04X}
        stub:
            LDA $0777,X
            CMP caps,X
            RTS
        caps:
            .byte {a}, {m}, {l}
        ",
        site = LEVEL_CAP_SITE.1,
        a = caps[0].clamp(1, 8),
        m = caps[1].clamp(1, 8),
        l = caps[2].clamp(1, 8),
    );
    let out = crate::asm::assemble(&src)?;
    rom.apply_asm(0, &out)
}

fn spoil_experience(ctx: &mut Ctx) -> Result<(), RandoError> {
    let rows = read_experience(&ctx.rom)?;
    for (s, row) in rows.iter().enumerate() {
        let txt = row.iter().map(u16::to_string).collect::<Vec<_>>().join(" ");
        ctx.spoiler.line(
            "Stats",
            format!("{} experience (L2-L8, 1-up): {txt}", STAT_NAMES[s]),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Attack, life and magic effectiveness.
// ---------------------------------------------------------------------------

/// `floor(v * num / den)` in integers.
fn scale(v: u8, num: u32, den: u32) -> i64 {
    i64::from(u32::from(v) * num / den)
}

/// Randomize the sword damage table. Returns whether it changed.
pub fn randomize_attack(
    rom: &mut Rom,
    rng: &mut Rng,
    mode: AttackEffectiveness,
) -> Result<bool, RandoError> {
    let off = attack_off(rom)?;
    let vanilla: [u8; 8] = rom.read_slice(off, 8)?.try_into().expect("8 bytes");
    // (numerator/denominator of the low and high multipliers, minimum level
    // index and minimum value).
    let ranged = |lo: (u32, u32), hi: (u32, u32)| (lo, hi);
    let new: [u8; 8] = match mode {
        AttackEffectiveness::Vanilla => return Ok(false),
        AttackEffectiveness::Low => [1, 2, 3, 4, 5, 6, 9, 12],
        AttackEffectiveness::High => [3, 4, 6, 9, 13, 18, 27, 36],
        AttackEffectiveness::Ohko => {
            // The matching hit points are set by `enemies`, after it
            // scales them.
            rom.write(off, &[OHKO_DAMAGE; 8])?;
            return Ok(true);
        }
        AttackEffectiveness::AverageLow
        | AttackEffectiveness::Average
        | AttackEffectiveness::AverageHigh => {
            let ((ln, ld), (hn, hd)) = match mode {
                AttackEffectiveness::AverageLow => ranged((1, 2), (1, 1)),
                AttackEffectiveness::Average => ranged((2, 3), (3, 2)),
                _ => ranged((1, 1), (3, 2)),
            };
            let mut out = [0u8; 8];
            let mut prev = 0i64;
            for (i, &v) in vanilla.iter().enumerate() {
                let mut n = half_open(rng, scale(v, ln, ld), scale(v, hn, hd));
                let floor = match (mode, i) {
                    (AttackEffectiveness::AverageLow, 1) => 2,
                    (AttackEffectiveness::Average, 0) => 2,
                    _ => 1,
                };
                n = n.max(floor).max(prev).min(255);
                out[i] = n as u8;
                prev = n;
            }
            out
        }
    };
    rom.write(off, &new)?;
    Ok(new != vanilla)
}

/// Integer draw in `[floor(v*lo), floor(v*hi))` with `lo`/`hi` as
/// fractions; equal bounds give the lower one.
fn draw_scaled(rng: &mut Rng, v: u8, lo: (u32, u32), hi: (u32, u32)) -> i64 {
    half_open(rng, scale(v, lo.0, lo.1), scale(v, hi.0, hi.1))
}

/// Shared shape of the life and magic tables: `rows` rows of 8 levels,
/// values stored doubled. Each cell becomes a draw from the halved vanilla
/// value times the range, capped at 120, and never above the same row's
/// previous level. Iterates level-outer, row-inner.
fn randomize_doubled_grid(
    rng: &mut Rng,
    table: &mut [u8],
    rows: usize,
    lo: (u32, u32),
    hi: (u32, u32),
) {
    let vanilla = table.to_vec();
    for level in 0..8 {
        for row in 0..rows {
            let i = row * 8 + level;
            let base = vanilla[i] >> 1;
            let mut n = draw_scaled(rng, base, lo, hi).clamp(0, 120);
            if level > 0 {
                n = n.min(i64::from(table[i - 1] >> 1));
            }
            table[i] = (n as u8) << 1;
        }
    }
}

/// Randomize the damage Link takes. Returns whether it changed.
pub fn randomize_life(
    rom: &mut Rom,
    rng: &mut Rng,
    mode: LifeEffectiveness,
) -> Result<bool, RandoError> {
    let off = life_off(rom)?;
    let mut table = rom.read_slice(off, 56)?.to_vec();
    let vanilla = table.clone();
    match mode {
        LifeEffectiveness::Vanilla => return Ok(false),
        LifeEffectiveness::Ohko => table.fill(0xFF),
        LifeEffectiveness::Invincible => table.fill(0x00),
        LifeEffectiveness::AverageLow => {
            randomize_doubled_grid(rng, &mut table, 7, (1, 1), (3, 2));
        }
        LifeEffectiveness::Average => {
            randomize_doubled_grid(rng, &mut table, 7, (3, 4), (3, 2));
        }
        LifeEffectiveness::AverageHigh => {
            randomize_doubled_grid(rng, &mut table, 7, (1, 2), (1, 1));
        }
        LifeEffectiveness::High => randomize_doubled_grid(rng, &mut table, 7, (1, 2), (1, 2)),
    }
    rom.write(off, &table)?;
    Ok(table != vanilla)
}

/// Randomize the spell costs. Returns whether they changed.
pub fn randomize_magic(
    rom: &mut Rom,
    rng: &mut Rng,
    mode: MagicEffectiveness,
) -> Result<bool, RandoError> {
    let off = rom.cpu_offset(MAGIC_TABLE.0, MAGIC_TABLE.1)?;
    let mut table = rom.read_slice(off, 64)?.to_vec();
    let vanilla = table.clone();
    match mode {
        MagicEffectiveness::Vanilla => return Ok(false),
        MagicEffectiveness::Free => table.fill(0),
        MagicEffectiveness::HighCost => {
            randomize_doubled_grid(rng, &mut table, 8, (3, 2), (3, 2));
        }
        MagicEffectiveness::AverageHighCost => {
            randomize_doubled_grid(rng, &mut table, 8, (1, 1), (3, 2));
        }
        MagicEffectiveness::Average => {
            randomize_doubled_grid(rng, &mut table, 8, (1, 2), (3, 2));
        }
        MagicEffectiveness::AverageLowCost => {
            randomize_doubled_grid(rng, &mut table, 8, (1, 2), (1, 1));
        }
        MagicEffectiveness::LowCost => randomize_doubled_grid(rng, &mut table, 8, (1, 2), (1, 2)),
    }
    rom.write(off, &table)?;
    Ok(table != vanilla)
}

/// Magic containers the logic asks for before casting each spell (indexed
/// by vanilla spell), rescaled to new costs. `menu[slot]` is the vanilla
/// spell in cost row `slot`. A requirement of `n` containers is tied to the
/// cost at magic level `n`; it becomes `ceil(n * new / old)`, at most 8.
#[must_use]
pub fn spell_containers(old: &[u8; 8], menu: &[u8; 8], before: &[u8], after: &[u8]) -> [u8; 8] {
    let mut out = *old;
    for (slot, &spell) in menu.iter().enumerate() {
        let s = usize::from(spell & 7);
        let n = old[s];
        if n == 0 {
            continue;
        }
        let i = slot * 8 + usize::from(n.min(8)) - 1;
        let (was, now) = (u32::from(before[i]), u32::from(after[i]));
        if was == 0 {
            continue;
        }
        out[s] = (u32::from(n) * now).div_ceil(was).min(8) as u8;
    }
    out
}

fn spoil_grid(
    ctx: &mut Ctx,
    title: &str,
    off: usize,
    row_names: &[&str],
) -> Result<(), RandoError> {
    let table = ctx.rom.read_slice(off, row_names.len() * 8)?.to_vec();
    ctx.spoiler.line("Stats", format!("{title}:"));
    for (r, name) in row_names.iter().enumerate() {
        let halves: Vec<u8> = table[r * 8..r * 8 + 8].iter().map(|b| b >> 1).collect();
        ctx.spoiler
            .line("Stats", format!("  {name:<8} {}", join(&halves)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::VANILLA_BODY_LEN;

    /// A synthetic image with the vanilla-shaped stat tables in place.
    fn rom() -> Rom {
        let mut rom = Rom::from_body(&vec![0u8; VANILLA_BODY_LEN]).unwrap();
        let exp: [[u16; 8]; 3] = [
            [200, 500, 1000, 2000, 3000, 5000, 8000, 9000],
            [100, 300, 700, 1200, 2200, 3500, 6000, 9000],
            [50, 150, 400, 800, 1500, 2500, 4000, 9000],
        ];
        write_experience(&mut rom, &exp).unwrap();
        rom.write_cpu(7, ATTACK_TABLE, &[2, 3, 4, 6, 9, 12, 18, 24])
            .unwrap();
        let life: Vec<u8> = (0..56).map(|i| 0xE0 - (i as u8 % 8) * 0x10).collect();
        rom.write_cpu(7, LIFE_TABLE, &life).unwrap();
        let magic: Vec<u8> = (0..64).map(|i| 0xA0 - (i as u8 % 8) * 0x10).collect();
        rom.write_cpu(MAGIC_TABLE.0, MAGIC_TABLE.1, &magic).unwrap();
        rom.write_cpu(
            LEVEL_CAP_SITE.0,
            LEVEL_CAP_SITE.1,
            &[0xBD, 0x77, 0x07, 0xC9, 0x08],
        )
        .unwrap();
        rom
    }

    #[test]
    fn spell_cost_spoiler_follows_the_menu_order() {
        use crate::flags::{Flags, MagicEffectiveness};
        let r = rom();
        let mut flags = Flags::default();
        flags.stats.magic_effectiveness = MagicEffectiveness::Free;
        let mut ctx = Ctx {
            rom: r.clone(),
            vanilla: r,
            rng: Rng::new(3),
            flags,
            seed: String::new(),
            attempt: 0,
            state: crate::State::default(),
            spoiler: crate::spoiler::Spoiler::new(),
            log: Vec::new(),
            extras: crate::Extras::default(),
        };
        // Slot 0 holds Thunder, slot 7 Shield; Fire became Dash.
        ctx.state.spells.menu = [7, 1, 2, 3, 4, 5, 6, 0];
        ctx.state.spells.fire = crate::spells::FireMode::Dash;
        apply(&mut ctx).unwrap();
        let text = ctx.spoiler.render();
        let rows: Vec<&str> = text
            .lines()
            .skip_while(|l| !l.contains("Spell costs"))
            .skip(1)
            .take(8)
            .map(|l| l.split_whitespace().next().unwrap_or(""))
            .collect();
        assert_eq!(
            rows,
            ["Thunder", "Jump", "Life", "Fairy", "Dash", "Reflect", "Spell", "Shield"],
            "{text}"
        );
    }

    #[test]
    fn digits_match_the_vanilla_layout() {
        assert_eq!(digit_tiles(200), [0xD0, 0xD2, BLANK_TILE]);
        assert_eq!(digit_tiles(50), [0xD5, BLANK_TILE, BLANK_TILE]);
        assert_eq!(digit_tiles(9000), [0xD0, 0xD0, 0xD9]);
        assert_eq!(digit_tiles(1230), [0xD3, 0xD2, 0xD1]);
    }

    #[test]
    fn experience_round_trips() {
        let r = rom();
        let rows = read_experience(&r).unwrap();
        assert_eq!(rows[0][0], 200);
        assert_eq!(rows[2][7], 9000);
    }

    #[test]
    fn default_flags_change_nothing() {
        let mut r = rom();
        let before = r.clone();
        let mut rng = Rng::new(1);
        let f = StatsFlags::default();
        assert!(!randomize_experience(&mut r, &mut rng, &f).unwrap());
        assert!(!randomize_attack(&mut r, &mut rng, f.attack_effectiveness).unwrap());
        assert!(!randomize_life(&mut r, &mut rng, f.life_effectiveness).unwrap());
        assert!(!randomize_magic(&mut r, &mut rng, f.magic_effectiveness).unwrap());
        assert_eq!(r, before);
    }

    #[test]
    fn shuffled_experience_is_monotone_and_in_range() {
        for seed in 0..50 {
            let mut r = rom();
            let vanilla = read_experience(&r).unwrap();
            let mut rng = Rng::new(seed);
            let f = StatsFlags {
                shuffle_attack_exp: true,
                shuffle_magic_exp: true,
                shuffle_life_exp: true,
                ..StatsFlags::default()
            };
            assert!(randomize_experience(&mut r, &mut rng, &f).unwrap());
            let rows = read_experience(&r).unwrap();
            let one_up = rows[0][7];
            for s in 0..3 {
                assert_eq!(rows[s][7], one_up);
                for i in 0..7 {
                    let v = rows[s][i];
                    assert_eq!(v % 10, 0);
                    assert!(v >= vanilla[s][i] * 3 / 4 - 10, "{s} {i} {v}");
                    assert!(v <= vanilla[s][i] * 5 / 4, "{s} {i} {v}");
                    if i > 0 {
                        assert!(v >= rows[s][i - 1]);
                    }
                    assert!(one_up > v);
                }
            }
        }
    }

    #[test]
    fn caps_fill_with_one_up_and_patch_the_check() {
        let mut r = rom();
        let mut rng = Rng::new(9);
        let f = StatsFlags {
            attack_level_cap: 3,
            magic_level_cap: 8,
            life_level_cap: 1,
            scale_level_requirements_to_cap: true,
            ..StatsFlags::default()
        };
        assert!(randomize_experience(&mut r, &mut rng, &f).unwrap());
        let rows = read_experience(&r).unwrap();
        let one_up = rows[1][7];
        assert!(rows[0][2..].iter().all(|&v| v == one_up));
        assert!(rows[2].iter().all(|&v| v == one_up));
        // Scaled: attack's last regular level costs what L8 cost.
        assert_eq!(rows[0][1], 8000);
        assert!(rows[0][0] < rows[0][1]);
        assert!(one_up > 8000);
        patch_level_caps(&mut r, [3, 8, 1]).unwrap();
        let site = r.read_cpu(0, LEVEL_CAP_SITE.1).unwrap();
        assert_eq!(site, 0x20, "JSR");
        let stub = r.read_cpu_word(0, LEVEL_CAP_SITE.1 + 1).unwrap();
        assert_eq!(r.read_cpu(0, stub).unwrap(), 0xBD);
        assert_eq!(r.read_cpu(0, stub + 3).unwrap(), 0xDD, "CMP abs,X");
        let caps = r.read_cpu_word(0, stub + 4).unwrap();
        assert_eq!(
            r.read_slice(r.cpu_offset(0, caps).unwrap(), 3).unwrap(),
            &[3, 8, 1]
        );
    }

    #[test]
    fn scale_row_spans_the_vanilla_curve() {
        let v = [200, 500, 1000, 2000, 3000, 5000, 8000, 9000];
        let s = scale_row(&v, 2);
        assert_eq!(s[0], 8000);
        assert!(s[1..].iter().all(|&x| x == EXP_MAX));
        let s = scale_row(&v, 8);
        assert_eq!(&s[..7], &v[..7]);
    }

    #[test]
    fn attack_modes() {
        for mode in AttackEffectiveness::ALL.iter().copied() {
            for seed in 0..20 {
                let mut r = rom();
                let mut rng = Rng::new(seed);
                randomize_attack(&mut r, &mut rng, mode).unwrap();
                let t = r.read_slice(attack_off(&r).unwrap(), 8).unwrap().to_vec();
                assert!(t.windows(2).all(|w| w[0] <= w[1]), "{mode:?} {t:?}");
                assert!(t.iter().all(|&v| v >= 1));
                if mode == AttackEffectiveness::Ohko {
                    assert!(t.iter().all(|&v| v == OHKO_DAMAGE));
                }
            }
        }
    }

    #[test]
    fn life_and_magic_never_rise_with_level() {
        for seed in 0..20 {
            for mode in LifeEffectiveness::ALL.iter().copied() {
                let mut r = rom();
                randomize_life(&mut r, &mut Rng::new(seed), mode).unwrap();
                let t = r.read_slice(life_off(&r).unwrap(), 56).unwrap().to_vec();
                for row in t.chunks(8) {
                    assert!(row.windows(2).all(|w| w[0] >= w[1]), "{mode:?} {row:?}");
                    if !matches!(mode, LifeEffectiveness::Ohko) {
                        assert!(row.iter().all(|&b| b % 2 == 0 && b >> 1 <= 120));
                    }
                }
            }
            for mode in MagicEffectiveness::ALL.iter().copied() {
                let mut r = rom();
                randomize_magic(&mut r, &mut Rng::new(seed), mode).unwrap();
                let off = r.cpu_offset(MAGIC_TABLE.0, MAGIC_TABLE.1).unwrap();
                let t = r.read_slice(off, 64).unwrap().to_vec();
                for row in t.chunks(8) {
                    assert!(row.windows(2).all(|w| w[0] >= w[1]), "{mode:?} {row:?}");
                }
                if mode == MagicEffectiveness::Free {
                    assert!(t.iter().all(|&b| b == 0));
                }
            }
        }
    }

    #[test]
    fn spell_containers_follow_the_cost() {
        let old = [0, 2, 0, 4, 0, 4, 4, 0];
        let menu = [0, 1, 2, 3, 4, 5, 6, 7];
        let before = vec![0x40u8; 64];
        let mut after = before.clone();
        after[8 + 1] = 0x60; // Jump at level 2: 1.5x
        after[3 * 8 + 3] = 0x20; // Fairy at level 4: 0.5x
        after[5 * 8 + 3] = 0; // Reflect free
        let new = spell_containers(&old, &menu, &before, &after);
        assert_eq!(new, [0, 3, 0, 2, 0, 0, 4, 0]);
        // Swapped menu: slot 1 holds Fairy.
        let menu = [0, 3, 2, 1, 4, 5, 6, 7];
        let new = spell_containers(&old, &menu, &before, &after);
        assert_eq!(new[3], 4, "Fairy row (slot 1) at level 4 unchanged");
    }

    #[test]
    fn high_life_halves_damage_exactly() {
        let mut r = rom();
        let before = r.read_slice(life_off(&r).unwrap(), 56).unwrap().to_vec();
        randomize_life(&mut r, &mut Rng::new(3), LifeEffectiveness::High).unwrap();
        let after = r.read_slice(life_off(&r).unwrap(), 56).unwrap().to_vec();
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(u32::from(*b >> 1), u32::from(*a >> 1) / 2);
        }
    }
}
