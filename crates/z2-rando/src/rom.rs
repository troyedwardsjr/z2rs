//! The ROM image the randomizer edits.
//!
//! # Offset conventions (read this before poking bytes)
//!
//! * **Offsets in this crate are headerless PRG offsets**: byte 0 is the
//!   first byte of PRG bank 0. There is no iNES header anywhere in a [`Rom`].
//! * The behavioural catalog (and most community notes) quote **iNES file
//!   offsets**, which include the 16-byte header. Convert with
//!   [`Rom::prg_from_ines`] (or subtract [`INES_HEADER_LEN`] by hand). A file
//!   offset in the vanilla fixed bank (`0x1C010..0x20010`) must go through
//!   [`Rom::vanilla_offset`] so it still lands in the fixed bank after
//!   [`Rom::expand_prg`].
//! * CPU addresses are `(bank, addr)` pairs. `$8000-$BFFF` is the switchable
//!   window, so `bank` matters there; `$C000-$FFFF` is always the fixed
//!   last bank, whatever `bank` says ([`Rom::cpu_offset`]).
//! * CHR is addressed separately ([`Rom::chr`] / [`Rom::write_chr`]) from 0.
//!
//! # Expansion
//!
//! The vanilla cartridge is MMC1 with 8 x 16 KiB PRG banks (the last one
//! fixed at `$C000`). [`Rom::expand_prg`] grows PRG to 16 banks (256 KiB):
//! banks 0-7 stay where they are, a copy of the fixed bank goes to bank 15
//! (the new fixed bank), and banks 8-14 are filled with `$FF` and handed to
//! the free-space allocator. The game's own bank switches write 3-bit
//! values, so they still reach banks 0-7; only our patches use 8-14.
//! Expansion is lazy: a seed whose modules never ask for space keeps the
//! vanilla 256 KiB body (vanilla flags give a byte-identical ROM).

use std::collections::BTreeMap;
use std::fmt;

use crate::RandoError;

/// iNES header length in bytes.
pub const INES_HEADER_LEN: usize = 16;
/// One PRG bank.
pub const PRG_BANK_LEN: usize = 0x4000;
/// One iNES CHR unit (8 KiB).
pub const CHR_UNIT_LEN: usize = 0x2000;
/// Vanilla PRG size (8 banks).
pub const VANILLA_PRG_LEN: usize = 8 * PRG_BANK_LEN;
/// Expanded PRG size (16 banks).
pub const EXPANDED_PRG_LEN: usize = 16 * PRG_BANK_LEN;
/// CHR size (unchanged by expansion).
pub const CHR_LEN: usize = 0x20000;
/// Headerless vanilla body (PRG + CHR).
pub const VANILLA_BODY_LEN: usize = VANILLA_PRG_LEN + CHR_LEN;
/// Headerless expanded body (PRG + CHR).
pub const EXPANDED_BODY_LEN: usize = EXPANDED_PRG_LEN + CHR_LEN;
/// Vanilla bank number of the fixed bank.
pub const VANILLA_FIXED_BANK: u8 = 7;
/// Headerless offset of the vanilla fixed bank.
pub const VANILLA_FIXED_BANK_OFFSET: usize = 7 * PRG_BANK_LEN;
/// Fill byte for unused space in new banks.
pub const FREE_FILL: u8 = 0xFF;

/// Synthetic iNES header for a headerless body (MMC1, mapper 1). Matches the
/// header the frontends build for the vanilla image, with the PRG/CHR unit
/// counts taken from the body.
#[must_use]
pub fn ines_header(prg_units: u8, chr_units: u8) -> [u8; INES_HEADER_LEN] {
    let mut h = [0u8; INES_HEADER_LEN];
    h[0..4].copy_from_slice(b"NES\x1A");
    h[4] = prg_units;
    h[5] = chr_units;
    h[6] = 0x10; // mapper low nibble 1 (MMC1), horizontal-mirror seed
    h
}

/// PRG unit count (16 KiB) for a headerless body length, if it is one of
/// the two layouts this project uses (vanilla or expanded, 128 KiB CHR).
#[must_use]
pub fn prg_units_for_body_len(len: usize) -> Option<u8> {
    match len {
        VANILLA_BODY_LEN => Some(8),
        EXPANDED_BODY_LEN => Some(16),
        _ => None,
    }
}

/// Free-space ranges per bank, as CPU address half-open ranges
/// `[start, end)`. For the switchable banks the window is `$8000-$BFFF`; for
/// the fixed bank it is `$C000-$FFFF`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FreeSpace {
    banks: BTreeMap<u8, Vec<(u32, u32)>>,
}

