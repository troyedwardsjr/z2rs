//! `towns` module: wizard shortcuts and the New Kasuto basement requirement.
//!
//! Options owned: [`crate::flags::TownFlags`] (`ctx.flags.towns`).
//!
//! Catalog: section 04 (towns, 8.3) and section 06 (shorten wizards).
//!
//! * **New Kasuto requirement** (`randomize_new_kasuto_jar_requirements`):
//!   the old woman's basement in New Kasuto opens once Link holds enough
//!   magic containers. The game sets the "enough containers" bit when a
//!   container pickup brings the count to the threshold (the `CPY #$07`
//!   operand at fixed-bank `$E7D8`, inside `bank7_get_item`, which always
//!   runs as ROM code). This module rolls 5-7, patches that operand and
//!   the digit in her line (dialog 90), and records the value in
//!   [`crate::State::new_kasuto_containers`] for the item logic.
//! * **Shorter wizard visits** (`shorten_wizards`): every wizard, the two
//!   stab teachers and the New Kasuto basement are reached through an
//!   intermediate room. The street door is pointed straight at the inner
//!   room and the inner room's exit straight back at that street door. The
//!   room numbers are discovered from the player's ROM (town connection
//!   tables in bank 3), not hardcoded. Gates stay as they are: the quest
//!   townsfolk still have to lead Link to the door.
//!
//! With default options this module leaves `ctx.rom` untouched.

use crate::{text, Ctx, RandoError};

/// Fixed-bank address of the container threshold operand.
pub const KASUTO_THRESHOLD_ADDR: u16 = 0xE7D8;
/// Dialog index of the New Kasuto basement line ("... 7 magic containers").
pub const KASUTO_DIALOG: usize = 90;
/// Vanilla threshold.
pub const VANILLA_KASUTO_CONTAINERS: u8 = 7;

/// Town map left/right connection table (bank 3): 4 bytes per map, target
/// map in bits 2-7, entry point in bits 0-1.
pub const TOWN_CONNECTIONS: u16 = 0x871B;
/// Town map door table (bank 3): 4 bytes per map, one per door slot.
pub const TOWN_DOORS: u16 = 0x8817;
/// Bank holding the town tables.
pub const TOWN_BANK: u8 = 3;
/// Outdoor town maps (street screens).
pub const STREET_MAPS: std::ops::RangeInclusive<u8> = 1..=24;
/// Inner rooms behind an intermediate room: the eight wizards (36-43),
/// the downward-stab teacher (44), the upward-stab teacher (45, entered by
/// falling, so only its exit changes) and the New Kasuto basement (46).
pub const INNER_ROOMS: std::ops::RangeInclusive<u8> = 36..=46;
/// The inner room Link drops into; its entrance is left alone.
pub const DROP_IN_ROOM: u8 = 45;

