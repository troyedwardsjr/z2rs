//! Save-slot RAM logic: new-game init, presence, name entry, lives/deaths,
//! continue/save/retry.
//!
//! Self-contained (no intra-crate imports) so `rustc --edition 2021 --test`
//! compiles this file standalone. Byte-protocol SRAM work lives in
//! [`crate::save_format`]; title/ending sequencing in [`crate::title_flow`];
//! trap shims in `title_traps.rs` (interp-gated).
//!
//! All labels cite bank-5 CPU addresses (what some notes call "bank 0" is bank 5 —
//! see [`crate::save_format`]'s misfiling note) except `bank0_*`/`bank7_*`
//! ones, which cite their real bank.
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`reset_stats_to_beginning`] | `function_reset_link_stats_to_beginning_values` | bank 5 `$B2CA` | ported |
//! | [`reset_item_bits`] | `bank5_Load_Initial_Item_Presence_Bits` | bank 5 `$B2D7` | ported |
//! | [`new_game_ram_clear`] | `LB2EE` | bank 5 `$B2EE` | ported |
//! | [`slot_empty`] / [`empty_bits`] | `bank5_code26` | bank 5 `$B502` | ported |
//! | [`slot_present`] / [`presence_bits`] | — (complement helper) | — | helper |
//! | [`letter_tile`] | `LB8AE` over `bank5_Table_for_Letters_Tile_Mappings` | bank 5 `$B8AE`/`$BD75` | ported |
//! | [`name_store_index`] | `LB8AE` | bank 5 `$B8AE` | ported |
//! | [`refill_meter`] | `LCB18_fill_hp_or_mp_to_full__provide_x_register__maybe` | bank 7 `$CB18` | ported |
//! | [`die_step`] | `bank7_code16` tail / `LCA6C` | bank 7 `$CA44`/`$CA6C` | ported |
//! | [`continue_deaths`] | `LCA85` | bank 7 `$CAA3` | ported |
//! | [`respawn_select`] | `LCAC4`-`LCAF0` | bank 7 `$CAC4` | ported |
//! | [`respawn_music`] | `bank7_code11` | bank 7 `$C41E` | ported |
//! | [`manual_save_gate`] | `bank0_Manual_Save_Game_Routine_UP_AND_A` | bank 0 `$A19C` | ported |
//! | [`second_quest_gate`] | `LB2B4` | bank 5 `$B2BC` | ported |
//! | [`second_quest_setup`] | `L921C` | bank 5 `$921C` | ported |
//!
//! # Preserved quirks
//!
//! * `$B2CC` resets only `$783-$7A0` (`Y = $29..$0C`); attack/magic/life
//!   (`$777-$779`), `$77A` and the name survive a new-game+ reset.
//! * Deaths `$79F` saturates at `$FF` (`CMP #$FF : BEQ` skips the `INC`,
//!   `$CAAF-$CAB3`) instead of wrapping.
//! * Name writes go to `pos-1` (`$1E == 0` writes slot 7, `$B8A8-$B8AE`).
//! * Digit-row last cell (`row 3, col 10`, index 43) reads past the
//!   43-byte letter table into `$BDA1` `$FF` padding.
//!
//! # Gaps (honest)
//!
//! * B-button name delete was not found in the `$B6AC-$B8C4` region;
//!   only A (commit) is modelled.
//! * `$760`/`$75F` music bytes are passed through opaquely.
//! * PPU-macro/draw work (`$0725` selectors, `$0301` packets, OAM) is
//!   interp-only; shims record selector bytes for the PPU layer.

// ---------------------------------------------------------------------------
// RAM addresses.
// ---------------------------------------------------------------------------

