//! Town engine port root.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod town;` (see `lib.rs`; trap shims live in
//! `town_traps.rs`, which is the only file that takes `&mut Game`).
//!
//! Bank 3 `third_party/z2disassembly/src/prg3.asm` (`prg3.asm $xxxx` below)
//! owns the town half: 8 towns, NPC/enemy lists, wise men, dialog conditions,
//! the text renderer, healers, thrust teachers and side-quest flags. NPC
//! slots reuse the 6 enemy slots (`$2A-2F`/`$4E-53`/`$3C-41`/`$60-65`/
//! `$71-76`/`$A1-A6`/`$B6-BB`); text bytes live in the `DIALOG` section and
//! are NEVER copied here (see `RomTable` descriptors + `read_table_byte`).
//!
//! Pieces live in sibling modules so each stays small:
//!
//! * [`town`](self) — this file: town codes, spell order, ROM descriptors,
//!   shared address constants and the [`TrapEntry`] alias (data only;
//!   registration lives in `town_traps` so `traps.rs` stays untouched).
//! * `town_npc` (`town_npc.rs`): NPC lists, walking AI, talk gates.
//! * `town_dialog` (`town_dialog.rs`): dialog box + text renderer + index
//!   resolution (charset decode via the TBL alphabet, `$FD`/`$FE`/`$FF`).
//! * `town_quest` (`town_quest.rs`): wise men / healers / thrust teachers /
//!   side quests / hidden Kasuto / Bagu bridge / River Devil.
//! * `town_traps` (`town_traps.rs`): `Game`-shim trap bodies +
//!   `TOWN_TRAPS` and `register_town_traps` (interp-gated).
//!
//! # Design rules
//!
//! * Pure functions over explicit params / `&[u8]` slices — no `Game`
//!   dependency, no ROM reads — so every test is ROM-free. Only
//!   `town_traps.rs` takes `&mut Game`.
//! * Each `town*.rs` pure file is intentionally self-contained: shared
//!   constants are repeated per file (documented as such) rather than
//!   imported, so standalone `rustc --test` keeps working.
//! * Every routine cites its disassembly entry label + address in doc
//!   comments (bank 3 = `prg3.asm`, bank 7 = `prg7.asm`, bank 0 =
//!   `prg0.asm`; file offsets from `z2-assets` `extract_tables.rs`).
//! * Bugs are preserved with `BUG:` comments rather than fixed.
//!
//! # Frame pipeline (town talk → dialog)
//!
//! ```text
//! bank3_Enemy_Routines1_ManyNPC ($9AE8) / Wise_Man ($9AC8): per-NPC AI tick
//! bank3_Check_for_B_button_to_talk_to_people ($9A2C): B-gate → $074C=2, $DE=1
//! bank3_Dialog_Routines_Set_text_pointer… ($B480): conditions → text pointer
//! LB6A2 ($B6A2): typewriter (delay $0566, col $0489, row $048A, ptr $0569/6A)
//! bank3_Load_a_letter ($B6DD): tile map → $0301-$0307 PPU macro
//! bank3_End_of_Line_Routine ($B656): $FD/$FE line break (delays $0B/$2D)
//! bank3_Dialog_Routines_life_magic_restore ($B75B): healer/magic refill
//! bank3_Dialog_Routines_wait_for_B_button ($B7A6): hold until B
//! LB7B2 ($B7B2): teardown (+ 26% Ache transform in towns 2/5)
//! ```
//!
//! # Key RAM (verified against `third_party/z2disassembly/ram-map.txt`)
//!
//! `$056B` town code · `$0561` scene layout · `$0707` world · `$074C` dialog
//! type · `$077B-$0782` spells · `$0785-$078C` items · `$0796` thrust flags ·
//! `$0798` trophy · `$0799` mirror · `$079A` Bagu note/medicine · `$079B`
//! water · `$079C` lost child · `$070C` pending magic · `$070D` pending life ·
//! `$0489` text col · `$048A` text row · `$048B` townfolk slot · `$0569/$056A`
//! text pointer · `$0566` letter delay · `$0301-$0307` text PPU macro.
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`town_of_code`] | `$056B` Town Code | `$056B` | verified |
//! | [`spell_of_town`] | `bank3_Dialog_Conditions_Wise_Man` | `$B518` | verified |
//! | [`world_of_town`] | `$0707` world | `$0707` | verified |
//! | [`read_table_byte`] | `z2-assets` DIALOG contract | file `0x00E390` | verified |
//!
//! # Preserved quirks (BUG comments)
//!
//! * `bank3_code17` (`$9A9B`): frozen NPCs (`$05C3 != 0`) steal Link's facing
//!   (`$9F`) and tail-jump to `bank7_Display`, skipping the caller's return
//!   path (double-`PLA` + `JMP`).
//! * Wise-man first-spell path (`$B53A-$B548`): when the granted spell is
//!   Link's first ever, `$0749` (magic selector) is overwritten with the
//!   town index — the menu cursor jumps to the new spell.
//! * Ache transform (`LB7B2`, `$B7B2`): towns 2/5 only, generated townfolk
//!   (`$19-$1C`), `RNG < $67` (~40%, comment says 26%) → Ache code `$03`.
//!
//! # Gaps (honest)
//!
//! * Sprite/OAM emission (`bank7_Display`, `$EF11`) is display-only.
//! * PPU macro drain (`$0301-$0362`, `$0725`) lives with the interpreter;
//!   [`town_dialog`](crate::town_dialog) models the packet bytes only.
//! * Overworld gating (hidden-Kasuto forest patch, Bagu-bridge overworld
//!   tile, River Devil encounter) lives with the interpreter; predicates
//!   here model the in-town flag half.

