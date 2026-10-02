//! Zelda II dialog text codec.
//!
//! The game's dialog font is not ASCII. The mapping below was derived from
//! how the game draws its own dialog (the town text renderer in
//! `z2-core::town_dialog` and the behavioural catalog): digits and capital
//! letters are two contiguous runs, a few punctuation glyphs sit elsewhere,
//! and three control bytes drive the dialog box.
//!
//! | text | byte(s) |
//! |---|---|
//! | `0`-`9` | `$D0`-`$D9` |
//! | `A`-`Z` | `$DA`-`$F3` (input is upper-cased) |
//! | space | `$F4` |
//! | `-` | `$F6` |
//! | `.` | `$CF` |
//! | `/` | `$CE` |
//! | `,` | `$9C` |
//! | `!` | `$36` |
//! | `?` | `$34` |
//! | `*` | `$32` |
//! | line break (`\n` or `$`) | `$FD` |
//! | pause (`~`) | `$FE` |
//! | end of message | `$FF` |
//!
//! Any other byte round-trips as a `{XX}` hex escape, and `{XX}` in input
//! emits that byte, so odd glyphs (borders, symbols) stay reachable.
//!
//! Vanilla messages live in PRG bank 3 behind a 98-entry pointer table
//! ([`read_vanilla_messages`]); they are read from the player's ROM at run
//! time and never stored in this repository.

use crate::rom::Rom;
use crate::RandoError;

/// Line break control byte.
pub const NEWLINE: u8 = 0xFD;
/// Pause control byte.
pub const PAUSE: u8 = 0xFE;
/// End-of-message byte.
pub const END: u8 = 0xFF;
/// Space glyph.
pub const SPACE: u8 = 0xF4;

/// Vanilla message pointer table: bank 3, CPU address.
pub const VANILLA_POINTER_TABLE: u16 = 0xAFBE;
/// Bank holding the vanilla messages and their pointer table.
pub const TEXT_BANK: u8 = 3;
/// Number of vanilla messages (two town tables back to back).
pub const VANILLA_MESSAGE_COUNT: usize = 98;
/// Widest dialog line the box shows, in characters.
pub const MAX_LINE_CHARS: usize = 11;
/// Lines the vanilla dialog box shows at once.
pub const VANILLA_MAX_LINES: usize = 4;

/// Game byte for one printable character, if it has one.
#[must_use]
pub fn char_to_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_uppercase();
    Some(match c {
        '0'..='9' => 0xD0 + (c as u8 - b'0'),
        'A'..='Z' => 0xDA + (c as u8 - b'A'),
        ' ' => SPACE,
        '-' => 0xF6,
        '.' => 0xCF,
        '/' => 0xCE,
        ',' => 0x9C,
        '!' => 0x36,
        '?' => 0x34,
        '*' => 0x32,
        _ => return None,
    })
}

/// Printable character for one game byte (controls and unknown glyphs
/// return `None`).
#[must_use]
pub fn byte_to_char(b: u8) -> Option<char> {
    Some(match b {
        0xD0..=0xD9 => (b'0' + (b - 0xD0)) as char,
        0xDA..=0xF3 => (b'A' + (b - 0xDA)) as char,
        SPACE => ' ',
        0xF6 => '-',
        0xCF => '.',
        0xCE => '/',
        0x9C => ',',
        0x36 => '!',
        0x34 => '?',
        0x32 => '*',
        _ => return None,
    })
}

/// Encode `s` into game bytes, **without** the end byte. `\n` or `$` is a
/// line break, `~` a pause, `{XX}` a raw byte.
pub fn encode(s: &str) -> Result<Vec<u8>, RandoError> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' | '$' => out.push(NEWLINE),
            '~' => out.push(PAUSE),
            '{' => {
                let hex: String = chars.by_ref().take(2).collect();
                if chars.next() != Some('}') || hex.len() != 2 {
                    return Err(RandoError::Text(format!("bad escape in {s:?}")));
                }
                out.push(
                    u8::from_str_radix(&hex, 16)
                        .map_err(|_| RandoError::Text(format!("bad escape {{{hex}}}")))?,
                );
            }
            _ => out.push(char_to_byte(c).ok_or_else(|| {
                RandoError::Text(format!("character {c:?} has no glyph in the dialog font"))
            })?),
        }
    }
    Ok(out)
}