/// Current game slot (`$0772`, set from fairy `$19`, `$B2B6`).
pub const ADDR_SLOT: u16 = 0x0772;
/// Fairy cursor (`$19`: 0-2 slots, 3 register, 4 elim).
pub const ADDR_FAIRY: u16 = 0x0019;
/// Save-presence bits (`$1A`: bit0/1/2 slots, bit3 register screen latch).
pub const ADDR_PRESENCE: u16 = 0x001A;
/// Fairy Y (`$1B`) / X (`$004D`).
pub const ADDR_FAIRY_Y: u16 = 0x001B;
/// Name letter position (`$1E`, 0-7) and grid cursor (`$1F` col / `$20` row).
pub const ADDR_LETTER_POS: u16 = 0x001E;
/// Name grid column (`$1F`, 0-10).
pub const ADDR_GRID_COL: u16 = 0x001F;
/// Name grid row (`$20`, 0-3).
pub const ADDR_GRID_ROW: u16 = 0x0020;
/// Lives (`$0700`).
pub const ADDR_LIVES: u16 = 0x0700;
/// Deaths/continues (`$079F`).
pub const ADDR_DEATHS: u16 = 0x079F;
/// Second-quest flag (`$07A0`: 0 new, 1 loaded quest-2 file, 2 active).
pub const ADDR_QUEST: u16 = 0x07A0;
/// Name bytes (`$7A1-$7A8`, tile codes).
pub const ADDR_NAME: u16 = 0x07A1;
/// Game state (`$076C`); `1` in-game and `3`/`4` ending live in
/// [`crate::palace`] (read-only reuse) — only death states here.
pub const ADDR_GAME_STATE: u16 = 0x076C;
/// Magic selector (`$0749`).
pub const ADDR_MAGIC_SEL: u16 = 0x0749;
/// Menu selection (`$0488`: continue/save cursor on the game-over screen).
pub const ADDR_MENU_SEL: u16 = 0x0488;
/// Magic/heart containers (`$0783`/`$0784`, input to `LCB18`).
pub const ADDR_CONTAINERS: u16 = 0x0783;
/// Meters (`$0773` magic / `$0774` life, output of `LCB18`).
pub const ADDR_METERS: u16 = 0x0773;
/// Down/up thrust bits (`$0796`; `$14` preserved across quest-2 reset).
pub const ADDR_THRUST: u16 = 0x0796;
/// Thrust bits preserved (`AND #$14`, `$B2C6`).
pub const THRUST_KEEP: u8 = 0x14;

/// `$076C` death states (Data Crystal + listing `76C begin a special
/// routine`; `1`/`3`/`4` are [`crate::palace`] constants, reused there).
pub const STATE_RESTART: u8 = 0;
/// Dying (`$76C = 2` → `bank7_code16` `$CA24` decrements lives).
pub const STATE_DYING: u8 = 2;
/// New life (`$76C = 6`, `LCA6C` `$CA6C`).
pub const STATE_NEW_LIFE: u8 = 6;

/// Starting lives (`bank7_Reset_Number_of_Lives__to_3_`, `$C358`).
pub const LIVES_START: u8 = 3;
/// Game-over delay (`LDA #$F0 : STA $0501`, `$CA4D`).
pub const GAMEOVER_TIMER: u8 = 0xF0;
/// Save-choice flash timer (`LDA #$40 : STA $07B0`, `$CABC`).
pub const SAVE_FLASH_TIMER: u8 = 0x40;
/// Manual-save combo: Up (`$08`) + A (`$80`) on pad 2 (`$A19F-$A1A3`).
pub const MANUAL_SAVE_COMBO: u8 = 0x88;
/// Blank name tile (`$F4`, `bank5_table_for_blanking_out_name` `$BB0D`).
pub const NAME_BLANK: u8 = 0xF4;
/// Name length (8 tiles, `$7A1-$7A8` / `$C3CB` loop `LDY #7`).
pub const NAME_LEN: usize = 8;
/// Beginning attack/magic/life (`bank5_Beginning_Values` `$BAE3-$BAE5`).
pub const BEGIN_ATK_MAG_LIFE: u8 = 1;
/// Beginning magic/heart containers (`$BAEF-$BAF0`).
pub const BEGIN_CONTAINERS: u8 = 4;
/// Beginning crystals left (`$BB00`).
pub const BEGIN_CRYSTALS: u8 = 6;
/// Beginning-values window reset by `$B2CC` (`$783-$7A0`, `Y $29..$0C`).
pub const RESET_LO: u8 = 0x0C;
/// Beginning-values window top (inclusive).
pub const RESET_HI: u8 = 0x29;

// ---------------------------------------------------------------------------
// New-game init (`LB2B4` `$B2B4` → `$B2CA` → `$B2D7` → `LB2EE` `$B2EE`).
// ---------------------------------------------------------------------------

