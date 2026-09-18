//! Input-movie parsing for the web frontend.
//!
//! Wasm-safe port of the `z2-verify` movie parsers:
//! [`parse_fm2`](z2_verify::movie_fm2) (strict mnemonics) and
//! [`parse_input_log`](z2_verify::movie_bk2) (lenient cells). `z2-web`
//! deliberately does **not** depend on `z2-verify`: that crate embeds the
//! `tetanes-core` lockstep oracle, which would drag a second full NES
//! emulator into the browser bundle. The contracts below are kept identical
//! by construction and covered by the same vectors:
//!
//! * `.fm2` pad columns are left-to-right `RLDUTSBA`; BizHawk pad columns
//!   follow the `LogKey:` mnemonics, defaulting to BizHawk's NES order
//!   `UDLRSsBA`. Both map onto the NES shift-register bit order
//!   `A,B,Select,Start,Up,Down,Left,Right` = bits 0..7 (the shared input
//!   contract in [`z2_core::game`]).
//! * `.fm2` text: `|command|pad0|pad1|…|` lines; command bit 0 = reset.
//! * BizHawk input-log text (`Input Log.txt` member of a `.bk2`, pasted or
//!   dropped as text): `[Input]` section, optional `LogKey:`, frame lines
//!   `|<counter>|<console>|<p1>|<p2>|…|` with an optional decimal counter
//!   and a console cell (`Power|Reset`) before the pads; a pressed console
//!   button becomes a warning, like an `.fm2` reset/power command.
//!
//! A whole binary `.bk2` (ZIP) is not unpacked in-tab (no zip reader here;
//! `z2-verify` carries one plus `flate2`): the frontend accepts the `.fm2`
//! file or the extracted `Input Log.txt` text.
//! See `site/README` and [`parse_movie`].

use std::fmt;

/// Left-to-right pad column mnemonics for the all-pressed line `RLDUTSBA`.
pub const COL_MNEMONIC: [char; 8] = ['R', 'L', 'D', 'U', 'T', 'S', 'B', 'A'];

/// NES shift-register bit index for each left-to-right `.fm2` pad column.
pub const COL_BITS: [u8; 8] = [7, 6, 5, 4, 3, 2, 1, 0];

/// BizHawk's NES pad column order (`P1 Up|P1 Down|…|P1 A`) as NES
/// shift-register bits — NOT the `.fm2` display order (mirrors
/// `z2_verify::movie_bk2::BK2_COL_BITS`).
pub const BK2_COL_BITS: [u8; 8] = [4, 5, 6, 7, 3, 2, 1, 0];

/// Console cell BizHawk writes before the pads on NES (lowercased), used
/// when the log has no usable `LogKey:`.
const BK2_DEFAULT_CONSOLE: [&str; 2] = ["power", "reset"];

/// Command-byte bit: soft reset this frame (`.fm2` only).
pub const CMD_RESET: u8 = 0x01;
/// Command-byte bit: power cycle this frame (`.fm2` only).
pub const CMD_POWER: u8 = 0x02;

/// Which text format [`parse_movie`] detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovieKind {
    /// FCEUX `.fm2` text.
    Fm2,
    /// BizHawk `Input Log.txt` text (`.bk2` member).
    Bk2Log,
}

/// Parsed player-1 input track plus provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieTrack {
    /// Detected format.
    pub kind: MovieKind,
    /// One NES pad byte per frame (bit 0 = A … bit 7 = Right).
    pub pads: Vec<u8>,
    /// Non-fatal notes (reset/power commands seen, extra ports, …).
    pub warnings: Vec<String>,
}

/// Movie parse failure (1-based `line`; `0` = whole-file error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieError {
    /// 1-based line number (`0` = whole-file error).
    pub line: u32,
    /// Human-readable cause.
    pub msg: String,
}

impl fmt::Display for MovieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "movie: {}", self.msg)
        } else {
            write!(f, "movie line {}: {}", self.line, self.msg)
        }
    }
}

impl std::error::Error for MovieError {}

