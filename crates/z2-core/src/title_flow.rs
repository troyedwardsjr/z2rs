//! Title / file-select / name-entry / death / ending state machines.
//!
//! Pure logic over explicit params (no `Game` dep), following the
//! `palace.rs` pattern: read-only reuse of sibling pure modules
//! ([`crate::save`], [`crate::title`], [`crate::palace`]'s `$76C = 3/4`
//! ending states, [`crate::save_format`]); trap entry
//! points are listed in [`crate::title_traps::TITLE_TRAPS`].
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`file_advance`] / [`file_settle`] | `LB303` / `LB319` | bank 5 `$B303`/`$B319` | ported |
//! | [`file_start`] | `LB28B` | bank 5 `$B28B` | ported |
//! | [`register_advance`] / [`register_settle`] | `LB6AC` / `LB6C6` | bank 5 `$B6AC`/`$B6C6` | ported |
//! | [`register_start`] | `LB692` | bank 5 `$B692` | ported |
//! | [`elim_advance`] / [`elim_settle`] | `LB4A6` / `LB4BC` | bank 5 `$B4A6`/`$B4BC` | ported |
//! | [`elim_start`] | `LB443` | bank 5 `$B443` | ported |
//! | [`eliminate_slot`] | `bank5_elimination_mode` | bank 5 `$B462` | ported |
//! | [`title_start_press`] | `LA7AB` | bank 5 `$A7BD` | ported |
//! | [`gameover_wait_advance`] | `LCA72` | bank 7 `$CA72` | ported |
//! | [`lives_digit_tile`] | `bank7_Load_Lives_Remaining_Screen` | bank 7 `$C3B5` | ported |
//! | [`story_skip_tick`] | `L9248` | bank 5 `$9248` | ported |
//! | [`credits_next`] | `bank5_Pointer_table_for_End_Credits` walk | bank 5 `$9259` | ported |
//!
//! # How the settle chains read
//!
//! The `*_settle` fns are cursor *assist*, not restrictions: after every
//! Select press the hardware re-runs the chain, which auto-advances the
//! fairy past currently-useless stops and parks on REGISTER/END when
//! nothing else qualifies. The `$1A` bits they test are *empty* bits
//! (`bank5_code26` `$B502` sets a bit iff the slot's name is all-`$F4`;
//! see [`crate::save::slot_empty`]):
//!
//! * file select (`LB319` `$B319`) skips *empty* slots and parks on
//!   *occupied* (loadable) ones; with no occupied slot it parks on
//!   REGISTER (fresh cart: `$1A = $07` → REGISTER), with no empty slot it
//!   parks on ELIMINATION (all-full: `$1A = $00` → ELIMINATION).
//! * register (`LB6C6` `$B6C6`, `$1A` pre-ORed with the `$08` latch
//!   `$B6C8`) skips *occupied* slots and parks on the first *empty* one
//!   (fresh cart: slot 0) or END (3) when none is empty.
//! * elimination (`LB4BC` `$B4BC`, no latch) skips *empty* slots and parks
//!   on the first *occupied* (erasable) one or END.
//!
//! The user can still Select-navigate anywhere; [`file_start`] /
//! [`register_start`] / [`elim_start`] act on the parked position.
//!
//! # Preserved quirks
//!
//! * Elimination settle parks at END (3) whenever the doubled bit misses
//!   (`8 & $1A == 0` always holds for real `code26` bytes — slots are bits
//!   0-2 and the `$08` latch is register-screen-only — `$B4CF-$B4D2`);
//!   cursor 4 (+ the `bank5_table12` over-read, modelled as `None`) is
//!   reachable only with foreign bit3+ presence — kept for exactness.
//! * Register settle's bit-doubling (`TYA : ASL : JMP LB6D8`, `$B6DF-$B6E1`)
//!   walks slots in order and always terminates at END via the `$08` latch.
//!
//! # Gaps (honest)
//!
//! * `$73B` sequencing side effects (PPU macros, `$0726` fades, sound)
//!   are interp-only; the `*_SEQ` tables name the steps for the ledger.
//! * The credits page-advance timer was not isolated (page fetch is PPU);
//!   [`credits_next`] models the 18-entry table walk only.
//! * Fresh-cart first-naming UX, verified live on ROM: with no
//!   saves (`$1A = $07`, all empty) file-select parks at REGISTER and the
//!   register screen parks at slot 0 — name entry is live there at once
//!   (A commits letters into SRAM backup `$602C+`, `$1E` advances).

use crate::palace;
use crate::save;
use crate::save_format;
use crate::title;

