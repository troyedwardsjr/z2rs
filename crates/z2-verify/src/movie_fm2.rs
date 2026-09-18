//! FCEUX `.fm2` movie parser for the snapshot corpus.
//!
//! `std`-only: no ROM, no network, no third-party crates,
//! so this file compiles standalone (`rustc --test movie_fm2.rs`) as well as
//! wired as `z2_verify::movie_fm2`.
//!
//! ## Format recap
//!
//! * Header: `key value` text lines (e.g. `version 3`, `palFlag 0`,
//!   `romFilename Zelda II - The Adventure of Link (USA).nes`).
//! * Input lines: `|command|pad0|pad1|...|` where `command` is a decimal byte
//!   and each pad field is 8 columns, left-to-right, with the all-pressed
//!   mnemonics `RLDUTSBA`:
//!
//! | col | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
//! |-----|---|---|---|---|---|---|---|----|
//! | key | R | L | D | U | T | S | B | A  |
//! | NES bit | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
//!
//! i.e. display order is `RLDUTSBA` while the NES shift-register bit order is
//! `A,B,Select,Start,Up,Down,Left,Right` (bit 0 = A … bit 7 = Right).
//! Released cells are `.` (FCEUX canonical) or a space.
//!
//! Command byte (best-effort per FCEUX `movie.h`; the raw byte is always
//! preserved): bit 0 (`0x01`) = soft reset, bit 1 (`0x02`) = power cycle.
//!
//! ## Edge cases handled
//!
//! 1. LF / CRLF / lone-CR line endings.
//! 2. UTF-8 BOM stripped.
//! 3. Blank lines and trailing whitespace ignored everywhere.
//! 4. Header keys looked up case-insensitively; duplicates keep first value.
//! 5. `comment` / `subtitle` lines are header lines, preserved verbatim.
//! 6. Extra pad ports (`|0|…|…|`) preserved; port 0 is the player track.
//! 7. Pad field must be exactly 8 cells; anything else errors with line no.
//! 8. Unknown pad glyphs error with line no + column (strict mnemonics).
//! 9. Bad command bytes error with line no.
//! 10. Semantic oddities (PAL flag, missing ROM name, fourscore, …) are
//!     surfaced via [`Fm2Movie::warnings`] instead of failing the parse.

use std::error::Error;
use std::fmt;

/// Left-to-right pad column mnemonics for the all-pressed line `RLDUTSBA`.
pub const FM2_COL_MNEMONIC: [char; 8] = ['R', 'L', 'D', 'U', 'T', 'S', 'B', 'A'];

/// NES shift-register bit index for each left-to-right pad column.
pub const FM2_COL_BITS: [u8; 8] = [7, 6, 5, 4, 3, 2, 1, 0];

/// NES button names in LSB-first bit order (bit 0 = A … bit 7 = Right).
pub const NES_BIT_NAMES: [&str; 8] = ["A", "B", "Select", "Start", "Up", "Down", "Left", "Right"];

/// Command-byte bit: soft reset this frame.
pub const CMD_RESET: u8 = 0x01;
/// Command-byte bit: power cycle this frame.
pub const CMD_POWER: u8 = 0x02;

/// Substring match (case-insensitive) for the pinned USA ROM file name.
pub const Z2_ROM_NAME_HINT: &str = "zelda";

/// Parse error with a 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fm2Error {
    /// 1-based line number (`0` = whole-file error, e.g. bad UTF-8).
    pub line: u32,
    /// Human-readable cause.
    pub msg: String,
}

impl fmt::Display for Fm2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "fm2: {}", self.msg)
        } else {
            write!(f, "fm2 line {}: {}", self.line, self.msg)
        }
    }
}

impl Error for Fm2Error {}

fn err(line: u32, msg: impl Into<String>) -> Fm2Error {
    Fm2Error {
        line,
        msg: msg.into(),
    }
}

/// Ordered `key value` header. Lookup is case-insensitive on the key.
#[derive(Debug, Clone, Default)]
pub struct Fm2Header {
    fields: Vec<(String, String)>,
}

impl Fm2Header {
    /// First value for `key` (case-insensitive), if present.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// Raw ordered fields, in file order.
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    fn get_u8(&self, key: &str) -> Option<u8> {
        self.get(key)?.trim().parse::<u8>().ok()
    }

    fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key)?.trim().parse::<u64>().ok()
    }

    /// `version` header (FCEUX writes `3`).
    pub fn version(&self) -> Option<u64> {
        self.get_u64("version")
    }
    /// `palFlag` header (`0` = NTSC, `1` = PAL).
    pub fn pal_flag(&self) -> Option<u8> {
        self.get_u8("palFlag")
    }
    /// `rerecordCount` header.
    pub fn rerecord_count(&self) -> Option<u64> {
        self.get_u64("rerecordCount")
    }
    /// `romFilename` header, if present.
    pub fn rom_filename(&self) -> Option<&str> {
        self.get("romFilename")
    }
    /// `romChecksum` header (usually `base64:<md5>`), if present.
    pub fn rom_checksum(&self) -> Option<&str> {
        self.get("romChecksum")
    }
    /// `guid` header, if present.
    pub fn guid(&self) -> Option<&str> {
        self.get("guid")
    }
    /// True when the ROM file name looks like Zelda II USA.
    pub fn rom_name_looks_like_z2_usa(&self) -> bool {
        self.rom_filename()
            .map(|n| n.to_ascii_lowercase().contains(Z2_ROM_NAME_HINT))
            .unwrap_or(false)
    }
}

/// One parsed input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fm2Frame {
    /// Raw command byte (bit 0 = reset, bit 1 = power).
    pub command: u8,
    /// Pad bytes, port 0 first, in NES bit order (bit 0 = A … bit 7 = Right).
    pub pads: Vec<u8>,
    /// 1-based source line number (for divergence reports).
    pub line_no: u32,
}

impl Fm2Frame {
    /// Player-1 pad byte (port 0).
    pub fn pad1(&self) -> u8 {
        self.pads.first().copied().unwrap_or(0)
    }
    /// Soft-reset flag for this frame.
    pub fn is_reset(&self) -> bool {
        self.command & CMD_RESET != 0
    }
    /// Power-cycle flag for this frame.
    pub fn is_power(&self) -> bool {
        self.command & CMD_POWER != 0
    }
    /// Is button `bit` (0 = A … 7 = Right) held on port 0?
    pub fn button(&self, bit: u8) -> bool {
        bit < 8 && (self.pad1() >> bit) & 1 == 1
    }
}

/// A fully parsed `.fm2` movie.
#[derive(Debug, Clone)]
pub struct Fm2Movie {
    /// File header.
    pub header: Fm2Header,
    /// One entry per `|…|` input line, in order.
    pub frames: Vec<Fm2Frame>,
    /// Max port count seen on any line (Zelda II uses 1).
    pub port_count: usize,
}

impl Fm2Movie {
    /// Player-1 input track: one NES pad byte per frame.
    pub fn pad1_track(&self) -> Vec<u8> {
        self.frames.iter().map(|f| f.pad1()).collect()
    }

    /// Non-fatal semantic warnings (PAL flag, ROM-name mismatch, …).
    /// Empty for a clean NTSC Zelda II USA movie.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        match self.header.version() {
            Some(3) => {}
            Some(v) => w.push(format!("unexpected version {v} (want 3)")),
            None => w.push("missing version header".to_string()),
        }
        match self.header.pal_flag() {
            Some(0) => {}
            Some(1) => w.push("palFlag=1: PAL movie vs pinned NTSC USA ROM".to_string()),
            Some(v) => w.push(format!("odd palFlag={v}")),
            None => w.push("missing palFlag header".to_string()),
        }
        match self.header.rom_filename() {
            Some(_) if self.header.rom_name_looks_like_z2_usa() => {}
            Some(n) => w.push(format!("romFilename {n:?} does not look like Zelda II")),
            None => w.push("missing romFilename header".to_string()),
        }
        if self
            .header
            .get("fourscore")
            .is_some_and(|v| v.trim() != "0")
        {
            w.push("fourscore set; Zelda II uses 2 controllers max".to_string());
        }
        if self.frames.iter().any(|f| f.is_power()) {
            w.push("power-cycle command present; replay must power-on there".to_string());
        }
        w
    }
}

/// Parse one 8-cell pad field into a NES pad byte.
///
/// Strict: each column accepts only its mnemonic (case-insensitive) for
/// pressed, or `.`/space for released.
fn parse_pad_field(field: &str, line: u32) -> Result<u8, Fm2Error> {
    let cells: Vec<char> = field.chars().collect();
    if cells.len() != 8 {
        return Err(err(
            line,
            format!("pad field {field:?} has {} cells, want 8", cells.len()),
        ));
    }
    let mut bits: u8 = 0;
    for (i, &c) in cells.iter().enumerate() {
        let want = FM2_COL_MNEMONIC[i];
        if c == want || c == want.to_ascii_lowercase() {
            bits |= 1 << FM2_COL_BITS[i];
        } else if c == '.' || c == ' ' {
            // released
        } else {
            return Err(err(
                line,
                format!("bad glyph {c:?} in column {i} (want {want:?}, '.' or space)"),
            ));
        }
    }
    Ok(bits)
}