impl FreeSpace {
    /// Hand `[start, end)` in `bank` to the allocator (merging neighbours).
    pub fn free(&mut self, bank: u8, start: u16, end: u32) {
        let start = u32::from(start);
        if end <= start {
            return;
        }
        let list = self.banks.entry(bank).or_default();
        list.push((start, end));
        list.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(list.len());
        for &(s, e) in list.iter() {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        *list = merged;
    }

    /// First-fit allocation of `len` bytes in `bank`; returns the CPU start
    /// address.
    pub fn alloc_in(&mut self, bank: u8, len: usize) -> Option<u16> {
        let len = u32::try_from(len).ok()?;
        if len == 0 {
            return None;
        }
        let list = self.banks.get_mut(&bank)?;
        let i = list.iter().position(|&(s, e)| e - s >= len)?;
        let (s, e) = list[i];
        if e - s == len {
            list.remove(i);
        } else {
            list[i].0 = s + len;
        }
        Some(s as u16)
    }

    /// First-fit allocation in the first of `banks` (in order) with room.
    pub fn alloc_any(&mut self, banks: &[u8], len: usize) -> Option<(u8, u16)> {
        banks
            .iter()
            .find_map(|&b| self.alloc_in(b, len).map(|a| (b, a)))
    }

    /// Total free bytes in `bank`.
    #[must_use]
    pub fn free_in(&self, bank: u8) -> usize {
        self.banks
            .get(&bank)
            .map_or(0, |l| l.iter().map(|&(s, e)| (e - s) as usize).sum())
    }

    /// Banks that have any free space, ascending.
    #[must_use]
    pub fn banks(&self) -> Vec<u8> {
        self.banks
            .iter()
            .filter(|(_, l)| !l.is_empty())
            .map(|(&b, _)| b)
            .collect()
    }
}

/// Who may place bytes in a [`FreeRange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owner {
    /// Shared pool: any module, through [`Rom::alloc_vanilla`] (first fit,
    /// so two allocations never overlap).
    Pool,
    /// Reserved for one pipeline module (by its [`crate::PIPELINE`] name),
    /// which places bytes at fixed addresses or through its own allocator.
    /// [`Rom::alloc_vanilla`] never hands these out.
    Module(&'static str),
    /// Reserved for one pipeline module, handed out to it (and only to it)
    /// by [`Rom::alloc_vanilla`] before the shared pool.
    ModuleAlloc(&'static str),
}

/// One claimed range of vanilla padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeRange {
    /// Vanilla bank (7 = the fixed bank, which stays at `$C000`).
    pub bank: u8,
    /// First CPU address.
    pub start: u16,
    /// End CPU address (exclusive).
    pub end: u32,
    /// Who may use it.
    pub owner: Owner,
    /// What it is / why it is safe.
    pub note: &'static str,
}

const fn fr(bank: u8, start: u16, end: u32, owner: Owner, note: &'static str) -> FreeRange {
    FreeRange {
        bank,
        start,
        end,
        owner,
        note,
    }
}

const POOL: Owner = Owner::Pool;
const PALACES: Owner = Owner::Module("palaces");
const OVERWORLD: Owner = Owner::Module("overworld");
const HINTS: Owner = Owner::ModuleAlloc("hints");
const ASM_FEATURES: Owner = Owner::Module("asm_features");

/// **The** registry of vanilla-bank space the randomizer writes new bytes
/// into. Every range is `$FF` fill in the vanilla ROM that no game code or
/// data reads (checked against the player's ROM by a ROM-gated test, which
/// also checks that no two ranges overlap and that none touches a
/// protected range, see [`PROTECTED_RANGES`]).
///
/// Layout of the end of every switchable bank 0-6: a copy of the reset stub
/// and the MMC1 bank-switch helpers at `$BF70-$BFDF`, 26 bytes of `$FF` at
/// `$BFE0-$BFF9`, the vectors at `$BFFA`. The stub and the vectors are code
/// and data and are never claimed. In bank 4, `$BF00-$BF5F` is the palette
/// table of palaces 1-6 (read by the fixed bank's palace palette loader
/// with `LDA $BF00,Y`), not padding.
///
/// Patches that *replace* vanilla bytes in place (tables, operands, hooks
/// over existing code) are not listed here; they are tracked per byte by
/// the write-ownership guard ([`Rom::set_writer`]), which turns a byte
/// written by two different modules into an error.
///
/// Expansion banks 8-14 are not listed: they belong to [`Rom::alloc`].
pub const FREE_SPACE_REGISTRY: &[FreeRange] = &[
    // Shared pool (`Rom::alloc_vanilla`).
    fr(
        0,
        0xAA40,
        0xBF70,
        POOL,
        "bank 0 padding after the overworld code and tables, up to the reset stub",
    ),
    fr(
        7,
        0xD39A,
        0xD3CA,
        POOL,
        "fixed-bank padding between two routines",
    ),
    fr(
        7,
        0xFEAA,
        0xFED0,
        POOL,
        "fixed-bank padding before the bank-switch helpers",
    ),
    // asm_features: knockback trampolines, same address in every bank.
    fr(
        0,
        0xBFEC,
        0xBFF9,
        ASM_FEATURES,
        "knockback trampoline (bank 0 goes on to call the handler)",
    ),
    fr(1, 0xBFEC, 0xBFF2, ASM_FEATURES, "knockback trampoline"),
    fr(2, 0xBFEC, 0xBFF2, ASM_FEATURES, "knockback trampoline"),
    fr(3, 0xBFEC, 0xBFF2, ASM_FEATURES, "knockback trampoline"),
    fr(4, 0xBFEC, 0xBFF2, ASM_FEATURES, "knockback trampoline"),
    fr(5, 0xBFEC, 0xBFF2, ASM_FEATURES, "knockback trampoline"),
    fr(6, 0xBFEC, 0xBFF2, ASM_FEATURES, "knockback trampoline"),
    // overworld: re-encoded maps (two 1408-byte copies per bank).
    fr(
        1,
        0xB470,
        0xBF70,
        OVERWORLD,
        "West Hyrule map at $B470, Death Mountain map at $B9F0",
    ),
    fr(
        2,
        0xB470,
        0xBF70,
        OVERWORLD,
        "East Hyrule map at $B470, Maze Island map at $B9F0",
    ),
    // hints: hidden-jar marker hooks (run with the sideview bank mapped).
    fr(
        4,
        0x9FC0,
        0xA000,
        HINTS,
        "last 64 bytes of the bank 4 padding before $A000",
    ),
    fr(
        5,
        0xBF20,
        0xBF60,
        HINTS,
        "bank 5 padding before the Great Palace palette at $BF60",
    ),
    // palaces: new or duplicated room data.
    fr(4, 0x83DC, 0x8409, PALACES, "palace room data padding"),
    fr(4, 0x84F0, 0x8500, PALACES, "palace room data padding"),
    fr(4, 0x870E, 0x871B, PALACES, "palace room data padding"),
    fr(4, 0x8817, 0x88A0, PALACES, "palace room data padding"),
    fr(4, 0x8EC3, 0x8FB7, PALACES, "palace room data padding"),
    fr(
        4,
        0x9EE0,
        0x9FC0,
        PALACES,
        "palace room data padding (the last 64 bytes go to hints)",
    ),
    fr(4, 0xA1E3, 0xA1F8, PALACES, "palace room data padding"),
    fr(4, 0xA3FB, 0xA440, PALACES, "palace room data padding"),
    fr(4, 0xA539, 0xA610, PALACES, "palace room data padding"),
    fr(4, 0xA765, 0xA7AB, PALACES, "palace room data padding"),
    fr(5, 0x84C2, 0x84D0, PALACES, "Great Palace room data padding"),
    fr(5, 0x84E0, 0x84F0, PALACES, "Great Palace room data padding"),
    fr(5, 0x84F4, 0x8500, PALACES, "Great Palace room data padding"),
    fr(5, 0x8705, 0x871B, PALACES, "Great Palace room data padding"),
    fr(5, 0x8891, 0x88A0, PALACES, "Great Palace room data padding"),
    fr(5, 0x8B31, 0x8B50, PALACES, "Great Palace room data padding"),
    fr(5, 0x93AE, 0x9400, PALACES, "Great Palace room data padding"),
    fr(5, 0xA54F, 0xA5AF, PALACES, "Great Palace room data padding"),
    fr(5, 0xBDA1, 0xBF00, PALACES, "Great Palace room data padding"),
];

/// Vanilla bytes no module may change, as `(bank, start, end, what)`: the
/// reset stubs and bank-switch helpers at the end of every bank, and all
/// vectors.
pub const PROTECTED_RANGES: &[(u8, u16, u32, &str)] = &[
    (0, 0xBF70, 0xBFE0, "reset stub"),
    (1, 0xBF70, 0xBFE0, "reset stub"),
    (2, 0xBF70, 0xBFE0, "reset stub"),
    (3, 0xBF70, 0xBFE0, "reset stub"),
    (4, 0xBF70, 0xBFE0, "reset stub"),
    (5, 0xBF70, 0xBFE0, "reset stub"),
    (6, 0xBF70, 0xBFE0, "reset stub"),
    (0, 0xBFFA, 0xC000, "vectors"),
    (1, 0xBFFA, 0xC000, "vectors"),
    (2, 0xBFFA, 0xC000, "vectors"),
    (3, 0xBFFA, 0xC000, "vectors"),
    (4, 0xBFFA, 0xC000, "vectors"),
    (5, 0xBFFA, 0xC000, "vectors"),
    (6, 0xBFFA, 0xC000, "vectors"),
    (
        7,
        0xFF70,
        0x1_0000,
        "fixed-bank reset stub, bank-switch helpers and vectors",
    ),
];

/// The registry ranges reserved for `owner`, as `(bank, start, end)`.
#[must_use]
pub fn claimed_ranges(owner: Owner) -> Vec<(u8, u16, u32)> {
    FREE_SPACE_REGISTRY
        .iter()
        .filter(|r| r.owner == owner)
        .map(|r| (r.bank, r.start, r.end))
        .collect()
}

/// Where a [`SharedWrite`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// CPU range `[start, end)` in a vanilla bank (7 = the fixed bank).
    Cpu(u8, u16, u32),
    /// CHR offsets `[start, end)`.
    Chr(usize, usize),
    /// Every [`FREE_SPACE_REGISTRY`] range reserved for this module.
    ClaimsOf(&'static str),
}

/// Bytes two modules may both write: the later module deliberately builds
/// on the earlier one's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedWrite {
    /// Module that writes first.
    pub first: &'static str,
    /// Module that rewrites the bytes later.
    pub second: &'static str,
    /// Where.
    pub span: Span,
    /// Why this is fine.
    pub why: &'static str,
}