// ---------------------------------------------------------------------------
// File select (`bank5_Load_Saved_Games_Data` `$B261` → `LB303` `$B303`).
// ---------------------------------------------------------------------------

/// Start-button action on the file-select screen (`LB28B` `$B28B-$B2B3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStart {
    /// Cursor 3: register mode (`$736 = 1`).
    Register,
    /// Cursor 4: elimination mode (`$736 = 2`).
    Elimination,
    /// Else: load slot `s` (`$0772 = s`, `LB911` `$B911`).
    LoadSlot(u8),
}

/// Start dispatch from the fairy cursor (`$19`).
pub const fn file_start(cursor: u8) -> FileStart {
    match cursor {
        title::PICK_REGISTER => FileStart::Register,
        title::PICK_ELIMINATION => FileStart::Elimination,
        s => FileStart::LoadSlot(s),
    }
}

/// Select advance (`LB303` `$B30D-$B317`: `INC $19`, wrap past 4 to 0).
pub const fn file_advance(cursor: u8) -> u8 {
    let next = cursor.wrapping_add(1);
    if next >= title::FILE_PICKS {
        0
    } else {
        next
    }
}

/// Settle the fairy after Select (`LB319-$B384`, `$B319-$B384`):
/// skip empty slots (hardware `$1A` bits are *empty* bits —
/// [`save::slot_empty`]) and park on the first occupied (loadable) slot;
/// park on REGISTER iff any slot is empty (all-empty `$07` collapses to
/// REGISTER via the `LB375` `DEC $19`), on ELIMINATION iff no slot is
/// empty (all-full `$00`). Returns `(cursor, fairy_y)`.
pub fn file_settle(cursor: u8, presence: u8) -> (u8, u8) {
    let mut c = cursor;
    // LB319: slot 0.
    if c == 0 {
        if presence & save::presence_bit(0) == 0 {
            return (0, title::FILE_CURSOR_Y[0]);
        }
        c = 1;
    }
    // LB32F: slot 1.
    if c == 1 {
        if presence & save::presence_bit(1) == 0 {
            return (1, title::FILE_CURSOR_Y[1]);
        }
        c = 2;
    }
    // LB347: slot 2.
    if c == 2 {
        if presence & save::presence_bit(2) == 0 {
            return (2, title::FILE_CURSOR_Y[2]);
        }
        c = 3;
    }
    // LB35F: register (park iff any save exists, else fall through).
    if c == 3 && presence != 0 {
        return (3, title::FILE_CURSOR_Y[3]);
    }
    // LB375: elimination (or all-full → back to register, which then
    // displays since presence != 0 holds).
    if presence != 0x07 {
        return (4, title::FILE_CURSOR_Y[4]);
    }
    (3, title::FILE_CURSOR_Y[3])
}

// ---------------------------------------------------------------------------
// Register screen (`LB678` `$B678` → `LB6AC` `$B6AC`).
// ---------------------------------------------------------------------------

/// Start-button action on the register screen (`LB692` `$B69B-$B6AB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterStart {
    /// Cursor 3 (END): back to file select (`$73E = 0`, `$73B++`).
    Back,
    /// Else: name entry for slot `s`.
    EnterName(u8),
}

/// Register Start dispatch.
pub const fn register_start(cursor: u8) -> RegisterStart {
    match cursor {
        title::PICK_REGISTER => RegisterStart::Back,
        s => RegisterStart::EnterName(s),
    }
}

/// Select advance on the register screen (`LB6AC` `$B6B6-$B6C4`: `INC $19`
/// mod 4, letter position `$1E` reset by the caller).
pub const fn register_advance(cursor: u8) -> u8 {
    let next = cursor.wrapping_add(1);
    if next >= title::REG_PICKS {
        0
    } else {
        next
    }
}

/// Settle on the register screen (`LB6C6-$B6E4`): skip occupied slots via
/// the bit-doubling walk (hardware `$1A` bits are *empty* bits —
/// [`save::slot_empty`] — so the `BNE LB6E4` parks on empty), park on the
/// first empty slot or END (3).
/// `presence` must already include the `$08` latch (`ORA #$08`, `$B6C8`).
/// Returns `(cursor, fairy_y)`.
pub fn register_settle(cursor: u8, presence: u8) -> (u8, u8) {
    if cursor == title::PICK_REGISTER {
        return (3, title::REG_CURSOR_Y[3]);
    }
    // Bit for the cursor (`ASL : BNE : CLC : ADC #1`, `$B6D2-$B6D6`).
    let mut bit = if cursor == 0 {
        1u8
    } else {
        cursor.wrapping_mul(2)
    };
    let mut c = cursor;
    loop {
        if presence & bit != 0 {
            let y = title::REG_CURSOR_Y.get(c as usize).copied().unwrap_or(0);
            return (c, y);
        }
        // `INC $19 : TYA : ASL : JMP LB6D8` (`$B6DD-$B6E1`).
        c = c.wrapping_add(1);
        bit = bit.wrapping_mul(2);
        if c == title::PICK_REGISTER {
            return (3, title::REG_CURSOR_Y[3]);
        }
    }
}

