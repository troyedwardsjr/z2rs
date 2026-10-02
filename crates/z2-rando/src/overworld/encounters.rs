//! Overworld encounters: how often the wandering enemies appear, and which
//! battle scene each terrain leads to.
//!
//! * The spawn routine (bank 0 `$8284`) restarts a per-terrain wave timer
//!   from a table at bank 0 `$8240` (grass, desert, forest, swamp, grave,
//!   lava; in units of about 21 frames). "Half" doubles those values;
//!   "None" makes the routine return at once.
//! * When an enemy touches Link, the scene comes from a table of area bytes
//!   indexed by `(terrain - desert) * 2 + south` (bank 1 for West Hyrule and
//!   Death Mountain, bank 2 for East Hyrule and Maze Island, `$8409`).
//!   Shuffling those bytes changes which scene each terrain uses.

use crate::flags::EncounterRate;
use crate::rng::Rng;
use crate::rom::Rom;
use crate::RandoError;

/// Wave-timer table entries (grass .. lava), bank 0.
const TIMER_TABLE: u16 = 0x8240;
const TIMER_COUNT: u16 = 6;
/// Spawn routine entry, bank 0.
const SPAWN: u16 = 0x8284;
/// Scene selector table, banks 1 and 2.
const SCENES: u16 = 0x8409;

/// Apply an encounter rate to the whole game.
pub fn set_rate(rom: &mut Rom, rate: EncounterRate) -> Result<(), RandoError> {
    match rate {
        EncounterRate::Normal | EncounterRate::Random => {}
        EncounterRate::None => {
            if rom.read_cpu(0, SPAWN)? != 0xAD {
                return Err(RandoError::Rom("unexpected encounter spawn routine".into()));
            }
            rom.write_cpu(0, SPAWN, &[0x60])?;
        }
        EncounterRate::Half => {
            for k in 0..TIMER_COUNT {
                let v = rom.read_cpu(0, TIMER_TABLE + k)?;
                rom.write_cpu(0, TIMER_TABLE + k, &[v.saturating_mul(2)])?;
            }
        }
    }
    Ok(())
}

