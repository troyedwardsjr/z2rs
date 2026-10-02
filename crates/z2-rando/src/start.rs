//! `start` module: Starting inventory, stats, lives and techniques.
//!
//! Options owned: [`crate::flags::StartFlags`] (`ctx.flags.start`): starting
//! items and spells (with their shuffle and limit switches), heart/magic
//! container ranges, total hearts, starting techniques, lives and levels.
//! The random-switch rate is read by [`crate::Ctx::tri`]; "share seed across
//! difficulty" only changes the hash code (see README.md).
//!
//! Catalog: section 01 (starting values), 02 (Start tab).
//!
//! This module runs first. It builds the shared [`World`] from the vanilla
//! ROM (`ctx.state.world`), decides the starting inventory, records it in
//! [`World::start`] / [`World::max_hearts`], and writes the save-file
//! defaults the game copies into a new file:
//!
//! | iNES offset | Meaning |
//! |---|---|
//! | `0x17AF3-0x17AF5` | starting attack, magic, life level |
//! | `0x17AF7-0x17AFE` | spells known (menu order, 1 = known) |
//! | `0x17AFF` | magic containers |
//! | `0x17B00` | heart containers |
//! | `0x17B01-0x17B08` | candle .. magic key (1 = owned) |
//! | `0x17B12` | techniques (upstab `$04`, downstab `$10`) |
//! | `0x1C369` | starting lives (operand of the reset routine; honoured by the trapped port) |
//!
//! Nothing is written when an option keeps its vanilla value, so default
//! flags leave the ROM untouched.

use crate::flags::{MaxHearts, StartLimit, StartingLives, StartingTechs};
use crate::world::{ItemId, World};
use crate::{Ctx, RandoError};

const LEVELS_INES: usize = 0x17AF3;
const SPELLS_INES: usize = 0x17AF7;
const MAGIC_INES: usize = 0x17AFF;
const HEARTS_INES: usize = 0x17B00;
const TOOLS_INES: usize = 0x17B01;
const TECHS_INES: usize = 0x17B12;
const LIVES_INES: usize = 0x1C369;

/// Technique bits in the save byte.
pub const UPSTAB_BIT: u8 = 0x04;
/// Technique bits in the save byte.
pub const DOWNSTAB_BIT: u8 = 0x10;

/// The starting values this module decided (also mirrored into the world).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartingValues {
    /// Tools owned at the start (subset of [`ItemId::TOOLS`]).
    pub tools: Vec<ItemId>,
    /// Spells known at the start (subset of [`ItemId::SPELLS`]).
    pub spells: Vec<ItemId>,
    /// Starting heart containers.
    pub hearts: u8,
    /// Total heart containers in the game.
    pub max_hearts: u8,
    /// Starting magic containers.
    pub magic: u8,
    /// Upstab known.
    pub upstab: bool,
    /// Downstab known.
    pub downstab: bool,
    /// Starting lives.
    pub lives: u8,
    /// Attack, magic and life levels.
    pub levels: [u8; 3],
}

fn limit(l: StartLimit) -> usize {
    match l {
        StartLimit::NoLimit => 8,
        StartLimit::One => 1,
        StartLimit::Two => 2,
        StartLimit::Four => 4,
    }
}

/// Pick a starting set: walk the candidates in random order taking the
/// checked ones up to `cap`; with `shuffle` walk again and add each
/// unchecked one with a 1-in-4 chance, still within `cap`.
fn pick_start(
    ctx: &mut Ctx,
    candidates: &[ItemId],
    checked: &[bool],
    shuffle: bool,
    cap: usize,
) -> Vec<ItemId> {
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    ctx.rng.shuffle(&mut order);
    let mut take = vec![false; candidates.len()];
    let mut n = 0;
    for &i in &order {
        if checked[i] && n < cap {
            take[i] = true;
            n += 1;
        }
    }
    if shuffle {
        for &i in &order {
            if !take[i] && n < cap && ctx.rng.chance(1, 4) {
                take[i] = true;
                n += 1;
            }
        }
    }
    (0..candidates.len())
        .filter(|&i| take[i])
        .map(|i| candidates[i])
        .collect()
}

