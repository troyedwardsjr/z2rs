//! Battery-save format: SRAM layout, writer/loader, validity + recovery.
//!
//! Self-contained (no intra-crate imports) so `rustc --edition 2021 --test`
//! compiles this file standalone. Trap shims live in `title_traps.rs`
//! (interp-gated); RAM-side slot logic (reset/presence/name/lives) lives in
//! [`crate::save`].
//!
//! # Misfiling note (read `prg0.asm` first)
//!
//! Some notes call this "bank 0", but `src/prg0.asm` holds overworld,
//! sideview, spell, pause/level-up and game-over *text* code — the entire
//! title / file-select / SRAM / ending engine lives in bank 5
//! (`src/prg5.asm`, PRG `$8000-$BFFF` when mapped). All `bank5_*` labels
//! below cite bank-5 CPU addresses; bank-0 residents cite `bank0_*`.
//!
//! # SRAM layout (`$6000-$7FFF`, 8 KiB battery RAM)
//!
//! `Game.wram` mirrors this whole window (`wram[i]` = `$6000+i`). Three
//! save slots share one header protocol. Per slot `s` (0-2), resolved by
//! `LBA6C` (bank 5 `$BA6C`) from `bank5_pointer_table7` (bank 5 `$BAC5`):
//!
//! ```text
//! slot  header  part1 (50 B)      part2 (224 B)     bak1 (50 B)       bak2 (224 B)
//! 0     $7400   $7402-$7433       $749A-$7579       $6002-$6033       $6098-$6177
//! 1     $7400→$7498  $7434-$7465  $757A-$7659       $6034-$6065       $6178-$6257
//! 2     $773A   $7466-$7497       $765A-$7739       $6066-$6097       $6258-$6337
//! ```
//!
//! (Slot 1's header is `$7498`, slot 2's `$773A`; each header is followed
//! by one reserved pad byte — the data pointers skip `+2`.)
//!
//! * **part1** (50 B, `Y = $31..0`, cites `LB911` `$B911` / `LBA6C` `$BA6C`)
//!   mirrors CPU RAM `$777-$7A8`: attack/magic/life levels, `$77A`,
//!   spells `$77B-$782`, containers `$783-$784`, items `$785-$78C`,
//!   crystals `$78D-$792`, keys `$793`, crystals-left `$794`, quest flags
//!   `$795-$79E`, deaths `$79F`, second-quest `$7A0`, name `$7A1-$7A8`.
//! * **part2** (224 B, `$0600,X` walk to `$BBF5`, cites `LB91D` `$B91D`)
//!   mirrors CPU RAM `$600-$6DF`: item-presence bits (West `$600-$61F`,
//!   DM/MI `$620-$63F`, East `$640-$65F`, towns `$660-$67F`, palaces-A
//!   `$680-$69F`, palaces-B `$6A0-$6BF`, Great Palace `$6C0-$6DF`).
//! * **bak1/bak2** are the staged copies: the writer stores RAM→backup
//!   first, then backup→main, so a torn write always leaves one good copy.
//!
//! # Header protocol (cites `bank5_code27` `$B960`, `LB9CA` `$B9CA`)
//!
//! | byte | meaning | handler |
//! |---|---|---|
//! | `$A5` | valid | keep (`LB9A3` `$B9A3`: next slot) |
//! | `$5A` | staged backup, main untouched | mark `$A5` (`LB99D` `$B99D`) |
//! | `$69` | torn main copy | restore backup→main, mark `$A5` (`LB9A7` `$B9A7`) |
//! | other (`$FF` blank, garbage) | uninitialised | init from beginning values + ROM item bits, mark `$A5` (`LB978` `$B978`) |
//!
//! Save (`LB9CA` `$B9CA`): mark `$5A` → RAM→backup (`LB9CA`/`LB9DE`) →
//! mark `$69` → backup→main (`LB9F8`/`LBA00`) → mark `$A5` (`LBA13`
//! `$BA13`). `LBA40` (`$BA40`) is the stage half alone (mark `$5A`,
//! main→backup); `LBA18` (`$BA18`) the commit half (mark `$69`,
//! backup→main, mark `$A5`). `LBAB8` (`$BAB8`) is the 6-pointer ladder
//! (`$00/$02/$04/$06/$08/$0A += 1` with carry) both copy loops share.
//!
//! # Preserved quirks
//!
//! * The `$69` restore path copies backup→main even when the backup itself
//!   was never written (blank `$FF` SRAM restores `$FF` over main, then
//!   marks `$A5`): no checksum exists anywhere — validity is one byte.
//! * A torn write that dies *between* the `$5A` mark and the backup copy
//!   finalizes a stale-but-untouched main as `$A5` (data loss is silent).
//! * `LBA18`/`LBA40` take the slot in `A` (`TXA` at the call sites,
//!   `$B3E1`/`$B40C`); [`commit_slot`] / [`stage_slot`] take it explicitly.
//!
//! # Gaps (honest)
//!
//! * SRAM bus behaviour (battery enable/disable around writes) is
//!   interpreter scope; these fns model the byte protocol only.
//! * z2se-edited saves load iff they follow this layout + `$A5` headers,
//!   which is exactly what z2se writes (cross-check is ROM-gated; see
//!   `tests/save_tests.rs`).