fn err(line: u32, msg: impl Into<String>) -> MovieError {
    MovieError {
        line,
        msg: msg.into(),
    }
}

/// Detect the text format: `[Input]` / `LogKey:` markers mean a BizHawk
/// input log, otherwise `.fm2`.
pub fn detect_kind(text: &str) -> MovieKind {
    for raw in text.lines() {
        let t = raw.trim();
        if t.eq_ignore_ascii_case("[Input]") || t.to_ascii_lowercase().starts_with("logkey:") {
            return MovieKind::Bk2Log;
        }
    }
    MovieKind::Fm2
}

/// Parse movie text in either accepted format (see [`detect_kind`]).
pub fn parse_movie(text: &str) -> Result<MovieTrack, MovieError> {
    match detect_kind(text) {
        MovieKind::Bk2Log => parse_bk2_log(text),
        MovieKind::Fm2 => parse_fm2(text),
    }
}

/// Strict parse of one 8-cell `.fm2` pad field into a NES pad byte.
///
/// Each column accepts only its mnemonic (case-insensitive) for pressed,
/// or `.`/space for released — identical to `z2-verify`'s `movie_fm2`.
fn parse_fm2_field(field: &str, line: u32) -> Result<u8, MovieError> {
    let cells: Vec<char> = field.chars().collect();
    if cells.len() != 8 {
        return Err(err(
            line,
            format!("pad field {field:?} has {} cells, want 8", cells.len()),
        ));
    }
    let mut bits: u8 = 0;
    for (i, &c) in cells.iter().enumerate() {
        let want = COL_MNEMONIC[i];
        if c == want || c == want.to_ascii_lowercase() {
            bits |= 1 << COL_BITS[i];
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

/// Parse `.fm2` text (FCEUX movie format).
pub fn parse_fm2(text: &str) -> Result<MovieTrack, MovieError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut pads = Vec::new();
    let mut warnings = Vec::new();
    let mut resets = 0u32;
    let mut powers = 0u32;
    // `lines()` treats LF and CRLF alike, so `line_no` matches the file.
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx as u32 + 1;
        // Trailing whitespace is ignored everywhere (matches z2-verify).
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('|') {
            continue; // header / comment line: `key value…`
        }
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
        if command & CMD_RESET != 0 {
            resets += 1;
        }
        if command & CMD_POWER != 0 {
            powers += 1;
        }
        let mut frame_pads = Vec::new();
        for field in parts.iter().skip(2) {
            if field.is_empty() {
                continue; // trailing `|`
            }
            frame_pads.push(parse_fm2_field(field, line_no)?);
        }
        if frame_pads.is_empty() {
            return Err(err(line_no, "input line has no pad fields"));
        }
        if frame_pads.len() > 1 {
            // Extra ports are preserved by z2-verify; the web player only
            // runs port 0 (single-player Zelda II).
            if warnings.is_empty() {
                warnings.push(format!(
                    "multi-port movie: using port 0 of {}",
                    frame_pads.len()
                ));
            }
        }
        pads.push(frame_pads[0]);
    }
    if resets > 0 {
        warnings.push(format!(
            "{resets} soft-reset command(s): replay resets there"
        ));
    }
    if powers > 0 {
        warnings.push(format!(
            "{powers} power-cycle command(s): replay must power-on there"
        ));
    }
    Ok(MovieTrack {
        kind: MovieKind::Fm2,
        pads,
        warnings,
    })
}

/// Lenient decode of one BizHawk pad field: `.`/space/`-`/`_` mean
/// released, any other glyph means pressed (identical to `z2-verify`'s
/// `movie_bk2::decode_cells`, so core-revision glyph renames still parse).
/// `bits` maps each column to its NES bit (from the `LogKey`, else
/// [`BK2_COL_BITS`]).
fn decode_bk2_cells(field: &str, line: u32, bits: &[u8; 8]) -> Result<u8, MovieError> {
    let cells: Vec<char> = field.chars().collect();
    if cells.len() < 8 {
        return Err(MovieError {
            line,
            msg: format!("pad field {field:?} has {} cells, want >= 8", cells.len()),
        });
    }
    let mut out: u8 = 0;
    for (i, &c) in cells.iter().enumerate().take(8) {
        match c {
            '.' | ' ' | '-' | '_' => {}
            _ => out |= 1 << bits[i],
        }
    }
    Ok(out)
}

/// Last whitespace-separated token of a `LogKey` mnemonic (`"#P1 Up"` → `"Up"`).
fn mnemonic_button(m: &str) -> &str {
    m.trim_start_matches('#')
        .split_whitespace()
        .last()
        .unwrap_or("")
}

/// NES shift-register bit for a button mnemonic; `None` for console keys
/// (`Power`, `Reset`) and anything unrecognized.
fn button_bit(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "a" => Some(0),
        "b" => Some(1),
        "select" => Some(2),
        "start" => Some(3),
        "up" => Some(4),
        "down" => Some(5),
        "left" => Some(6),
        "right" => Some(7),
        _ => None,
    }
}

