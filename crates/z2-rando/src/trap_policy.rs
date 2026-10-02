//! Which ported routines must run as patched 6502 code.
//!
//! z2rs runs the game as a hybrid: many fixed-bank (`$C000-$FFFF`) routines
//! are replaced by Rust ports ("traps", `z2-core::traps`) that fire when the
//! CPU jumps to their entry address. A trap reimplements the **vanilla**
//! bytes, so a randomizer patch inside a trapped routine would be silently
//! ignored. Patches to banks 0-6 (and 8-14) always execute as ROM code and
//! need nothing here.
//!
//! Policy: diff the patched fixed bank against the vanilla one, and untrap
//! every registered trap whose code range overlaps a changed byte. A trap's
//! range is conservative:
//!
//! * the ledger routine (`ports.toml`) that contains the trap address,
//!   up to the next ledger label (the ledger names only some labels, so a
//!   routine's own size can stop short of its unnamed tail),
//! * the following routine when the previous one can fall through into it
//!   (its last decoded instruction is not `RTS`/`RTI`/`JMP`),
//! * and, transitively, every fixed-bank routine it calls (`JSR`/`JMP`
//!   edges from the ledger), because a port usually inlines its callees.
//!
//! Untrapping is always safe (the interpreter runs the real bytes; traps are
//! exact ports), only slower. Bytes listed in [`TRAP_HONORED_BYTES`] are
//! read by the ports themselves at run time (immediate operands the ports
//! fetch through the bus), so changing them does not untrap anything.
//!
//! Frontends: collect the fixed-bank trap addresses (`addr >= $C000`) from
//! the game's trap table, call [`untrap_list`] with the vanilla and patched
//! bodies, `set_untrapped(addr, true)` each result, and fold the list into
//! the netplay session identity so both peers agree.

use crate::rom::{prg_units_for_body_len, PRG_BANK_LEN, VANILLA_FIXED_BANK_OFFSET};

/// One fixed-bank routine from the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerRoutine {
    /// Entry address.
    pub addr: u16,
    /// Bytes up to the next ledger label.
    pub size: u16,
    /// Code (true) or data/jump table (false).
    pub code: bool,
    /// Fixed-bank routines it calls or jumps to.
    pub callees: &'static [u16],
}

/// Bank-7 ledger, generated at build time from `ports.toml` (empty when the
/// ledger is not in the source tree).
pub static BANK7_LEDGER: &[LedgerRoutine] = include!(concat!(env!("OUT_DIR"), "/bank7_ledger.rs"));

/// Fixed-bank bytes that trapped ports read through the bus at run time,
/// so editing them is honoured without untrapping. Keep in step with the
/// ports in `z2-core` (each entry names its reader).
pub const TRAP_HONORED_BYTES: &[(u16, &str)] = &[
    (0xC359, "title_traps::tt_lives3: starting lives immediate"),
    (0xDE0F, "enemy_traps boss key: key X position immediate"),
    (0xDE19, "enemy_traps boss key: dropped item immediate"),
    (
        0xCCB0,
        "sideview_traps::sv_go_outside: hidden palace exit row immediate",
    ),
    (
        0xCCC4,
        "sideview_traps::sv_go_outside: hidden town column immediate",
    ),
    (
        0xCCCB,
        "sideview_traps::sv_go_outside: hidden town exit row immediate",
    ),
    (0xDF9C, "sideview_traps::ow_chop: hidden town row immediate"),
    (
        0xDFA2,
        "sideview_traps::ow_chop: hidden town column immediate",
    ),
    (0xDFA6, "sideview_traps::ow_chop: hidden town tile 1"),
    (0xDFAB, "sideview_traps::ow_chop: hidden town tile 2"),
    (0xDFB0, "sideview_traps::ow_chop: hidden town tile 3"),
    (0xDFB5, "sideview_traps::ow_chop: hidden town tile 4"),
    (0xE2FE, "player_traps::pl_link_hit: small experience drain"),
    (0xE300, "player_traps::pl_link_hit: big-drain enemy id"),
    (0xE304, "player_traps::pl_link_hit: big experience drain"),
    (0xE8A0, "enemy_traps::en_death: kills per drop"),
    // The death-flash colour table sits right after the ported item-spawn
    // routine (`LC9AF`, whose ledger size stops one byte short), so the
    // ledger lumps it in; only the unported death sequence at `$C9F1`
    // reads it.
    (0xC9EA, "death flash colour 0 (read by unported $C9FD)"),
    (0xC9EB, "death flash colour 1 (read by unported $C9FD)"),
    (0xC9EC, "death flash colour 2 (read by unported $C9FD)"),
    (0xC9ED, "death flash colour 3 (read by unported $C9FD)"),
];

