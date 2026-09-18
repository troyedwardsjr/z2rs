//! BizHawk `.bk2` movie parser for the snapshot corpus.
//!
//! `std` + `flate2` (pure-Rust backend, wasm-safe): no
//! ROM, no network. Wired as `z2_verify::movie_bk2`.
//!
//! A `.bk2` is a ZIP archive. The frames live in `Input Log.txt`:
//!
//! ```text
//! [Input]
//! LogKey:#Power|Reset|#P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|…
//! |..|........|........|
//! |..|....S...|........|
//! ```
//!
//! Each frame line is `|<counter>|<console>|<p1>|<p2>|…|`: the counter is
//! optional (present when BizHawk writes it) and the console cell (`Power`,
//! `Reset`) precedes the pads. Pad columns follow the `LogKey:` mnemonics
//! when present, else BizHawk's NES order `UDLRSsBA` (Up, Down, Left, Right,
//! Start, Select, B, A — NOT the `.fm2` display order), mapped to NES bit
//! order A(bit0)…Right(bit7). Cell decoding is lenient: `.`, space, `-`, `_`
//! mean released, any other glyph means pressed, so a BizHawk glyph rename
//! still parses (the canonical glyphs are [`BK2_COL_MNEMONIC`]). A pressed
//! console button is recorded on the frame ([`Bk2Frame::command`], mirroring
//! `.fm2`) and surfaced by [`Bk2Movie::warnings`]; the driver decides.
//!
//! `Header.txt` (`Platform` / `GameName` / digests) is surfaced for ROM
//! validation against the pinned ROM; see [`Bk2Movie::warnings`].
//!
//! ## ZIP support (minimal reader + `flate2`)
//!
//! A minimal reader parses End-of-Central-Directory + central directory +
//! local headers from `&[u8]`. `Stored` (method 0) entries extract directly;
//! `Deflated` (method 8) entries inflate via `flate2` (pure-Rust backend,
//! wasm-safe). Real BizHawk `.bk2` files typically store `Input Log.txt`
//! deflated, so the oracle replay harness reads them directly.
//!
//! ## Edge cases handled
//!
//! 1. LF / CRLF line endings; UTF-8 BOM on `Input Log.txt`.
//! 2. Optional frame counter (`| 12|…|`) vs counter-less (`|…|`) lines.
//! 3. `LogKey` mnemonics size the console cell and map columns; extra ports kept.
//! 4. `[Input]` section header + `LogKey:` line skipped, never parsed as frames.
//! 5. Non-frame trailer lines (e.g. `[/Input]`) stop the parse cleanly.
//! 6. Missing `Input Log.txt` / missing EOCD / truncated ZIP → typed errors.
//! 7. ZIP64 + multi-disk + encrypted entries rejected with a clear message.
//! 8. Data-descriptor flag handled via central-directory sizes.
//! 9. Case-insensitive fallback when locating `Input Log.txt` / `Header.txt`.

use std::error::Error;
use std::fmt;

pub use crate::movie_fm2::{CMD_POWER, CMD_RESET};

/// BizHawk's NES pad glyphs in `LogKey` group order (`P1 Up|P1 Down|…|P1 A`):
/// Up, Down, Left, Right, Start (`S`), Select (`s`), B, A. NOTE: this is
/// NOT the `.fm2` display order (FCEUX lists Right first). The bit values
/// are the same NES shift-register bits (Up=4 … A=0); only the column
/// order differs.
pub const BK2_COL_MNEMONIC: [char; 8] = ['U', 'D', 'L', 'R', 'S', 's', 'B', 'A'];
/// NES shift-register bit index per left-to-right pad column (NES order).
pub const BK2_COL_BITS: [u8; 8] = [4, 5, 6, 7, 3, 2, 1, 0];
/// Console (system) cell BizHawk writes before the pads on NES, in order.
/// Used when `Input Log.txt` has no usable `LogKey:` (e.g. an empty one).
pub const BK2_DEFAULT_CONSOLE: [&str; 2] = ["Power", "Reset"];

/// Expected archive member carrying the input track.
pub const INPUT_LOG_NAME: &str = "Input Log.txt";
/// Expected archive member carrying movie metadata.
pub const HEADER_NAME: &str = "Header.txt";

/// Typed parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bk2Error {
    /// What failed (parse stage + cause). No line numbers for ZIP-level errors;
    /// input-log errors embed the 1-based line number in `msg`.
    pub msg: String,
}