// ---------------------------------------------------------------------------
// Layout constants (CPU-view SRAM addresses; `wram` index = addr - $6000).
// ---------------------------------------------------------------------------

/// SRAM window base (`Game.wram[0]`).
pub const SRAM_BASE: u16 = 0x6000;
/// SRAM window length (8 KiB).
pub const SRAM_LEN: usize = 0x2000;

/// part1 length: `$777-$7A8` (50 B, `LDY #$31`, `$B911`/`$BA6C`).
pub const PART1_LEN: usize = 0x32;
/// part2 length: `$600-$6DF` (224 B, `$BBF5` walk end, `$B91D`).
pub const PART2_LEN: usize = 0xE0;

/// CPU RAM mirror of part1 (`$777-$7A8`).
pub const RAM_PART1: u16 = 0x0777;
/// CPU RAM mirror of part2 (`$600-$6DF`).
pub const RAM_PART2: u16 = 0x0600;

/// Header byte: slot valid (`LB99D`/`LBA13`, `$B99D`/`$BA13`).
pub const HDR_VALID: u8 = 0xA5;
/// Header byte: backup staged, main untouched (`LB9CA`/`LBA40`).
pub const HDR_STAGED: u8 = 0x5A;
/// Header byte: main copy torn, backup authoritative (`LB9DE`→`$B9F1`).
pub const HDR_TORN: u8 = 0x69;

/// Slot header addresses (`bank5_pointer_table7+$0C`, `$BADD/$BADF/$BAE1`).
pub const HDR_ADDR: [u16; 3] = [0x7400, 0x7498, 0x773A];
/// Slot part1 addresses (`bank5_pointer_table7+0`, `$BAC5/$BAC7/$BAC9`).
pub const PART1_ADDR: [u16; 3] = [0x7402, 0x7434, 0x7466];
/// Slot part2 addresses (`bank5_pointer_table7+6`, `$BACB/$BACD/$BACF`).
pub const PART2_ADDR: [u16; 3] = [0x749A, 0x757A, 0x765A];
/// Slot backup-part1 addresses (`bank5_pointer_table7+$0C`, `$BAD1/...`).
pub const BAK1_ADDR: [u16; 3] = [0x6002, 0x6034, 0x6066];
/// Slot backup-part2 addresses (`bank5_pointer_table7+$12`, `$BAD7/...`).
pub const BAK2_ADDR: [u16; 3] = [0x6098, 0x6178, 0x6258];

// ---------------------------------------------------------------------------
// Slot pointers (`LBA6C`, bank 5 `$BA6C` over `bank5_pointer_table7` `$BAC5`).
// ---------------------------------------------------------------------------

/// Resolved SRAM pointers for one slot (CPU-view addresses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotPointers {
    /// part1 main (`($00)`).
    pub part1: u16,
    /// part2 main (`($02)`).
    pub part2: u16,
    /// part1 backup (`($08)`).
    pub bak1: u16,
    /// part2 backup (`($0A)`).
    pub bak2: u16,
    /// header (`($0C)`).
    pub header: u16,
}

