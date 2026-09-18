//! Labelled snapshot corpus schema.
//!
//! `std`-only: compiles standalone (`rustc --test snapshot.rs`) and later as
//! `z2_verify::snapshot`.
//!
//! ## Model
//!
//! A [`Snapshot`] is full oracle state at a labelled frame, **plus the
//! complete zelda3-style input history from power-on**, so any snapshot is
//! replayable from boot: restore the blobs, then feed `input_history`
//! (filtered through the [`LagMap`](crate::lag::LagMap) logic-frame track)
//! and the oracle must reproduce the state. The 600-frame restore+replay
//! check (oracle-vs-oracle, gated on `Z2_ROM`) is the corpus acceptance test;
//! the harness lives with the oracle and consumes this schema.
//!
//! State blobs are opaque `Vec<u8>` sized by the constants below. The oracle
//! owns the canonical byte layouts; until it lands, producers fill
//! the blobs per [`BlobLayoutV1`] and set [`Snapshot::state_source`] to
//! describe what wrote them.
//!
//! ## Layout mapping for [`BlobLayoutV1::OracleV1`] (corpus mint)
//!
//! * `ram` / `wram` — live CPU images at the capture frame, for diffing and
//!   for the lockstep prefix-replay check (`verify --snapshot` replays
//!   `input_history` from power-on and compares these bytes).
//! * `ppu` / `apu` / `mapper` — empty (reserved): `TetanesOracle` exposes no
//!   split PPU/APU/mapper export, so there is nothing faithful to put here.
//! * `oracle_blob` — the full opaque oracle save-state (`save_state` bytes,
//!   measured 46,112 B on the pinned ROM); the restore source for
//!   `load_state`. This is what makes a snapshot restorable, the live images
//!   are what make it comparable.
//!
//! Snapshots, movies and ROMs are **never committed**: the corpus lives in
//! the gitignored out-of-tree `corpus/` dir (see README.md).

use std::error::Error;
use std::fmt;

// ---------------------------------------------------------------------------
// Pinned ROM identity (see LEGAL.md).
// ---------------------------------------------------------------------------

/// Body (header-stripped) CRC32 of the pinned No-Intro USA ROM.
pub const PINNED_BODY_CRC32: u32 = 0xBA32_2865;
/// Body (header-stripped) SHA1 of the pinned No-Intro USA ROM.
pub const PINNED_BODY_SHA1: &str = "11333adb723a5975e0ecca3aee8f4747aa8d2d26";
/// `Z2_ROM` env var pointing at the local reference ROM (read-only).
pub const Z2_ROM_ENV: &str = "Z2_ROM";

// ---------------------------------------------------------------------------
// Snapshot schema.
// ---------------------------------------------------------------------------

/// Magic bytes opening every encoded snapshot.
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"Z2SNAP01";
/// Current snapshot encoding version.
///
/// v2 adds the trailing [`Snapshot::oracle_blob`] field. Decoders accept v1
/// (blob reads back empty); v1 encodings stay byte-stable so old files and
/// old readers keep working.
pub const SNAPSHOT_VERSION: u16 = 2;
/// Previous encoding version (no oracle blob); still accepted by [`Snapshot::decode`].
pub const SNAPSHOT_VERSION_V1: u16 = 1;

/// NES internal RAM (`$0000-$07FF`).
pub const RAM_SIZE: usize = 2048;
/// Zelda II battery-backed WRAM (`$6000-$7FFF`, MMC1).
pub const WRAM_SIZE: usize = 8192;

/// Maximum accepted opaque-blob / history lengths (DoS bound for decode).
pub const MAX_BLOB_LEN: usize = 64 * 1024;
pub const MAX_HISTORY_LEN: usize = 4 * 1024 * 1024;
/// Maximum accepted [`Snapshot::oracle_blob`] length on decode/attach.
///
/// Same bound as [`MAX_BLOB_LEN`]: the measured oracle save-state on the
/// pinned ROM is 46,112 B, so 64 KiB leaves ~40% headroom. If a future
/// oracle backend exceeds it, bump this with a codec revision, not silently.
pub const MAX_ORACLE_BLOB_LEN: usize = MAX_BLOB_LEN;