impl Bk2Error {
    fn parse(msg: impl Into<String>) -> Self {
        Bk2Error { msg: msg.into() }
    }
}

impl fmt::Display for Bk2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bk2: {}", self.msg)
    }
}

impl Error for Bk2Error {}

// ---------------------------------------------------------------------------
// Minimal ZIP reader (Stored + Deflate via flate2).
// ---------------------------------------------------------------------------

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;

fn u16le(b: &[u8], off: usize) -> Result<u16, Bk2Error> {
    b.get(off..off + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| Bk2Error::parse(format!("zip truncated at offset {off}")))
}

fn u32le(b: &[u8], off: usize) -> Result<u32, Bk2Error> {
    b.get(off..off + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| Bk2Error::parse(format!("zip truncated at offset {off}")))
}

struct CentralEntry {
    name: String,
    method: u16,
    csize: u32,
    usize: u32,
    local_off: u32,
    encrypted: bool,
}

/// Locate End-of-Central-Directory (handles an arbitrary comment).
fn find_eocd(zip: &[u8]) -> Result<usize, Bk2Error> {
    if zip.len() < 22 {
        return Err(Bk2Error::parse("zip too short for EOCD"));
    }
    let start = zip.len().saturating_sub(22 + 0xFFFF);
    let mut i = zip.len() - 22;
    loop {
        if u32le(zip, i).is_ok_and(|s| s == SIG_EOCD) {
            return Ok(i);
        }
        if i == start {
            break;
        }
        i -= 1;
    }
    Err(Bk2Error::parse("zip End-of-Central-Directory not found"))
}

fn parse_central(zip: &[u8]) -> Result<Vec<CentralEntry>, Bk2Error> {
    let eocd = find_eocd(zip)?;
    let disks = u16le(zip, eocd + 4)?;
    let disk_cd = u16le(zip, eocd + 6)?;
    let count = u16le(zip, eocd + 8)? as usize;
    let total = u16le(zip, eocd + 10)? as usize;
    if disks != 0 || disk_cd != 0 {
        return Err(Bk2Error::parse("multi-disk zip not supported"));
    }
    if count != total {
        return Err(Bk2Error::parse("split zip not supported"));
    }
    if u16le(zip, eocd + 12)? == 0xFFFF || u16le(zip, eocd + 14)? == 0xFFFF {
        return Err(Bk2Error::parse("ZIP64 not supported"));
    }
    let cd_off = u32le(zip, eocd + 16)? as usize;
    let mut entries = Vec::with_capacity(count);
    let mut off = cd_off;
    for _ in 0..count {
        if u32le(zip, off)? != SIG_CENTRAL {
            return Err(Bk2Error::parse(format!(
                "bad central-directory signature at offset {off}"
            )));
        }
        let flags = u16le(zip, off + 8)?;
        let method = u16le(zip, off + 10)?;
        let csize = u32le(zip, off + 20)?;
        let usize_ = u32le(zip, off + 24)?;
        let nlen = u16le(zip, off + 28)? as usize;
        let elen = u16le(zip, off + 30)? as usize;
        let clen = u16le(zip, off + 32)? as usize;
        let local_off = u32le(zip, off + 42)?;
        if csize == 0xFFFF_FFFF || usize_ == 0xFFFF_FFFF || local_off == 0xFFFF_FFFF {
            return Err(Bk2Error::parse("ZIP64 entry not supported"));
        }
        let name_off = off + 46;
        let name_end = name_off + nlen;
        let name_bytes = zip
            .get(name_off..name_end)
            .ok_or_else(|| Bk2Error::parse("zip truncated in central-directory name"))?;
        // Bit 11 = UTF-8; BizHawk names are ASCII so lossy is only a fallback.
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        entries.push(CentralEntry {
            name,
            method,
            csize,
            usize: usize_,
            local_off,
            encrypted: flags & 0x01 != 0,
        });
        off = name_end + elen + clen;
    }
    Ok(entries)
}

fn find_entry<'a>(entries: &'a [CentralEntry], want: &str) -> Option<&'a CentralEntry> {
    entries
        .iter()
        .find(|e| e.name == want)
        .or_else(|| entries.iter().find(|e| e.name.eq_ignore_ascii_case(want)))
}