/// Run this module's part of the pipeline.
pub fn apply(ctx: &mut Ctx) -> Result<(), RandoError> {
    if ctx.flags.towns.randomize_new_kasuto_jar_requirements {
        let n = ctx.rng.range_u8(5, 7);
        match set_kasuto_requirement(ctx, n) {
            Ok(()) => ctx.spoiler.line(
                "Towns",
                format!("New Kasuto basement: {n} magic containers"),
            ),
            // Only a synthetic test image lacks the vanilla bytes.
            Err(RandoError::Rom(m) | RandoError::Text(m)) => {
                ctx.log(format!("towns: New Kasuto requirement skipped: {m}"));
            }
            Err(e) => return Err(e),
        }
    }
    if ctx.flags.towns.shorten_wizards {
        match shorten_wizards(&mut ctx.rom) {
            Ok(changed) => ctx.spoiler.line(
                "Towns",
                format!("Shorter wizard visits: {changed} rooms rewired"),
            ),
            Err(RandoError::Rom(m)) => ctx.log(format!("towns: wizard shortcuts skipped: {m}")),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Patch the New Kasuto threshold and the matching dialog digit.
pub fn set_kasuto_requirement(ctx: &mut Ctx, n: u8) -> Result<(), RandoError> {
    if !(1..=8).contains(&n) {
        return Err(RandoError::Other(format!("bad container count {n}")));
    }
    let cur = ctx.rom.read_cpu(7, KASUTO_THRESHOLD_ADDR)?;
    if cur != VANILLA_KASUTO_CONTAINERS {
        return Err(RandoError::Rom(format!(
            "New Kasuto threshold at ${KASUTO_THRESHOLD_ADDR:04X} is {cur}, expected 7"
        )));
    }
    let t = text::table(ctx)?;
    let mut msg = t.bytes(KASUTO_DIALOG).to_vec();
    let digit = msg
        .iter()
        .position(|b| (0xD0..=0xD9).contains(b))
        .ok_or_else(|| RandoError::Text("New Kasuto line has no digit".into()))?;
    msg[digit] = 0xD0 + n;
    t.set_bytes(KASUTO_DIALOG, msg)?;
    ctx.rom.write_cpu(7, KASUTO_THRESHOLD_ADDR, &[n])?;
    ctx.state.new_kasuto_containers = Some(n);
    Ok(())
}

/// One rewired inner room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    /// Inner room (wizard, teacher, basement).
    pub room: u8,
    /// Street map whose door used to lead to the intermediate room.
    pub street: u8,
    /// Door slot on that street map.
    pub slot: u8,
}

/// Find the shortcuts in the town tables of `rom`.
pub fn find_shortcuts(rom: &crate::rom::Rom) -> Result<Vec<Shortcut>, RandoError> {
    let mut out = Vec::new();
    for room in INNER_ROOMS {
        let exit = rom.read_cpu(TOWN_BANK, TOWN_CONNECTIONS + 4 * u16::from(room))?;
        let middle = exit >> 2;
        // The middle room must lead on to this room (an exit or a door).
        let mut leads_on = false;
        for k in 0..4u16 {
            let base = 4 * u16::from(middle) + k;
            let c = rom.read_cpu(TOWN_BANK, TOWN_CONNECTIONS + base)?;
            let d = rom.read_cpu(TOWN_BANK, TOWN_DOORS + base)?;
            leads_on |= c >> 2 == room || d >> 2 == room;
        }
        if !leads_on || middle == room {
            continue;
        }
        let mut found = None;
        'streets: for street in STREET_MAPS {
            for slot in 0..4u8 {
                let d = rom.read_cpu(
                    TOWN_BANK,
                    TOWN_DOORS + 4 * u16::from(street) + u16::from(slot),
                )?;
                if d != 0 && d < 0xFC && d >> 2 == middle {
                    found = Some((street, slot));
                    break 'streets;
                }
            }
        }
        if let Some((street, slot)) = found {
            out.push(Shortcut { room, street, slot });
        }
    }
    Ok(out)
}

/// Rewire the town tables; returns how many rooms changed.
pub fn shorten_wizards(rom: &mut crate::rom::Rom) -> Result<usize, RandoError> {
    let cuts = find_shortcuts(rom)?;
    if cuts.len() != INNER_ROOMS.count() {
        return Err(RandoError::Rom(format!(
            "town tables look unusual: only {} wizard shortcuts found",
            cuts.len()
        )));
    }
    for c in &cuts {
        if c.room != DROP_IN_ROOM {
            rom.write_cpu(
                TOWN_BANK,
                TOWN_DOORS + 4 * u16::from(c.street) + u16::from(c.slot),
                &[c.room << 2],
            )?;
        }
        rom.write_cpu(
            TOWN_BANK,
            TOWN_CONNECTIONS + 4 * u16::from(c.room),
            &[(c.street << 2) | c.slot],
        )?;
    }
    Ok(cuts.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::Rom;

    /// A blank image with a miniature town: street 1 slot 2 -> room 48,
    /// whose exit 1 leads to wizard 36, which exits back to 48.
    fn tiny_town() -> Rom {
        let mut rom = Rom::from_body(&vec![0u8; crate::rom::VANILLA_BODY_LEN]).unwrap();
        for (k, room) in INNER_ROOMS.enumerate() {
            let k = k as u8;
            let street = 1 + k;
            let middle = 48 + k;
            rom.write_cpu(
                TOWN_BANK,
                TOWN_DOORS + 4 * u16::from(street) + 2,
                &[middle << 2],
            )
            .unwrap();
            rom.write_cpu(
                TOWN_BANK,
                TOWN_CONNECTIONS + 4 * u16::from(middle) + 1,
                &[room << 2],
            )
            .unwrap();
            rom.write_cpu(
                TOWN_BANK,
                TOWN_CONNECTIONS + 4 * u16::from(room),
                &[(middle << 2) | 1],
            )
            .unwrap();
        }
        rom
    }

    #[test]
    fn shortcuts_rewire_doors_and_exits() {
        let mut rom = tiny_town();
        assert_eq!(shorten_wizards(&mut rom).unwrap(), 11);
        for (k, room) in INNER_ROOMS.enumerate() {
            let street = 1 + k as u8;
            let door = rom
                .read_cpu(TOWN_BANK, TOWN_DOORS + 4 * u16::from(street) + 2)
                .unwrap();
            if room == DROP_IN_ROOM {
                assert_eq!(door >> 2, 48 + k as u8, "drop-in entrance untouched");
            } else {
                assert_eq!(door, room << 2);
            }
            let exit = rom
                .read_cpu(TOWN_BANK, TOWN_CONNECTIONS + 4 * u16::from(room))
                .unwrap();
            assert_eq!(exit, (street << 2) | 2);
        }
    }

    #[test]
    fn unusual_tables_are_refused() {
        let mut rom = Rom::from_body(&vec![0u8; crate::rom::VANILLA_BODY_LEN]).unwrap();
        assert!(shorten_wizards(&mut rom).is_err());
    }

    /// ROM-gated: the shortcuts found in the real tables are the expected
    /// eleven, and every flag combination keeps the rest of the ROM.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn rom_town_options() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let rom = Rom::from_body(&body).unwrap();
        let cuts = find_shortcuts(&rom).unwrap();
        let rooms: Vec<u8> = cuts.iter().map(|c| c.room).collect();
        assert_eq!(rooms, INNER_ROOMS.collect::<Vec<_>>());
        for c in &cuts {
            assert!(STREET_MAPS.contains(&c.street));
        }

        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..24 {
            let mut f = crate::flags::Flags::default();
            f.towns.randomize_new_kasuto_jar_requirements = true;
            f.towns.shorten_wizards = true;
            let out = crate::randomize(&body, &format!("towns{seed}"), &f).unwrap();
            let r = Rom::from_body(&out.body).unwrap();
            let n = r.read_cpu(7, KASUTO_THRESHOLD_ADDR).unwrap();
            assert!((5..=7).contains(&n));
            seen.insert(n);
            let msgs = text::read_vanilla_messages(&r).unwrap();
            let line = text::decode(&msgs[KASUTO_DIALOG]);
            assert!(line.contains(&format!("{n} MAGIC")), "{line}");
            // The other 97 messages are unchanged.
            let van = text::read_vanilla_messages(&rom).unwrap();
            for (i, (a, b)) in msgs.iter().zip(&van).enumerate() {
                if i != KASUTO_DIALOG {
                    assert_eq!(a, b, "message {i}");
                }
            }
            // Only the threshold changed in the fixed bank.
            let want: Vec<u16> = if n == VANILLA_KASUTO_CONTAINERS {
                Vec::new()
            } else {
                vec![KASUTO_THRESHOLD_ADDR]
            };
            assert_eq!(out.fixed_bank_changes, want);
            assert!(out.spoiler.contains("New Kasuto basement"));
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
    }
}