/// Every byte range two modules may both write. Any other byte written by
/// two different modules is a [`WriteConflict`].
pub const SHARED_WRITES: &[SharedWrite] = &[
    SharedWrite {
        first: "start",
        second: "spells",
        span: Span::Cpu(5, 0xBAE7, 0xBAEF),
        why: "the save template's spells-known bytes: start sets them by spell, \
              the spell menu permutation then moves each byte to its spell's new slot",
    },
    SharedWrite {
        first: "spells",
        second: "stats",
        span: Span::Cpu(0, 0x8D7B, 0x8DBB),
        why: "the spell cost table: spells moves the rows into menu order, \
              stats then rescales the costs row by row",
    },
    SharedWrite {
        first: "palaces",
        second: "items",
        span: Span::ClaimsOf("palaces"),
        why: "item bytes inside room data the palaces module copied into its \
              own space (recorded in `State::palaces`); items fills them in",
    },
];

/// Whether CPU `[start, end)` of vanilla bank `bank` contains headerless
/// PRG offset `off` (either fixed-bank location for bank 7).
fn cpu_span_contains(bank: u8, start: u16, end: u32, off: usize) -> bool {
    let len = end.saturating_sub(u32::from(start)) as usize;
    let bases: &[usize] = if bank == VANILLA_FIXED_BANK {
        &[7, 15]
    } else {
        &[usize::from(bank)]
    };
    let window = if bank == VANILLA_FIXED_BANK {
        0xC000
    } else {
        0x8000
    };
    bases.iter().any(|&b| {
        let base = b * PRG_BANK_LEN + usize::from(start) - window;
        (base..base + len).contains(&off)
    })
}