/// Extract one entry's raw bytes. `Stored` copies directly; `Deflated`
/// inflates via `flate2` (pure-Rust backend, wasm-safe).
fn extract(zip: &[u8], e: &CentralEntry) -> Result<Vec<u8>, Bk2Error> {
    if e.encrypted {
        return Err(Bk2Error::parse(format!(
            "zip entry {:?} is encrypted",
            e.name
        )));
    }
    let lo = e.local_off as usize;
    if u32le(zip, lo)? != SIG_LOCAL {
        return Err(Bk2Error::parse(format!(
            "bad local-header signature for {:?}",
            e.name
        )));
    }
    let nlen = u16le(zip, lo + 26)? as usize;
    let elen = u16le(zip, lo + 28)? as usize;
    let data = lo + 30 + nlen + elen;
    let end = data + e.csize as usize;
    let comp = zip
        .get(data..end)
        .ok_or_else(|| Bk2Error::parse(format!("zip truncated in {:?}", e.name)))?;
    if e.method == METHOD_DEFLATED {
        return inflate(comp, e.usize as usize, &e.name);
    }
    if e.method != METHOD_STORED {
        return Err(Bk2Error::parse(format!(
            "zip entry {:?} uses unsupported method {}",
            e.name, e.method
        )));
    }
    if e.csize != e.usize {
        return Err(Bk2Error::parse(format!(
            "Stored entry {:?} has csize != usize",
            e.name
        )));
    }
    Ok(comp.to_vec())
}

/// Inflate raw-deflate bytes (zip method 8). `want` is the central-directory
/// uncompressed size, used as an exact expectation (zip bomb guard: entries
/// over 64 MiB are rejected before allocating, and a stream that outgrows
/// `want` stops one byte past it instead of ballooning memory).
fn inflate(comp: &[u8], want: usize, name: &str) -> Result<Vec<u8>, Bk2Error> {
    const MAX_UNCOMPRESSED: usize = 64 << 20;
    if want > MAX_UNCOMPRESSED {
        return Err(Bk2Error::parse(format!(
            "zip entry {name:?} claims {want} uncompressed bytes (over the 64 MiB cap)"
        )));
    }
    let dec = flate2::read::DeflateDecoder::new(comp);
    let mut out = Vec::new();
    // Read at most one byte past the declared size: a lying directory
    // size fails the exact-size check below without inflating the rest.
    use std::io::Read as _;
    dec.take(want as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| Bk2Error::parse(format!("zip entry {name:?} deflate error: {e}")))?;
    let wrote = out.len();
    if wrote != want {
        return Err(Bk2Error::parse(format!(
            "zip entry {name:?} inflated to {wrote} bytes, directory says {want}"
        )));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Input Log.txt
// ---------------------------------------------------------------------------

/// One BizHawk input frame (player-1 byte in NES bit order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bk2Frame {
    /// Player-1 pad byte (bit 0 = A … bit 7 = Right).
    pub pad1: u8,
    /// All ports on this line, port 0 first.
    pub pads: Vec<u8>,
    /// Declared frame counter, when the line carries one.
    pub counter: Option<u64>,
    /// 1-based line number inside `Input Log.txt`.
    pub line_no: u32,
    /// Console buttons pressed this frame ([`CMD_RESET`] / [`CMD_POWER`]
    /// bits, the `Fm2Frame::command` layout). Lockstep models neither;
    /// drivers refuse or warn (see [`Bk2Movie::warnings`]).
    pub command: u8,
}

impl Bk2Frame {
    /// `Reset` pressed this frame.
    pub fn is_reset(&self) -> bool {
        self.command & CMD_RESET != 0
    }
    /// `Power` pressed this frame.
    pub fn is_power(&self) -> bool {
        self.command & CMD_POWER != 0
    }
}

/// Parsed `Input Log.txt`.
#[derive(Debug, Clone, Default)]
pub struct Bk2InputLog {
    /// Raw `LogKey:` payload (column descriptors), if present.
    pub log_key: Option<String>,
    /// Frames in order.
    pub frames: Vec<Bk2Frame>,
}

impl Bk2InputLog {
    /// Player-1 input track: one NES pad byte per frame.
    pub fn pad1_track(&self) -> Vec<u8> {
        self.frames.iter().map(|f| f.pad1).collect()
    }
    /// First frame with a console button (`Reset`/`Power`) pressed, if any.
    pub fn first_console_press(&self) -> Option<&Bk2Frame> {
        self.frames.iter().find(|f| f.command != 0)
    }
}

fn decode_cells(field: &str, line: u32, bits: &[u8; 8]) -> Result<u8, Bk2Error> {
    let cells: Vec<char> = field.chars().collect();
    if cells.len() < 8 {
        return Err(Bk2Error::parse(format!(
            "Input Log line {line}: pad field {field:?} has {} cells, want >= 8",
            cells.len()
        )));
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

/// Last whitespace-separated token of a `LogKey` mnemonic, lowercased
/// (`"#P1 Up"` → `"up"`).
fn mnemonic_button(m: &str) -> &str {
    m.trim_start_matches('#')
        .split_whitespace()
        .last()
        .unwrap_or("")
}

/// NES shift-register bit for a button mnemonic. `None` for system keys
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

/// True for a `LogKey` mnemonic of a numbered player port (`P1 Up`,
/// `#P2 A`), per BizHawk's `^P(\d+) ` grouping rule; everything else
/// (`Power`, `Reset`, `FDS Eject`, …) is a console control.
fn is_player_mnemonic(m: &str) -> bool {
    let Some(rest) = m.trim_start_matches('#').strip_prefix('P') else {
        return false;
    };
    let after_digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    after_digits.len() < rest.len() && after_digits.starts_with(' ')
}

/// Console (system) mnemonics declared before the first player group of
/// the `LogKey:` line, lowercased (e.g. `["power", "reset"]`). `None` when
/// the key is absent or empty (callers fall back to [`BK2_DEFAULT_CONSOLE`]);
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

/// Per-field bit tables derived from the `LogKey:` line, aligned to every
/// cell after the counter (the console cell included, so pad tables land
/// at their field index). `None` when the key is absent or doesn't cover
/// the fields — callers fall back to [`BK2_COL_BITS`] (BizHawk NES order).
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
            // The console cell gets a table too (it keeps the alignment)
            // but is never decoded: `parse_input_log` steps past it.
            table[i] = button_bit(mnemonic_button(m)).unwrap_or(BK2_COL_BITS[i]);
        }
        tables.push(table);
    }
    Some(tables)
}

