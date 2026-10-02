//! `drops` module: enemy drop pools, drop frequency and standardized drops.
//!
//! Options owned: [`crate::flags::DropFlags`] (`ctx.flags.drops`). (The
//! P-bag experience amounts, `items.shuffle_pbag_amounts`, are done by the
//! `items` module.)
//!
//! Catalog: section 05 (drops, sections 5.1-5.3 and 5.5).
//!
//! Everything lives in the fixed bank and is edited in place:
//!
//! | Data | Where | Reader |
//! |---|---|---|
//! | Small-enemy drops (8 item codes) | `$E870` | `enemy_traps::en_death` via the bus |
//! | Large-enemy drops (8 item codes) | `$E878` | same |
//! | Kills per drop (`CMP #` operand) | `$E8A0` | `enemy_traps::en_death` reads the operand from the ROM (`TRAP_HONORED_BYTES`) |
//!
//! Standardized drops replace the random draw in the drop routine
//! (`LDA $051B,X` at `$E8AD`) with a call to a small counter routine in
//! fixed-bank padding, so every player sees the same drop sequence. That
//! edit is inside the ported `bank7_monster_death`, which the trap policy
//! then runs as ROM code.
//!
//! With default options it does nothing.

use crate::flags::DropPool;
use crate::rng::Rng;
use crate::rom::Rom;
use crate::{Ctx, RandoError};

/// Small-enemy drop table (8 item codes; the large table follows).
pub const SMALL_DROPS: u16 = 0xE870;
/// Large-enemy drop table.
pub const LARGE_DROPS: u16 = 0xE878;
/// `CMP #` operand: kills per drop.
pub const DROP_FREQUENCY: u16 = 0xE8A0;
/// `LDA $051B,X` (the random draw that picks a drop slot).
pub const DROP_DRAW_SITE: u16 = 0xE8AD;
/// RAM counters for standardized drops (large group, other groups).
pub const STANDARD_COUNTER_LARGE: u16 = 0x06FE;
/// See [`STANDARD_COUNTER_LARGE`].
pub const STANDARD_COUNTER_SMALL: u16 = 0x06FF;

/// Item codes in [`DropPool`] order: blue jar, red jar, 50, 100, 200 and 500
/// bags, 1-up, key.
pub const POOL_CODES: [u8; 8] = [0x90, 0x91, 0x8A, 0x8B, 0x8C, 0x8D, 0x92, 0x88];
const POOL_NAMES: [&str; 8] = [
    "blue jar", "red jar", "50 bag", "100 bag", "200 bag", "500 bag", "1-up", "key",
];

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    let f = ctx.flags.drops.clone();
    let mut small = f.small_pool;
    let mut large = f.large_pool;
    if f.randomize_drops {
        fill_pool(&mut ctx.rng, &mut small);
        fill_pool(&mut ctx.rng, &mut large);
    }
    for (addr, pool, who) in [(SMALL_DROPS, small, "Small"), (LARGE_DROPS, large, "Large")] {
        if let Some(table) = drop_table(&mut ctx.rng, &pool) {
            ctx.rom.write_cpu(7, addr, &table)?;
            let names: Vec<&str> = table.iter().map(|&c| code_name(c)).collect();
            ctx.spoiler
                .line("Drops", format!("{who} enemies drop: {}", names.join(", ")));
        }
    }
    if f.shuffle_drop_frequency {
        let n = ctx.rng.range_u8(4, 8);
        set_drop_frequency(&mut ctx.rom, n)?;
        ctx.spoiler
            .line("Drops", format!("An item drops every {n} kills"));
    }
    if f.standardize_drops {
        standardize(&mut ctx.rom)?;
        ctx.spoiler.line("Drops", "Drops are standardized");
    }
    Ok(())
}

fn code_name(c: u8) -> &'static str {
    POOL_CODES
        .iter()
        .position(|&x| x == c)
        .map_or("?", |i| POOL_NAMES[i])
}

fn pool_items(p: &DropPool) -> Vec<u8> {
    let mut p = *p;
    p.entries_mut()
        .iter()
        .zip(POOL_CODES)
        .filter(|((_, on), _)| **on)
        .map(|(_, c)| c)
        .collect()
}

/// "Randomize drops": every unticked item joins with a coin flip, retried
/// until the pool has something in it.
pub fn fill_pool(rng: &mut Rng, p: &mut DropPool) {
    let base = *p;
    loop {
        *p = base;
        for (_, v) in p.entries_mut() {
            if !*v {
                *v = rng.coin();
            }
        }
        if !pool_items(p).is_empty() {
            return;
        }
    }
}