/// Resolve slot 0-2 pointers (`LBA6C` `$BA6C`); `None` for slot > 2.
///
/// The hardware indexes `bank5_pointer_table7` (`$BAC5`) with `A*2`
/// (`ASL : TAY`, `$BA6F`); slots past 2 read into the header words and
/// are rejected here (total fn).
pub const fn slot_pointers(slot: u8) -> Option<SlotPointers> {
    if slot > 2 {
        return None;
    }
    let s = slot as usize;
    Some(SlotPointers {
        part1: PART1_ADDR[s],
        part2: PART2_ADDR[s],
        bak1: BAK1_ADDR[s],
        bak2: BAK2_ADDR[s],
        header: HDR_ADDR[s],
    })
}

/// `wram` index for a CPU-view SRAM address (`None` outside `$6000-$7FFF`).
pub const fn sram_index(addr: u16) -> Option<usize> {
    if addr < SRAM_BASE || addr > 0x7FFF {
        return None;
    }
    Some((addr - SRAM_BASE) as usize)
}

// ---------------------------------------------------------------------------
// Header validity (`bank5_code27`, bank 5 `$B960-$B9A6`).
// ---------------------------------------------------------------------------

/// Classified header byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderState {
    /// `$A5`: valid, keep.
    Valid,
    /// `$5A`: backup staged, main untouched → finalize to `$A5`.
    Staged,
    /// `$69`: main torn → restore backup→main, then `$A5`.
    Torn,
    /// Anything else (blank `$FF`, garbage): initialise fresh.
    Blank(u8),
}

/// Classify one header byte (`$B968-$B974`: `CMP #$A5 / #$5A / #$69`).
pub const fn classify_header(b: u8) -> HeaderState {
    match b {
        HDR_VALID => HeaderState::Valid,
        HDR_STAGED => HeaderState::Staged,
        HDR_TORN => HeaderState::Torn,
        other => HeaderState::Blank(other),
    }
}

// ---------------------------------------------------------------------------
// Low-level copy helpers (all bounds-checked; `false` = out of range).
// ---------------------------------------------------------------------------

fn copy_within(sram: &mut [u8], dst: usize, src: usize, len: usize) -> bool {
    if dst.checked_add(len).is_none_or(|e| e > sram.len()) {
        return false;
    }
    if src.checked_add(len).is_none_or(|e| e > sram.len()) {
        return false;
    }
    // Overlap is impossible between the tabled regions, but `copy_within`
    // is overlap-safe anyway (matches the byte-at-a-time `($xx),y` loops).
    sram.copy_within(src..src + len, dst);
    true
}

fn read_from(sram: &[u8], src: usize, out: &mut [u8]) -> bool {
    if src.checked_add(out.len()).is_none_or(|e| e > sram.len()) {
        return false;
    }
    out.copy_from_slice(&sram[src..src + out.len()]);
    true
}

fn write_to(sram: &mut [u8], dst: usize, data: &[u8]) -> bool {
    if dst.checked_add(data.len()).is_none_or(|e| e > sram.len()) {
        return false;
    }
    sram[dst..dst + data.len()].copy_from_slice(data);
    true
}

// ---------------------------------------------------------------------------
// Save / load / stage / commit (`LB9CA` `$B9CA`, `LB911` `$B911`,
// `LBA40` `$BA40`, `LBA18` `$BA18`).
// ---------------------------------------------------------------------------

/// Full save of one slot (`LB9CA`, bank 5 `$B9CA-$BA17`).
///
/// `part1` = 50 RAM bytes for `$777-$7A8`, `part2` = 224 RAM bytes for
/// `$600-$6DF`, `sram` = the 8 KiB `$6000-$7FFF` window. Runs the exact
/// 4-phase protocol: mark `$5A` → RAM→backup → mark `$69` →
/// backup→main → mark `$A5`. Returns `false` (leaving `sram` untouched
/// only when the *slot* is bad — a bounds failure mid-protocol mirrors
/// the hardware's torn state) when the slot or any region is out of range.
pub fn save_slot(
    sram: &mut [u8],
    slot: u8,
    part1: &[u8; PART1_LEN],
    part2: &[u8; PART2_LEN],
) -> bool {
    let Some(p) = slot_pointers(slot) else {
        return false;
    };
    let (Some(h), Some(m1), Some(m2), Some(b1), Some(b2)) = (
        sram_index(p.header),
        sram_index(p.part1),
        sram_index(p.part2),
        sram_index(p.bak1),
        sram_index(p.bak2),
    ) else {
        return false;
    };
    if sram.len() < SRAM_LEN {
        return false;
    }
    // `$5A`: backup staged (`LBA40` head, `$BA46`).
    sram[h] = HDR_STAGED;
    // RAM→backup (`LB9CA` part1 `$B9D5`, part2 `LB9DE` `$B9DE`).
    if !write_to(sram, b1, part1) || !write_to(sram, b2, part2) {
        return false;
    }
    // `$69`: main copy torn (`$B9F1`).
    sram[h] = HDR_TORN;
    // backup→main (`LB9F8` `$B9F8`, `LBA00` `$BA00`).
    if !copy_within(sram, m1, b1, PART1_LEN) || !copy_within(sram, m2, b2, PART2_LEN) {
        return false;
    }
    // `$A5`: valid (`LBA13` `$BA13`).
    sram[h] = HDR_VALID;
    true
}