/// Reset `$783-$7A0` from the beginning-values image
/// (`function_reset_link_stats_to_beginning_values`, `$B2CA-$B2D5`).
///
/// `part1` is the 50-byte `$777-$7A8` image (mutable); `beginning` the
/// caller-loaded 50-byte `$777-$7A8` init image (`bank5_Beginning_Values`
/// `$BAE3` + blank-name tail `$BB0D`). Only `Y = $29..$0C` are stored —
/// levels `$777-$782` and the name survive. Returns `false` on short
/// slices (total fn).
pub fn reset_stats_to_beginning(part1: &mut [u8], beginning: &[u8]) -> bool {
    if part1.len() < crate::save_format::PART1_LEN
        || beginning.len() < crate::save_format::PART1_LEN
    {
        return false;
    }
    // `LDY #$29 : LDA Beginning,y : STA $777,y : DEY : CPY #$0C : BCS`.
    let mut y = RESET_HI;
    loop {
        part1[y as usize] = beginning[y as usize];
        if y == RESET_LO {
            break;
        }
        y -= 1;
    }
    true
}

/// Reset `$600-$6DF` from the ROM item-presence image
/// (`bank5_Load_Initial_Item_Presence_Bits`, `$B2D7-$B2E2`: `LDY #$DF`
/// down to `$00`, 224 B).
pub fn reset_item_bits(ram600: &mut [u8], initial: &[u8]) -> bool {
    if ram600.len() < crate::save_format::PART2_LEN || initial.len() < crate::save_format::PART2_LEN
    {
        return false;
    }
    ram600[..crate::save_format::PART2_LEN]
        .copy_from_slice(&initial[..crate::save_format::PART2_LEN]);
    true
}

/// Clear `$7DA-$7FF` (`LB2EE` `$B2EE-$B2FD`: `LDY #$DA`,
/// `STA $700,y : INY : BNE` — walks `$7DA-$7FF`, then `Y` wraps to
/// `$00` and the loop exits WITHOUT storing `$700`: lives keeps its
/// value here and bank 7 (`LC34F` `$C34F` →
/// `bank7_Reset_Number_of_Lives__to_3_`) sets 3 afterwards).
pub fn new_game_ram_clear(ram: &mut [u8; 0x800]) {
    let mut y: u8 = 0xDA;
    loop {
        ram[0x700 + y as usize] = 0;
        y = y.wrapping_add(1);
        if y == 0 {
            break;
        }
    }
}

/// Second-quest gate (`LB2B4` `$B2BC-$B2C1`): a loaded file with
/// `$7A0 == 1` takes the new-game+ reset path (thrust bits kept).
pub const fn second_quest_gate(quest: u8) -> bool {
    quest == 0x01
}

/// Keep mask for the quest-2 reset (`LDA $0796 : AND #$14`, `$B2C3`).
pub const fn thrust_preserve(thrust: u8) -> u8 {
    thrust & THRUST_KEEP
}

/// Second-quest setup on beating the game (`L921C`, bank 5 `$921C-$9247`):
/// `$7A0 = 1`, XP cleared. The `$700-$769`/`$7C0-$7FF`/`$E0-$EF` clears
/// and `INC $076C` (3 wake → 4 credits) are the trap shim's writes —
/// see [`SECOND_QUEST_CLEARS`].
pub const fn second_quest_setup_flag() -> u8 {
    0x01
}

/// RAM ranges `L921C` zeroes (`LDY #$69 : STA $700,y` → `$700-$769`;
/// `LDY #$C0` walk → `$7C0-$7FF`; `LDY #$0F` → `$E0-$EF`).
pub const SECOND_QUEST_CLEARS: [(u16, u8); 3] = [(0x0700, 0x69), (0x0700, 0xC0), (0x00E0, 0x0F)];

// ---------------------------------------------------------------------------
// Presence bits (`bank5_code26`, bank 5 `$B502-$B528`).
//
// HARDWARE TRUTH (verified live on ROM): `bank5_code26` sets a
// `$1A` bit when the slot's 8 name bytes are ALL `$F4` (blank) — the bit
// means EMPTY/available, not occupied. The scan (`LB514` `$B514-$B51B`)
// `BNE`s out on the first non-`$F4` byte (occupied: bit stays clear) and
// only falls through to the `ORA` (`$B51D-$B522`, weights from
// `bank5_table13` `$B4FD`: `01 01 02 02 04`, `X = 4, 2, 0` → slots 2, 1, 0)
// when every byte matched `$F4`. Live trace on a fresh cart: `$1A = $07`
// (all empty) parks the file-select fairy at REGISTER; after naming slot 0
// `$1A = $06` (bit 0 clear = occupied) parks it at slot 0.
// ---------------------------------------------------------------------------