/// True for a `LogKey` mnemonic of a numbered player port (`P1 Up`), per
/// BizHawk's `^P(\d+) ` grouping rule; everything else is a console control.
fn is_player_mnemonic(m: &str) -> bool {
    let Some(rest) = m.trim_start_matches('#').strip_prefix('P') else {
        return false;
    };
    let after_digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    after_digits.len() < rest.len() && after_digits.starts_with(' ')
}

/// Console mnemonics before the first player group of the `LogKey:`,
/// lowercased. `None` for an absent/empty key (use [`BK2_DEFAULT_CONSOLE`]);
/// empty when the key opens with a player group (no console cell).
fn logkey_console(log_key: Option<&str>) -> Option<Vec<String>> {
    let key = log_key?.trim();
    if key.is_empty() {
        return None;
    }
    let first = key.split('#').find(|g| !g.trim().is_empty())?;
    let names: Vec<&str> = first
        .split('|')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .collect();
    if names.iter().any(|m| is_player_mnemonic(m)) {
        return Some(Vec::new());
    }
    Some(names.iter().map(|m| m.to_ascii_lowercase()).collect())
}

/// Per-field bit tables from the `LogKey:`, aligned to every cell after the
/// counter (console cell included). `None` when the key is absent or does
/// not cover the fields — callers fall back to [`BK2_COL_BITS`].
fn logkey_bits(log_key: Option<&str>, fields: &[&str]) -> Option<Vec<[u8; 8]>> {
    let key = log_key?;
    let mnemonics: Vec<&str> = key.split('|').collect();
    let mut off = 0usize;
    let mut tables = Vec::with_capacity(fields.len());
    for f in fields {
        let n = f.chars().count();
        let group = mnemonics.get(off..off + n)?;
        off += n;
        let mut table = [0u8; 8];
        for (i, m) in group.iter().enumerate().take(8) {
            table[i] = button_bit(mnemonic_button(m)).unwrap_or(BK2_COL_BITS[i]);
        }
        tables.push(table);
    }
    Some(tables)
}

/// LogKey bit tables cached against the field widths they were built for
/// (`None` tables = no usable key; fall back to the BizHawk NES order).
type TablesCache = (Vec<usize>, Option<Vec<[u8; 8]>>);

