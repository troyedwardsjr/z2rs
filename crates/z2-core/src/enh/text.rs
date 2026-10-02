//! Text and HUD enhancements (group `T`).
//!
//! Options ([`TextHudOpts`]):
//!
//! * `dialogue_speed` (`0` = original, `1..=5`, `5` = fastest): how fast
//!   dialogue boxes print. ZALiA reference (`mod_DLG_SPEED`, default 2): the
//!   next-character delay per level is 5, 4, 3, 2, 1, 0 frames (the original
//!   is `$05`); the first-character, next-line and long-line delays scale with
//!   it, floored at `$0A` / `$02` / `$08`.
//! * `protect_spell_name`: show the SHIELD spell as "PROTECT" in the spell
//!   menu (ZALiA `mod_SpellSHIELD_NAME=1`).
//!
//! # Mechanism
//!
//! Both options are **PRG patches** ([`super::patch_prg_at`]); no trap, no
//! per-frame work, no [`super::EnhState`] slot.
//!
//! **Dialogue speed.** The town/NPC typewriter is bank-3 ROM code that runs
//! in the interpreter (the shipped trap table has nothing below `$C000`).
//! Its only timer is `$0566`: every frame `LB6A2` (`$B6A2`) decrements it
//! while it is non-zero (`$B6BA: LDA $0566 / BNE -> $B69E: DEC $0566`) and
//! prints the next byte once it reaches zero, so a delay of `d` costs `d + 1`
//! frames. Four immediate operands load it, and only those are patched:
//!
//! | CPU (bank 3) | instruction | original | meaning | floor |
//! |---|---|---|---|---|
//! | `$B614` | `LDA #imm` (`$B615`) | `$2A` | lead-in before the first letter | `$0A` |
//! | `$B656` | `LDY #imm` (`$B657`) | `$0B` | after a `$FD` line break | `$02` |
//! | `$B65C` | `LDY #imm` (`$B65D`) | `$2D` | after a `$FE` long break | `$08` |
//! | `$B74D` | `LDA #imm` (`$B74E`) | `$05` | between letters | `$00` |
//!
//! Level `L` loads `floor + (original - floor) * (5 - L) / 5` (integer
//! division): level 0 is the original byte, level 5 the floor, and the
//! letter delay is exactly ZALiA's `5 - L`. The bytes that are drawn, the
//! cursor walk (`$0489`/`$048A`), the `$FF` teardown and the B-to-skip check
//! are untouched, so every glyph lands exactly where it did, only sooner.
//!
//! **PROTECT.** The pause pane's spell list is a tile table in bank 0
//! (`$9C29`: `" SHIELD....."`, twelve tiles: a space, the name, `$CF` dot
//! padding; THUNDER, the longest original name, is seven letters). "PROTECT"
//! is seven letters, so it fits the slot by overwriting the name and its first
//! dot (`$9C2A-$9C30`), leaving four dots exactly like THUNDER's row (on
//! screen the pane writes the spell's MP cost over the last three dots, so
//! the row reads `PROTECT.` plus the cost, again like THUNDER). The
//! wise man's line in Rauru ("...STRENGTHEN A SHIELD.", bank 3 `$ADB5`) is
//! dialogue, not the spell's name, and is left alone (the longer word would
//! need the text re-flowed and re-pointed).
//!
//! Each patch first checks that the ROM holds the expected original byte(s)
//! and skips otherwise (other revisions, synthetic test images).

use serde::{Deserialize, Serialize};

use crate::game::Game;

/// Largest [`TextHudOpts::dialogue_speed`] (fastest: one letter a frame).
pub const MAX_DIALOGUE_SPEED: u8 = 5;

/// Text and HUD options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TextHudOpts {
    /// `0` = original speed, `1..=5` faster (`5` = one letter a frame).
    pub dialogue_speed: u8,
    /// Rename SHIELD to PROTECT.
    pub protect_spell_name: bool,
}

impl TextHudOpts {
    /// Whether anything in this group is on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        out.push(self.dialogue_speed);
        out.push(u8::from(self.protect_spell_name));
    }
}

/// PRG bank holding the town/NPC dialogue typewriter.
pub const DIALOG_BANK: u8 = 3;

/// One dialogue delay: the CPU address of the immediate operand (bank 3),
/// the original value and the floor reached at [`MAX_DIALOGUE_SPEED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogDelay {
    /// CPU address of the `#imm` operand byte.
    pub operand: u16,
    /// Value in the original ROM.
    pub original: u8,
    /// Value at the fastest level.
    pub floor: u8,
}

/// The four dialogue delays (see the module docs).
pub const DIALOG_DELAYS: [DialogDelay; 4] = [
    // Lead-in before the first letter ($B614: LDA #$2A / STA $0566).
    DialogDelay {
        operand: 0xB615,
        original: 0x2A,
        floor: 0x0A,
    },
    // After a $FD line break ($B656: LDY #$0B).
    DialogDelay {
        operand: 0xB657,
        original: 0x0B,
        floor: 0x02,
    },
    // After a $FE long break ($B65C: LDY #$2D).
    DialogDelay {
        operand: 0xB65D,
        original: 0x2D,
        floor: 0x08,
    },
    // Between letters ($B74D: LDA #$05 / STA $0566).
    DialogDelay {
        operand: 0xB74E,
        original: 0x05,
        floor: 0x00,
    },
];

/// The delay `d` loads at dialogue speed `level` (clamped to
/// [`MAX_DIALOGUE_SPEED`]): `floor + (original - floor) * (5 - level) / 5`.
#[must_use]
pub const fn scaled_delay(d: DialogDelay, level: u8) -> u8 {
    let level = if level > MAX_DIALOGUE_SPEED {
        MAX_DIALOGUE_SPEED
    } else {
        level
    };
    let span = (d.original - d.floor) as u16;
    let rest = (MAX_DIALOGUE_SPEED - level) as u16;
    d.floor + (span * rest / MAX_DIALOGUE_SPEED as u16) as u8
}

