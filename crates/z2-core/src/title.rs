//! Title / intro / file-select / game-over / ending tables + descriptors.
//!
//! Self-contained (no intra-crate imports) so `rustc --edition 2021 --test`
//! compiles this file standalone. State machines live in
//! [`crate::title_flow`]; trap shims in `title_traps.rs` (interp-gated).
//!
//! Text bytes are NEVER copied here: callers load the bank-5 text/table
//! slices at runtime (via `Z2_ROM`) and pass them as slices — the same
//! contract as [`crate::town_dialog`]'s `DIALOG` (read-only reuse: the
//! title codec IS the dialog codec — `$DA+` letters, `$D0+` digits,
//! `$F4` space, `$FD`/`$FE` newlines, `$FF` end; decode with
//! `town_dialog::{classify_byte, decode_tile, render_tiles}`).
//!
//! | item | label | addr | status |
//! |---|---|---|
//! | [`STORY_TEXT`] | `bank5_table_intro_screen_text` | bank 5 `$A932` | ported |
//! | [`STORY_TEXT2`] | `LAA08` | bank 5 `$AA08` | ported |
//! | [`INTRO_SPRITES`] | `bank5_Intro_Sprites` | bank 5 `$A7C1` | ported |
//! | [`SELECT_TEXT`] | `bank5_Tables_for_Selection_Screen_Text_` | bank 5 `$BC19` | ported |
//! | [`LETTERS`] | `bank5_Table_for_Letters_Tile_Mappings` | bank 5 `$BD75` | ported |
//! | [`GAMEOVER_TEXT`] | `Tables_for_Game_Over_screen_text` | bank 0 `$8000` | ported |
//! | [`GANON_TILES`] | `Tables_for_Ganon_Shadow_Tile_Mapping_and_Palette_Mapping` | bank 0 `$801F` | ported |
//! | [`GANON_PAL`] | `bank0_Return_of_Ganon_screen_Palettes` | bank 0 `$80C9` | ported |
//! | [`CREDITS_TABLE`] / [`CREDITS_TEXT`] | `bank5_Pointer_table_for_End_Credits` / `bank5_End_Credits` | bank 5 `$9259`/`$927D` | ported |
//! | [`ENDING_TEXT_ZELDA`] | `bank5_Ending_Text_Zelda_` | bank 5 `$8DDE` | ported |
//!
//! # Preserved quirks
//!
//! * The credits pointer table repeats `L9325` three times (`$927D`…
//!   entries 10/12/14) — the same page shows thrice, kept as-is.
//! * Game-over packet 2's `$21` prefix byte lives at the tail of the
//!   `L8001` blob (`$800D`); [`GAMEOVER_TEXT`] documents both packets
//!   with their true starts.
//!
//! # Gaps (honest)
//!
//! * Star/glint animation (`bank5_Animation_of_Stars_in_the_Sky`
//!   `$A8CF`, `..._Glints_in_Water` `$A918`), palette fade
//!   (`bank5_Probably_related_to_Palette_fading` `$90DA`) and the
//!   Zelda-wake tile mixer (`L8CE3` `$8CE3`) are per-frame PPU work —
//!   descriptors only here, bytes with the interpreter.
//! * `$073B` sub-mode tables are sequenced in [`crate::title_flow`];
//!   PPU-macro drain (`$0725` → `$2006/$2007`) is interp-only.

// ---------------------------------------------------------------------------
// ROM-offset descriptors (never bytes — cf. `palace::RomOff`).
// ---------------------------------------------------------------------------

/// One ROM-resident table: `(PRG bank, CPU address, length)`.
///
/// Lengths are measured off the disassembly spans cited per constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomOff {
    /// PRG bank.
    pub bank: u8,
    /// CPU address of the first byte.
    pub addr: u16,
    /// Length in bytes.
    pub len: u16,
}