/// Slot emptiness in the `bank5_code26` sense: all 8 name bytes are `$F4`
/// (`LB514` `$B514-$B51B` falls through to the `ORA $1A` only then).
pub fn slot_empty(name8: &[u8]) -> bool {
    name8.iter().copied().all(|b| b == NAME_BLANK)
}

/// Fold per-slot name images into the hardware `$1A` byte (`LB508`/`LB5BD`
/// callers pass the SRAM/SRAM-copy name slices; never embedded here).
///
/// Bit weight per slot is [`presence_bit`] (`bank5_table13` `$B4FD`).
pub fn empty_bits(names: [&[u8]; 3]) -> u8 {
    let mut acc = 0u8;
    for (slot, n) in names.iter().enumerate() {
        if slot_empty(n) {
            acc |= presence_bit(slot as u8);
        }
    }
    acc
}

/// Slot presence (complement of the hardware `$1A` bit): any of the 8 name
/// bytes is not `$F4`.
///
/// NOTE: this is NOT what `bank5_code26` (`$B502`) stores — the hardware
/// sets the bit for all-blank (empty) names; see [`slot_empty`]. Kept
/// (with this contract) for existing callers; new code that models `$1A`
/// must use [`slot_empty`] / [`empty_bits`].
pub fn slot_present(name8: &[u8]) -> bool {
    !slot_empty(name8)
}

/// Presence-bit weight per slot (`bank5_table13` `$B4FD`: `01 01 02 02 04`;
/// `X = 4, 2, 0` maps to slots 2, 1, 0).
pub const fn presence_bit(slot: u8) -> u8 {
    match slot {
        0 => 0x01,
        1 => 0x02,
        2 => 0x04,
        _ => 0x00,
    }
}

/// Fold per-slot occupied flags into a byte with [`presence_bit`] weights.
///
/// NOTE: complement of the hardware `$1A` byte (see [`slot_present`]);
/// the `bank5_code26` fold is [`empty_bits`].
pub fn presence_bits(names: [&[u8]; 3]) -> u8 {
    let mut acc = 0u8;
    for (slot, n) in names.iter().enumerate() {
        if slot_present(n) {
            acc |= presence_bit(slot as u8);
        }
    }
    acc
}

// ---------------------------------------------------------------------------
// Name entry (`LB678` `$B678` → `$B6AC-$B8C4`).
// ---------------------------------------------------------------------------

/// Name-grid columns (11, `$1F`; `CMP #$0B`, `$B7B4`).
pub const GRID_COLS: u8 = 11;
/// Name-grid rows (4, `$20`; `CMP #$04`, `$B818`).
pub const GRID_ROWS: u8 = 4;
/// Row-2 usable columns (6: `W X Y Z - .`; `CMP #$06`, `$B805`).
pub const GRID_ROW2_COLS: u8 = 6;

/// Letter index for a grid cell (`LB89C` `$B89C`: `ADC #$0B` per row).
pub const fn letter_index(col: u8, row: u8) -> u8 {
    col.wrapping_add(row.wrapping_mul(GRID_COLS))
}

/// Tile for a letter index from the caller-loaded 43-byte
/// `bank5_Table_for_Letters_Tile_Mappings` (`$BD75`: A-Z, WXYZ-.,
/// digits; `LDA Table,x`, `$B8BF`).
///
/// Returns `None` past the table end — notably index 43 (row 3, col 10),
/// which on hardware reads `$BDA1` `$FF` padding (quirk documented, not
/// reproduced as a byte: callers render `None` as blank).
pub fn letter_tile(letters: &[u8], idx: u8) -> Option<u8> {
    letters.get(idx as usize).copied()
}

/// Name slot written by A (`LB8A7` `$B8A7`: `LDY $1E : BNE +2 : LDY #8 :
/// DEY` — `$1E == 0` writes index 7, else `pos-1`).
pub const fn name_store_index(letter_pos: u8) -> usize {
    if letter_pos == 0 {
        NAME_LEN - 1
    } else {
        (letter_pos - 1) as usize
    }
}