/// [`encode`] plus the end byte.
pub fn encode_message(s: &str) -> Result<Vec<u8>, RandoError> {
    let mut v = encode(s)?;
    v.push(END);
    Ok(v)
}

/// Decode game bytes up to (not including) the first end byte. Line breaks
/// become `\n`, pauses `~`, unknown bytes `{XX}`.
#[must_use]
pub fn decode(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            END => break,
            NEWLINE => s.push('\n'),
            PAUSE => s.push('~'),
            _ => match byte_to_char(b) {
                Some(c) => s.push(c),
                None => s.push_str(&format!("{{{b:02X}}}")),
            },
        }
    }
    s
}

/// Check that `s` fits a dialog box: at most `max_lines` lines of at most
/// [`MAX_LINE_CHARS`] characters.
///
/// A pause (`~`, byte `$FE`) is a line break with a longer delay, not a new
/// page: the game's end-of-line routine (bank 3 `$B656`) treats `$FD` and
/// `$FE` the same apart from the delay, so both count as line breaks here.
pub fn validate_dialog(s: &str, max_lines: usize) -> Result<(), RandoError> {
    let lines: Vec<&str> = s.split(['\n', '$', '~']).collect();
    if lines.len() > max_lines {
        return Err(RandoError::Text(format!(
            "{s:?}: {} lines, the box holds {max_lines}",
            lines.len()
        )));
    }
    for l in lines {
        let width = l.chars().count() - 3 * l.matches('{').count();
        if width > MAX_LINE_CHARS {
            return Err(RandoError::Text(format!(
                "{l:?} is {width} characters, a line holds {MAX_LINE_CHARS}"
            )));
        }
    }
    Ok(())
}

/// Word-wrap plain text into dialog lines of at most [`MAX_LINE_CHARS`]
/// characters joined with `\n`. Returns `None` when a word is too long or
/// the result needs more than `max_lines` lines.
#[must_use]
pub fn wrap(text: &str, max_lines: usize) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        if word.chars().count() > MAX_LINE_CHARS {
            return None;
        }
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= MAX_LINE_CHARS => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    if lines.is_empty() || lines.len() > max_lines {
        return None;
    }
    Some(lines.join("\n"))
}

/// First byte of the vanilla dialog text block (bank 3).
pub const TEXT_DATA_START: u16 = 0xA380;
/// End (exclusive) of the vanilla dialog text block: the pointer table
/// follows directly.
pub const TEXT_DATA_END: u16 = VANILLA_POINTER_TABLE;
/// Bytes available for dialog text in the vanilla block (3134).
pub const TEXT_BUDGET: usize = (TEXT_DATA_END - TEXT_DATA_START) as usize;

/// The whole dialog table, editable, and written back in one go.
///
/// Modules get it through [`table`] (it lives in [`crate::State`]), change
/// messages by index, and the pipeline writes it once at the end
/// ([`flush`]). Writing re-packs every message into the vanilla text block
/// (`$A380-$AFBD` in bank 3, 3134 bytes), sharing identical messages and
/// messages that are the tail of another, and rewrites the 98-entry
/// pointer table in place. The game reads every dialog through that table
/// (bank 3 `$B5F8`, run as ROM code), so moved text is always found.
///
/// An untouched table is never written, so vanilla flags keep the ROM
/// byte-identical.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextTable {
    msgs: Vec<Vec<u8>>,
    dirty: bool,
}

impl TextTable {
    /// Read the current table from `rom`.
    pub fn read(rom: &Rom) -> Result<TextTable, RandoError> {
        Ok(TextTable {
            msgs: read_vanilla_messages(rom)?,
            dirty: false,
        })
    }

    /// Number of messages (98).
    #[must_use]
    pub fn len(&self) -> usize {
        self.msgs.len()
    }