// ---------------------------------------------------------------------------
// Addresses (duplicated per town*.rs file on purpose; keeps each file
// standalone-compilable — see sideview*.rs convention).
// ---------------------------------------------------------------------------

/// Town code (`$056B`: used by the wise man to pick magic to give).
pub const ADDR_TOWN_CODE: u16 = 0x056B;
/// Scene layout index (`$0561`).
pub const ADDR_SCENE: u16 = 0x0561;
/// World/area-type byte (`$0707`: 1 west towns, 2 east towns).
pub const ADDR_WORLD: u16 = 0x0707;
/// Dialog type (`$074C`: 0 none, 1 level-up, 2 talking, 4 healer).
pub const ADDR_DIALOG: u16 = 0x074C;
/// Spells possessed base (`$077B-$0782`).
pub const ADDR_SPELLS: u16 = 0x077B;
/// Items possessed base (`$0785-$078C`).
pub const ADDR_ITEMS: u16 = 0x0785;
/// Sword-technique flags (`$0796`: `$10` down, `$04` up).
pub const ADDR_THRUST: u16 = 0x0796;
/// Trophy in inventory (`$0798`, `$10` = yes).
pub const ADDR_TROPHY: u16 = 0x0798;
/// Mirror collected (`$0799`, `$01` = yes).
pub const ADDR_MIRROR: u16 = 0x0799;
/// Bagu note (`$079A` bit 3) / medicine (`$079A` bit 6).
pub const ADDR_BAGU_MED: u16 = 0x079A;
/// Water collected (`$079B`, `$01` = yes).
pub const ADDR_WATER: u16 = 0x079B;
/// Lost child (`$079C`, `$20` = yes).
pub const ADDR_CHILD: u16 = 0x079C;
/// Pending magic refill (`$070C`).
pub const ADDR_PEND_MAG: u16 = 0x070C;
/// Pending life refill (`$070D`).
pub const ADDR_PEND_LIFE: u16 = 0x070D;
/// Text column (`$0489`: letter X offset).
pub const ADDR_TEXT_COL: u16 = 0x0489;
/// Text row (`$048A`: letter Y offset).
pub const ADDR_TEXT_ROW: u16 = 0x048A;
/// Townfolk slot (`$048B`: conversation pointer / NPC slot).
pub const ADDR_TALK_SLOT: u16 = 0x048B;
/// Text pointer hi/lo (`$0569/$056A`).
pub const ADDR_TEXT_PTR_HI: u16 = 0x0569;
/// Text pointer lo.
pub const ADDR_TEXT_PTR_LO: u16 = 0x056A;
/// Delay between letters (`$0566`).
pub const ADDR_TEXT_DELAY: u16 = 0x0566;