/// Load one slot into RAM buffers (`LB911`, bank 5 `$B911-$B930`).
///
/// Copies main part1 → `out_part1` (`LB914` `$B914`, `Y = $31..0`) and
/// main part2 → `out_part2` (the `LB91D` `$600,X` walk). No header check:
///
/// the caller (`LB2B4` `$B2B4`) loads whatever the slot holds.
pub fn load_slot(
    sram: &[u8],
    slot: u8,
    out_part1: &mut [u8; PART1_LEN],
    out_part2: &mut [u8; PART2_LEN],
) -> bool {
    let Some(p) = slot_pointers(slot) else {
        return false;
    };
    let (Some(m1), Some(m2)) = (sram_index(p.part1), sram_index(p.part2)) else {
        return false;
    };
    read_from(sram, m1, out_part1) && read_from(sram, m2, out_part2)
}

/// Stage half: mark `$5A`, copy main→backup (`LBA40`, bank 5 `$BA40-$BA6B`).
///
/// Takes the slot in `A` at the call sites (`TXA`, `$B3E1`/`$B40C`); the
/// `$A5` finalize is a later [`commit_slot`].
pub fn stage_slot(sram: &mut [u8], slot: u8) -> bool {
    let Some(p) = slot_pointers(slot) else {
        return false;
    };
    let (Some(h), Some(m1), Some(m2), Some(b1), Some(b2)) = (
        sram_index(p.header),
        sram_index(p.part1),
        sram_index(p.part2),
        sram_index(p.bak1),
        sram_index(p.bak2),
    ) else {
        return false;
    };
    if sram.len() < SRAM_LEN {
        return false;
    }
    sram[h] = HDR_STAGED;
    copy_within(sram, b1, m1, PART1_LEN) && copy_within(sram, b2, m2, PART2_LEN)
}

/// Commit half: mark `$69`, copy backup→main, mark `$A5` (`LBA18`,
/// bank 5 `$BA18-$BA3F`, run for `X = 2..0` by `bank5_code23` `$B3DF`).
pub fn commit_slot(sram: &mut [u8], slot: u8) -> bool {
    let Some(p) = slot_pointers(slot) else {
        return false;
    };
    let (Some(h), Some(m1), Some(m2), Some(b1), Some(b2)) = (
        sram_index(p.header),
        sram_index(p.part1),
        sram_index(p.part2),
        sram_index(p.bak1),
        sram_index(p.bak2),
    ) else {
        return false;
    };
    if sram.len() < SRAM_LEN {
        return false;
    }
    sram[h] = HDR_TORN;
    if !copy_within(sram, m1, b1, PART1_LEN) || !copy_within(sram, m2, b2, PART2_LEN) {
        return false;
    }
    sram[h] = HDR_VALID;
    true
}

// ---------------------------------------------------------------------------
// Boot validation + interrupted-save recovery (`bank5_code27` `$B960`).
// ---------------------------------------------------------------------------

/// Recovery outcome for one slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// `$A5`: untouched.
    Kept,
    /// `$5A`: main was untouched, finalized to `$A5` (`LB99D` `$B99D`).
    Finalized,
    /// `$69`: backup restored over main, marked `$A5` (`LB9A7` `$B9A7`).
    Restored,
    /// Blank/garbage: fresh init from beginning values + ROM item bits,
    /// marked `$A5` (`LB978` `$B978`).
    Initialized,
}