/// Describes which oracle layout version wrote the opaque blobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobLayoutV1 {
    /// Placeholder layout for blobs minted without an oracle state export.
    /// `ppu`/`apu`/`mapper` blobs are empty; `ram`/`wram` hold raw bytes.
    Uninit = 0,
    /// First real oracle layout.
    OracleV1 = 1,
}

impl BlobLayoutV1 {
    fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(BlobLayoutV1::Uninit),
            1 => Some(BlobLayoutV1::OracleV1),
            _ => None,
        }
    }
}

/// Full labelled emulator state + replay history.
///
/// Field order here is the canonical (zelda3-style) schema:
/// `ram, wram, ppu, apu, mapper, input_history, label, source_movie, frame`.
/// v2 appends `oracle_blob` after `frame_logic` (see [`SNAPSHOT_VERSION`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Encoding version (see [`SNAPSHOT_VERSION`]).
    pub version: u16,
    /// Which layout wrote the opaque blobs.
    pub state_source: BlobLayoutV1,
    /// 2 KiB NES RAM.
    pub ram: Vec<u8>,
    /// 8 KiB battery WRAM.
    pub wram: Vec<u8>,
    /// Opaque PPU state blob (oracle-defined).
    pub ppu: Vec<u8>,
    /// Opaque APU state blob (oracle-defined).
    pub apu: Vec<u8>,
    /// Opaque mapper (MMC1) state blob (oracle-defined).
    pub mapper: Vec<u8>,
    /// Full per-frame NES pad bytes from power-on (raw frame track;
    /// pair with the lag map to derive the logic-frame track).
    pub input_history: Vec<u8>,
    /// Label id from [`LABEL_PLAN`] (e.g. `"palace1-enter"`).
    pub label: String,
    /// Source movie file name (e.g. `"zelda2-anypct-4367M.fm2"`).
    pub source_movie: String,
    /// Raw (emulator) frame index at mint time.
    pub frame_raw: u64,
    /// Logic (non-lag) frame index at mint time.
    pub frame_logic: u64,
    /// Full opaque oracle save-state (`save_state` bytes): the restore
    /// source for `load_state`. Empty for v1 snapshots and for producers
    /// without an oracle. See the module-level layout mapping.
    pub oracle_blob: Vec<u8>,
}

/// Snapshot encode/decode failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotError(pub String);

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "snapshot: {}", self.0)
    }
}

impl Error for SnapshotError {}

impl Snapshot {
    /// Build a snapshot, checking fixed sizes and label membership.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ram: Vec<u8>,
        wram: Vec<u8>,
        ppu: Vec<u8>,
        apu: Vec<u8>,
        mapper: Vec<u8>,
        input_history: Vec<u8>,
        label: impl Into<String>,
        source_movie: impl Into<String>,
        frame_raw: u64,
        frame_logic: u64,
    ) -> Result<Self, SnapshotError> {
        let label = label.into();
        if ram.len() != RAM_SIZE {
            return Err(SnapshotError(format!(
                "ram is {} bytes, want {RAM_SIZE}",
                ram.len()
            )));
        }
        if wram.len() != WRAM_SIZE {
            return Err(SnapshotError(format!(
                "wram is {} bytes, want {WRAM_SIZE}",
                wram.len()
            )));
        }
        for (name, b) in [("ppu", &ppu), ("apu", &apu), ("mapper", &mapper)] {
            if b.len() > MAX_BLOB_LEN {
                return Err(SnapshotError(format!("{name} blob too large: {}", b.len())));
            }
        }
        if input_history.len() > MAX_HISTORY_LEN {
            return Err(SnapshotError(format!(
                "input_history too large: {}",
                input_history.len()
            )));
        }
        if !label_is_known(&label) {
            return Err(SnapshotError(format!("unknown label {label:?}")));
        }
        Ok(Snapshot {
            version: SNAPSHOT_VERSION,
            state_source: BlobLayoutV1::Uninit,
            ram,
            wram,
            ppu,
            apu,
            mapper,
            input_history,
            label,
            source_movie: source_movie.into(),
            frame_raw,
            frame_logic,
            oracle_blob: Vec::new(),
        })
    }

    /// Attach the full opaque oracle save-state (`save_state` bytes).
    ///
    /// Upgrading path for v1 snapshots: `decode` a v1 file (blob empty),
    /// set `version = SNAPSHOT_VERSION`, attach the blob here, `encode` —
    /// the result decodes as v2. Callers restoring via `load_state` should
    /// also set `state_source = BlobLayoutV1::OracleV1`.
    pub fn with_oracle_blob(mut self, blob: Vec<u8>) -> Result<Self, SnapshotError> {
        if blob.len() > MAX_ORACLE_BLOB_LEN {
            return Err(SnapshotError(format!(
                "oracle_blob too large: {}",
                blob.len()
            )));
        }
        self.oracle_blob = blob;
        Ok(self)
    }

    /// Frames available for the 600-frame restore+replay check.
    pub fn replay_len(&self) -> usize {
        self.input_history.len()
    }
}