/// The vanilla-layout fixed bank of a headerless body (either layout).
fn fixed_bank(body: &[u8]) -> Option<&[u8]> {
    let units = usize::from(prg_units_for_body_len(body.len())?);
    let base = (units - 1) * PRG_BANK_LEN;
    Some(&body[base..base + PRG_BANK_LEN])
}

/// CPU addresses (`$C000-$FFFF`) where the patched fixed bank differs from
/// the vanilla one. Empty when either body has an unknown layout.
#[must_use]
pub fn changed_fixed_bank_addrs(vanilla_body: &[u8], patched_body: &[u8]) -> Vec<u16> {
    let (Some(a), Some(b)) = (fixed_bank(vanilla_body), fixed_bank(patched_body)) else {
        return Vec::new();
    };
    a.iter()
        .zip(b)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, _)| 0xC000 + i as u16)
        .collect()
}

fn routine_index_containing(addr: u16) -> Option<usize> {
    let i = BANK7_LEDGER.partition_point(|r| r.addr <= addr);
    (i > 0).then(|| i - 1)
}

/// Whether the routine at `idx` may run into the next one: walk its
/// instructions in the vanilla bytes and look at the last one.
fn falls_through(idx: usize, fixed: &[u8]) -> bool {
    let r = BANK7_LEDGER[idx];
    if !r.code {
        return false;
    }
    let start = usize::from(r.addr - 0xC000);
    let end = start + usize::from(r.size);
    let mut pc = start;
    let mut last_op = None;
    while pc < end && pc < fixed.len() {
        let op = fixed[pc];
        let Some(len) = crate::asm::opcode_len(op) else {
            return true; // data inside code: be conservative
        };
        last_op = Some(op);
        pc += usize::from(len);
    }
    !(pc == end && last_op.is_some_and(crate::asm::ends_flow))
}