// ---------------------------------------------------------------------------
// Elimination screen (`bank5_code25` `$B425` → `LB4A6` `$B4A6`).
// ---------------------------------------------------------------------------

/// Start-button action on the elimination screen (`LB443` `$B44C-$B461`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElimStart {
    /// Cursor 3 (END): back to register (`$73E/$73B = 0`, `$736 = 1`).
    BackToRegister,
    /// Else: erase slot `s` ([`eliminate_slot`]).
    Eliminate(u8),
}

/// Elimination Start dispatch.
pub const fn elim_start(cursor: u8) -> ElimStart {
    match cursor {
        title::PICK_REGISTER => ElimStart::BackToRegister,
        s => ElimStart::Eliminate(s),
    }
}

/// Select advance on the elimination screen (`LB4A6` `$B4B0-$B4BA`).
pub const fn elim_advance(cursor: u8) -> u8 {
    let next = cursor.wrapping_add(1);
    if next >= title::REG_PICKS {
        0
    } else {
        next
    }
}

/// Settle on the elimination screen (`LB4BC-$B4D5` `$B4BC-$B4D5`): skip
/// empty slots (hardware `$1A` bits are *empty* bits —
/// [`save::slot_empty`] — so the `BEQ LB4D5` parks on occupied), park on
/// the first occupied (erasable) slot or END. Returns
/// `(cursor, Option<fairy_y>)` — `None` is the cursor-4 `bank5_table12`
/// over-read (`$B425` garbage), reachable only with foreign bit3+
/// presence (quirk preserved for exactness; `code26` bytes park ≤ 3).
pub fn elim_settle(cursor: u8, presence: u8) -> (u8, Option<u8>) {
    if cursor == title::PICK_REGISTER {
        return (3, title::REG_CURSOR_Y.get(3).copied());
    }
    let mut bit = if cursor == 0 {
        1u8
    } else {
        cursor.wrapping_mul(2)
    };
    let mut c = cursor;
    loop {
        if presence & bit == 0 {
            return (c, title::REG_CURSOR_Y.get(c as usize).copied());
        }
        // `INC $19 : ASL : JMP LB4CB` (`$B4CF-$B4D2`).
        c = c.wrapping_add(1);
        bit = bit.wrapping_mul(2);
        if bit == 0 {
            // Shifted out (past cursor 4): hardware keeps going with
            // `A = 0`…`AND $1A = 0` → parks at 4 with the OOB row.
            return (c.min(4), title::REG_CURSOR_Y.get(c as usize).copied());
        }
        if c > title::PICK_REGISTER + 1 {
            return (c, None);
        }
    }
}

/// Erase one slot (`bank5_elimination_mode`, `$B462-$B4A4`): beginning
/// values → backup part1, ROM item bits → backup part2 (the main-copy
/// propagation happens at the later commit-all, `bank5_code23` `$B3DF`).
/// `beginning50`/`item_bits` are caller slices (cf. [`save_format`]).
pub fn eliminate_slot(
    sram: &mut [u8],
    slot: u8,
    beginning50: &[u8; save_format::PART1_LEN],
    item_bits: &[u8; save_format::PART2_LEN],
) -> bool {
    let Some(p) = save_format::slot_pointers(slot) else {
        return false;
    };
    let (Some(b1), Some(b2)) = (
        save_format::sram_index(p.bak1),
        save_format::sram_index(p.bak2),
    ) else {
        return false;
    };
    if sram.len() < save_format::SRAM_LEN {
        return false;
    }
    sram[b1..b1 + save_format::PART1_LEN].copy_from_slice(beginning50);
    sram[b2..b2 + save_format::PART2_LEN].copy_from_slice(item_bits);
    true
}

// ---------------------------------------------------------------------------
// Title Start, game-over wait, lives screen, story skip, credits, ending.
// ---------------------------------------------------------------------------

/// Title Start (`LA7AB`, bank 5 `$A7AB-$A7BD`): new-press Start clears
/// `$0727`/`$0761` and `INC $076C` (title 0 → 1, enter game).
pub const fn title_start_press(state: u8) -> u8 {
    state.wrapping_add(1)
}

/// Bytes `LA7AB` clears on title Start (`STA $0727`, `STA $0761`).
pub const TITLE_START_CLEARS: [u16; 2] = [0x0727, 0x0761];