/// The 8-slot drop table for a pool: the shuffled pool first, then uniform
/// picks from it. `None` (keep vanilla) when the pool is empty.
#[must_use]
pub fn drop_table(rng: &mut Rng, p: &DropPool) -> Option<[u8; 8]> {
    let mut items = pool_items(p);
    if items.is_empty() {
        return None;
    }
    rng.shuffle(&mut items);
    let mut out = [0u8; 8];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = if i < items.len() {
            items[i]
        } else {
            items[rng.index(items.len())]
        };
    }
    Some(out)
}

/// Kills per drop (vanilla 6). Only the `CMP #` operand changes.
pub fn set_drop_frequency(rom: &mut Rom, n: u8) -> Result<(), RandoError> {
    if rom.read_cpu(7, DROP_FREQUENCY - 1)? != 0xC9 {
        return Ok(());
    }
    rom.write_cpu(7, DROP_FREQUENCY, &[n.max(1)])
}

/// Replace the random drop-slot draw with per-group counters: the large
/// group (`Y = 2`) counts in [`STANDARD_COUNTER_LARGE`], the others in
/// [`STANDARD_COUNTER_SMALL`]; each returns its value, then increments.
pub fn standardize(rom: &mut Rom) -> Result<(), RandoError> {
    let site = rom.read_slice(rom.cpu_offset(7, DROP_DRAW_SITE)?, 3)?;
    if site != [0xBD, 0x1B, 0x05] {
        // Not the vanilla code (synthetic test images): leave it alone.
        return Ok(());
    }
    let stub = rom.alloc_vanilla(7, 18)?;
    let src = format!(
        "
        .org ${site:04X}
            JSR stub
        .org ${stub:04X}
        stub:
            CPY #$02
            BEQ large
            LDA ${small:04X}
            INC ${small:04X}
            RTS
        large:
            LDA ${large:04X}
            INC ${large:04X}
            RTS
        ",
        site = DROP_DRAW_SITE,
        small = STANDARD_COUNTER_SMALL,
        large = STANDARD_COUNTER_LARGE,
    );
    let out = crate::asm::assemble(&src)?;
    rom.apply_asm(7, &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::VANILLA_BODY_LEN;

    #[test]
    fn empty_pool_keeps_vanilla() {
        let mut rng = Rng::new(1);
        assert!(drop_table(&mut rng, &DropPool::default()).is_none());
    }

    #[test]
    fn tables_use_only_pool_items_and_all_of_them() {
        for seed in 0..50 {
            let mut rng = Rng::new(seed);
            let p = DropPool {
                red_jar: true,
                key: true,
                xl_bag: true,
                ..DropPool::default()
            };
            let t = drop_table(&mut rng, &p).unwrap();
            for c in [0x91, 0x88, 0x8D] {
                assert!(t.contains(&c));
            }
            assert!(t.iter().all(|c| [0x91, 0x88, 0x8D].contains(c)));
        }
    }

    #[test]
    fn randomized_pools_are_never_empty_and_keep_ticks() {
        for seed in 0..100 {
            let mut rng = Rng::new(seed);
            let mut p = DropPool {
                one_up: true,
                ..DropPool::default()
            };
            fill_pool(&mut rng, &mut p);
            assert!(p.one_up);
            let mut q = DropPool::default();
            fill_pool(&mut rng, &mut q);
            assert!(!pool_items(&q).is_empty());
        }
    }

    #[test]
    fn frequency_and_standardize() {
        let mut rom = Rom::from_body(&vec![0u8; VANILLA_BODY_LEN]).unwrap();
        rom.write_cpu(7, DROP_FREQUENCY - 1, &[0xC9, 0x06]).unwrap();
        set_drop_frequency(&mut rom, 4).unwrap();
        assert_eq!(rom.read_cpu(7, DROP_FREQUENCY).unwrap(), 4);
        rom.write_cpu(7, DROP_DRAW_SITE, &[0xBD, 0x1B, 0x05])
            .unwrap();
        standardize(&mut rom).unwrap();
        assert_eq!(rom.read_cpu(7, DROP_DRAW_SITE).unwrap(), 0x20);
        let stub = rom.read_cpu_word(7, DROP_DRAW_SITE + 1).unwrap();
        assert_eq!(rom.read_cpu(7, stub).unwrap(), 0xC0, "CPY #");
    }
}