/// Parse BizHawk `Input Log.txt` text (the `.bk2` member carrying frames).
/// Same contract as `z2-verify`'s `movie_bk2::parse_input_log`.
pub fn parse_bk2_log(text: &str) -> Result<MovieTrack, MovieError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut pads = Vec::new();
    let mut warnings = Vec::new();
    let mut resets = 0u32;
    let mut powers = 0u32;
    let mut log_key: Option<String> = None;
    // Per-log caches derived from the LogKey (dropped when it changes).
    let mut console_cache: Option<Vec<String>> = None;
    let mut tables_cache: Option<TablesCache> = None;
    let mut in_input = false;
    // `lines()` treats LF and CRLF alike, so `line_no` matches the file.
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx as u32 + 1;
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.eq_ignore_ascii_case("[Input]") {
            in_input = true;
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            if in_input {
                break; // next section ends the input track
            }
            continue;
        }
        if t.to_ascii_lowercase().starts_with("logkey:") {
            log_key = Some(t["logkey:".len()..].trim().to_string());
            console_cache = None;
            tables_cache = None;
            in_input = true;
            continue;
        }
        if !t.starts_with('|') {
            if in_input && !pads.is_empty() {
                break;
            }
            continue;
        }
        // Frame line: `|<counter>|<console>|<p1>|<p2>|…|` (counter optional).
        let parts: Vec<&str> = t.split('|').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let mut pads_start = match parts[0].trim().parse::<u64>() {
            Ok(_) if parts.len() >= 2 => 1,
            _ => 0,
        };
        let fields_start = pads_start;
        // Console cell (BizHawk `Power|Reset`): its width is the number of
        // console mnemonics in the LogKey (2 without one); any other width
        // is a pad and must be >= 8 glyphs, so a truncated pad still errors.
        let console = console_cache.get_or_insert_with(|| {
            logkey_console(log_key.as_deref())
                .unwrap_or_else(|| BK2_DEFAULT_CONSOLE.iter().map(|s| s.to_string()).collect())
        });
        if !console.is_empty()
            && pads_start + 1 < parts.len()
            && parts[pads_start].chars().count() == console.len()
        {
            for (i, c) in parts[pads_start].chars().enumerate() {
                if ". -_".contains(c) {
                    continue;
                }
                match console[i].as_str() {
                    "reset" => resets += 1,
                    "power" => powers += 1,
                    other => {
                        return Err(err(
                            line_no,
                            format!("console button {other:?} is not modeled"),
                        ))
                    }
                }
            }
            pads_start += 1;
        }
        if parts.len() <= pads_start {
            return Err(err(line_no, "frame line has no pad fields"));
        }
        // Column→bit tables from the LogKey, cached per field layout.
        let fields = &parts[fields_start..];
        let cached = tables_cache.as_ref().is_some_and(|(w, _)| {
            w.len() == fields.len() && w.iter().zip(fields).all(|(w, f)| *w == f.chars().count())
        });
        if !cached {
            tables_cache = Some((
                fields.iter().map(|f| f.chars().count()).collect(),
                logkey_bits(log_key.as_deref(), fields),
            ));
        }
        let bits = tables_cache
            .as_ref()
            .and_then(|(_, t)| t.as_ref())
            .and_then(|t| t.get(pads_start - fields_start))
            .unwrap_or(&BK2_COL_BITS);
        pads.push(decode_bk2_cells(parts[pads_start], line_no, bits)?);
    }
    if resets > 0 {
        warnings.push(format!(
            "{resets} Reset press(es): replay does not model reset"
        ));
    }
    if powers > 0 {
        warnings.push(format!(
            "{powers} Power press(es): replay must power-cycle there"
        ));
    }
    Ok(MovieTrack {
        kind: MovieKind::Bk2Log,
        pads,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_FM2: &str = "version 3\n\
         palFlag 0\n\
         romFilename Zelda II - The Adventure of Link (USA).nes\n\
         |0|........|........|\n\
         |0|.......A|........|\n\
         |0|...U....|........|\n\
         |1|....T...|........|\n";

    #[test]
    fn fm2_vectors_match_z2_verify_contract() {
        let m = parse_fm2(TINY_FM2).unwrap();
        assert_eq!(m.kind, MovieKind::Fm2);
        assert_eq!(m.pads, vec![0x00, 0x01, 0x10, 0x08]);
        assert!(m.warnings.iter().any(|w| w.contains("soft-reset")));
    }

    #[test]
    fn fm2_bit_order_matches_nes_shift_register() {
        let m = parse_fm2("|0|RLDUTSBA|\n").unwrap();
        assert_eq!(m.pads, vec![0xFF]);
        for (col, want_bit) in COL_BITS.iter().enumerate() {
            let mut cells = ['.'; 8];
            cells[col] = COL_MNEMONIC[col];
            let field: String = cells.iter().collect();
            let got = parse_fm2(&format!("|0|{field}|\n")).unwrap().pads[0];
            assert_eq!(got, 1 << want_bit, "col {col}");
        }
    }

    #[test]
    fn fm2_rejects_bad_glyph_with_line_number() {
        let e = parse_fm2("|0|........|\n|0|...X....|\n").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn bk2_log_vectors_match_z2_verify_contract() {
        let text = "[Input]\nLogKey: #NES P1|#NES P2\n| 1|........|........|\n| 2|.......A|........|\n[/Input]\n";
        let m = parse_bk2_log(text).unwrap();
        assert_eq!(m.kind, MovieKind::Bk2Log);
        assert_eq!(m.pads, vec![0x00, 0x01]);
    }

    #[test]
    fn bk2_log_counter_is_optional_and_cells_lenient() {
        // No LogKey → BizHawk NES order: first cell = Up = bit 4.
        let m = parse_bk2_log("|U.......|\n").unwrap();
        assert_eq!(m.pads, vec![0x10]);
        // `-`/`_` count as released (z2-verify leniency).
        let m = parse_bk2_log("|--__ --_|\n").unwrap();
        assert_eq!(m.pads, vec![0x00]);
    }

    #[test]
    fn detect_kind_routes_correctly() {
        assert_eq!(detect_kind(TINY_FM2), MovieKind::Fm2);
        assert_eq!(detect_kind("[Input]\n|0|........|\n"), MovieKind::Bk2Log);
        assert_eq!(
            detect_kind("LogKey: #NES P1\n|........|\n"),
            MovieKind::Bk2Log
        );
    }

    #[test]
    fn parse_movie_dispatches_on_content() {
        assert_eq!(parse_movie(TINY_FM2).unwrap().pads.len(), 4);
        assert_eq!(
            parse_movie("[Input]\n|........|\n").unwrap().kind,
            MovieKind::Bk2Log
        );
    }

    /// Same vector as `crates/z2-verify/src/movie_bk2.rs` (corpus shape:
    /// 2-glyph console cell, then `UDLRSsBA` pads per port).
    const REAL_LOGKEY: &str = "#Power|Reset|#P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|#P2 Up|P2 Down|P2 Left|P2 Right|P2 Start|P2 Select|P2 B|P2 A|";

    #[test]
    fn bk2_real_shape_console_cell_and_logkey_order() {
        let text = format!(
            "[Input]\r\nLogKey:{REAL_LOGKEY}\r\n|..|........|........|\r\n|..|U.......|........|\r\n|..|....S...|........|\r\n|..|UDLRSsBA|........|\r\n[/Input]\r\n"
        );
        let m = parse_bk2_log(&text).unwrap();
        assert_eq!(m.pads, vec![0x00, 0x10, 0x08, 0xFF]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        // Empty `LogKey:` (BizHawk 2.5.2) falls back to the same layout.
        let m = parse_bk2_log("[Input]\nLogKey:\n|..|U.......|........|\n").unwrap();
        assert_eq!(m.pads, vec![0x10]);
        // A non-NES-order key is honored.
        let m = parse_bk2_log(
            "[Input]\nLogKey:#Power|Reset|#P1 A|P1 B|P1 Select|P1 Start|P1 Up|P1 Down|P1 Left|P1 Right|\n|..|A.......|\n|..|.......R|\n",
        )
        .unwrap();
        assert_eq!(m.pads, vec![0x01, 0x80]);
    }

    #[test]
    fn bk2_console_press_warns_and_truncated_pad_errors() {
        let m = parse_bk2_log("|..|........|\n|.r|.......A|\n|P.|........|\n").unwrap();
        assert_eq!(m.pads, vec![0x00, 0x01, 0x00]);
        assert!(
            m.warnings.iter().any(|w| w.contains("Reset")),
            "{:?}",
            m.warnings
        );
        assert!(
            m.warnings.iter().any(|w| w.contains("Power")),
            "{:?}",
            m.warnings
        );
        // 7 glyphs is neither a console cell (2) nor a pad (>= 8).
        let e = parse_bk2_log("|..|.......|........|\n").unwrap_err();
        assert!(e.msg.contains("7 cells"), "{e}");
        assert_eq!(e.line, 1);
    }
}