/// Trap entry: (routine name, PRG bank, entry address).
///
/// `bank = None` for slot-swapped bank 3 (`$8000-$BFFF`): the same CPU
/// address runs under different banks, so `main` must resolve the bank at
/// registration/call time. All town entries are `None` today (see
/// `traps.rs` aliasing caveat); [`register_town_traps`](crate::town_traps::register_town_traps)
/// keeps them data-only and exposes direct `tw_*` shims instead.
pub type TrapEntry = (&'static str, Option<u8>, u16);

// ---------------------------------------------------------------------------
// Towns (8) + spell order.
// ---------------------------------------------------------------------------

/// Town count (bank 3 covers all 8).
pub const TOWN_COUNT: usize = 8;

/// Town id (`$056B` town code).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TownId {
    /// Rauru (west): Shield wise man.
    Rauru = 0,
    /// Ruto (west): Jump wise man.
    Ruto = 1,
    /// Saria (west): Life wise man; River Man + Bagu bridge.
    Saria = 2,
    /// Mido (west): Fairy wise man.
    Mido = 3,
    /// Nabooru (east): Fire wise man.
    Nabooru = 4,
    /// Darunia (east): Reflect wise man.
    Darunia = 5,
    /// New Kasuto (east, hidden): Spell wise man + magic key.
    NewKasuto = 6,
    /// Old Kasuto (east): Thunder wise man.
    OldKasuto = 7,
}

/// Town names in `$056B` order.
pub const TOWN_NAMES: [&str; TOWN_COUNT] = [
    "Rauru",
    "Ruto",
    "Saria",
    "Mido",
    "Nabooru",
    "Darunia",
    "New Kasuto",
    "Old Kasuto",
];

/// Spell index granted by a town's wise man (`$077B + town`).
///
/// `bank3_Dialog_Conditions_Wise_Man` (bank 3 `$B518`):
/// `LDY $056B : LDA $077B,y` — the town code IS the spell index, so the
/// order below is structural, not a lookup table:
/// shield Rauru, jump Ruto, life Saria, fairy Mido, fire Nabooru, reflect
/// Darunia, spell New Kasuto, thunder Old Kasuto.
pub const SPELL_OF_TOWN: [usize; TOWN_COUNT] = [0, 1, 2, 3, 4, 5, 6, 7];

/// Spell names in `$077B-$0782` order (for reports, not ROM bytes).
pub const SPELL_NAMES: [&str; 8] = [
    "shield", "jump", "life", "fairy", "fire", "reflect", "spell", "thunder",
];

/// Decode a `$056B` town code (`None` = out of range; total fn).
pub const fn town_of_code(code: u8) -> Option<TownId> {
    match code {
        0 => Some(TownId::Rauru),
        1 => Some(TownId::Ruto),
        2 => Some(TownId::Saria),
        3 => Some(TownId::Mido),
        4 => Some(TownId::Nabooru),
        5 => Some(TownId::Darunia),
        6 => Some(TownId::NewKasuto),
        7 => Some(TownId::OldKasuto),
        _ => None,
    }
}

/// Spell index (`$077B,y`) for a town (identity: town code = spell index).
pub const fn spell_of_town(town: TownId) -> usize {
    town as usize
}

/// World byte (`$0707`) owning a town: 1 west (0-3), 2 east (4-7).
///
/// `ram-map.txt` (`707 world`: 1 = west hyrule towns, 2 = east hyrule
/// towns); `bank3_Dialog_Conditions_default` (`$B5E7`) branches on
/// `$0707 == 2` for the east dialog tables.
pub const fn world_of_town(town: TownId) -> u8 {
    match town {
        TownId::Rauru | TownId::Ruto | TownId::Saria | TownId::Mido => 1,
        TownId::Nabooru | TownId::Darunia | TownId::NewKasuto | TownId::OldKasuto => 2,
    }
}

/// Whether a town is the hidden one (New Kasuto, town 6).
///
/// Hidden-town overworld reveal (hammer forest patch) is interp-only (gap);
/// this flags the in-town side (magic-key basement + Spell wise man).
pub const fn is_hidden(town: TownId) -> bool {
    matches!(town, TownId::NewKasuto)
}

// ---------------------------------------------------------------------------
// ROM-offset table descriptors (never bytes).
// ---------------------------------------------------------------------------