/// Advance the letter position on A (`INC $1E`, wrap at 8, `$B71B-$B725`).
pub const fn letter_pos_advance(pos: u8) -> u8 {
    let next = pos.wrapping_add(1);
    if next >= NAME_LEN as u8 {
        0
    } else {
        next
    }
}

/// Grid cursor right (`$B7AE-$B7CC`): col+1, wrap 11→0 + row+1; row 2
/// with col ≥ 6 jumps to row 3 (`$B7BA-$B7CA`); row overflows 4→0.
pub const fn grid_right(col: u8, row: u8) -> (u8, u8) {
    let mut c = col.wrapping_add(1);
    let mut r = row;
    if c >= GRID_COLS {
        c = 0;
        r = r.wrapping_add(1);
    }
    if r == 2 && c >= GRID_ROW2_COLS {
        c = 0;
        r = 3;
    }
    if r >= GRID_ROWS {
        // `CMP #$04 : BNE : LDA #0 : STA $1F/$20` (`$B7B0-$B7B8`).
        return (0, 0);
    }
    (c, r)
}

/// Grid cursor left (`LB7CF` `$B7CF`: col-1, borrow into row-1 with
/// col 10 (5 when landing on row 2); row underflow wraps to 3/10.
pub const fn grid_left(col: u8, row: u8) -> (u8, u8) {
    if col > 0 {
        return (col - 1, row);
    }
    // Borrow: `DEC $20 : LDA #$0A : STA $1F` (`$B7D3-$B7D7`).
    let r = row.wrapping_sub(1);
    if r == 2 {
        // `CMP #$02 : BNE : LDA #$05` (`$B7DB-$B7E1`).
        return (GRID_ROW2_COLS - 1, 2);
    }
    if row == 0 {
        // `BPL : LDA #$03/$0A` underflow (`$B7E3-$B7ED`).
        return (GRID_COLS - 1, GRID_ROWS - 1);
    }
    (GRID_COLS - 1, r)
}

/// Grid cursor up on held-Up (`LB7F2` `$B7F2`: row-1, wrap 0→3; col ≥ 6
/// on row 2 steps up again to row 1).
pub const fn grid_up(col: u8, row: u8) -> (u8, u8) {
    let mut r = if row == 0 { GRID_ROWS - 1 } else { row - 1 };
    if col >= GRID_ROW2_COLS && r == 2 {
        r = 1;
    }
    (col, r)
}

/// Grid cursor down on newly-held direction (`LB814` `$B814`: row+1,
/// wrap 4→0; col ≥ 6 on row 2 steps down again to row 3).
pub const fn grid_down(col: u8, row: u8) -> (u8, u8) {
    let mut r = row.wrapping_add(1);
    if r >= GRID_ROWS {
        r = 0;
    }
    if col >= GRID_ROW2_COLS && r == 2 {
        r = 3;
    }
    (col, r)
}

// ---------------------------------------------------------------------------
// Meters, death, continue (`LCB18` `$CB18`, `bank7_code16` `$CA24`,
// `LCA85` `$CA85`, `LCAC4` `$CAC4`, `bank7_code11` `$C41E`).
// ---------------------------------------------------------------------------

/// Refill one meter (`LCB18`, bank 7 `$CB18-$CB26`):
/// `containers * 32 - 1` (`ASL ×5 : SEC : SBC #1`), wrapping like the 6502.
pub const fn refill_meter(containers: u8) -> u8 {
    containers.wrapping_mul(32).wrapping_sub(1)
}

/// Death step (`bank7_code16` tail `$CA44-$CA47` + `LCA6C` `$CA6C`):
/// `DEC $0700`; zero → game over, else new life (`STA $076C = 6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DieStep {
    /// Lives remain: `$76C = 6` (lives-after-decrement included).
    NewLife {
        /// `$0700` after `DEC`.
        lives: u8,
    },
    /// No lives left: game-over screen path.
    GameOver,
}

/// Death step with the post-decrement value made explicit.
pub const fn die_step(lives: u8) -> DieStep {
    let left = lives.wrapping_sub(1);
    if left == 0 {
        DieStep::GameOver
    } else {
        DieStep::NewLife { lives: left }
    }
}