    /// Whether the table has no messages (never true for a real ROM).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.msgs.is_empty()
    }

    /// Whether anything was changed.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Raw bytes of message `i` (including the end byte).
    #[must_use]
    pub fn bytes(&self, i: usize) -> &[u8] {
        &self.msgs[i]
    }

    /// Message `i` as text (see [`decode`]).
    #[must_use]
    pub fn get(&self, i: usize) -> String {
        decode(&self.msgs[i])
    }

    /// Replace message `i` with `s`, which must fit the dialog box
    /// ([`VANILLA_MAX_LINES`] lines of [`MAX_LINE_CHARS`]).
    pub fn set(&mut self, i: usize, s: &str) -> Result<(), RandoError> {
        validate_dialog(s, VANILLA_MAX_LINES)?;
        let bytes = encode_message(s)?;
        self.set_bytes(i, bytes)
    }

    /// Replace message `i` with raw bytes (an end byte is appended if
    /// missing).
    pub fn set_bytes(&mut self, i: usize, mut bytes: Vec<u8>) -> Result<(), RandoError> {
        if i >= self.msgs.len() {
            return Err(RandoError::Text(format!("no message {i}")));
        }
        if bytes.last() != Some(&END) {
            bytes.push(END);
        }
        if self.msgs[i] != bytes {
            self.msgs[i] = bytes;
            self.dirty = true;
        }
        Ok(())
    }

    /// Lay out the messages: returns the packed block and each message's
    /// offset into it. Identical messages and messages that end another
    /// message share bytes. Longest messages are placed first so their
    /// tails are available for sharing.
    #[must_use]
    pub fn pack(&self) -> (Vec<u8>, Vec<usize>) {
        let mut order: Vec<usize> = (0..self.msgs.len()).collect();
        // Stable order: longest first, then by index.
        order.sort_by(|&a, &b| self.msgs[b].len().cmp(&self.msgs[a].len()).then(a.cmp(&b)));
        let mut block: Vec<u8> = Vec::new();
        // Start offsets of whole placed messages (a tail of one ends in END
        // at the same place, so any suffix match inside a placed message is
        // valid).
        let mut placed: Vec<(usize, usize)> = Vec::new(); // (start, len)
        let mut offs = vec![0usize; self.msgs.len()];
        for i in order {
            let m = &self.msgs[i];
            let shared = placed.iter().find_map(|&(start, len)| {
                let whole = &block[start..start + len];
                whole.ends_with(m).then(|| start + len - m.len())
            });
            offs[i] = match shared {
                Some(o) => o,
                None => {
                    let o = block.len();
                    block.extend_from_slice(m);
                    placed.push((o, m.len()));
                    o
                }
            };
        }
        (block, offs)
    }

    /// Bytes [`TextTable::write`] would use.
    #[must_use]
    pub fn packed_len(&self) -> usize {
        self.pack().0.len()
    }

    /// Write the table to `rom` (does nothing when unchanged). Fails if the
    /// packed text exceeds [`TEXT_BUDGET`].
    pub fn write(&self, rom: &mut Rom) -> Result<(), RandoError> {
        if !self.dirty {
            return Ok(());
        }
        let (mut block, offs) = self.pack();
        if block.len() > TEXT_BUDGET {
            return Err(RandoError::Text(format!(
                "dialog text needs {} bytes, the block holds {TEXT_BUDGET}",
                block.len()
            )));
        }
        // Clear the rest of the block so stale text does not linger.
        block.resize(TEXT_BUDGET, END);
        rom.write_cpu(TEXT_BANK, TEXT_DATA_START, &block)?;
        for (i, off) in offs.iter().enumerate() {
            let addr = TEXT_DATA_START + u16::try_from(*off).expect("fits the bank");
            rom.write_cpu_word(TEXT_BANK, VANILLA_POINTER_TABLE + 2 * i as u16, addr)?;
        }
        Ok(())
    }
}

/// The shared dialog table for this attempt, read from `ctx.rom` on first
/// use.
pub fn table(ctx: &mut crate::Ctx) -> Result<&mut TextTable, RandoError> {
    if ctx.state.text.is_none() {
        ctx.state.text = Some(TextTable::read(&ctx.rom)?);
    }
    Ok(ctx.state.text.as_mut().expect("just set"))
}