/// One ROM-resident table: `(PRG bank, CPU address, length)`.
///
/// Contract: town/dialog bytes are NEVER copied into this file.
/// Every table below is an offset triple; bytes are loaded at runtime by
/// the caller (`read_table_byte` over a `Z2_ROM` slice). Unit tests use
/// synthetic slices. File offsets (`file_off`) from `z2-assets`
/// `extract_tables.rs` are cited per entry for the ROM-gated tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomTable {
    /// PRG bank (`None` = WRAM mirror, not ROM).
    pub bank: Option<u8>,
    /// CPU address of the first byte.
    pub addr: u16,
    /// Length in bytes.
    pub len: u16,
    /// Headered iNES file offset (for `Z2_ROM` slicing).
    pub file_off: u32,
}

/// Town area-pointer table (bank 3 `$8523`, 63 maps).
///
/// `z2dis:prg3:bank3_Area_Pointers__Towns` (`$8523`); `z2-assets`
/// `SB3_MAPPTR_A` file `0x00C533` len 126.
pub const TOWN_MAP_PTR: RomTable = RomTable {
    bank: Some(3),
    addr: 0x8523,
    len: 126,
    file_off: 0x00C533,
};
/// Town enemy-pointer table (bank 3 `$85A1`).
///
/// `z2dis:prg3:bank3_Enemy_Pointers__Towns` (`$85A1`); `z2-assets`
/// `SB3_ENEMYPTR_A` file `0x00C5B1` len 126.
pub const TOWN_ENEMY_PTR: RomTable = RomTable {
    bank: Some(3),
    addr: 0x85A1,
    len: 126,
    file_off: 0x00C5B1,
};
/// Town room-connectivity table (bank 3 `$871B`, 63x4 assumed).
///
/// `z2dis:prg3:bank3_Room_Connectivity_Data_size_unknown` (`$871B`);
/// `z2-assets` `SB3_CONN_A` file `0x00C72B` len 252.
pub const TOWN_CONN: RomTable = RomTable {
    bank: Some(3),
    addr: 0x871B,
    len: 252,
    file_off: 0x00C72B,
};
/// Town enemy-data blob (bank 3 `$88A0`, copied to SRAM `$7000-$73FF`).
///
/// `z2dis:prg3:bank3_Enemy_Data_Towns_` (`$88A0`); `z2-assets`
/// `SB3_ENEMIES` file `0x00C8B0` len 1024.
pub const TOWN_ENEMIES: RomTable = RomTable {
    bank: Some(3),
    addr: 0x88A0,
    len: 1024,
    file_off: 0x00C8B0,
};
/// Dialog index table 1 (bank 3 `$A238`).
///
/// `z2dis:prg3:bank3_related_to_dialog_indexes1` (`$A238`);
/// adjacent `DLG_IDX2` file `0x00E2AC` len 64 is the runtime slice root
/// for the ROM-gated index test (indexes1 itself is the same bank-3
/// region; offset cited per `extract_tables.rs`).
pub const DLG_INDEXES1: RomTable = RomTable {
    bank: Some(3),
    addr: 0xA238,
    len: 64,
    file_off: 0x00E26C,
};
/// Dialog index table 2 (bank 3 `$A29C`).
///
/// `z2dis:prg3:bank3_related_to_dialog_indexes2`; `z2-assets` `DLG_IDX2`
/// file `0x00E2AC` len 64.
pub const DLG_INDEXES2: RomTable = RomTable {
    bank: Some(3),
    addr: 0xA29C,
    len: 64,
    file_off: 0x00E2AC,
};
/// Dialog index table 3 (bank 3 `$A2DC`, east towns).
///
/// `z2dis:prg3:bank3_related_to_dialog_indexes3`; `z2-assets` `DLG_IDX3`
/// file `0x00E2EC` len 100.
pub const DLG_INDEXES3: RomTable = RomTable {
    bank: Some(3),
    addr: 0xA2DC,
    len: 100,
    file_off: 0x00E2EC,
};
/// Dialog index table 4 (bank 3 `$A340`, east alt).
///
/// `z2dis:prg3:bank3_related_to_dialog_indexes4`; `z2-assets` `DLG_IDX4`
/// file `0x00E350` len 64.
pub const DLG_INDEXES4: RomTable = RomTable {
    bank: Some(3),
    addr: 0xA340,
    len: 64,
    file_off: 0x00E350,
};
/// Dialog text table (bank 3 `$A380` / CPU `$E390-$EFCC`).
///
/// `z2dis:prg3:bank3_Dialogs_Text_Table`; Data Crystal TBL
/// `$E390-$EFCC`; `z2-assets` `DIALOG` file `0x00E390` len 3133.
/// Control codes: `$FD` next line, `$FE` delay, `$FF` end talk
/// (see `town_dialog.rs`); alphabet in `z2-assets` `DIALOG_ALPHABET`.
pub const DIALOG_TEXT: RomTable = RomTable {
    bank: Some(3),
    addr: 0xE390,
    len: 3133,
    file_off: 0x00E390,
};
/// West-town dialog pointer table (bank 3 `$AFBE`).
///
/// `z2dis:prg3:bank3_Dialogs_Pointer_Table_Towns_in_West_Hyrule`
/// (`$AFBE`); selected when `$0707 != 2` (`$B5E7`).
pub const DLG_PTR_WEST: RomTable = RomTable {
    bank: Some(3),
    addr: 0xAFBE,
    len: 64,
    file_off: 0x00EFCE,
};
/// East-town dialog pointer table (bank 3 `$B026`).
///
/// `z2dis:prg3:bank3_Dialogs_Pointer_Table_Towns_in_East_Hyrule`
/// (`$B026`); selected when `$0707 == 2` (`$B5E7`).
pub const DLG_PTR_EAST: RomTable = RomTable {
    bank: Some(3),
    addr: 0xB026,
    len: 64,
    file_off: 0x00F036,
};