/// Whether [`SHARED_WRITES`] lets `second` rewrite a byte `first` wrote
/// (`off` is a headerless PRG or CHR offset in the current layout).
#[must_use]
pub fn shared_write_allowed(chr: bool, off: usize, first: &str, second: &str) -> bool {
    SHARED_WRITES
        .iter()
        .any(|w| w.covers(chr, off, first, second))
}

impl SharedWrite {
    fn covers(&self, chr: bool, off: usize, first: &str, second: &str) -> bool {
        if self.first != first || self.second != second {
            return false;
        }
        match self.span {
            Span::Chr(s, e) => chr && (s..e).contains(&off),
            Span::Cpu(b, s, e) => !chr && cpu_span_contains(b, s, e, off),
            Span::ClaimsOf(m) => {
                !chr && FREE_SPACE_REGISTRY.iter().any(|r| {
                    matches!(r.owner, Owner::Module(o) | Owner::ModuleAlloc(o) if o == m)
                        && cpu_span_contains(r.bank, r.start, r.end, off)
                })
            }
        }
    }
}

/// Two modules wrote the same byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteConflict {
    /// `true` for a CHR byte.
    pub chr: bool,
    /// Headerless PRG (or CHR) offset.
    pub offset: usize,
    /// Module that wrote it first.
    pub first: &'static str,
    /// Module that wrote it again.
    pub second: &'static str,
}

impl fmt::Display for WriteConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = if self.chr { "CHR" } else { "PRG" };
        write!(
            f,
            "{what} byte {:#X} written by {} and then by {}",
            self.offset, self.first, self.second
        )
    }
}

/// One run of bytes written by one module (see [`Rom::write_runs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteRun {
    /// Module name.
    pub owner: &'static str,
    /// `true` for CHR.
    pub chr: bool,
    /// Headerless offset of the first byte.
    pub start: usize,
    /// Length.
    pub len: usize,
}

/// A headerless MMC1 image (PRG + CHR), a free-space allocator, and a
/// per-byte record of which pipeline module wrote what (the write-ownership
/// guard, see [`Rom::set_writer`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rom {
    prg: Vec<u8>,
    chr: Vec<u8>,
    free: FreeSpace,
    /// Registry space per owner.
    vanilla_free: BTreeMap<Owner, FreeSpace>,
    /// Names of the writers seen so far (index + 1 is the id).
    writers: Vec<&'static str>,
    /// Current writer id (0 = untracked, e.g. unit tests poking a ROM).
    writer: u8,
    prg_owner: Vec<u8>,
    chr_owner: Vec<u8>,
    conflicts: Vec<WriteConflict>,
}

impl Rom {
    /// Wrap a headerless body in either supported layout (vanilla 256 KiB or
    /// expanded 384 KiB). The bytes are copied; no hash check is done here.
    pub fn from_body(body: &[u8]) -> Result<Rom, RandoError> {
        let units = prg_units_for_body_len(body.len()).ok_or_else(|| {
            RandoError::Rom(format!(
                "unexpected ROM body size {} (want {VANILLA_BODY_LEN} or {EXPANDED_BODY_LEN})",
                body.len()
            ))
        })?;
        let prg_len = usize::from(units) * PRG_BANK_LEN;
        let mut rom = Rom {
            prg: body[..prg_len].to_vec(),
            chr: body[prg_len..].to_vec(),
            free: FreeSpace::default(),
            vanilla_free: BTreeMap::new(),
            writers: Vec::new(),
            writer: 0,
            prg_owner: vec![0; prg_len],
            chr_owner: vec![0; body.len() - prg_len],
            conflicts: Vec::new(),
        };
        for r in FREE_SPACE_REGISTRY {
            rom.vanilla_free
                .entry(r.owner)
                .or_default()
                .free(r.bank, r.start, r.end);
        }
        if rom.is_expanded() {
            for b in 8..15 {
                rom.free.free(b, 0x8000, 0xC000);
            }
        }
        Ok(rom)
    }