/// Validate + recover one slot at power-on (`bank5_code27` `$B960-$B9A6`).
///
/// `beginning` = the 42 `bank5_Beginning_Values` bytes (`$BAE3`, stored to
/// `$777`, i.e. part1 `[0..42]`); `item_bits` = the 224 ROM item-presence
/// bytes (`$BB15`, `bank5_Initial_Item_Presence_Bits_*`). Both are caller
/// slices (ROM or synthetic) — never embedded here.
pub fn recover_slot(
    sram: &mut [u8],
    slot: u8,
    beginning: &[u8],
    item_bits: &[u8],
) -> Option<Recovery> {
    let p = slot_pointers(slot)?;
    let (h, m1, m2, b1, b2) = (
        sram_index(p.header)?,
        sram_index(p.part1)?,
        sram_index(p.part2)?,
        sram_index(p.bak1)?,
        sram_index(p.bak2)?,
    );
    if sram.len() < SRAM_LEN || item_bits.len() < PART2_LEN {
        return None;
    }
    match classify_header(sram[h]) {
        HeaderState::Valid => Some(Recovery::Kept),
        HeaderState::Staged => {
            // Main untouched (`$5A` was marked before any main write):
            // finalize (`LB99D` `$B99D`).
            sram[h] = HDR_VALID;
            Some(Recovery::Finalized)
        }
        HeaderState::Torn => {
            // Torn main (`$69`): backup is authoritative — restore both
            // halves (`LB9A7`/`LB9B1` `$B9A7`), then `$A5` (`LB99D`).
            sram.copy_within(b1..b1 + PART1_LEN, m1);
            sram.copy_within(b2..b2 + PART2_LEN, m2);
            sram[h] = HDR_VALID;
            Some(Recovery::Restored)
        }
        HeaderState::Blank(_) => {
            // Fresh slot: beginning values → main part1 (`LB978`
            // `$B978`: 50 B from `bank5_Beginning_Values`), ROM item
            // bits → main part2 (`LB981` `$B981` walk to `$BBF5`).
            // NOTE: the hardware copies all 50 part1 bytes from the
            // 42-byte table region (reads 8 bytes past into the blank-name
            // table `$BB0D`); the caller passes the full 50-byte
            // `$777-$7A8` init image (`beginning` + blank name tail).
            if beginning.len() < PART1_LEN {
                return None;
            }
            sram[m1..m1 + PART1_LEN].copy_from_slice(&beginning[..PART1_LEN]);
            sram[m2..m2 + PART2_LEN].copy_from_slice(&item_bits[..PART2_LEN]);
            sram[h] = HDR_VALID;
            Some(Recovery::Initialized)
        }
    }
}

/// Validate + recover all three slots (`bank5_code27` outer `LDX #2` loop,
/// `$B960-$B9A6`).
pub fn recover_all(sram: &mut [u8], beginning: &[u8], item_bits: &[u8]) -> Option<[Recovery; 3]> {
    // Hardware order is `X = 2, 1, 0` (`DEX : BPL LB962`); order is
    // unobservable (slots are disjoint), reported slot-ascending.
    let mut out = [Recovery::Kept; 3];
    for slot in [2u8, 1, 0] {
        out[slot as usize] = recover_slot(sram, slot, beginning, item_bits)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_tables_are_disjoint_and_in_window() {
        let mut seen = [false; SRAM_LEN];
        for slot in 0..3u8 {
            let p = slot_pointers(slot).unwrap();
            for (base, len) in [
                (p.header, 1usize),
                (p.part1, PART1_LEN),
                (p.part2, PART2_LEN),
                (p.bak1, PART1_LEN),
                (p.bak2, PART2_LEN),
            ] {
                let i = sram_index(base).unwrap();
                for k in 0..len {
                    assert!(!seen[i + k], "overlap at {base:04X}+{k}");
                    seen[i + k] = true;
                }
            }
        }
        assert_eq!(slot_pointers(3), None);
        assert_eq!(sram_index(0x5FFF), None);
    }

    #[test]
    fn header_protocol_classifies() {
        assert_eq!(classify_header(0xA5), HeaderState::Valid);
        assert_eq!(classify_header(0x5A), HeaderState::Staged);
        assert_eq!(classify_header(0x69), HeaderState::Torn);
        assert_eq!(classify_header(0xFF), HeaderState::Blank(0xFF));
        assert_eq!(classify_header(0x00), HeaderState::Blank(0x00));
    }
}