/// Read one byte from a runtime-loaded table slice.
///
/// `table` is the caller-loaded bytes for `desc` (exact length
/// `desc.len`); returns `None` on out-of-range (total fn, never panics).
pub const fn read_table_byte(table: &[u8], index: usize) -> Option<u8> {
    if index < table.len() {
        Some(table[index])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn town_codes_cover_all_eight() {
        assert_eq!(TOWN_COUNT, 8);
        for code in 0..8u8 {
            let t = town_of_code(code).expect("town code");
            assert_eq!(t as u8, code);
            assert_eq!(spell_of_town(t) as u8, code);
            assert!(!TOWN_NAMES[code as usize].is_empty());
        }
        assert_eq!(town_of_code(8), None);
        assert_eq!(SPELL_OF_TOWN, [0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn spell_order_matches_town_order() {
        // shield Rauru, jump Ruto, life Saria, fairy Mido, fire Nabooru,
        // reflect Darunia, spell New Kasuto, thunder Old Kasuto.
        let expect = [
            (TownId::Rauru, "shield"),
            (TownId::Ruto, "jump"),
            (TownId::Saria, "life"),
            (TownId::Mido, "fairy"),
            (TownId::Nabooru, "fire"),
            (TownId::Darunia, "reflect"),
            (TownId::NewKasuto, "spell"),
            (TownId::OldKasuto, "thunder"),
        ];
        for (town, name) in expect {
            assert_eq!(SPELL_NAMES[spell_of_town(town)], name);
        }
    }

    #[test]
    fn worlds_split_west_east() {
        for t in [TownId::Rauru, TownId::Ruto, TownId::Saria, TownId::Mido] {
            assert_eq!(world_of_town(t), 1);
        }
        for t in [
            TownId::Nabooru,
            TownId::Darunia,
            TownId::NewKasuto,
            TownId::OldKasuto,
        ] {
            assert_eq!(world_of_town(t), 2);
        }
        assert!(is_hidden(TownId::NewKasuto));
        assert!(!is_hidden(TownId::OldKasuto));
    }

    #[test]
    fn rom_descriptors_point_at_bank3() {
        assert_eq!((TOWN_MAP_PTR.addr, TOWN_ENEMY_PTR.addr), (0x8523, 0x85A1));
        assert_eq!((DLG_INDEXES2.addr, DIALOG_TEXT.addr), (0xA29C, 0xE390));
        assert_eq!((DLG_PTR_WEST.addr, DLG_PTR_EAST.addr), (0xAFBE, 0xB026));
        assert_eq!(DIALOG_TEXT.file_off, 0x00E390);
        assert_eq!(DIALOG_TEXT.len, 3133);
    }

    #[test]
    fn runtime_loads_never_copy_bytes() {
        let fake = [0xE9u8, 0xFF, 0xFD];
        assert_eq!(read_table_byte(&fake, 1), Some(0xFF));
        assert_eq!(read_table_byte(&fake, 9), None);
    }
}