/// Decide the starting values (draws from `ctx.rng` only when an option asks
/// for randomness).
pub fn decide(ctx: &mut Ctx) -> StartingValues {
    let f = ctx.flags.start.clone();
    let tool_checks = [
        f.start_with_candle,
        f.start_with_glove,
        f.start_with_raft,
        f.start_with_boots,
        f.start_with_flute,
        f.start_with_cross,
        f.start_with_hammer,
        f.start_with_magic_key,
    ];
    let spell_checks = [
        f.start_with_shield,
        f.start_with_jump,
        f.start_with_life,
        f.start_with_fairy,
        f.start_with_fire,
        f.start_with_reflect,
        f.start_with_spell,
        f.start_with_thunder,
    ];
    let tools = if tool_checks.iter().any(|&b| b) || f.shuffle_starting_items {
        pick_start(
            ctx,
            &ItemId::TOOLS,
            &tool_checks,
            f.shuffle_starting_items,
            limit(f.start_items_limit),
        )
    } else {
        Vec::new()
    };
    let spells = if spell_checks.iter().any(|&b| b) || f.shuffle_starting_spells {
        pick_start(
            ctx,
            &ItemId::SPELLS,
            &spell_checks,
            f.shuffle_starting_spells,
            limit(f.start_spells_limit),
        )
    } else {
        Vec::new()
    };
    let roll = |ctx: &mut Ctx, lo: u8, hi: u8| {
        if lo == hi {
            lo
        } else {
            ctx.rng.range_u8(lo.min(hi), lo.max(hi))
        }
    };
    let hearts = roll(ctx, f.heart_containers_min, f.heart_containers_max).clamp(1, 8);
    let max_hearts = match f.max_heart_containers {
        MaxHearts::One => 1,
        MaxHearts::Two => 2,
        MaxHearts::Three => 3,
        MaxHearts::Four => 4,
        MaxHearts::Five => 5,
        MaxHearts::Six => 6,
        MaxHearts::Seven => 7,
        MaxHearts::Eight => 8,
        MaxHearts::PlusOne => (hearts + 1).min(8),
        MaxHearts::PlusTwo => (hearts + 2).min(8),
        MaxHearts::PlusThree => (hearts + 3).min(8),
        MaxHearts::PlusFour => (hearts + 4).min(8),
        MaxHearts::Random => ctx.rng.range_u8(hearts, 8),
    }
    .max(hearts);
    let magic = roll(ctx, f.magic_containers_min, f.magic_containers_max).clamp(1, 8);
    let (upstab, downstab) = match f.starting_techs {
        StartingTechs::None => (false, false),
        StartingTechs::Downstab => (false, true),
        StartingTechs::Upstab => (true, false),
        StartingTechs::Both => (true, true),
        // Same odds as the original: 4 in 7 none, then one each.
        StartingTechs::Random => match ctx.rng.below(7) {
            4 => (false, true),
            5 => (true, false),
            6 => (true, true),
            _ => (false, false),
        },
    };
    let lives = match f.starting_lives {
        StartingLives::L1 => 1,
        StartingLives::L2 => 2,
        StartingLives::L3 => 3,
        StartingLives::L4 => 4,
        StartingLives::L5 => 5,
        StartingLives::L8 => 8,
        StartingLives::L16 => 16,
        StartingLives::Random => ctx.rng.range_u8(2, 5),
    };
    StartingValues {
        tools,
        spells,
        hearts,
        max_hearts,
        magic,
        upstab,
        downstab,
        lives,
        levels: [
            f.attack_level.clamp(1, 8),
            f.magic_level.clamp(1, 8),
            f.life_level.clamp(1, 8),
        ],
    }
}

/// Mirror the starting values into the world's starting inventory.
pub fn apply_to_world(w: &mut World, v: &StartingValues) {
    for &t in &v.tools {
        w.start.add(t);
    }
    for &s in &v.spells {
        w.start.add(s);
    }
    if v.upstab {
        w.start.add(ItemId::Upstab);
    }
    if v.downstab {
        w.start.add(ItemId::Downstab);
    }
    w.start.hearts = v.hearts;
    w.start.magic = v.magic;
    w.max_hearts = v.max_hearts;
    w.max_magic = 8;
}