/// Code ranges `[start, end)` a trap at `trap_addr` stands for (see the
/// module docs). `vanilla_fixed` is the vanilla `$C000-$FFFF` bank.
#[must_use]
pub fn trap_coverage(trap_addr: u16, vanilla_fixed: &[u8]) -> Vec<(u16, u32)> {
    let Some(first) = routine_index_containing(trap_addr) else {
        return vec![(trap_addr, u32::from(trap_addr) + 1)];
    };
    let mut seen = vec![false; BANK7_LEDGER.len()];
    let mut work = vec![first];
    let mut out = Vec::new();
    while let Some(i) = work.pop() {
        if seen[i] {
            continue;
        }
        seen[i] = true;
        let r = BANK7_LEDGER[i];
        // The ledger lists only named labels, so the bytes up to the next
        // label (unnamed branch targets inside the routine) belong to it.
        let own_end = u32::from(r.addr) + u32::from(r.size);
        // Registered padding (`rom::FREE_SPACE_REGISTRY`) is not part of it.
        let mut end = BANK7_LEDGER
            .get(i + 1)
            .map_or(own_end, |n| own_end.max(u32::from(n.addr)));
        for r in crate::rom::FREE_SPACE_REGISTRY {
            let (b, start) = (r.bank, u32::from(r.start));
            if b == 7 && start >= own_end && start < end {
                end = start;
            }
        }
        out.push((r.addr, end));
        if falls_through(i, vanilla_fixed) && i + 1 < BANK7_LEDGER.len() {
            work.push(i + 1);
        }
        for &c in r.callees {
            if let Some(j) = BANK7_LEDGER.iter().position(|x| x.addr == c) {
                work.push(j);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Fixed-bank traps (from `trap_addrs`) that must be disabled for the
/// patched body to behave as patched. Sorted, without duplicates.
#[must_use]
pub fn untrap_list(trap_addrs: &[u16], vanilla_body: &[u8], patched_body: &[u8]) -> Vec<u16> {
    let changed: Vec<u16> = changed_fixed_bank_addrs(vanilla_body, patched_body)
        .into_iter()
        .filter(|a| !TRAP_HONORED_BYTES.iter().any(|(h, _)| h == a))
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let mut traps: Vec<u16> = trap_addrs
        .iter()
        .copied()
        .filter(|&a| a >= 0xC000)
        .collect();
    traps.sort_unstable();
    traps.dedup();
    if BANK7_LEDGER.is_empty() {
        // No ledger in this source tree: untrap everything in the fixed bank.
        return traps;
    }
    let Some(vanilla_fixed) =
        vanilla_body.get(VANILLA_FIXED_BANK_OFFSET..VANILLA_FIXED_BANK_OFFSET + PRG_BANK_LEN)
    else {
        return traps;
    };
    traps
        .into_iter()
        .filter(|&t| {
            trap_coverage(t, vanilla_fixed)
                .iter()
                .any(|&(s, e)| changed.iter().any(|&c| c >= s && u32::from(c) < e))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::VANILLA_BODY_LEN;

    #[test]
    fn ledger_is_sorted_and_in_the_fixed_bank() {
        assert!(!BANK7_LEDGER.is_empty(), "ports.toml is in this tree");
        assert!(BANK7_LEDGER.windows(2).all(|w| w[0].addr < w[1].addr));
        assert!(BANK7_LEDGER.iter().all(|r| r.addr >= 0xC000));
        assert_eq!(BANK7_LEDGER[0].addr, 0xC000);
    }

    #[test]
    fn identical_bodies_untrap_nothing() {
        let body = vec![0u8; VANILLA_BODY_LEN];
        assert!(changed_fixed_bank_addrs(&body, &body).is_empty());
        assert!(untrap_list(&[0xC358, 0xD000], &body, &body).is_empty());
    }

    #[test]
    fn changed_byte_untraps_the_containing_routine_only() {
        let vanilla = vec![0x60u8; VANILLA_BODY_LEN]; // all RTS: nothing falls through
        let mut patched = vanilla.clone();
        // Pick a code routine that has no callees and change its first byte.
        let r = BANK7_LEDGER
            .iter()
            .find(|r| r.code && r.callees.is_empty() && r.size > 1)
            .copied()
            .unwrap();
        patched[VANILLA_FIXED_BANK_OFFSET + usize::from(r.addr - 0xC000)] = 0xEA;
        assert_eq!(changed_fixed_bank_addrs(&vanilla, &patched), vec![r.addr]);
        let other = BANK7_LEDGER
            .iter()
            .rev()
            .find(|x| x.callees.is_empty() && x.addr > r.addr + r.size)
            .unwrap()
            .addr;
        let out = untrap_list(&[r.addr, other, 0x8000], &vanilla, &patched);
        assert_eq!(out, vec![r.addr]);
    }

    #[test]
    fn honored_bytes_do_not_untrap() {
        let vanilla = vec![0x60u8; VANILLA_BODY_LEN];
        let mut patched = vanilla.clone();
        patched[VANILLA_FIXED_BANK_OFFSET + (0xC359 - 0xC000)] = 5;
        assert!(untrap_list(&[0xC358], &vanilla, &patched).is_empty());
    }

    #[test]
    fn expanded_body_compares_against_its_last_bank() {
        let vanilla = vec![0u8; VANILLA_BODY_LEN];
        let mut rom = crate::rom::Rom::from_body(&vanilla).unwrap();
        rom.expand_prg();
        assert!(changed_fixed_bank_addrs(&vanilla, &rom.body()).is_empty());
        rom.write_cpu(0, 0xC010, &[1]).unwrap();
        assert_eq!(
            changed_fixed_bank_addrs(&vanilla, &rom.body()),
            vec![0xC010]
        );
    }

    /// ROM-gated: the bytes the ports read hold the values the ports used to
    /// hard-code, so reading them changes nothing on the vanilla ROM.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn honored_bytes_hold_the_vanilla_operands() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let fixed = &body[VANILLA_FIXED_BANK_OFFSET..VANILLA_FIXED_BANK_OFFSET + PRG_BANK_LEN];
        let want: [(u16, u8); 20] = [
            (0xC359, 0x03),
            (0xDE0F, 0x80),
            (0xDE19, 0x08),
            (0xCCB0, 0x66),
            (0xCCC4, 0x3D),
            (0xCCCB, 0x51),
            (0xDF9C, 0x33),
            (0xDFA2, 0x3E),
            (0xDFA6, 0x5C),
            (0xDFAB, 0x5D),
            (0xDFB0, 0x5E),
            (0xDFB5, 0x5F),
            (0xE2FE, 0x0A),
            (0xE300, 0x06),
            (0xE304, 0x14),
            (0xE8A0, 0x06),
            (0xC9EA, 0x12),
            (0xC9EB, 0x16),
            (0xC9EC, 0x2A),
            (0xC9ED, 0x16),
        ];
        assert_eq!(want.len(), TRAP_HONORED_BYTES.len());
        for ((addr, v), (h, what)) in want.iter().zip(TRAP_HONORED_BYTES) {
            assert_eq!(addr, h, "{what}");
            assert_eq!(fixed[usize::from(addr - 0xC000)], *v, "{what}");
        }
    }

    #[test]
    fn unnamed_tail_of_a_routine_is_covered_but_padding_is_not() {
        let vanilla = vec![0x60u8; VANILLA_BODY_LEN];
        // `bank7_monster_death` ($E880) has a 17-byte ledger entry; its drop
        // roll at $E8AD sits in the unnamed tail before the next label.
        let mut patched = vanilla.clone();
        patched[VANILLA_FIXED_BANK_OFFSET + (0xE8AD - 0xC000)] = 0x20;
        assert_eq!(untrap_list(&[0xE880], &vanilla, &patched), vec![0xE880]);
        // Code placed in listed padding untraps nothing.
        for r in crate::rom::FREE_SPACE_REGISTRY {
            let (b, start) = (r.bank, r.start);
            if b != 7 {
                continue;
            }
            let mut patched = vanilla.clone();
            patched[VANILLA_FIXED_BANK_OFFSET + usize::from(start - 0xC000)] = 0xEA;
            let traps: Vec<u16> = BANK7_LEDGER.iter().map(|r| r.addr).collect();
            assert!(
                untrap_list(&traps, &vanilla, &patched).is_empty(),
                "padding at ${start:04X}"
            );
        }
    }

    #[test]
    fn callers_of_a_patched_callee_are_untrapped() {
        let vanilla = vec![0x60u8; VANILLA_BODY_LEN];
        let caller = BANK7_LEDGER
            .iter()
            .find(|r| r.code && !r.callees.is_empty())
            .copied()
            .unwrap();
        let callee = caller.callees[0];
        let mut patched = vanilla.clone();
        patched[VANILLA_FIXED_BANK_OFFSET + usize::from(callee - 0xC000)] = 0xEA;
        let out = untrap_list(&[caller.addr], &vanilla, &patched);
        assert_eq!(out, vec![caller.addr]);
    }
}