/// LogKey bit tables cached against the field widths they were built for
/// (`None` tables = no usable key; fall back to the BizHawk NES order).
type TablesCache = (Vec<usize>, Option<Vec<[u8; 8]>>);

/// Parse `Input Log.txt` text.
pub fn parse_input_log(text: &str) -> Result<Bk2InputLog, Bk2Error> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut log = Bk2InputLog::default();
    let mut in_input = false;
    // Per-log caches derived from the LogKey (dropped when it changes).
    let mut console_cache: Option<Vec<String>> = None;
    let mut tables_cache: Option<TablesCache> = None;
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
            log.log_key = Some(t["logkey:".len()..].trim().to_string());
            console_cache = None;
            tables_cache = None;
            in_input = true;
            continue;
        }
        if !t.starts_with('|') {
            // Header.txt-style stray line or trailer like `[/Input]`.
            if in_input && !log.frames.is_empty() {
                break;
            }
            continue;
        }
        // Frame line.
        let mut parts: Vec<&str> = t.split('|').collect();
        parts.retain(|p| !p.is_empty());
        if parts.is_empty() {
            continue;
        }
        let (counter, mut pads_start) = match parts[0].trim().parse::<u64>() {
            Ok(n) if parts.len() >= 2 => (Some(n), 1),
            _ => (None, 0),
        };
        let fields_start = pads_start; // LogKey alignment includes the console cell

        // Console cell (BizHawk `Power|Reset`) precedes the pads. Its width
        // is the number of console mnemonics in the LogKey (2 without one);
        // any other width is a pad and must be >= 8 glyphs, so a truncated
        // pad fails loudly in `decode_cells` instead of being skipped. A
        // pressed console button is recorded like `.fm2`'s command byte —
        // lockstep can't model it, so the driver decides what to do.
        let console = console_cache.get_or_insert_with(|| {
            logkey_console(log.log_key.as_deref()).unwrap_or_else(|| {
                BK2_DEFAULT_CONSOLE
                    .iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect()
            })
        });
        let mut command = 0u8;
        if !console.is_empty()
            && pads_start + 1 < parts.len()
            && parts[pads_start].chars().count() == console.len()
        {
            for (i, c) in parts[pads_start].chars().enumerate() {
                if ". -_".contains(c) {
                    continue;
                }
                match console[i].as_str() {
                    "reset" => command |= CMD_RESET,
                    "power" => command |= CMD_POWER,
                    other => {
                        return Err(Bk2Error::parse(format!(
                            "Input Log line {line_no}: console button {other:?} is not modeled"
                        )))
                    }
                }
            }
            pads_start += 1;
        }
        if parts.len() <= pads_start {
            return Err(Bk2Error::parse(format!(
                "Input Log line {line_no}: frame line has no pad fields"
            )));
        }

        // Column→bit tables from the LogKey, aligned to every cell after the
        // counter (the key lists the console mnemonics too). Cached per field
        // layout, so a real log derives them once rather than per frame.
        let fields = &parts[fields_start..];
        let cached = tables_cache.as_ref().is_some_and(|(w, _)| {
            w.len() == fields.len() && w.iter().zip(fields).all(|(w, f)| *w == f.chars().count())
        });
        if !cached {
            tables_cache = Some((
                fields.iter().map(|f| f.chars().count()).collect(),
                logkey_bits(log.log_key.as_deref(), fields),
            ));
        }
        let tables = tables_cache.as_ref().and_then(|(_, t)| t.as_ref());
        let skip = pads_start - fields_start;
        let mut pads = Vec::with_capacity(fields.len() - skip);
        for (j, field) in parts[pads_start..].iter().enumerate() {
            let bits = tables
                .and_then(|t| t.get(j + skip))
                .unwrap_or(&BK2_COL_BITS);
            pads.push(decode_cells(field, line_no, bits)?);
        }
        log.frames.push(Bk2Frame {
            pad1: pads[0],
            pads,
            counter,
            line_no,
            command,
        });
    }
    Ok(log)
}