/// Story text part 1 (`bank5_table_intro_screen_text`, `$A932-$AA07`, 214 B).
pub const STORY_TEXT: RomOff = RomOff {
    bank: 5,
    addr: 0xA932,
    len: 214,
};
/// Story text part 2 (`LAA08`, `$AA08-$AAE4`, 221 B).
pub const STORY_TEXT2: RomOff = RomOff {
    bank: 5,
    addr: 0xAA08,
    len: 221,
};
/// Intro sprites (`bank5_Intro_Sprites`, `$A7C1`, 3 OAM triplets shown).
pub const INTRO_SPRITES: RomOff = RomOff {
    bank: 5,
    addr: 0xA7C1,
    len: 24,
};
/// File-select screen text/PPU macros (`bank5_Tables_for_Selection_Screen_Text_`,
/// `$BC19-$BCBE`, 166 B, `$FF`-terminated).
pub const SELECT_TEXT: RomOff = RomOff {
    bank: 5,
    addr: 0xBC19,
    len: 166,
};
/// Name-grid PPU rows inside [`SELECT_TEXT`] (`$BD00/$BD18/$BD30/$BD3E`):
/// letter rows A-K / L-V / W-.-digits live here; geometry in
/// [`crate::save`] (`GRID_COLS`/`GRID_ROWS`).
pub const NAME_GRID_TEXT: RomOff = RomOff {
    bank: 5,
    addr: 0xBD00,
    len: 85,
};
/// Letter tiles (`bank5_Table_for_Letters_Tile_Mappings`, `$BD75-$BD9F`,
/// 43 B: A-Z, `-`, `.`, `0-9`).
pub const LETTERS: RomOff = RomOff {
    bank: 5,
    addr: 0xBD75,
    len: 43,
};
/// Game-over PPU text (`Tables_for_Game_Over_screen_text`, bank 0
/// `$8000-$801E`: `GAME OVER` @ `$216B` + `RETURN OF GANON` @ `$21C8`).
pub const GAMEOVER_TEXT: RomOff = RomOff {
    bank: 0,
    addr: 0x8000,
    len: 31,
};
/// Ganon-shadow tile/palette mapping (bank 0 `$801F-$80C8`, 170 B).
pub const GANON_TILES: RomOff = RomOff {
    bank: 0,
    addr: 0x801F,
    len: 170,
};
/// Return-of-Ganon palettes (bank 0 `$80C9-$80D4`, 12 B).
pub const GANON_PAL: RomOff = RomOff {
    bank: 0,
    addr: 0x80C9,
    len: 12,
};
/// Game-over magic-bag PPU line (bank 0 `$A99C`, 8 B, cites the
/// `this line contains ppu instruction for drawing the magic bag/jar on
/// the game over screen` comment).
pub const GAMEOVER_BAG_LINE: RomOff = RomOff {
    bank: 0,
    addr: 0xA99C,
    len: 8,
};
/// Credits pointer table (`bank5_Pointer_table_for_End_Credits`, `$9259`,
/// 18 entries; repeats `L9325` ×3).
pub const CREDITS_TABLE: RomOff = RomOff {
    bank: 5,
    addr: 0x9259,
    len: 36,
};
/// Credits entry count (18 pages, `$9216`-adjacent `$9057` reload at 8
/// notwithstanding — the table walk itself is 18).
pub const CREDITS_COUNT: u8 = 18;
/// Credits text (`bank5_End_Credits` `$927D` … `L9396` `$9396-$93AD`).
pub const CREDITS_TEXT: RomOff = RomOff {
    bank: 5,
    addr: 0x927D,
    len: 353,
};
/// Zelda ending text (`bank5_Ending_Text_Zelda_`, bank 5 `$8DDE`).
pub const ENDING_TEXT_ZELDA: RomOff = RomOff {
    bank: 5,
    addr: 0x8DDE,
    len: 0, // variable: caller-bounded, `$FF`-terminated codec
};
/// New-game init code (`startup_init_begin_game`, bank 0 `$AA08`).
pub const INIT_BEGIN_GAME: RomOff = RomOff {
    bank: 0,
    addr: 0xAA08,
    len: 0, // code
};

// ---------------------------------------------------------------------------
// Game-over packet shapes (bank 0 `$8000`).
// ---------------------------------------------------------------------------

/// Packet 1: nametable `$216B`, 10 tiles (`GAME␣␣OVER`).
pub const GAMEOVER_LINE1_ADDR: u16 = 0x216B;
/// Packet 1 tile count.
pub const GAMEOVER_LINE1_LEN: u8 = 10;
/// Packet 2: nametable `$21C8`, 15 tiles (`RETURN␣OF␣GANON`).
/// Its `$21` prefix sits at `$800D` (tail of the `L8001` blob).
pub const GAMEOVER_LINE2_ADDR: u16 = 0x21C8;
/// Packet 2 tile count.
pub const GAMEOVER_LINE2_LEN: u8 = 15;

// ---------------------------------------------------------------------------
// PPU-macro selectors (`$0725` → `bank7_PPU_Adresses_according_to_725…`
// bank 7 `$C04D`).
// ---------------------------------------------------------------------------