// --- little-endian helpers (no serde dependency) ---

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}
fn put_str(out: &mut Vec<u8>, s: &str) -> Result<(), SnapshotError> {
    if s.len() > 4096 {
        return Err(SnapshotError("string field too long".to_string()));
    }
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

struct Cursor<'a> {
    b: &'a [u8],
    off: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], SnapshotError> {
        let end = self.off + n;
        if end > self.b.len() {
            return Err(SnapshotError(format!("truncated snapshot in {what}")));
        }
        let s = &self.b[self.off..end];
        self.off = end;
        Ok(s)
    }
    fn u16(&mut self, what: &str) -> Result<u16, SnapshotError> {
        let s = self.take(2, what)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self, what: &str) -> Result<u32, SnapshotError> {
        let s = self.take(4, what)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self, what: &str) -> Result<u64, SnapshotError> {
        let s = self.take(8, what)?;
        Ok(u64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }
    fn bytes(&mut self, what: &str, limit: usize) -> Result<Vec<u8>, SnapshotError> {
        let n = self.u32(what)? as usize;
        if n > limit {
            return Err(SnapshotError(format!("{what} too large: {n}")));
        }
        Ok(self.take(n, what)?.to_vec())
    }
    fn string(&mut self, what: &str) -> Result<String, SnapshotError> {
        let b = self.bytes(what, 4096)?;
        String::from_utf8(b).map_err(|_| SnapshotError(format!("{what} not UTF-8")))
    }
}

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