// ---------------------------------------------------------------------------
// Whole-movie view.
// ---------------------------------------------------------------------------

/// Parsed `.bk2` movie.
#[derive(Debug, Clone)]
pub struct Bk2Movie {
    /// All ZIP member names (for corpus bookkeeping).
    pub entry_names: Vec<String>,
    /// Parsed `Header.txt` fields (`key value…`), if the member exists.
    pub header_fields: Vec<(String, String)>,
    /// Parsed input track.
    pub input: Bk2InputLog,
}

impl Bk2Movie {
    /// `Header.txt` value lookup (case-insensitive key).
    pub fn header(&self, key: &str) -> Option<&str> {
        self.header_fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
    /// `GameName` from `Header.txt`, if present.
    pub fn game_name(&self) -> Option<&str> {
        self.header("GameName").or_else(|| self.header("Game Name"))
    }
    /// Declared platform (want `NES`).
    pub fn platform(&self) -> Option<&str> {
        self.header("Platform")
    }
    /// True when `GameName` looks like Zelda II.
    pub fn rom_name_looks_like_z2_usa(&self) -> bool {
        self.game_name()
            .map(|n| n.to_ascii_lowercase().contains("zelda"))
            .unwrap_or(false)
    }
    /// Player-1 input track.
    pub fn pad1_track(&self) -> Vec<u8> {
        self.input.pad1_track()
    }
    /// First frame with a console button (`Reset`/`Power`) pressed, if any.
    pub fn first_console_press(&self) -> Option<&Bk2Frame> {
        self.input.first_console_press()
    }
    /// Non-fatal semantic warnings.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        match self.platform() {
            Some(p) if p.eq_ignore_ascii_case("NES") => {}
            Some(p) => w.push(format!("Platform {p:?} is not NES")),
            None => w.push("Header.txt missing Platform".to_string()),
        }
        match self.game_name() {
            Some(_) if self.rom_name_looks_like_z2_usa() => {}
            Some(n) => w.push(format!("GameName {n:?} does not look like Zelda II")),
            None => w.push("Header.txt missing GameName".to_string()),
        }
        let resets = self.input.frames.iter().filter(|f| f.is_reset()).count();
        if resets > 0 {
            w.push(format!(
                "{resets} Reset press(es) present; lockstep does not model reset"
            ));
        }
        let powers = self.input.frames.iter().filter(|f| f.is_power()).count();
        if powers > 0 {
            w.push(format!(
                "{powers} Power press(es) present; replay must power-cycle there"
            ));
        }
        w
    }
}

fn parse_header_txt(text: &str) -> Vec<(String, String)> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut out = Vec::new();
    for raw in text.replace('\r', "\n").split('\n') {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        match s.find(char::is_whitespace) {
            Some(i) => out.push((s[..i].to_string(), s[i..].trim_start().to_string())),
            None => out.push((s.to_string(), String::new())),
        }
    }
    out
}