/// `$0725 = 5`: Return-of-Ganon palettes (`$80C9`, `$C047`).
pub const PPU_MACRO_GANON_PAL: u8 = 5;
/// `$0725 = 8`: continue/save screen tiles (`$FDCB`, `$C04D`).
pub const PPU_MACRO_CONTINUE_SAVE: u8 = 8;
/// `$0725 = 9`: game-over text (`$8000`, `$C04F`; `LDA #9`, `$CA58`).
pub const PPU_MACRO_GAMEOVER_TEXT: u8 = 9;

// ---------------------------------------------------------------------------
// Title-mode values (`$0736` Game Mode; `$073B` routine index).
// ---------------------------------------------------------------------------

/// Game modes this module touches (`$0736`; gameplay 8/`$14` gated by
/// the `$C010` power-on loop, owned by bank 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    /// Title/story (`LB3F4` `$B3F4` returns here with `$736 = 0`).
    Title,
    /// Register-your-name (`LB28B` `$B2A6`).
    Register,
    /// Elimination mode (`LB2AA` `$B2B0`).
    Elimination,
    /// Manual-save death (`bank0_Manual_Save…` `$A1B5`).
    ManualSave,
    /// Respawn paths (`LCF05`/`LCF09` `$CF05`/`$CF09`: 6/7).
    Respawn(u8),
    /// Gameplay or other (pass-through).
    Other(u8),
}

/// Decode a `$0736` byte.
pub const fn game_mode(m: u8) -> GameMode {
    match m {
        0 => GameMode::Title,
        1 => GameMode::Register,
        2 => GameMode::Elimination,
        5 => GameMode::ManualSave,
        6 | 7 => GameMode::Respawn(m),
        other => GameMode::Other(other),
    }
}

/// Title `$73B` sequence table (`bank5_pointer_table3`, `$A705`):
/// `[bank5_code20, LAF1F, LA72E, LA737, LAB6D]` — boot/clear, story
/// setup, intro anim A/B, title wait (`LAB6D` `$AB6D` starts with the
/// `$2002` vblank wait + sprite setup).
pub const TITLE_SEQ_LEN: u8 = 5;
/// File-select `$73B` sequence (`bank5_pointer_table4`, `$B3D5`):
/// `[LB412, bank5_code22, LB678, bank5_code23, LB3F4]`.
pub const FILE_SEQ_LEN: u8 = 5;
/// Elimination `$73B` sequence (`bank5_pointer_table5`, `$B400`):
/// `[LB412, bank5_code22, bank5_code25, bank5_code23, LB3F4]`.
pub const ELIM_SEQ_LEN: u8 = 5;

// ---------------------------------------------------------------------------
// Fairy cursor rows.
// ---------------------------------------------------------------------------

/// File-select fairy Y rows (`LB328`/`LB340`/`LB358`/`LB36E`/`LB380`:
/// `$40 $58 $70` slots, `$90` register, `$A8` elimination).
pub const FILE_CURSOR_Y: [u8; 5] = [0x40, 0x58, 0x70, 0x90, 0xA8];
/// Register/elimination fairy Y rows (`bank5_table12`, `$B421`:
/// `$30 $48 $60 $78`).
pub const REG_CURSOR_Y: [u8; 4] = [0x30, 0x48, 0x60, 0x78];
/// Fairy X (`LDA #$1C : STA $4D`, `$B384`; elim `$4C`, `$B4DC`).
pub const FAIRY_X_FILE: u8 = 0x1C;
/// Elimination fairy X.
pub const FAIRY_X_ELIM: u8 = 0x4C;
/// Cursor-pick positions: slots 0-2, register 3, elimination 4.
pub const PICK_REGISTER: u8 = 3;
/// Elimination pick.
pub const PICK_ELIMINATION: u8 = 4;
/// Select-button wrap (`CMP #$05`, `$B311` file / `CMP #$04`, `$B4B4` elim).
pub const FILE_PICKS: u8 = 5;
/// Register-screen picks (0-2 slots + 3 END, `CMP #$04`, `$B6BA`).
pub const REG_PICKS: u8 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_decode() {
        assert_eq!(game_mode(0), GameMode::Title);
        assert_eq!(game_mode(1), GameMode::Register);
        assert_eq!(game_mode(2), GameMode::Elimination);
        assert_eq!(game_mode(5), GameMode::ManualSave);
        assert_eq!(game_mode(7), GameMode::Respawn(7));
        assert_eq!(game_mode(8), GameMode::Other(8));
    }

    #[test]
    fn cursor_tables_have_expected_rows() {
        assert_eq!(FILE_CURSOR_Y[3], 0x90);
        assert_eq!(FILE_CURSOR_Y[4], 0xA8);
        assert_eq!(REG_CURSOR_Y, [0x30, 0x48, 0x60, 0x78]);
    }
}