/// Parse `.fm2` text.
pub fn parse_fm2(text: &str) -> Result<Fm2Movie, Fm2Error> {
    let mut header = Fm2Header::default();
    let mut frames: Vec<Fm2Frame> = Vec::new();
    let mut port_count = 0usize;

    // `lines()` treats LF and CRLF alike, so `line_no` matches the file.
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx as u32 + 1;
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('|') {
            // Header / comment line: `key value…`.
            let s = line.trim_start();
            match s.find(char::is_whitespace) {
                Some(i) => header
                    .fields
                    .push((s[..i].to_string(), s[i..].trim_start().to_string())),
                None => header.fields.push((s.to_string(), String::new())),
            }
            continue;
        }
        // Input line: `|command|pad0|pad1|…|`.
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 4 {
            return Err(err(line_no, format!("malformed input line {line:?}")));
        }
        let command: u8 = parts[1].trim().parse().map_err(|_| {
            err(
                line_no,
                format!("bad command byte {:?} (want decimal 0-255)", parts[1]),
            )
        })?;
        let mut pads = Vec::new();
        for field in parts.iter().skip(2) {
            if field.is_empty() {
                continue; // trailing `|`
            }
            pads.push(parse_pad_field(field, line_no)?);
        }
        if pads.is_empty() {
            return Err(err(line_no, "input line has no pad fields"));
        }
        port_count = port_count.max(pads.len());
        frames.push(Fm2Frame {
            command,
            pads,
            line_no,
        });
    }
    Ok(Fm2Movie {
        header,
        frames,
        port_count,
    })
}

/// Parse `.fm2` bytes (strips a UTF-8 BOM, rejects non-UTF-8).
pub fn parse_fm2_bytes(bytes: &[u8]) -> Result<Fm2Movie, Fm2Error> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|e| err(0, format!("not valid UTF-8: {e}")))?;
    parse_fm2(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY: &str = "version 3\n\
         emuVersion 20501\n\
         rerecordCount 42\n\
         palFlag 0\n\
         romFilename Zelda II - The Adventure of Link (USA).nes\n\
         romChecksum base64:AAAAAAAAAAAAAAAAAAAAAA==\n\
         guid 12345678-1234-1234-1234-123456789abc\n\
         fourscore 0\n\
         port0 1\n\
         port1 0\n\
         |0|........|........|\n\
         |0|.......A|........|\n\
         |0|...U....|........|\n\
         |1|....T...|........|\n";

    #[test]
    fn parses_header_and_frames() {
        let m = parse_fm2(TINY).unwrap();
        assert_eq!(m.frames.len(), 4);
        assert_eq!(m.port_count, 2);
        assert_eq!(m.header.version(), Some(3));
        assert_eq!(m.header.pal_flag(), Some(0));
        assert_eq!(m.header.rerecord_count(), Some(42));
        assert!(m.header.rom_name_looks_like_z2_usa());
        assert_eq!(m.frames[0].pad1(), 0x00);
        assert_eq!(m.frames[1].pad1(), 0x01); // A = bit 0
        assert_eq!(m.frames[2].pad1(), 0x10); // Up = bit 4
        assert_eq!(m.frames[3].pad1(), 0x08); // Start = bit 3
        assert!(m.frames[3].is_reset());
        assert!(!m.frames[0].is_reset());
        assert!(m.warnings().is_empty());
    }

    #[test]
    fn bit_order_matches_nes_shift_register() {
        // `RLDUTSBA` all pressed == 0xFF.
        let m = parse_fm2("|0|RLDUTSBA|\n").unwrap();
        assert_eq!(m.frames[0].pad1(), 0xFF);
        // Each column maps to its documented bit.
        for (col, want_bit) in FM2_COL_BITS.iter().enumerate() {
            let mut cells = ['.'; 8];
            cells[col] = FM2_COL_MNEMONIC[col];
            let field: String = cells.iter().collect();
            let line = format!("|0|{field}|\n");
            let got = parse_fm2(&line).unwrap().frames[0].pad1();
            assert_eq!(got, 1 << want_bit, "col {col}");
        }
    }

    #[test]
    fn crlf_and_blank_lines_tolerated() {
        let text = "\u{FEFF}version 3\r\npalFlag 0\r\n\r\n|0|R.......|\r\n\r\n";
        let m = parse_fm2_bytes(text.as_bytes()).unwrap();
        assert_eq!(m.frames.len(), 1);
        assert_eq!(m.frames[0].pad1(), 0x80); // Right = bit 7
    }

    #[test]
    fn bad_glyph_errors_with_line_number() {
        let e = parse_fm2("|0|........|\n|0|...X....|\n").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn short_pad_field_errors() {
        assert!(parse_fm2("|0|....|\n").is_err());
    }

    #[test]
    fn bad_command_errors() {
        assert!(parse_fm2("|x|........|\n").is_err());
    }

    #[test]
    fn warnings_flag_pal_and_rom_mismatch() {
        let m = parse_fm2("palFlag 1\nromFilename SMB3.nes\n|0|........|\n").unwrap();
        let w = m.warnings();
        assert!(w.iter().any(|s| s.contains("PAL")));
        assert!(w.iter().any(|s| s.contains("romFilename")));
    }

    #[test]
    fn non_utf8_rejected() {
        assert!(parse_fm2_bytes(&[0xFF, 0xFE, 0x7C]).is_err());
    }
}