impl Snapshot {
    /// Encode to the canonical binary form (magic + LE fields + CRC32).
    ///
    /// v1 snapshots (`version == 1`, empty blob) encode to the legacy
    /// layout byte-for-byte; v2 appends the blob length + bytes. Encoding
    /// a v1 version with a non-empty blob is an error (bump `version` to
    /// [`SNAPSHOT_VERSION`] first — see [`Snapshot::with_oracle_blob`]).
    pub fn encode(&self) -> Result<Vec<u8>, SnapshotError> {
        if self.version != SNAPSHOT_VERSION_V1 && self.version != SNAPSHOT_VERSION {
            return Err(SnapshotError(format!(
                "unsupported version {}",
                self.version
            )));
        }
        if self.version == SNAPSHOT_VERSION_V1 && !self.oracle_blob.is_empty() {
            return Err(SnapshotError(
                "v1 snapshot with non-empty oracle_blob: set version = SNAPSHOT_VERSION"
                    .to_string(),
            ));
        }
        let mut out = Vec::new();
        out.extend_from_slice(SNAPSHOT_MAGIC);
        put_u16(&mut out, self.version);
        put_u16(
            &mut out,
            match self.state_source {
                BlobLayoutV1::Uninit => 0,
                BlobLayoutV1::OracleV1 => 1,
            },
        );
        put_bytes(&mut out, &self.ram);
        put_bytes(&mut out, &self.wram);
        put_bytes(&mut out, &self.ppu);
        put_bytes(&mut out, &self.apu);
        put_bytes(&mut out, &self.mapper);
        put_bytes(&mut out, &self.input_history);
        put_str(&mut out, &self.label)?;
        put_str(&mut out, &self.source_movie)?;
        put_u64(&mut out, self.frame_raw);
        put_u64(&mut out, self.frame_logic);
        if self.version == SNAPSHOT_VERSION {
            put_bytes(&mut out, &self.oracle_blob);
        }
        let crc = crc32_ieee(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    /// Decode + verify magic, version and CRC32.
    ///
    /// Accepts v1 (no trailing blob; `oracle_blob` reads back empty) and
    /// v2 (trailing blob length + bytes, bounded by
    /// [`MAX_ORACLE_BLOB_LEN`]).
    pub fn decode(b: &[u8]) -> Result<Self, SnapshotError> {
        if b.len() < 8 + 4 {
            return Err(SnapshotError("too short".to_string()));
        }
        let (body, tail) = b.split_at(b.len() - 4);
        let want = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        if crc32_ieee(body) != want {
            return Err(SnapshotError("CRC32 mismatch".to_string()));
        }
        let mut c = Cursor { b: body, off: 0 };
        if c.take(8, "magic")? != SNAPSHOT_MAGIC {
            return Err(SnapshotError("bad magic".to_string()));
        }
        let version = c.u16("version")?;
        if version != SNAPSHOT_VERSION_V1 && version != SNAPSHOT_VERSION {
            return Err(SnapshotError(format!("unsupported version {version}")));
        }
        let layout = c.u16("layout")?;
        let state_source = BlobLayoutV1::from_u16(layout)
            .ok_or_else(|| SnapshotError(format!("bad layout {layout}")))?;
        let ram = c.bytes("ram", MAX_BLOB_LEN)?;
        let wram = c.bytes("wram", MAX_BLOB_LEN)?;
        if ram.len() != RAM_SIZE || wram.len() != WRAM_SIZE {
            return Err(SnapshotError("bad ram/wram size".to_string()));
        }
        let ppu = c.bytes("ppu", MAX_BLOB_LEN)?;
        let apu = c.bytes("apu", MAX_BLOB_LEN)?;
        let mapper = c.bytes("mapper", MAX_BLOB_LEN)?;
        let input_history = c.bytes("input_history", MAX_HISTORY_LEN)?;
        let label = c.string("label")?;
        let source_movie = c.string("source_movie")?;
        let frame_raw = c.u64("frame_raw")?;
        let frame_logic = c.u64("frame_logic")?;
        let oracle_blob = if version == SNAPSHOT_VERSION {
            c.bytes("oracle_blob", MAX_ORACLE_BLOB_LEN)?
        } else {
            Vec::new()
        };
        if c.off != body.len() {
            return Err(SnapshotError("trailing bytes".to_string()));
        }
        Ok(Snapshot {
            version,
            state_source,
            ram,
            wram,
            ppu,
            apu,
            mapper,
            input_history,
            label,
            source_movie,
            frame_raw,
            frame_logic,
            oracle_blob,
        })
    }
}

// ---------------------------------------------------------------------------
// Label plan (≥30). Every id is mintable once movies are present; the plan
// itself is the acceptance artefact when movies are absent.
// ---------------------------------------------------------------------------

/// One mintable snapshot label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelDef {
    /// Stable id used as [`Snapshot::label`] and file stem.
    pub id: &'static str,
    /// What the oracle must observe at the mint frame.
    pub description: &'static str,
    /// Preferred source: `any` = first movie that reaches it (any% 4367M
    /// preferred for speed), or a specific movie tag.
    pub movie: &'static str,
}

/// The full labelled-snapshot plan: 42 labels.
pub const LABEL_PLAN: &[LabelDef] = &[
    // Boot / early game (4).
    LabelDef {
        id: "boot-title",
        description: "Title screen up, press Start pending",
        movie: "any",
    },
    LabelDef {
        id: "file-select",
        description: "File-select cursor on empty slot",
        movie: "any",
    },
    LabelDef {
        id: "game-start",
        description: "Game started, North Palace area loaded",
        movie: "any",
    },
    LabelDef {
        id: "first-overworld-step",
        description: "First overworld step taken from start",
        movie: "any",
    },
    // Towns: entrances (8).
    LabelDef {
        id: "town-rauru-enter",
        description: "Enter Rauru (Shield spell town)",
        movie: "any",
    },
    LabelDef {
        id: "town-ruto-enter",
        description: "Enter Ruto (Jump spell town)",
        movie: "any",
    },
    LabelDef {
        id: "town-saria-enter",
        description: "Enter Saria (Life spell town)",
        movie: "any",
    },
    LabelDef {
        id: "town-mido-enter",
        description: "Enter Mido (Fairy spell town)",
        movie: "any",
    },
    LabelDef {
        id: "town-nabooru-enter",
        description: "Enter Nabooru (Fire spell town)",
        movie: "any",
    },
    LabelDef {
        id: "town-darunia-enter",
        description: "Enter Darunia (Reflect spell town)",
        movie: "any",
    },
    LabelDef {
        id: "town-new-kasuto-enter",
        description: "Enter New Kasuto (Spell magic town)",
        movie: "any",
    },
    LabelDef {
        id: "town-old-kasuto-enter",
        description: "Enter Old Kasuto (Thunder spell town)",
        movie: "any",
    },
    // Spell pickups (8).
    LabelDef {
        id: "spell-shield",
        description: "Shield spell learned in Rauru",
        movie: "any",
    },
    LabelDef {
        id: "spell-jump",
        description: "Jump spell learned in Ruto",
        movie: "any",
    },
    LabelDef {
        id: "spell-life",
        description: "Life spell learned in Saria",
        movie: "any",
    },
    LabelDef {
        id: "spell-fairy",
        description: "Fairy spell learned in Mido",
        movie: "any",
    },
    LabelDef {
        id: "spell-fire",
        description: "Fire spell learned in Nabooru",
        movie: "any",
    },
    LabelDef {
        id: "spell-reflect",
        description: "Reflect spell learned in Darunia",
        movie: "any",
    },
    LabelDef {
        id: "spell-spell",
        description: "Spell magic learned in New Kasuto",
        movie: "any",
    },
    LabelDef {
        id: "spell-thunder",
        description: "Thunder spell learned in Old Kasuto",
        movie: "any",
    },
    // Key items / skills (6).
    LabelDef {
        id: "skill-downstab",
        description: "Downward thrust learned in Mido basement",
        movie: "any",
    },
    LabelDef {
        id: "skill-upstab",
        description: "Upward thrust learned in Nabooru basement",
        movie: "any",
    },
    LabelDef {
        id: "item-candle",
        description: "Candle picked up (Parapa Palace)",
        movie: "any",
    },
    LabelDef {
        id: "item-glove",
        description: "Handy Glove picked up (Midoro Palace)",
        movie: "any",
    },
    LabelDef {
        id: "item-raft",
        description: "Raft picked up (Island Palace)",
        movie: "any",
    },
    LabelDef {
        id: "item-magic-key",
        description: "Magical Key picked up (Maze Palace)",
        movie: "any",
    },
    // Palace entrances + bosses (12).
    LabelDef {
        id: "palace1-enter",
        description: "Enter Parapa Palace (palace 1)",
        movie: "any",
    },
    LabelDef {
        id: "boss-horsehead",
        description: "Horsehead boss room (palace 1)",
        movie: "any",
    },
    LabelDef {
        id: "palace2-enter",
        description: "Enter Midoro Palace (palace 2)",
        movie: "any",
    },
    LabelDef {
        id: "boss-helmethead",
        description: "Helmethead boss room (palace 2)",
        movie: "any",
    },
    LabelDef {
        id: "palace3-enter",
        description: "Enter Island Palace (palace 3)",
        movie: "any",
    },
    LabelDef {
        id: "boss-rebonack",
        description: "Rebonack boss room (palace 3)",
        movie: "any",
    },
    LabelDef {
        id: "palace4-enter",
        description: "Enter Maze Palace (palace 4)",
        movie: "any",
    },
    LabelDef {
        id: "boss-carock",
        description: "Carock boss room (palace 4)",
        movie: "any",
    },
    LabelDef {
        id: "palace5-enter",
        description: "Enter Ocean Palace (palace 5)",
        movie: "any",
    },
    LabelDef {
        id: "boss-gooma",
        description: "Gooma boss room (palace 5)",
        movie: "any",
    },
    LabelDef {
        id: "palace6-enter",
        description: "Enter Three-Eye-Rock Palace (palace 6)",
        movie: "any",
    },
    LabelDef {
        id: "boss-barba",
        description: "Barba boss room (palace 6)",
        movie: "any",
    },
    // Endgame (4).
    LabelDef {
        id: "great-palace-enter",
        description: "Enter the Great Palace",
        movie: "any",
    },
    LabelDef {
        id: "boss-thunderbird",
        description: "Thunderbird boss room (Great Palace)",
        movie: "any",
    },
    LabelDef {
        id: "boss-dark-link",
        description: "Dark Link final fight",
        movie: "any",
    },
    LabelDef {
        id: "ending",
        description: "Ending sequence after Dark Link",
        movie: "any",
    },
];

/// True when `id` is in [`LABEL_PLAN`].
pub fn label_is_known(id: &str) -> bool {
    LABEL_PLAN.iter().any(|l| l.id == id)
}

// ---------------------------------------------------------------------------
// Mint plan + RAM-edit variants.
// ---------------------------------------------------------------------------

/// Mint-time state variant (RAM edit at mint time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotVariant {
    /// As-played state, full HP/magic.
    Full,
    /// Same frame, HP edited down to near-death (tests damage paths).
    LowHp,
    /// Same frame, magic container emptied (tests no-magic routing).
    NoMagic,
}