/// Write the values that differ from the vanilla save defaults.
pub fn write(ctx: &mut Ctx, v: &StartingValues) -> Result<(), RandoError> {
    let rom = &mut ctx.rom;
    let mut put = |ines: usize, bytes: &[u8]| -> Result<(), RandoError> {
        let off = rom.prg_from_ines(ines)?;
        rom.write(off, bytes)
    };
    if v.levels != [1, 1, 1] {
        put(LEVELS_INES, &v.levels)?;
    }
    if !v.spells.is_empty() {
        let mut b = [0u8; 8];
        for s in &v.spells {
            if let Some(i) = s.spell_index() {
                b[i] = 1;
            }
        }
        put(SPELLS_INES, &b)?;
    }
    if v.magic != 4 {
        put(MAGIC_INES, &[v.magic])?;
    }
    if v.hearts != 4 {
        put(HEARTS_INES, &[v.hearts])?;
    }
    if !v.tools.is_empty() {
        let mut b = [0u8; 8];
        for t in &v.tools {
            b[usize::from(t.byte())] = 1;
        }
        put(TOOLS_INES, &b)?;
    }
    if v.upstab || v.downstab {
        let mut b = 0;
        if v.upstab {
            b |= UPSTAB_BIT;
        }
        if v.downstab {
            b |= DOWNSTAB_BIT;
        }
        put(TECHS_INES, &[b])?;
    }
    if v.lives != 3 {
        put(LIVES_INES, &[v.lives])?;
    }
    Ok(())
}

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    ctx.state.world = World::vanilla(&ctx.vanilla)?;
    let relaxed = ctx.state.world.relax_to_solvable();
    if !relaxed.is_empty() {
        ctx.log(format!(
            "start: the vanilla world does not look like the game; not required: {}",
            relaxed.join(", ")
        ));
    }
    let v = decide(ctx);
    apply_to_world(&mut ctx.state.world, &v);
    write(ctx, &v)?;
    let names = |items: &[ItemId]| {
        if items.is_empty() {
            "none".to_string()
        } else {
            items
                .iter()
                .map(|i| i.name())
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    if v.tools.is_empty()
        && v.spells.is_empty()
        && v.hearts == 4
        && v.magic == 4
        && v.max_hearts == 8
        && !v.upstab
        && !v.downstab
        && v.lives == 3
        && v.levels == [1, 1, 1]
    {
        return Ok(());
    }
    let mut techs = Vec::new();
    if v.downstab {
        techs.push(ItemId::Downstab);
    }
    if v.upstab {
        techs.push(ItemId::Upstab);
    }
    let lines = [
        format!("Items: {}", names(&v.tools)),
        format!("Spells: {}", names(&v.spells)),
        format!("Techniques: {}", names(&techs)),
        format!(
            "Heart containers: {} (of {} in the game)",
            v.hearts, v.max_hearts
        ),
        format!("Magic containers: {}", v.magic),
        format!("Lives: {}", v.lives),
        format!(
            "Levels: attack {}, magic {}, life {}",
            v.levels[0], v.levels[1], v.levels[2]
        ),
    ];
    for l in lines {
        ctx.spoiler.line("Start", l);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::Flags;
    use crate::rng::Rng;
    use crate::rom::{Rom, VANILLA_BODY_LEN};
    use crate::spoiler::Spoiler;
    use crate::{Extras, State};

    fn ctx(flags: Flags, seed: u64) -> Ctx {
        let mut r = Rng::new(9);
        let body: Vec<u8> = (0..VANILLA_BODY_LEN).map(|_| r.next_u32() as u8).collect();
        let rom = Rom::from_body(&body).unwrap();
        Ctx {
            rom: rom.clone(),
            vanilla: rom,
            rng: Rng::new(seed),
            flags,
            seed: String::new(),
            attempt: 0,
            state: State::default(),
            spoiler: Spoiler::new(),
            log: Vec::new(),
            extras: Extras::default(),
        }
    }

    #[test]
    fn defaults_change_nothing() {
        let mut c = ctx(Flags::default(), 1);
        let before = c.rom.body();
        apply(&mut c).unwrap();
        assert_eq!(c.rom.body(), before);
        assert!(c.spoiler.is_empty());
        assert!(c.state.world.built);
        assert_eq!(c.state.world.start.hearts, 4);
        assert_eq!(c.state.world.max_hearts, 8);
    }

    #[test]
    fn checked_items_respect_the_limit() {
        let mut f = Flags::default();
        f.start.start_with_glove = true;
        f.start.start_with_raft = true;
        f.start.start_with_hammer = true;
        f.start.start_items_limit = StartLimit::Two;
        for seed in 0..20 {
            let mut c = ctx(f.clone(), seed);
            let v = decide(&mut c);
            assert_eq!(v.tools.len(), 2);
            assert!(v
                .tools
                .iter()
                .all(|t| [ItemId::Glove, ItemId::Raft, ItemId::Hammer].contains(t)));
        }
    }

    #[test]
    fn writes_start_bytes() {
        let mut f = Flags::default();
        f.start.start_with_boots = true;
        f.start.start_with_fairy = true;
        f.start.starting_techs = StartingTechs::Both;
        f.start.starting_lives = StartingLives::L5;
        f.start.heart_containers_min = 6;
        f.start.heart_containers_max = 6;
        f.start.attack_level = 3;
        let mut c = ctx(f, 3);
        apply(&mut c).unwrap();
        let rd = |c: &Ctx, ines: usize| c.rom.read(c.rom.prg_from_ines(ines).unwrap()).unwrap();
        assert_eq!(rd(&c, TOOLS_INES + 3), 1);
        assert_eq!(rd(&c, TOOLS_INES), 0);
        assert_eq!(rd(&c, SPELLS_INES + 3), 1);
        assert_eq!(rd(&c, TECHS_INES), UPSTAB_BIT | DOWNSTAB_BIT);
        assert_eq!(rd(&c, LIVES_INES), 5);
        assert_eq!(rd(&c, HEARTS_INES), 6);
        assert_eq!(rd(&c, LEVELS_INES), 3);
        let w = &c.state.world;
        assert!(w.start.has(ItemId::Boots) && w.start.has(ItemId::Fairy));
        assert!(w.start.has(ItemId::Upstab));
        assert_eq!(w.start.hearts, 6);
        assert!(c.spoiler.render().contains("Boots"));
    }

    #[test]
    fn max_hearts_never_below_start() {
        let mut f = Flags::default();
        f.start.heart_containers_min = 6;
        f.start.heart_containers_max = 6;
        f.start.max_heart_containers = MaxHearts::Three;
        let mut c = ctx(f.clone(), 1);
        assert_eq!(decide(&mut c).max_hearts, 6);
        f.start.max_heart_containers = MaxHearts::PlusOne;
        let mut c = ctx(f, 1);
        assert_eq!(decide(&mut c).max_hearts, 7);
    }
}