/// Apply a rate per region (West Hyrule, Death Mountain and Maze Island,
/// East Hyrule). Equal rates use [`set_rate`]; otherwise two small hooks
/// in bank 0 (our own code in bank 0's free space) look the region up:
/// the spawn routine's first instruction becomes a call that returns from
/// the routine for "None" regions, and the timer reload doubles the wait
/// for "Half" regions.
pub fn set_rates(rom: &mut Rom, rates: [EncounterRate; 3]) -> Result<(), RandoError> {
    if rates.iter().all(|&r| r == rates[0]) {
        return set_rate(rom, rates[0]);
    }
    if rom.read_slice(rom.cpu_offset(0, SPAWN)?, 3)? != [0xAD, 0x06, 0x07]
        || rom.read_slice(rom.cpu_offset(0, 0x82B4)?, 6)? != [0xB9, 0x3F, 0x82, 0x8D, 0x16, 0x05]
    {
        return Err(RandoError::Rom("unexpected encounter spawn routine".into()));
    }
    let flag = |want: EncounterRate| -> String {
        rates
            .iter()
            .map(|&r| if r == want { "1" } else { "0" })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let body = format!(
        "
        gate:
            tya
            pha
            ldy $0706
            lda none_t,y
            bne skip
            pla
            tay
            lda $0706
            rts
        skip:
            pla
            tay
            pla
            pla
            rts
        half:
            ldx $0706
            lda half_t,x
            beq keep
            lda $823F,y
            asl a
            sta $0516
            rts
        keep:
            lda $823F,y
            sta $0516
            rts
        none_t: .byte {}
        half_t: .byte {}
        ",
        flag(EncounterRate::None),
        flag(EncounterRate::Half)
    );
    let probe = crate::asm::Assembler::new().assemble(&format!(".org $8000\n{body}"))?;
    let len = probe.bytes().len();
    let at = rom.alloc_vanilla(0, len)?;
    let out = crate::asm::Assembler::new().assemble(&format!(".org ${at:04X}\n{body}"))?;
    rom.write_cpu(0, at, &out.bytes())?;
    let gate = out
        .symbol("gate")
        .ok_or_else(|| RandoError::Other("encounter hook".into()))?;
    let half = out
        .symbol("half")
        .ok_or_else(|| RandoError::Other("encounter hook".into()))?;
    rom.write_cpu(0, SPAWN, &[0x20, gate as u8, (gate >> 8) as u8])?;
    rom.write_cpu(
        0,
        0x82B4,
        &[0x20, half as u8, (half >> 8) as u8, 0xEA, 0xEA, 0xEA],
    )?;
    Ok(())
}

/// Shuffle the scene selectors of one bank (1 = West Hyrule and Death
/// Mountain, 2 = East Hyrule and Maze Island). Desert, grass, forest, swamp
/// and grave (north and south) always take part; roads join with
/// `roads`, lava with `lava`.
pub fn shuffle_scenes(
    rom: &mut Rom,
    bank: u8,
    rng: &mut Rng,
    roads: bool,
    lava: bool,
) -> Result<Vec<(usize, u8)>, RandoError> {
    let mut idx: Vec<u16> = (0..10).collect();
    if roads {
        idx.extend([10, 11]);
    }
    if lava {
        idx.extend([12, 13]);
    }
    let vals: Vec<u8> = idx
        .iter()
        .map(|&i| rom.read_cpu(bank, SCENES + i))
        .collect::<Result<_, _>>()?;
    let mut shuffled = vals.clone();
    rng.shuffle(&mut shuffled);
    let mut out = Vec::new();
    for (k, &i) in idx.iter().enumerate() {
        rom.write_cpu(bank, SCENES + i, &[shuffled[k]])?;
        out.push((usize::from(i), shuffled[k]));
    }
    Ok(out)
}

/// Spoiler name of a selector index.
#[must_use]
pub fn selector_name(i: usize) -> String {
    const T: [&str; 7] = [
        "desert",
        "grass",
        "forest",
        "swamp",
        "graveyard",
        "road",
        "lava",
    ];
    format!(
        "{} ({})",
        T.get(i / 2).unwrap_or(&"?"),
        if i.is_multiple_of(2) {
            "north"
        } else {
            "south"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom() -> Rom {
        let mut r = Rom::from_body(&vec![0u8; crate::rom::VANILLA_BODY_LEN]).unwrap();
        r.write_cpu(0, SPAWN, &[0xAD]).unwrap();
        r.write_cpu(0, TIMER_TABLE, &[0x20, 0x18, 0x18, 0x20, 0x09, 0x03])
            .unwrap();
        for b in 1..=2 {
            let v: Vec<u8> = (0..14).map(|i| 0x50 + i).collect();
            r.write_cpu(b, SCENES, &v).unwrap();
        }
        r
    }

    #[test]
    fn rates() {
        let mut r = rom();
        set_rate(&mut r, EncounterRate::Half).unwrap();
        assert_eq!(r.read_cpu(0, TIMER_TABLE).unwrap(), 0x40);
        assert_eq!(r.read_cpu(0, TIMER_TABLE + 5).unwrap(), 0x06);
        let mut r = rom();
        set_rate(&mut r, EncounterRate::None).unwrap();
        assert_eq!(r.read_cpu(0, SPAWN).unwrap(), 0x60);
    }

    #[test]
    fn mixed_rates_hook_the_spawn_routine() {
        let mut r = rom();
        r.write_cpu(0, SPAWN, &[0xAD, 0x06, 0x07]).unwrap();
        r.write_cpu(0, 0x82B4, &[0xB9, 0x3F, 0x82, 0x8D, 0x16, 0x05])
            .unwrap();
        set_rates(
            &mut r,
            [
                EncounterRate::None,
                EncounterRate::Half,
                EncounterRate::Normal,
            ],
        )
        .unwrap();
        assert_eq!(r.read_cpu(0, SPAWN).unwrap(), 0x20);
        assert_eq!(r.read_cpu(0, 0x82B4).unwrap(), 0x20);
        // Same rates everywhere: the plain patch.
        let mut r = rom();
        set_rates(&mut r, [EncounterRate::None; 3]).unwrap();
        assert_eq!(r.read_cpu(0, SPAWN).unwrap(), 0x60);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = rom();
        shuffle_scenes(&mut r, 1, &mut Rng::new(3), false, false).unwrap();
        let mut v: Vec<u8> = (0..10)
            .map(|i| r.read_cpu(1, SCENES + i).unwrap())
            .collect();
        v.sort_unstable();
        assert_eq!(v, (0..10).map(|i| 0x50 + i).collect::<Vec<u8>>());
        // Roads and lava untouched.
        assert_eq!(r.read_cpu(1, SCENES + 10).unwrap(), 0x5A);
        assert_eq!(r.read_cpu(1, SCENES + 13).unwrap(), 0x5D);
    }
}