impl SnapshotVariant {
    /// File-stem suffix (`""`, `"-lowhp"`, `"-nomagic"`).
    pub fn suffix(self) -> &'static str {
        match self {
            SnapshotVariant::Full => "",
            SnapshotVariant::LowHp => "-lowhp",
            SnapshotVariant::NoMagic => "-nomagic",
        }
    }
}

/// One absolute-CPU-address byte patch for mint-time RAM edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RamPatch {
    /// CPU address (`$0000-$07FF` → ram, `$6000-$7FFF` → wram mirrors incl.).
    pub addr: u16,
    /// Value to write.
    pub value: u8,
    /// Why (recorded in the mint log).
    pub why: &'static str,
}

/// Mint-time patch set for a variant.
///
/// Byte addresses are resolved against the ram-map at mint time:
/// until `ram-map.toml` lands, `LowHp`/`NoMagic` return `Err` naming the
/// blocker instead of guessing addresses. The *mechanism*
/// ([`apply_ram_patches`]) is final and unit-tested below.
pub fn variant_patches(variant: SnapshotVariant) -> Result<Vec<RamPatch>, &'static str> {
    match variant {
        SnapshotVariant::Full => Ok(Vec::new()),
        SnapshotVariant::LowHp => {
            Err("ram-map lookup NYI: HP container/current-HP addresses unknown")
        }
        SnapshotVariant::NoMagic => {
            Err("ram-map lookup NYI: magic container/current-magic addresses unknown")
        }
    }
}