/// Game-over wait gate (`LCA72`, bank 7 `$CA72-$CA82`): Start held or
/// `$0501` timer expiry advances (`LCF05`, `$736++`); else keep waiting.
pub const fn gameover_wait_advance(start_held: bool, timer: u8) -> bool {
    start_held || timer == 0
}

/// Lives digit tile (`bank7_Load_Lives_Remaining_Screen`, `$C3D8-$C3DA`:
/// `CLC : ADC #$D0 : STA $0314`).
pub const fn lives_digit_tile(lives: u8) -> u8 {
    lives.wrapping_add(0xD0)
}

/// Lives-screen delay (`LDA #$70 : STA $0501`, `$C3DD`).
///
/// Name bytes shown on the lives screen come from [`save::ADDR_NAME`]
/// (`LDY #7 : LDA $7A1,y : STA $305,y`, `$C3D9-$C3E2`).
pub const LIVES_SCREEN_TIMER: u8 = 0x70;

/// Story/ending skip tick (`L9248`, bank 5 `$9248-$9254`): held Start
/// `INC $0761` (story-variant/skip counter shared with the ending
/// select at `L8B63` `$B63`... `$8B63`).
pub const fn story_skip_tick(start_held: bool, skip: u8) -> u8 {
    if start_held {
        skip.wrapping_add(1)
    } else {
        skip
    }
}

/// Credits table walk (`bank5_Pointer_table_for_End_Credits`, `$9259`):
/// next page index, or `None` after the 18th (back to title).
pub const fn credits_next(page: u8) -> Option<u8> {
    if page.wrapping_add(1) >= title::CREDITS_COUNT {
        None
    } else {
        Some(page.wrapping_add(1))
    }
}

/// Wake-Zelda state (`STA $076C`, `$A6EC`; reuses the palace state).
pub const fn wake_zelda_state() -> u8 {
    palace::wake_zelda()
}

/// Credits state (`INC $076C`, `$9244`/`$A7BD`-adjacent `$9244`; reuse).
pub const fn credits_state_from_wake(wake: u8) -> u8 {
    palace::roll_credits(wake)
}

/// Full ending chain check (`3 → 4`; reuse).
pub const fn ending_chain(state: u8) -> bool {
    palace::ending_chain(state)
}

// ---------------------------------------------------------------------------
// `$73B` sequence tables (names + addrs for the ledger; PPU/timing side
// effects are interp-only).
// ---------------------------------------------------------------------------

/// Title `$73B` steps (`bank5_pointer_table3`, `$A705`).
pub const TITLE_SEQ: [(&str, u16); 5] = [
    ("bank5_code20", 0xA70F),
    ("LAF1F", 0xAF1F),
    ("LA72E", 0xA72E),
    ("LA737", 0xA737),
    ("LAB6D", 0xAB6D),
];
/// File-select `$73B` steps (`bank5_pointer_table4`, `$B3D5`).
pub const FILE_SEQ: [(&str, u16); 5] = [
    ("LB412", 0xB412),
    ("bank5_code22", 0xB24E),
    ("LB678", 0xB678),
    ("bank5_code23", 0xB3DF),
    ("LB3F4", 0xB3F4),
];
/// Elimination `$73B` steps (`bank5_pointer_table5`, `$B400`).
pub const ELIM_SEQ: [(&str, u16); 5] = [
    ("LB412", 0xB412),
    ("bank5_code22", 0xB24E),
    ("bank5_code25", 0xB425),
    ("bank5_code23", 0xB3DF),
    ("LB3F4", 0xB3F4),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_start_dispatch() {
        assert_eq!(file_start(3), FileStart::Register);
        assert_eq!(file_start(4), FileStart::Elimination);
        assert_eq!(file_start(1), FileStart::LoadSlot(1));
        assert_eq!(file_advance(4), 0);
        assert_eq!(file_advance(2), 3);
    }

    #[test]
    fn elim_settle_parks_at_end_when_full() {
        // Real presence (bits 0-2): doubled bit 8 always misses → END.
        let (c, y) = elim_settle(0, 0x07);
        assert_eq!((c, y), (3, Some(0x78)));
        // Foreign bit3 presence reproduces the cursor-4 OOB quirk exactly.
        let (c4, y4) = elim_settle(0, 0x0F);
        assert_eq!((c4, y4), (4, None));
    }

    #[test]
    fn credits_walk_is_18_then_done() {
        assert_eq!(credits_next(0), Some(1));
        assert_eq!(credits_next(16), Some(17));
        assert_eq!(credits_next(17), None);
    }
}