/// Continue deaths counter (`LCA85` `$CAA3-$CAB3`): `INC $079F` unless
/// already `$FF` (saturates — the `BEQ LCAA6` wrap quirk).
pub const fn continue_deaths(deaths: u8) -> u8 {
    if deaths == 0xFF {
        0xFF
    } else {
        deaths.wrapping_add(1)
    }
}

/// Game-over Start choice (`LCA85`-`LCAC4`, `$CA85-$CAC4`): `$488 == 0`
/// (CONTINUE) clears XP and respawns; `$488 != 0` (SAVE) stages the
/// `$7B0` flash timer and returns to the title/save screen (`LCF05`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOverChoice {
    /// CONTINUE: clear XP words, respawn via [`respawn_select`].
    Continue,
    /// SAVE: `$7B0 = $40`, back to title (`LCF05` `$CF05`).
    Save,
}

/// Game-over Start choice from the `$488` menu-selection byte.
pub const fn gameover_start_choice(menu_sel: u8) -> GameOverChoice {
    if menu_sel == 0 {
        GameOverChoice::Continue
    } else {
        GameOverChoice::Save
    }
}

/// Respawn target (`LCAC4`-`LCAF0`, `$CAC4-$CAF0`): Great-Palace runs
/// (`region*5+world == $0F`, `bank7_FUNCTION_CONVERT_706_and_707_to_Rx5plusW`
/// `$CF30`) restart in-palace with 3 lives; elsewhere restart from the
/// castle (`$76C = 0`) — or from town logic via [`respawn_music`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Respawn {
    /// Restart in the Great Palace (`$76C = 1`, lives reset to 3,
    /// area/facing/enter cleared, `$75F = 2`).
    GrandPalace,
    /// Restart from Zelda's castle (`$76C = 0`, `$75F` = music byte).
    Castle {
        /// `$075F` music byte from [`respawn_music`].
        music: u8,
    },
}

/// Region×5+world convert (`bank7_FUNCTION_CONVERT_706_and_707_to_Rx5plusW`
/// `$CF30`: `region*4 + region + world`); `$0F` ⟺ Great Palace.
pub const fn region_world_code(region: u8, world: u8) -> u8 {
    region
        .wrapping_mul(4)
        .wrapping_add(region)
        .wrapping_add(world)
}

/// Respawn target from region/world/town (`$CAD0-$CAF0`).
pub fn respawn_select(region: u8, world: u8, town: u8) -> Respawn {
    if region_world_code(region, world) == 0x0F {
        Respawn::GrandPalace
    } else {
        Respawn::Castle {
            music: respawn_music(world, town),
        }
    }
}

/// Respawn music byte (`bank7_code11` `$C41E-$C446`): 4 on the overworld
/// (`world == 0`) or in Rauru-side towns (`world 1-2, town == 7`),
/// else 2.
pub const fn respawn_music(world: u8, town: u8) -> u8 {
    if world == 0 {
        4
    } else if world >= 3 {
        2
    } else if town == 7 {
        4
    } else {
        2
    }
}

/// Manual-save combo gate (`bank0_Manual_Save_Game_Routine_UP_AND_A`,
/// bank 0 `$A19C-$A1A3`): pad-2 held `== $88` (Up `$08` + A `$80`).
///
/// On fire the game takes the death path (`$76C = $76D = 2`, `$736 = 5`,
/// dialog/menu/XP cleared, `$768 = 6`, HP+MP refilled) — the actual SRAM
/// write happens on the death flow (`LCF21_SaveGameWhenChooseSAVE…`
/// `$CF21` → `LB9CA`).
pub const fn manual_save_gate(pad2_held: u8) -> bool {
    pad2_held == MANUAL_SAVE_COMBO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refill_meter_matches_lcb18() {
        assert_eq!(refill_meter(4), 127);
        assert_eq!(refill_meter(8), 255);
        assert_eq!(refill_meter(0), 255); // wraps like SEC+SBC
    }

    #[test]
    fn deaths_saturate_at_ff() {
        assert_eq!(continue_deaths(0), 1);
        assert_eq!(continue_deaths(0xFE), 0xFF);
        assert_eq!(continue_deaths(0xFF), 0xFF);
    }
}