/// Map an absolute CPU address into `(ram|wram, offset)`.
///
/// Mirrors folded: `$0800-$1FFF` → `$0000-$07FF`.
pub fn cpu_addr_offset(addr: u16) -> Option<(bool, usize)> {
    match addr {
        0x0000..=0x07FF => Some((false, addr as usize)),
        0x0800..=0x1FFF => Some((false, (addr & 0x07FF) as usize)),
        0x6000..=0x7FFF => Some((true, (addr - 0x6000) as usize)),
        _ => None,
    }
}

/// Apply mint-time RAM patches to a snapshot (bounds-checked).
pub fn apply_ram_patches(snap: &mut Snapshot, patches: &[RamPatch]) -> Result<(), SnapshotError> {
    for p in patches {
        match cpu_addr_offset(p.addr) {
            Some((false, off)) => snap.ram[off] = p.value,
            Some((true, off)) => snap.wram[off] = p.value,
            None => {
                return Err(SnapshotError(format!(
                    "patch ${:04X} outside ram/wram",
                    p.addr
                )));
            }
        }
    }
    Ok(())
}

/// One row of the mint plan: pause the oracle at `frame_raw` of
/// `source_movie`, label it, and save one snapshot per `variants`.
#[derive(Debug, Clone)]
pub struct MintRequest {
    /// Label id from [`LABEL_PLAN`].
    pub label: &'static str,
    /// Source movie file name (out-of-tree `corpus/movies/…`).
    pub source_movie: &'static str,
    /// Raw frame to pause at (filled by the labeller; `u64::MAX` = TBD).
    pub frame_raw: u64,
    /// Variants to emit at that frame.
    pub variants: Vec<SnapshotVariant>,
}