    /// Headerless body (PRG then CHR).
    #[must_use]
    pub fn body(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.prg.len() + self.chr.len());
        v.extend_from_slice(&self.prg);
        v.extend_from_slice(&self.chr);
        v
    }

    /// Full iNES image (synthetic header + body).
    #[must_use]
    pub fn ines(&self) -> Vec<u8> {
        let mut v = ines_header(self.prg_units(), self.chr_units()).to_vec();
        v.extend_from_slice(&self.prg);
        v.extend_from_slice(&self.chr);
        v
    }

    /// PRG size in 16 KiB units.
    #[must_use]
    pub fn prg_units(&self) -> u8 {
        (self.prg.len() / PRG_BANK_LEN) as u8
    }

    /// CHR size in 8 KiB units.
    #[must_use]
    pub fn chr_units(&self) -> u8 {
        (self.chr.len() / CHR_UNIT_LEN) as u8
    }

    /// Number of 16 KiB PRG banks.
    #[must_use]
    pub fn bank_count(&self) -> u8 {
        self.prg_units()
    }

    /// Bank number of the fixed (`$C000`) bank: 7 vanilla, 15 expanded.
    #[must_use]
    pub fn fixed_bank(&self) -> u8 {
        self.bank_count() - 1
    }

    /// Whether [`Rom::expand_prg`] has run (or the body was already expanded).
    #[must_use]
    pub fn is_expanded(&self) -> bool {
        self.prg.len() == EXPANDED_PRG_LEN
    }

    /// PRG bytes.
    #[must_use]
    pub fn prg(&self) -> &[u8] {
        &self.prg
    }

    /// CHR bytes.
    #[must_use]
    pub fn chr(&self) -> &[u8] {
        &self.chr
    }

    /// Write `bytes` at CHR offset `off` (tracked like PRG writes).
    pub fn write_chr(&mut self, off: usize, bytes: &[u8]) -> Result<(), RandoError> {
        let dst = self.chr.get_mut(off..off + bytes.len()).ok_or_else(|| {
            RandoError::Rom(format!("CHR write {off:#X}+{} out of range", bytes.len()))
        })?;
        dst.copy_from_slice(bytes);
        self.claim(true, off, bytes.len());
        Ok(())
    }

    /// Make `name` the module that owns every byte written from now on
    /// (the pipeline calls this before each module). Writing a byte that a
    /// different module already wrote records a [`WriteConflict`] unless
    /// the pair is listed in [`SHARED_WRITES`].
    pub fn set_writer(&mut self, name: &'static str) {
        let id = match self.writers.iter().position(|&w| w == name) {
            Some(i) => i + 1,
            None => {
                self.writers.push(name);
                self.writers.len()
            }
        };
        self.writer = u8::try_from(id).expect("fewer than 256 writers");
    }

    /// The current writer, if any.
    #[must_use]
    pub fn writer(&self) -> Option<&'static str> {
        self.name_of(self.writer)
    }

    fn name_of(&self, id: u8) -> Option<&'static str> {
        id.checked_sub(1)
            .and_then(|i| self.writers.get(usize::from(i)).copied())
    }

    /// Conflicts recorded so far.
    #[must_use]
    pub fn conflicts(&self) -> &[WriteConflict] {
        &self.conflicts
    }

    /// The module that last wrote PRG byte `off` (current layout).
    #[must_use]
    pub fn prg_owner(&self, off: usize) -> Option<&'static str> {
        self.prg_owner.get(off).and_then(|&id| self.name_of(id))
    }

    /// Every tracked write as owner runs (PRG first, then CHR, ascending).
    #[must_use]
    pub fn write_runs(&self) -> Vec<WriteRun> {
        let mut out = Vec::new();
        for (chr, owners) in [(false, &self.prg_owner), (true, &self.chr_owner)] {
            let mut i = 0;
            while i < owners.len() {
                let id = owners[i];
                let mut j = i + 1;
                while j < owners.len() && owners[j] == id {
                    j += 1;
                }
                if let Some(owner) = self.name_of(id) {
                    out.push(WriteRun {
                        owner,
                        chr,
                        start: i,
                        len: j - i,
                    });
                }
                i = j;
            }
        }
        out
    }

    fn claim(&mut self, chr: bool, off: usize, len: usize) {
        let me = self.writer;
        if me == 0 {
            return;
        }
        let owners = if chr {
            &mut self.chr_owner
        } else {
            &mut self.prg_owner
        };
        let mut found = Vec::new();
        for (i, o) in owners[off..off + len].iter_mut().enumerate() {
            if *o != 0 && *o != me {
                found.push((off + i, *o));
            }
            *o = me;
        }
        let second = self.name_of(me).unwrap_or("?");
        for (offset, prev) in found {
            let first = self.name_of(prev).unwrap_or("?");
            if shared_write_allowed(chr, offset, first, second) {
                continue;
            }
            // Keep the list short; one entry per byte is plenty to debug.
            if self.conflicts.len() < 64 {
                self.conflicts.push(WriteConflict {
                    chr,
                    offset,
                    first,
                    second,
                });
            }
        }
    }

    /// The fixed bank's 16 KiB (`$C000-$FFFF`).
    #[must_use]
    pub fn fixed_bank_bytes(&self) -> &[u8] {
        let base = usize::from(self.fixed_bank()) * PRG_BANK_LEN;
        &self.prg[base..base + PRG_BANK_LEN]
    }

    /// Grow PRG to 256 KiB (see the module docs). Idempotent.
    pub fn expand_prg(&mut self) {
        if self.is_expanded() {
            return;
        }
        let fixed = self.prg[VANILLA_FIXED_BANK_OFFSET..VANILLA_PRG_LEN].to_vec();
        self.prg.resize(EXPANDED_PRG_LEN, FREE_FILL);
        let new_fixed = 15 * PRG_BANK_LEN;
        self.prg[new_fixed..].copy_from_slice(&fixed);
        let owners = self.prg_owner[VANILLA_FIXED_BANK_OFFSET..VANILLA_PRG_LEN].to_vec();
        self.prg_owner.resize(EXPANDED_PRG_LEN, 0);
        self.prg_owner[new_fixed..].copy_from_slice(&owners);
        for b in 8..15 {
            self.free.free(b, 0x8000, 0xC000);
        }
    }

    /// Map a headerless offset that was computed against the **vanilla**
    /// layout to the current layout (the vanilla fixed bank moves to the
    /// last bank after expansion).
    #[must_use]
    pub fn vanilla_offset(&self, off: usize) -> usize {
        if (VANILLA_FIXED_BANK_OFFSET..VANILLA_PRG_LEN).contains(&off) {
            usize::from(self.fixed_bank()) * PRG_BANK_LEN + (off - VANILLA_FIXED_BANK_OFFSET)
        } else {
            off
        }
    }

    /// Convert an iNES **file** offset of a vanilla-layout PRG byte (header
    /// included, as quoted by the catalog) into a current headerless PRG
    /// offset.
    pub fn prg_from_ines(&self, file_off: usize) -> Result<usize, RandoError> {
        let off = file_off
            .checked_sub(INES_HEADER_LEN)
            .filter(|&o| o < VANILLA_PRG_LEN)
            .ok_or_else(|| {
                RandoError::Rom(format!(
                    "file offset {file_off:#X} is not a vanilla PRG byte"
                ))
            })?;
        Ok(self.vanilla_offset(off))
    }

    /// Headerless PRG offset of CPU `addr` with `bank` in the switchable
    /// window. `$C000-$FFFF` always resolves to the fixed bank.
    pub fn cpu_offset(&self, bank: u8, addr: u16) -> Result<usize, RandoError> {
        match addr {
            0x8000..=0xBFFF => {
                if bank >= self.bank_count() {
                    return Err(RandoError::Rom(format!(
                        "bank {bank} does not exist (PRG has {} banks)",
                        self.bank_count()
                    )));
                }
                Ok(usize::from(bank) * PRG_BANK_LEN + usize::from(addr - 0x8000))
            }
            0xC000..=0xFFFF => {
                Ok(usize::from(self.fixed_bank()) * PRG_BANK_LEN + usize::from(addr - 0xC000))
            }
            _ => Err(RandoError::Rom(format!("${addr:04X} is not a PRG address"))),
        }
    }

    /// One PRG byte by headerless offset.
    pub fn read(&self, off: usize) -> Result<u8, RandoError> {
        self.prg
            .get(off)
            .copied()
            .ok_or_else(|| RandoError::Rom(format!("PRG offset {off:#X} out of range")))
    }

    /// `len` PRG bytes by headerless offset.
    pub fn read_slice(&self, off: usize, len: usize) -> Result<&[u8], RandoError> {
        self.prg
            .get(off..off + len)
            .ok_or_else(|| RandoError::Rom(format!("PRG range {off:#X}+{len} out of range")))
    }

    /// Write `bytes` at headerless PRG offset `off`.
    pub fn write(&mut self, off: usize, bytes: &[u8]) -> Result<(), RandoError> {
        let dst = self.prg.get_mut(off..off + bytes.len()).ok_or_else(|| {
            RandoError::Rom(format!("PRG write {off:#X}+{} out of range", bytes.len()))
        })?;
        dst.copy_from_slice(bytes);
        self.claim(false, off, bytes.len());
        Ok(())
    }

    /// One byte at CPU `(bank, addr)`.
    pub fn read_cpu(&self, bank: u8, addr: u16) -> Result<u8, RandoError> {
        self.read(self.cpu_offset(bank, addr)?)
    }

    /// Little-endian word at CPU `(bank, addr)`.
    pub fn read_cpu_word(&self, bank: u8, addr: u16) -> Result<u16, RandoError> {
        let lo = self.read_cpu(bank, addr)?;
        let hi = self.read_cpu(bank, addr.wrapping_add(1))?;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    /// Write `bytes` at CPU `(bank, addr)`; must not cross the end of the
    /// 16 KiB window.
    pub fn write_cpu(&mut self, bank: u8, addr: u16, bytes: &[u8]) -> Result<(), RandoError> {
        let window_end: u32 = if addr >= 0xC000 { 0x1_0000 } else { 0xC000 };
        if u32::from(addr) + bytes.len() as u32 > window_end {
            return Err(RandoError::Rom(format!(
                "write at ${addr:04X}+{} crosses the bank window",
                bytes.len()
            )));
        }
        let off = self.cpu_offset(bank, addr)?;
        self.write(off, bytes)
    }

    /// Write a little-endian word at CPU `(bank, addr)`.
    pub fn write_cpu_word(&mut self, bank: u8, addr: u16, v: u16) -> Result<(), RandoError> {
        self.write_cpu(bank, addr, &v.to_le_bytes())
    }

    /// Write assembled chunks into `bank` (each chunk at its `.org`).
    pub fn apply_asm(&mut self, bank: u8, out: &crate::asm::Assembled) -> Result<(), RandoError> {
        for (org, bytes) in &out.chunks {
            self.write_cpu(bank, *org, bytes)?;
        }
        Ok(())
    }

    /// The free-space allocator.
    pub fn free_space(&mut self) -> &mut FreeSpace {
        &mut self.free
    }

    /// Allocate `len` bytes in any expansion bank (expanding PRG first if
    /// needed). Returns `(bank, cpu_addr)`.
    pub fn alloc(&mut self, len: usize) -> Result<(u8, u16), RandoError> {
        self.expand_prg();
        let banks = self.free.banks();
        self.free
            .alloc_any(&banks, len)
            .ok_or_else(|| RandoError::Rom(format!("no free PRG space for {len} bytes")))
    }

    /// Allocate `len` bytes of registered vanilla padding
    /// ([`FREE_SPACE_REGISTRY`]) in vanilla bank `bank` (7 = the fixed
    /// bank, which stays at `$C000` after expansion): first from the
    /// [`Owner::ModuleAlloc`] ranges of the current writer
    /// ([`Rom::set_writer`]), then from the shared pool. Returns the CPU address; write with
    /// [`Rom::write_cpu`]`(bank, addr, ..)`.
    pub fn alloc_vanilla(&mut self, bank: u8, len: usize) -> Result<u16, RandoError> {
        let mine = self.writer().map(Owner::ModuleAlloc);
        for owner in mine.into_iter().chain([Owner::Pool]) {
            if let Some(a) = self
                .vanilla_free
                .get_mut(&owner)
                .and_then(|fs| fs.alloc_in(bank, len))
            {
                return Ok(a);
            }
        }
        Err(RandoError::Rom(format!(
            "no vanilla free space for {len} bytes in bank {bank}"
        )))
    }

    /// CRC32 (IEEE) of the headerless body.
    #[must_use]
    pub fn crc32(&self) -> u32 {
        z2_assets::rom::crc32_ieee(&self.body())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic() -> Rom {
        let mut body = vec![0u8; VANILLA_BODY_LEN];
        for (b, chunk) in body[..VANILLA_PRG_LEN].chunks_mut(PRG_BANK_LEN).enumerate() {
            chunk.fill(b as u8);
        }
        body[VANILLA_PRG_LEN..].fill(0xCC);
        Rom::from_body(&body).unwrap()
    }

    #[test]
    fn rejects_odd_sizes() {
        assert!(Rom::from_body(&[0u8; 100]).is_err());
    }

    #[test]
    fn cpu_offsets_follow_mmc1_fixed_last_bank() {
        let rom = synthetic();
        assert_eq!(rom.fixed_bank(), 7);
        assert_eq!(rom.cpu_offset(3, 0x8000).unwrap(), 3 * PRG_BANK_LEN);
        assert_eq!(rom.cpu_offset(0, 0xC000).unwrap(), 7 * PRG_BANK_LEN);
        assert_eq!(rom.read_cpu(5, 0xBFFF).unwrap(), 5);
        assert_eq!(rom.read_cpu(0, 0xFFFF).unwrap(), 7);
        assert!(rom.cpu_offset(9, 0x8000).is_err());
        assert!(rom.cpu_offset(0, 0x6000).is_err());
    }

    #[test]
    fn expand_moves_fixed_bank_and_frees_new_banks() {
        let mut rom = synthetic();
        let before_chr = rom.chr().to_vec();
        rom.expand_prg();
        assert!(rom.is_expanded());
        assert_eq!(rom.prg_units(), 16);
        assert_eq!(rom.fixed_bank(), 15);
        assert_eq!(
            rom.read_cpu(0, 0xC000).unwrap(),
            7,
            "fixed bank content kept"
        );
        assert_eq!(
            rom.read_cpu(7, 0x8000).unwrap(),
            7,
            "bank 7 copy kept in place"
        );
        assert_eq!(rom.read_cpu(9, 0x8000).unwrap(), FREE_FILL);
        assert_eq!(rom.chr(), &before_chr[..]);
        assert_eq!(rom.body().len(), EXPANDED_BODY_LEN);
        for b in 8..15 {
            assert_eq!(rom.free_space().free_in(b), PRG_BANK_LEN);
        }
        assert_eq!(rom.free_space().free_in(15), 0);
        // Idempotent.
        let snap = rom.clone();
        rom.expand_prg();
        assert_eq!(rom, snap);
        // Round trip through the body.
        assert_eq!(Rom::from_body(&rom.body()).unwrap().fixed_bank(), 15);
    }

    #[test]
    fn vanilla_offsets_track_the_fixed_bank() {
        let mut rom = synthetic();
        assert_eq!(rom.vanilla_offset(0x1C123), 0x1C123);
        assert_eq!(rom.prg_from_ines(0x1C133).unwrap(), 0x1C123);
        rom.expand_prg();
        assert_eq!(rom.vanilla_offset(0x1C123), 15 * PRG_BANK_LEN + 0x123);
        assert_eq!(rom.vanilla_offset(0x4000), 0x4000);
        assert!(rom.prg_from_ines(0x5).is_err());
        assert!(rom.prg_from_ines(0x20010).is_err());
    }

    #[test]
    fn writes_respect_bank_windows() {
        let mut rom = synthetic();
        rom.write_cpu(2, 0x9000, &[1, 2, 3]).unwrap();
        assert_eq!(
            rom.read_slice(2 * PRG_BANK_LEN + 0x1000, 3).unwrap(),
            &[1, 2, 3]
        );
        assert!(rom.write_cpu(2, 0xBFFF, &[1, 2]).is_err());
        rom.write_cpu_word(0, 0xFFFE, 0x1234).unwrap();
        assert_eq!(rom.read_cpu_word(0, 0xFFFE).unwrap(), 0x1234);
    }

    #[test]
    fn allocator_first_fit_and_merge() {
        let mut fs = FreeSpace::default();
        fs.free(3, 0x9000, 0x9010);
        fs.free(3, 0x9010, 0x9020);
        assert_eq!(fs.free_in(3), 0x20);
        assert_eq!(fs.alloc_in(3, 0x18), Some(0x9000));
        assert_eq!(fs.alloc_in(3, 0x10), None);
        assert_eq!(fs.alloc_in(3, 0x8), Some(0x9018));
        assert_eq!(fs.free_in(3), 0);
        assert_eq!(fs.alloc_in(4, 1), None);
        let mut rom = synthetic();
        let (bank, addr) = rom.alloc(0x100).unwrap();
        assert!(rom.is_expanded());
        assert_eq!((bank, addr), (8, 0x8000));
        assert_eq!(rom.alloc(0x100).unwrap(), (8, 0x8100));
    }

    #[test]
    fn ownership_guard_flags_a_second_writer() {
        let mut rom = synthetic();
        // Untracked writes (no writer set) are not recorded.
        rom.write_cpu(2, 0x9000, &[1]).unwrap();
        assert!(rom.write_runs().is_empty());
        rom.set_writer("overworld");
        rom.write_cpu(2, 0x9000, &[1, 2]).unwrap();
        rom.write_cpu(2, 0x9001, &[3]).unwrap(); // same module again: fine
        assert!(rom.conflicts().is_empty());
        rom.set_writer("enemies");
        rom.write_cpu(2, 0x9001, &[4, 5]).unwrap();
        assert_eq!(rom.conflicts().len(), 1);
        let c = &rom.conflicts()[0];
        assert_eq!((c.first, c.second, c.chr), ("overworld", "enemies", false));
        assert_eq!(c.offset, rom.cpu_offset(2, 0x9001).unwrap());
        assert_eq!(rom.prg_owner(c.offset), Some("enemies"));
        // CHR writes are tracked the same way.
        rom.write_chr(0x10, &[9]).unwrap();
        rom.set_writer("cosmetic");
        rom.write_chr(0x10, &[8]).unwrap();
        assert_eq!(rom.conflicts().len(), 2);
        assert!(rom.conflicts()[1].chr);
    }

    #[test]
    fn shared_writes_are_allowed_only_in_their_span() {
        let mut rom = synthetic();
        rom.set_writer("spells");
        rom.write_cpu(0, 0x8D7B, &[1]).unwrap();
        rom.write_cpu(0, 0x8DBB, &[1]).unwrap();
        rom.set_writer("stats");
        rom.write_cpu(0, 0x8D7B, &[2]).unwrap();
        assert!(rom.conflicts().is_empty());
        rom.write_cpu(0, 0x8DBB, &[2]).unwrap();
        assert_eq!(rom.conflicts().len(), 1, "just past the cost table");
        // The reverse order is not listed.
        let mut rom = synthetic();
        rom.set_writer("stats");
        rom.write_cpu(0, 0x8D7B, &[1]).unwrap();
        rom.set_writer("spells");
        rom.write_cpu(0, 0x8D7B, &[2]).unwrap();
        assert_eq!(rom.conflicts().len(), 1);
    }

    #[test]
    fn expansion_keeps_fixed_bank_owners() {
        let mut rom = synthetic();
        rom.set_writer("qol");
        rom.write_cpu(0, 0xD000, &[1]).unwrap();
        rom.expand_prg();
        let off = rom.cpu_offset(0, 0xD000).unwrap();
        assert_eq!(off / PRG_BANK_LEN, 15);
        assert_eq!(rom.prg_owner(off), Some("qol"));
        rom.set_writer("drops");
        rom.write_cpu(0, 0xD000, &[2]).unwrap();
        assert_eq!(rom.conflicts().len(), 1);
    }

    #[test]
    fn reserved_space_goes_to_its_owner_only() {
        let mut rom = synthetic();
        rom.set_writer("enemies");
        assert!(rom.alloc_vanilla(5, 8).is_err(), "bank 5 is hints-only");
        rom.set_writer("hints");
        let a = rom.alloc_vanilla(5, 8).unwrap();
        assert_eq!(a, 0xBF20);
        // Pool space is open to everyone.
        rom.set_writer("enemies");
        assert_eq!(rom.alloc_vanilla(0, 4).unwrap(), 0xAA40);
        // Fixed-address claims are never handed out.
        rom.set_writer("asm_features");
        assert_eq!(rom.alloc_vanilla(0, 4).unwrap(), 0xAA44);
    }

    #[test]
    fn registry_ranges_are_in_their_bank_and_disjoint() {
        for r in FREE_SPACE_REGISTRY {
            let window = if r.bank == 7 { 0xC000 } else { 0x8000 };
            assert!(
                u32::from(r.start) >= window && r.end <= window + 0x4000,
                "{r:?}"
            );
            assert!(u32::from(r.start) < r.end, "{r:?}");
            for q in FREE_SPACE_REGISTRY {
                if std::ptr::eq(r, q) || q.bank != r.bank {
                    continue;
                }
                assert!(
                    r.end <= u32::from(q.start) || q.end <= u32::from(r.start),
                    "{r:?} overlaps {q:?}"
                );
            }
            for &(b, s, e, what) in PROTECTED_RANGES {
                assert!(
                    b != r.bank || r.end <= u32::from(s) || e <= u32::from(r.start),
                    "{r:?} overlaps the {what}"
                );
            }
        }
        // Every module named in the registry is a pipeline module.
        for r in FREE_SPACE_REGISTRY {
            if let Owner::Module(m) | Owner::ModuleAlloc(m) = r.owner {
                assert!(crate::PIPELINE.iter().any(|(n, _)| *n == m), "{m}");
            }
        }
    }

    #[test]
    fn header_matches_frontend_convention() {
        let h = ines_header(16, 16);
        assert_eq!(&h[..8], b"NES\x1A\x10\x10\x10\x00");
        assert_eq!(synthetic().ines().len(), INES_HEADER_LEN + VANILLA_BODY_LEN);
    }
}