/// PRG bank of the pause-pane tile table.
pub const SPELL_MENU_BANK: u8 = 0;
/// CPU address (bank 0) of the first letter of "SHIELD" in the pause pane.
pub const SHIELD_NAME_ADDR: u16 = 0x9C2A;
/// Width of a spell-name slot after the leading space (name + `$CF` dots).
pub const SPELL_NAME_SLOT: usize = 11;
/// Dot padding tile in the pause pane.
const TILE_DOT: u8 = 0xCF;

/// Encode `A-Z` (and space) in the game's text tiles (`A` = `$DA`).
#[must_use]
pub fn encode_text(s: &str) -> Vec<u8> {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' => 0xDA + (c - b'A'),
            _ => 0xF4,
        })
        .collect()
}

/// Read the byte at `bank`:`cpu_addr` of the in-memory PRG, if in range.
fn prg_byte(game: &Game, bank: u8, cpu_addr: u16) -> Option<u8> {
    let off = usize::from(bank) * 0x4000 + usize::from(cpu_addr & 0x3FFF);
    game.prg.get(off).copied()
}

/// Install this group's hooks ([`super::apply`], only when active).
pub(crate) fn register(game: &mut Game, opts: &TextHudOpts) {
    if opts.dialogue_speed > 0 {
        for d in DIALOG_DELAYS {
            if prg_byte(game, DIALOG_BANK, d.operand) == Some(d.original) {
                let v = scaled_delay(d, opts.dialogue_speed);
                super::patch_prg_at(game, DIALOG_BANK, d.operand, &[v]);
            }
        }
    }
    if opts.protect_spell_name {
        let old = encode_text("SHIELD");
        let new = encode_text("PROTECT");
        // The slot must hold SHIELD followed by at least one dot, so the
        // seventh letter only replaces padding.
        let fits = new.len() <= SPELL_NAME_SLOT
            && (0..=old.len()).all(|i| {
                let want = old.get(i).copied().unwrap_or(TILE_DOT);
                prg_byte(game, SPELL_MENU_BANK, SHIELD_NAME_ADDR + i as u16) == Some(want)
            });
        if fits {
            super::patch_prg_at(game, SPELL_MENU_BANK, SHIELD_NAME_ADDR, &new);
        }
    }
}

/// Per-frame work after [`Game::step`] (only while any enhancement is on).
/// Nothing: both options are static PRG patches.
pub(crate) fn end_of_frame(game: &mut Game, opts: &TextHudOpts) {
    let _ = (game, opts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_delay_follows_zalia_table() {
        let letter = DIALOG_DELAYS[3];
        let got: Vec<u8> = (0..=5).map(|l| scaled_delay(letter, l)).collect();
        assert_eq!(got, vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn other_delays_scale_to_their_floors() {
        for d in DIALOG_DELAYS {
            assert_eq!(scaled_delay(d, 0), d.original);
            assert_eq!(scaled_delay(d, 5), d.floor);
            assert_eq!(scaled_delay(d, 200), d.floor, "clamped");
            for l in 0..5 {
                assert!(scaled_delay(d, l + 1) <= scaled_delay(d, l));
            }
        }
        assert_eq!(scaled_delay(DIALOG_DELAYS[0], 2), 0x0A + 32 * 3 / 5);
    }

    #[test]
    fn encode_matches_the_menu_alphabet() {
        assert_eq!(
            encode_text("SHIELD"),
            vec![0xEC, 0xE1, 0xE2, 0xDE, 0xE5, 0xDD]
        );
        assert_eq!(encode_text("PROTECT").len(), 7);
    }

    /// A synthetic PRG with the original bytes at the patched spots.
    fn synthetic() -> Game {
        let mut g = Game::new();
        g.prg = vec![0; 8 * 0x4000];
        for d in DIALOG_DELAYS {
            let off = usize::from(DIALOG_BANK) * 0x4000 + usize::from(d.operand & 0x3FFF);
            g.prg[off] = d.original;
        }
        let off = usize::from(SHIELD_NAME_ADDR & 0x3FFF);
        let mut row = encode_text("SHIELD");
        row.extend_from_slice(&[TILE_DOT; 5]);
        g.prg[off..off + row.len()].copy_from_slice(&row);
        g
    }

    #[test]
    fn patches_apply_and_undo() {
        let mut g = synthetic();
        let orig = g.prg.clone();
        let mut e = crate::enh::Enhancements::default();
        e.text.dialogue_speed = 5;
        e.text.protect_spell_name = true;
        g.set_enhancements(e);
        assert_eq!(g.traps.len(), Game::new().traps.len(), "no traps");
        for d in DIALOG_DELAYS {
            assert_eq!(prg_byte(&g, DIALOG_BANK, d.operand), Some(d.floor));
        }
        let off = usize::from(SHIELD_NAME_ADDR & 0x3FFF);
        let mut want = encode_text("PROTECT");
        want.extend_from_slice(&[TILE_DOT; 4]);
        assert_eq!(&g.prg[off..off + 11], &want[..]);
        g.set_enhancements(crate::enh::Enhancements::default());
        assert_eq!(g.prg, orig);
    }

    #[test]
    fn unexpected_rom_bytes_are_left_alone() {
        let mut g = Game::new();
        g.prg = vec![0x77; 8 * 0x4000];
        let orig = g.prg.clone();
        let mut e = crate::enh::Enhancements::default();
        e.text.dialogue_speed = 3;
        e.text.protect_spell_name = true;
        g.set_enhancements(e);
        assert_eq!(g.prg, orig);
    }
}