/// Write the shared dialog table into `ctx.rom` if any module changed it.
/// The pipeline calls this once after the last module.
pub fn flush(ctx: &mut crate::Ctx) -> Result<(), RandoError> {
    if let Some(t) = ctx.state.text.take() {
        t.write(&mut ctx.rom)?;
    }
    Ok(())
}

/// The vanilla messages (each including its end byte), read through the
/// pointer table in bank 3 of `rom`.
pub fn read_vanilla_messages(rom: &Rom) -> Result<Vec<Vec<u8>>, RandoError> {
    let mut out = Vec::with_capacity(VANILLA_MESSAGE_COUNT);
    for i in 0..VANILLA_MESSAGE_COUNT {
        let ptr = rom.read_cpu_word(TEXT_BANK, VANILLA_POINTER_TABLE + 2 * i as u16)?;
        let mut msg = Vec::new();
        let mut addr = ptr;
        loop {
            let b = rom.read_cpu(TEXT_BANK, addr)?;
            msg.push(b);
            if b == END {
                break;
            }
            if msg.len() > 512 || addr >= 0xBFFF {
                return Err(RandoError::Text(format!(
                    "message {i} at ${ptr:04X} has no end"
                )));
            }
            addr += 1;
        }
        out.push(msg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_punctuation() {
        assert_eq!(encode("A").unwrap(), vec![0xDA]);
        assert_eq!(encode("z").unwrap(), vec![0xF3]);
        assert_eq!(encode("0 9").unwrap(), vec![0xD0, SPACE, 0xD9]);
        assert_eq!(
            encode("-./,!?*").unwrap(),
            vec![0xF6, 0xCF, 0xCE, 0x9C, 0x36, 0x34, 0x32]
        );
        assert_eq!(
            encode("A\nB$C~D").unwrap(),
            vec![0xDA, NEWLINE, 0xDB, NEWLINE, 0xDC, PAUSE, 0xDD]
        );
        assert_eq!(encode_message("HI").unwrap(), vec![0xE1, 0xE2, END]);
    }

    #[test]
    fn round_trip_including_escapes() {
        let s = "HELLO, LINK!\nTAKE THIS.~{CA}{05} 123";
        let bytes = encode(s).unwrap();
        assert_eq!(decode(&bytes), s);
        // Every byte value survives decode -> encode.
        let all: Vec<u8> = (0..=0xFEu8).collect();
        assert_eq!(encode(&decode(&all)).unwrap(), all);
    }

    #[test]
    fn decode_stops_at_end() {
        assert_eq!(decode(&[0xDA, END, 0xDB]), "A");
    }

    #[test]
    fn rejects_unknown_characters_and_bad_escapes() {
        assert!(encode("#").is_err());
        assert!(encode("{G1}").is_err());
        assert!(encode("{1").is_err());
    }

    #[test]
    fn dialog_size_limits() {
        assert!(validate_dialog("ABCDEFGHIJK\nB\nC\nD", 4).is_ok());
        assert!(validate_dialog("ABCDEFGHIJKL", 4).is_err());
        assert!(validate_dialog("A\nB\nC\nD\nE", 4).is_err());
        // A pause is a line break, not a new page.
        assert!(validate_dialog("A\nB\nC\nD~E\nF", 4).is_err());
        assert!(validate_dialog("A\nB~C\nD", 4).is_ok());
        assert!(validate_dialog("{CA}BCDEFGHIJK", 4).is_ok());
    }

    /// ROM-gated: every vanilla message decodes into the known glyph set and
    /// re-encodes to the same bytes.
    #[test]
    #[ignore = "needs Z2_ROM"]
    fn vanilla_messages_round_trip() {
        let body = z2_assets::rom::open().expect("Z2_ROM");
        let rom = Rom::from_body(&body).unwrap();
        let msgs = read_vanilla_messages(&rom).unwrap();
        assert_eq!(msgs.len(), VANILLA_MESSAGE_COUNT);
        for m in &msgs {
            let s = decode(m);
            assert!(!s.contains('{'), "unexpected glyph in {s:?}");
            assert_eq!(encode_message(&s).unwrap(), *m);
            validate_dialog(&s, 6).unwrap();
        }
    }
}