/// Ordered mint requests. Frame numbers are TBD until movies replay in the
/// oracle; the label list itself is the acceptance artefact meanwhile.
#[derive(Debug, Clone, Default)]
pub struct MintPlan {
    /// Requests in mint order.
    pub requests: Vec<MintRequest>,
}

impl MintPlan {
    /// Plan covering every [`LABEL_PLAN`] entry from any movie, Full variant.
    /// Frame numbers stay TBD (`u64::MAX`) until the oracle labeller runs.
    pub fn all_labels_any_movie() -> Self {
        MintPlan {
            requests: LABEL_PLAN
                .iter()
                .map(|l| MintRequest {
                    label: l.id,
                    source_movie: "any",
                    frame_raw: u64::MAX,
                    variants: vec![SnapshotVariant::Full],
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_snapshot(label: &str) -> Snapshot {
        Snapshot::new(
            vec![0xAA; RAM_SIZE],
            vec![0xBB; WRAM_SIZE],
            vec![1, 2, 3],
            vec![4, 5],
            vec![6],
            vec![0x01, 0x00, 0x80],
            label,
            "test.fm2",
            1234,
            1200,
        )
        .unwrap()
    }

    #[test]
    fn label_plan_has_at_least_30_entries() {
        assert!(LABEL_PLAN.len() >= 30, "got {}", LABEL_PLAN.len());
        // Ids unique, kebab-case, non-empty descriptions.
        let mut ids: Vec<&str> = LABEL_PLAN.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), LABEL_PLAN.len(), "duplicate label ids");
        for l in LABEL_PLAN {
            assert!(!l.id.is_empty() && !l.description.is_empty());
            assert!(l
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        }
        // Required beats are present.
        for need in [
            "boot-title",
            "file-select",
            "first-overworld-step",
            "great-palace-enter",
            "boss-thunderbird",
            "boss-dark-link",
            "ending",
        ] {
            assert!(label_is_known(need), "missing {need}");
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let s = tiny_snapshot("boot-title");
        let bytes = s.encode().unwrap();
        assert_eq!(&bytes[..8], SNAPSHOT_MAGIC);
        let back = Snapshot::decode(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn decode_rejects_corruption() {
        let mut bytes = tiny_snapshot("ending").encode().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(Snapshot::decode(&bytes).is_err());
        assert!(Snapshot::decode(&bytes[..10]).is_err());
    }

    #[test]
    fn unknown_label_rejected() {
        assert!(!label_is_known("nope"));
        let r = Snapshot::new(
            vec![0; RAM_SIZE],
            vec![0; WRAM_SIZE],
            vec![],
            vec![],
            vec![],
            vec![],
            "nope",
            "m.fm2",
            0,
            0,
        );
        assert!(r.is_err());
    }

    #[test]
    fn ram_patch_mechanism_with_mirrors() {
        let mut s = tiny_snapshot("game-start");
        apply_ram_patches(
            &mut s,
            &[
                RamPatch {
                    addr: 0x0010,
                    value: 0x01,
                    why: "test ram",
                },
                RamPatch {
                    addr: 0x0900,
                    value: 0x02,
                    why: "mirror of $0100",
                },
                RamPatch {
                    addr: 0x6005,
                    value: 0x03,
                    why: "test wram",
                },
            ],
        )
        .unwrap();
        assert_eq!(s.ram[0x10], 0x01);
        assert_eq!(s.ram[0x100], 0x02);
        assert_eq!(s.wram[5], 0x03);
        assert!(apply_ram_patches(
            &mut s,
            &[RamPatch {
                addr: 0x8000,
                value: 0,
                why: "rom"
            }]
        )
        .is_err());
    }

    #[test]
    fn variants_pending_ram_map_fail_loudly() {
        assert!(variant_patches(SnapshotVariant::Full).unwrap().is_empty());
        assert!(variant_patches(SnapshotVariant::LowHp).is_err());
        assert!(variant_patches(SnapshotVariant::NoMagic).is_err());
    }
}