/// Parse a whole `.bk2` archive from memory.
pub fn parse_bk2_zip(zip: &[u8]) -> Result<Bk2Movie, Bk2Error> {
    let entries = parse_central(zip)?;
    let entry_names = entries.iter().map(|e| e.name.clone()).collect();
    let log_entry = find_entry(&entries, INPUT_LOG_NAME)
        .ok_or_else(|| Bk2Error::parse("zip has no Input Log.txt"))?;
    let log_bytes = extract(zip, log_entry)?;
    let log_text = String::from_utf8(log_bytes)
        .map_err(|e| Bk2Error::parse(format!("Input Log.txt is not UTF-8: {e}")))?;
    let input = parse_input_log(&log_text)?;
    // A present-but-unreadable Header.txt is an archive error, not a
    // missing header: propagate instead of degrading to empty fields.
    let header_fields = match find_entry(&entries, HEADER_NAME) {
        Some(h) => {
            let text = String::from_utf8(extract(zip, h)?)
                .map_err(|e| Bk2Error::parse(format!("Header.txt is not UTF-8: {e}")))?;
            parse_header_txt(&text)
        }
        None => Vec::new(),
    };
    Ok(Bk2Movie {
        entry_names,
        header_fields,
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- tiny in-test zip writer (mirrors the reader) ---

    fn crc32_ieee(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let m = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
                crc = (crc >> 1) ^ m;
            }
        }
        !crc
    }

    /// Raw writer: `stored` bytes go into the entry as-is under `method`;
    /// `plain` supplies the CRC-32 and the uncompressed size (both are over
    /// the UNCOMPRESSED bytes whatever the method).
    fn build_zip_raw(entries: &[(&str, &[u8], u16, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, stored, method, plain) in entries {
            let local_off = out.len() as u32;
            let crc = crc32_ieee(plain);
            out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // time
            out.extend_from_slice(&0u16.to_le_bytes()); // date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(stored);

            central.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            central.extend_from_slice(&(plain.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&local_off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&central);
        let cd_size = central.len() as u32;
        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// Raw deflate (zip method 8), the way BizHawk stores every member.
    fn deflate(plain: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(plain).unwrap();
        enc.finish().unwrap()
    }

    /// Entries as `(name, plain, method)`; method-8 entries are deflated here.
    fn build_zip(files: &[(&str, &[u8], u16)]) -> Vec<u8> {
        let stored: Vec<Vec<u8>> = files
            .iter()
            .map(|(_, plain, method)| {
                if *method == METHOD_DEFLATED {
                    deflate(plain)
                } else {
                    plain.to_vec()
                }
            })
            .collect();
        let raw: Vec<(&str, &[u8], u16, &[u8])> = files
            .iter()
            .zip(&stored)
            .map(|((name, plain, method), stored)| (*name, stored.as_slice(), *method, *plain))
            .collect();
        build_zip_raw(&raw)
    }

    fn build_stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let stored: Vec<(&str, &[u8], u16)> =
            files.iter().map(|(n, d)| (*n, *d, METHOD_STORED)).collect();
        build_zip(&stored)
    }

    /// Overwrite the central-directory uncompressed size of entry `name`.
    fn patch_central_usize(zip: &mut [u8], name: &str, usize_: u32) {
        // The central record is the LAST occurrence of the name; it starts
        // 46 bytes before the name and `usize` sits at +24.
        let last = zip
            .windows(name.len())
            .rposition(|w| w == name.as_bytes())
            .expect("entry name present");
        let at = last - 46 + 24;
        zip[at..at + 4].copy_from_slice(&usize_.to_le_bytes());
    }

    const LOG: &str = "[Input]\n\
         LogKey: #NES P1|#NES P2\n\
         | 0|........|........|\n\
         | 1|.......A|........|\n\
         | 2|....T...|........|\n";

    const HEADER: &str =
        "Platform NES\nGameName Zelda II - The Adventure of Link (USA)\nAuthor Arc\n";

    fn fixture_zip() -> Vec<u8> {
        build_stored_zip(&[
            ("Header.txt", HEADER.as_bytes()),
            ("Input Log.txt", LOG.as_bytes()),
        ])
    }

    #[test]
    fn parses_stored_bk2() {
        let m = parse_bk2_zip(&fixture_zip()).unwrap();
        assert_eq!(m.input.frames.len(), 3);
        assert_eq!(m.input.frames[0].pad1, 0x00);
        assert_eq!(m.input.frames[1].pad1, 0x01); // A
        assert_eq!(m.input.frames[2].pad1, 0x08); // Start
        assert_eq!(m.input.frames[2].counter, Some(2));
        assert!(m.input.log_key.is_some());
        assert!(m.rom_name_looks_like_z2_usa());
        assert!(m.warnings().is_empty());
    }

    #[test]
    fn counter_less_lines_parse() {
        // No LogKey → default BizHawk NES column order
        // (Up,Down,Left,Right,Start,Select,B,A): first cell = Up = bit 4.
        // (Any non-`. `/space glyph means pressed; the letter is arbitrary.)
        let log = parse_input_log("|........|\n|X.......|\n").unwrap();
        assert_eq!(log.frames.len(), 2);
        assert_eq!(log.frames[0].counter, None);
        assert_eq!(log.frames[1].pad1, 0x10); // Up
    }

    #[test]
    fn missing_input_log_errors() {
        let zip = build_stored_zip(&[("Header.txt", HEADER.as_bytes())]);
        assert!(parse_bk2_zip(&zip).is_err());
    }

    #[test]
    fn garbage_is_not_a_zip() {
        assert!(parse_bk2_zip(b"not a zip at all..............").is_err());
    }

    #[test]
    fn deflated_entries_inflate() {
        // Real BizHawk archives deflate every member, Header.txt included.
        let log = b"| 1|........|........|\n| 2|U.......|........|\n";
        let zip = build_zip(&[
            ("Header.txt", HEADER.as_bytes(), METHOD_DEFLATED),
            ("Input Log.txt", log, METHOD_DEFLATED),
        ]);
        let m = parse_bk2_zip(&zip).expect("deflated entries must inflate");
        assert_eq!(m.pad1_track(), vec![0x00, 0x10], "Up is bit 4");
        assert_eq!(m.platform(), Some("NES"));
        assert!(m.warnings().is_empty(), "{:?}", m.warnings());
    }

    #[test]
    fn deflate_size_lie_is_rejected() {
        let log = b"| 1|........|........|\n";
        let mut zip = build_zip(&[("Input Log.txt", log, METHOD_DEFLATED)]);
        patch_central_usize(&mut zip, "Input Log.txt", log.len() as u32 + 1);
        let e = parse_bk2_zip(&zip).unwrap_err();
        assert!(e.msg.contains("inflated to"), "{e}");
        patch_central_usize(&mut zip, "Input Log.txt", (64 << 20) + 1);
        let e = parse_bk2_zip(&zip).unwrap_err();
        assert!(e.msg.contains("64 MiB cap"), "{e}");
    }

    #[test]
    fn deflate_truncated_stream_is_rejected() {
        let log = b"| 1|........|........|\n| 2|........|........|\n";
        let comp = deflate(log);
        let cut = &comp[..comp.len() - 2];
        let zip = build_zip_raw(&[("Input Log.txt", cut, METHOD_DEFLATED, log)]);
        let e = parse_bk2_zip(&zip).unwrap_err();
        assert!(e.msg.contains("Input Log.txt"), "{e}");
    }

    #[test]
    fn unsupported_method_is_rejected() {
        let log = b"| 1|........|........|\n";
        let zip = build_zip_raw(&[("Input Log.txt", log, 99, log)]);
        let e = parse_bk2_zip(&zip).unwrap_err();
        assert!(e.msg.contains("unsupported method 99"), "{e}");
    }

    #[test]
    fn corrupt_header_txt_is_an_error_not_a_missing_header() {
        let comp = deflate(HEADER.as_bytes());
        let cut = &comp[..comp.len() - 2];
        let zip = build_zip_raw(&[
            ("Header.txt", cut, METHOD_DEFLATED, HEADER.as_bytes()),
            (
                "Input Log.txt",
                LOG.as_bytes(),
                METHOD_STORED,
                LOG.as_bytes(),
            ),
        ]);
        let e = parse_bk2_zip(&zip).unwrap_err();
        assert!(e.msg.contains("Header.txt"), "{e}");
    }

    #[test]
    fn header_warnings_flag_wrong_platform() {
        let zip = build_stored_zip(&[
            ("Header.txt", "Platform SNES\nGameName Zelda 3\n".as_bytes()),
            ("Input Log.txt", LOG.as_bytes()),
        ]);
        let m = parse_bk2_zip(&zip).unwrap();
        assert!(!m.warnings().is_empty());
    }

    /// Verbatim shape of the corpus movies (BizHawk 2.4–2.6, NES): a 2-glyph
    /// console cell, then one 8-glyph pad per port in `UDLRSsBA` order.
    /// Mirrored in `crates/z2-web/src/movie.rs` (same vector, same expectations).
    const REAL_LOGKEY: &str = "#Power|Reset|#P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|#P2 Up|P2 Down|P2 Left|P2 Right|P2 Start|P2 Select|P2 B|P2 A|";

    #[test]
    fn real_bizhawk_shape_console_cell_and_logkey_order() {
        let text = format!(
            "[Input]\r\nLogKey:{REAL_LOGKEY}\r\n|..|........|........|\r\n|..|U.......|........|\r\n|..|....S...|........|\r\n|..|UDLRSsBA|........|\r\n[/Input]\r\n"
        );
        let log = parse_input_log(&text).unwrap();
        assert_eq!(log.pad1_track(), vec![0x00, 0x10, 0x08, 0xFF]);
        assert!(log
            .frames
            .iter()
            .all(|f| f.pads.len() == 2 && f.command == 0));
        // CRLF is one line break: line numbers match the file.
        assert_eq!(log.frames[0].line_no, 3);
        assert_eq!(log.frames[3].line_no, 6);
        // An empty `LogKey:` (BizHawk 2.5.2 wrote one) falls back to the same
        // console cell and NES column order.
        let log = parse_input_log("[Input]\nLogKey:\n|..|U.......|........|\n").unwrap();
        assert_eq!(log.pad1_track(), vec![0x10]);
    }

    #[test]
    fn logkey_column_order_is_honored() {
        let text = "[Input]\nLogKey:#Power|Reset|#P1 A|P1 B|P1 Select|P1 Start|P1 Up|P1 Down|P1 Left|P1 Right|\n|..|A.......|\n|..|.......R|\n";
        let log = parse_input_log(text).unwrap();
        assert_eq!(log.pad1_track(), vec![0x01, 0x80]);
    }

    #[test]
    fn canonical_glyphs_decode_to_their_bits() {
        for (col, &bit) in BK2_COL_BITS.iter().enumerate() {
            let mut cells = ['.'; 8];
            cells[col] = BK2_COL_MNEMONIC[col];
            let field: String = cells.iter().collect();
            let log = parse_input_log(&format!("|..|{field}|\n")).unwrap();
            assert_eq!(log.pad1_track(), vec![1 << bit], "col {col}");
        }
    }

    #[test]
    fn console_press_is_recorded_and_warned_not_fatal() {
        let text = "[Input]\nLogKey:#Power|Reset|#P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|\n|..|........|\n|.r|.......A|\n|P.|........|\n";
        let log = parse_input_log(text).unwrap();
        assert_eq!(log.pad1_track(), vec![0x00, 0x01, 0x00]);
        assert_eq!(log.frames[0].command, 0);
        assert!(log.frames[1].is_reset() && !log.frames[1].is_power());
        assert!(log.frames[2].is_power() && !log.frames[2].is_reset());
        assert_eq!(log.first_console_press().map(|f| f.line_no), Some(4));
        // Without a LogKey the console cell is BizHawk's `Power|Reset`.
        let log = parse_input_log("|.r|........|\n").unwrap();
        assert!(log.frames[0].is_reset());
        // The whole-movie view surfaces presses like `.fm2` does.
        let zip = build_stored_zip(&[
            ("Header.txt", HEADER.as_bytes()),
            ("Input Log.txt", text.as_bytes()),
        ]);
        let m = parse_bk2_zip(&zip).unwrap();
        let w = m.warnings();
        assert!(w.iter().any(|w| w.contains("Reset")), "{w:?}");
        assert!(w.iter().any(|w| w.contains("Power")), "{w:?}");
        // Console buttons we cannot even name stay loud.
        let e = parse_input_log(
            "[Input]\nLogKey:#FDS Eject|Power|#P1 Up|P1 Down|P1 Left|P1 Right|P1 Start|P1 Select|P1 B|P1 A|\n|E.|........|\n",
        )
        .unwrap_err();
        assert!(e.msg.contains("not modeled"), "{e}");
    }

    #[test]
    fn truncated_pad_cell_is_an_error_not_a_console_cell() {
        // 7 glyphs is neither a console cell (2) nor a pad (>= 8).
        for text in [
            "| 5|.......|........|\n",
            "|..|.......|........|\n",
            "|..|.......|.......A|\n",
        ] {
            let e = parse_input_log(text).unwrap_err();
            assert!(e.msg.contains("7 cells"), "{text:?}: {e}");
        }
    }
}
