//! HD pack format (`pack.json` v1 + PNG sheets), validation and lookup.
//!
//! A pack is a directory holding `pack.json` and the PNG sheets it lists.
//! Replacement art is keyed by `(CHR page 0-31, tile 0-255)` plus an optional
//! palette triple: the three NES colour indices the tile's sub-palette
//! entries 1..=3 held when it was drawn. Lookup tries the exact triple first,
//! then the palette-independent default, else the caller falls back to the
//! original CHR art.
//!
//! Two ways to place cells on a sheet (both may be mixed in one pack):
//! - **page sheets** (`"page": N` on the sheet entry): the PNG is exactly
//!   `128 * scale` square and tile `t` is the cell at column `t % 16`, row
//!   `t / 16`. Cells whose pixels are all transparent are *not* registered,
//!   so an artist leaves a cell empty to keep the original art.
//! - **free placement** (`tiles[]` entries): each entry names a sheet (index
//!   into `sheets[]` or its `file`) and the top-left corner `x`, `y` of the
//!   `8 * scale` square cell in **sheet pixel coordinates**. Explicit entries
//!   are always registered, even when fully transparent (hides the tile).
//!
//! Three optional extensions ride on the same version 1 manifest (older
//! builds ignore the keys and still show the pack, without them):
//! - `"sprite_alpha": "art"`: a replaced sprite's silhouette is the cell's own
//!   alpha instead of the NES sprite's opaque pixels;
//! - `bleed` on a `tiles[]` entry: art around the cell, for a sprite drawn
//!   larger than its 8x8 box (needs `"sprite_alpha": "art"`);
//! - `layers[]`: whole images behind or over the background tiles of one
//!   scene, anchored to the level or to the screen ([`Layer`]).
//!
//! The loader never touches the filesystem: [`HdPack::load`] takes a reader
//! closure and [`HdPack::from_files`] an in-memory file list (web directory
//! picker); `crate::fs::load_pack_dir` wraps a native directory.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::chr::{CHR_PAGES, TILES_PER_PAGE};
use crate::palette::MasterPalette;
use crate::png_io::{decode_png_rgba, PngError, RgbaImage};
use crate::MAX_SCALE;

/// Value of the optional `format` key.
pub const PACK_FORMAT: &str = "z2rs-hdpack";
/// The only supported `version`.
pub const PACK_VERSION: u64 = 1;
/// Manifest file name at the pack root.
pub const PACK_MANIFEST: &str = "pack.json";
/// Default `alpha_threshold`.
pub const DEFAULT_ALPHA_THRESHOLD: u8 = 128;

// ---------------------------------------------------------------------------
// Manifest schema (serde). Integers are u64 so out-of-range values produce a
// validation error naming the entry instead of a bare serde message.
// ---------------------------------------------------------------------------

/// `pack.json` as written on disk. Unknown keys are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    /// Optional; must be `"z2rs-hdpack"` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Must be `1`.
    pub version: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// HD pixels per NES pixel, 1..=8. Every cell is `8 * scale` px square.
    pub scale: u64,
    /// HD alpha `>= threshold` is opaque, else transparent (default 128).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_threshold: Option<u64>,
    /// Optional master palette override for NES-coloured (fallback) pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palette: Option<PaletteSpec>,
    #[serde(default)]
    pub sheets: Vec<SheetEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiles: Vec<TileEntry>,
    /// `"nes"` (default): replaced sprites keep the NES silhouette. `"art"`:
    /// the cell's own alpha is the silhouette.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite_alpha: Option<String>,
    /// Scene layers, drawn in order (later entries over earlier ones).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerEntry>,
    /// Free-form artist notes (any JSON object); not interpreted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<serde_json::Value>,
}

/// One PNG sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetEntry {
    /// Path relative to `pack.json` (`/` separators, no `..`).
    pub file: String,
    /// `"page"` (requires `page`) or `"free"`; inferred from `page` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// CHR page this page sheet covers (0-31).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u64>,
    /// Palette variant: cells apply only when the tile's colours equal these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<u64>>,
    /// Optional self-check: must equal the pack `scale` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u64>,
}

/// One explicitly placed tile cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileEntry {
    pub page: u64,
    pub tile: u64,
    /// Index into `sheets[]` or the sheet's `file` string.
    pub sheet: SheetRef,
    /// Top-left corner of the cell in sheet pixels.
    pub x: u64,
    pub y: u64,
    /// Palette variant key; defaults to the referenced sheet's `colors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<u64>>,
    /// Reserved for a future brightness hint; accepted and ignored in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<serde_json::Value>,
    /// `[left, top, right, bottom]` NES pixels (0..=8 each) of art around the
    /// cell on the sheet, drawn when the tile is a sprite and the pack sets
    /// `"sprite_alpha": "art"`. `x`, `y` stay the corner of the 8x8 core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bleed: Option<Vec<u64>>,
}

/// One scene layer: a PNG at the pack scale behind or over the background.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerEntry {
    /// Path relative to `pack.json` (`/` separators, no `..`).
    pub file: String,
    /// Scene filter; an absent key matches any value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<LayerWhen>,
    /// `"back"` (default): shows where the background is transparent.
    /// `"front"`: covers the background tiles. Sprites are over both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
    /// Left edge in NES pixels: a level position at `scroll` 100, a position
    /// relative to the NES window's left edge at `scroll` 0.
    #[serde(default)]
    pub x: i64,
    /// Top edge in NES pixels from the top of the screen.
    #[serde(default)]
    pub y: i64,
    /// Percent of the camera's horizontal movement the layer follows,
    /// 0..=100 (default 100 = fixed to the level).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll: Option<u64>,
    /// Tile the image horizontally.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat_x: bool,
    /// Front layers only: cover these background tiles and nothing else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub over_tiles: Vec<OverTile>,
}

/// `layers[].when`: the scene a layer belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerWhen {
    /// `$0707`: 0 caves and fields, 1 West towns, 2 East towns, 3-5 palaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<u64>,
    /// `$0706`: 0 West, 1 Death Mountain and Maze Island, 2 East.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<u64>,
    /// `$0561`: scene layout index inside the world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<u64>,
}

/// `layers[].over_tiles[]` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverTile {
    pub page: u64,
    pub tile: u64,
}

/// Reference from a tile entry to a sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SheetRef {
    Index(u64),
    File(String),
}

/// `palette`: a `.pal` file path, or 64 inline entries (`"#RRGGBB"` or `[r, g, b]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaletteSpec {
    File(String),
    Entries(Vec<serde_json::Value>),
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Pack load / validation failure. `Display` names the file and entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    MissingFile {
        file: String,
        reason: String,
    },
    Json {
        file: String,
        msg: String,
    },
    Format {
        got: String,
    },
    Version {
        got: u64,
    },
    Scale {
        got: u64,
    },
    AlphaThreshold {
        got: u64,
    },
    Path {
        entry: String,
        path: String,
    },
    Layout {
        entry: String,
        msg: String,
    },
    Page {
        entry: String,
        got: u64,
    },
    Tile {
        entry: String,
        got: u64,
    },
    Color {
        entry: String,
        msg: String,
    },
    ScaleMismatch {
        entry: String,
        pack: u64,
        sheet: u64,
    },
    Png {
        file: String,
        err: PngError,
    },
    SheetDims {
        file: String,
        want: (u32, u32),
        got: (u32, u32),
        scale: u32,
    },
    SheetRef {
        entry: String,
        msg: String,
    },
    CellOutOfRange {
        entry: String,
        file: String,
        x: u64,
        y: u64,
        cell: u32,
        sheet: (u32, u32),
    },
    DuplicateTile {
        page: u8,
        tile: u8,
        colors: Option<[u8; 3]>,
        first: String,
        second: String,
    },
    Palette {
        msg: String,
    },
    SpriteAlpha {
        got: String,
    },
    Bleed {
        entry: String,
        msg: String,
    },
    Layer {
        entry: String,
        msg: String,
    },
}

fn fmt_colors(c: Option<[u8; 3]>) -> String {
    match c {
        None => "any palette".to_string(),
        Some([a, b, d]) => format!("colors [{a}, {b}, {d}]"),
    }
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use PackError as E;
        match self {
            E::MissingFile { file, reason } => write!(f, "cannot read \"{file}\": {reason}"),
            E::Json { file, msg } => write!(f, "{file}: invalid JSON: {msg}"),
            E::Format { got } => write!(
                f,
                "pack.json: \"format\" is \"{got}\", expected \"{PACK_FORMAT}\""
            ),
            E::Version { got } => write!(
                f,
                "pack.json: \"version\" is {got}; this build reads version {PACK_VERSION}"
            ),
            E::Scale { got } => write!(
                f,
                "pack.json: \"scale\" is {got}; must be an integer 1..={MAX_SCALE}"
            ),
            E::AlphaThreshold { got } => {
                write!(
                    f,
                    "pack.json: \"alpha_threshold\" is {got}; must be 0..=255"
                )
            }
            E::Path { entry, path } => write!(
                f,
                "pack.json {entry}: path \"{path}\" must be relative to pack.json without \"..\""
            ),
            E::Layout { entry, msg } => write!(f, "pack.json {entry}: {msg}"),
            E::Page { entry, got } => write!(
                f,
                "pack.json {entry}: page {got} out of range (0..={})",
                CHR_PAGES - 1
            ),
            E::Tile { entry, got } => {
                write!(f, "pack.json {entry}: tile {got} out of range (0..=255)")
            }
            E::Color { entry, msg } => write!(f, "pack.json {entry}: {msg}"),
            E::ScaleMismatch { entry, pack, sheet } => write!(
                f,
                "pack.json {entry}: sheet scale {sheet} does not match pack scale {pack}"
            ),
            E::Png { file, err } => write!(f, "{file}: {err}"),
            E::SheetDims {
                file,
                want,
                got,
                scale,
            } => write!(
                f,
                "{file}: page sheet is {}x{} px; at scale {scale} it must be {}x{} \
                 (16x16 cells of {} px)",
                got.0,
                got.1,
                want.0,
                want.1,
                8 * scale
            ),
            E::SheetRef { entry, msg } => write!(f, "pack.json {entry}: {msg}"),
            E::CellOutOfRange {
                entry,
                file,
                x,
                y,
                cell,
                sheet,
            } => write!(
                f,
                "pack.json {entry}: cell at x={x}, y={y} ({cell}x{cell} px) does not fit \
                 inside \"{file}\" ({}x{} px)",
                sheet.0, sheet.1
            ),
            E::DuplicateTile {
                page,
                tile,
                colors,
                first,
                second,
            } => write!(
                f,
                "page {page} tile {tile} ({}) is defined twice: by {first} and by {second}",
                fmt_colors(*colors)
            ),
            E::Palette { msg } => write!(f, "pack.json palette: {msg}"),
            E::SpriteAlpha { got } => write!(
                f,
                "pack.json: \"sprite_alpha\" is \"{got}\", expected \"nes\" or \"art\""
            ),
            E::Bleed { entry, msg } => write!(f, "pack.json {entry}: bleed {msg}"),
            E::Layer { entry, msg } => write!(f, "pack.json {entry}: {msg}"),
        }
    }
}

impl std::error::Error for PackError {}

// ---------------------------------------------------------------------------
// Resolved pack
// ---------------------------------------------------------------------------

/// A registered replacement cell: sheet index and top-left pixel corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellRef {
    pub sheet: u16,
    pub x0: u32,
    pub y0: u32,
    /// Every pixel is opaque (compositor may copy whole rows).
    pub solid: bool,
    /// Every pixel is transparent (only possible for explicit `tiles[]` entries).
    pub blank: bool,
    /// Art around the core, `[left, top, right, bottom]` in NES pixels.
    pub bleed: [u8; 4],
}

/// Largest `bleed` value, in NES pixels.
pub const MAX_BLEED: u8 = 8;

/// Where a [`Layer`] sits relative to the background tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerDepth {
    /// Shows where the background is transparent.
    Back,
    /// Covers the background tiles; sprites stay on top.
    Front,
}

/// The scene the frontend says is on screen (see README.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneView {
    /// `$0707`.
    pub world: u8,
    /// `$0706`.
    pub region: u8,
    /// `$0561`.
    pub scene: u8,
    /// Level pixel at the NES window's left edge.
    pub camera_x: i32,
}

/// A validated `layers[]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// Index into [`HdPack::sheets`].
    pub sheet: u16,
    pub depth: LayerDepth,
    /// Left / top edge in NES pixels.
    pub x: i32,
    pub y: i32,
    /// Percent of the camera movement followed, 0..=100.
    pub scroll: u8,
    pub repeat_x: bool,
    pub world: Option<u8>,
    pub region: Option<u8>,
    pub scene: Option<u8>,
    /// Sorted `(page, tile)` list; empty = no tile restriction.
    pub over_tiles: Vec<(u8, u8)>,
}

impl Layer {
    /// Whether the layer belongs to `scene`. With no scene known only a
    /// layer without any `when` key matches.
    #[must_use]
    pub fn matches(&self, scene: Option<&SceneView>) -> bool {
        match scene {
            Some(s) => {
                self.world.is_none_or(|w| w == s.world)
                    && self.region.is_none_or(|r| r == s.region)
                    && self.scene.is_none_or(|c| c == s.scene)
            }
            None => self.world.is_none() && self.region.is_none() && self.scene.is_none(),
        }
    }

    /// Whether a front layer may cover background tile `(page, tile)`.
    #[must_use]
    pub fn covers_tile(&self, page: u8, tile: u8) -> bool {
        self.over_tiles.is_empty() || self.over_tiles.binary_search(&(page, tile)).is_ok()
    }
}

#[derive(Debug, Clone, Default)]
struct TileSlot {
    default: Option<CellRef>,
    /// Sorted by colour triple (binary search).
    variants: Vec<([u8; 3], CellRef)>,
}

/// Borrowed view of one cell's pixels (`size` x `size` RGBA, alpha 0 or 255).
#[derive(Debug, Clone, Copy)]
pub struct CellPixels<'a> {
    sheet: &'a RgbaImage,
    x0: u32,
    y0: u32,
    size: u32,
    /// `[left, top, right, bottom]` bleed in sheet pixels.
    bleed_px: [u32; 4],
}

impl<'a> CellPixels<'a> {
    /// Edge length in pixels (`8 * scale`).
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Row `y` of the cell: `size * 4` RGBA bytes, a direct slice of the sheet.
    #[inline]
    #[must_use]
    pub fn row(&self, y: u32) -> &'a [u8] {
        let w = self.sheet.width as usize;
        let start = ((self.y0 + y) as usize * w + self.x0 as usize) * 4;
        &self.sheet.rgba[start..start + self.size as usize * 4]
    }

    /// Pixel `(x, y)` inside the cell.
    #[inline]
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.sheet.pixel(self.x0 + x, self.y0 + y)
    }

    /// Pixel at `(x, y)` relative to the core's corner, reaching into the
    /// bleed; `None` outside the cell's art.
    #[inline]
    #[must_use]
    pub fn pixel_rel(&self, x: i32, y: i32) -> Option<[u8; 4]> {
        let [l, t, r, b] = self.bleed_px;
        let (w, h) = ((self.size + r) as i32, (self.size + b) as i32);
        if x < -(l as i32) || y < -(t as i32) || x >= w || y >= h {
            return None;
        }
        Some(
            self.sheet
                .pixel((self.x0 as i32 + x) as u32, (self.y0 as i32 + y) as u32),
        )
    }

    /// Copy the cell out as `size * size * 4` row-major RGBA bytes.
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        (0..self.size).flat_map(|y| self.row(y)).copied().collect()
    }
}

/// A validated, decoded HD pack with dense lookup tables.
#[derive(Debug, Clone)]
pub struct HdPack {
    name: String,
    author: Option<String>,
    scale: u32,
    alpha_threshold: u8,
    sheets: Vec<RgbaImage>,
    sheet_files: Vec<String>,
    /// `page * 256 + tile`, `CHR_PAGES * 256` entries.
    index: Vec<TileSlot>,
    palette: Option<MasterPalette>,
    groups: Option<serde_json::Value>,
    tile_count: usize,
    variant_count: usize,
    sprite_alpha_art: bool,
    layers: Vec<Layer>,
    /// Largest vertical bleed of any cell, in NES pixels.
    max_bleed_v: u8,
}

impl HdPack {
    /// Load a pack through `read(relative_path)`, starting with `pack.json`.
    ///
    /// Only files referenced by the manifest are read.
    ///
    /// # Errors
    /// Any [`PackError`]; the message names the file and manifest entry.
    pub fn load(read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Self, PackError> {
        let bytes = read(PACK_MANIFEST).map_err(|reason| PackError::MissingFile {
            file: PACK_MANIFEST.to_string(),
            reason,
        })?;
        let manifest: PackManifest =
            serde_json::from_slice(&bytes).map_err(|e| PackError::Json {
                file: PACK_MANIFEST.to_string(),
                msg: e.to_string(),
            })?;
        Self::from_manifest(&manifest, read)
    }

    /// Load from an in-memory list of `(path, bytes)` (e.g. a web directory
    /// upload). Paths use `/` or `\`; the shallowest `pack.json` defines the
    /// pack root and its directory prefix is stripped from every path.
    ///
    /// # Errors
    /// As [`HdPack::load`].
    pub fn from_files(files: &[(String, Vec<u8>)]) -> Result<Self, PackError> {
        let norm: Vec<(String, &[u8])> = files
            .iter()
            .map(|(p, b)| {
                let mut s = p.replace('\\', "/");
                while let Some(rest) = s.strip_prefix("./") {
                    s = rest.to_string();
                }
                (s, b.as_slice())
            })
            .collect();
        let manifest_path = norm
            .iter()
            .map(|(p, _)| p)
            .filter(|p| *p == PACK_MANIFEST || p.ends_with("/pack.json"))
            .min_by_key(|p| (p.matches('/').count(), (*p).clone()))
            .ok_or_else(|| PackError::MissingFile {
                file: PACK_MANIFEST.to_string(),
                reason: format!("no pack.json among the {} provided files", files.len()),
            })?;
        let prefix = manifest_path[..manifest_path.len() - PACK_MANIFEST.len()].to_string();
        let map: BTreeMap<&str, &[u8]> = norm
            .iter()
            .filter_map(|(p, b)| p.strip_prefix(prefix.as_str()).map(|r| (r, *b)))
            .collect();
        Self::load(&|name: &str| {
            map.get(name).map(|b| b.to_vec()).ok_or_else(|| {
                format!("not among the provided files (looked for \"{prefix}{name}\")")
            })
        })
    }

    /// Validate an already-parsed manifest and read its sheets through `read`.
    ///
    /// # Errors
    /// As [`HdPack::load`].
    pub fn from_manifest(
        m: &PackManifest,
        read: &dyn Fn(&str) -> Result<Vec<u8>, String>,
    ) -> Result<Self, PackError> {
        if let Some(fmt) = &m.format {
            if fmt != PACK_FORMAT {
                return Err(PackError::Format { got: fmt.clone() });
            }
        }
        if m.version != PACK_VERSION {
            return Err(PackError::Version { got: m.version });
        }
        if m.scale == 0 || m.scale > u64::from(MAX_SCALE) {
            return Err(PackError::Scale { got: m.scale });
        }
        let scale = m.scale as u32;
        let cell = 8 * scale;
        let alpha_threshold = match m.alpha_threshold {
            None => DEFAULT_ALPHA_THRESHOLD,
            Some(v) => u8::try_from(v).map_err(|_| PackError::AlphaThreshold { got: v })?,
        };
        let palette = m
            .palette
            .as_ref()
            .map(|spec| parse_palette(spec, read))
            .transpose()?;
        if let Some(g) = &m.groups {
            if !g.is_object() {
                return Err(PackError::Layout {
                    entry: "groups".to_string(),
                    msg: "\"groups\" must be a JSON object".to_string(),
                });
            }
        }

        let sprite_alpha_art = match m.sprite_alpha.as_deref() {
            None | Some("nes") => false,
            Some("art") => true,
            Some(other) => {
                return Err(PackError::SpriteAlpha {
                    got: other.to_string(),
                })
            }
        };

        let mut images: Vec<RgbaImage> = Vec::new();
        let mut image_files: Vec<String> = Vec::new();
        let mut file_to_image: BTreeMap<String, usize> = BTreeMap::new();
        // Per sheet entry: (image index, normalised path, colours).
        let mut entries: Vec<(usize, String, Option<[u8; 3]>)> = Vec::new();
        let mut reg: Registry = BTreeMap::new();

        for (i, s) in m.sheets.iter().enumerate() {
            let entry = format!("sheets[{i}] (\"{}\")", s.file);
            let path = normalize_rel_path(&s.file).ok_or_else(|| PackError::Path {
                entry: entry.clone(),
                path: s.file.clone(),
            })?;
            if let Some(sc) = s.scale {
                if sc != m.scale {
                    return Err(PackError::ScaleMismatch {
                        entry,
                        pack: m.scale,
                        sheet: sc,
                    });
                }
            }
            let colors = parse_colors(s.colors.as_deref(), &entry)?;
            let page = match (s.layout.as_deref(), s.page) {
                (None | Some("page"), Some(p)) => Some(check_page(p, &entry)?),
                (Some("page"), None) => {
                    return Err(PackError::Layout {
                        entry,
                        msg: "layout \"page\" needs a \"page\" number".to_string(),
                    })
                }
                (None | Some("free"), None) => None,
                (Some("free"), Some(_)) => {
                    return Err(PackError::Layout {
                        entry,
                        msg: "layout \"free\" must not set \"page\"; place cells with tiles[]"
                            .to_string(),
                    })
                }
                (Some(other), _) => {
                    return Err(PackError::Layout {
                        entry,
                        msg: format!("unknown layout \"{other}\" (expected \"page\" or \"free\")"),
                    })
                }
            };
            let img_idx = if let Some(&k) = file_to_image.get(&path) {
                k
            } else {
                if images.len() >= usize::from(u16::MAX) {
                    return Err(PackError::Layout {
                        entry,
                        msg: "too many distinct sheet files".to_string(),
                    });
                }
                let bytes = read(&path).map_err(|reason| PackError::MissingFile {
                    file: path.clone(),
                    reason,
                })?;
                let mut img = decode_png_rgba(&bytes).map_err(|err| PackError::Png {
                    file: path.clone(),
                    err,
                })?;
                threshold_alpha(&mut img, alpha_threshold);
                images.push(img);
                image_files.push(path.clone());
                file_to_image.insert(path.clone(), images.len() - 1);
                images.len() - 1
            };
            entries.push((img_idx, path.clone(), colors));

            if let Some(page) = page {
                let img = &images[img_idx];
                let want = 16 * cell;
                if (img.width, img.height) != (want, want) {
                    return Err(PackError::SheetDims {
                        file: path,
                        want: (want, want),
                        got: (img.width, img.height),
                        scale,
                    });
                }
                for t in 0..=255u8 {
                    let x0 = u32::from(t % 16) * cell;
                    let y0 = u32::from(t / 16) * cell;
                    let (solid, blank) = scan_cell(img, x0, y0, cell);
                    if blank {
                        continue;
                    }
                    let cref = CellRef {
                        sheet: img_idx as u16,
                        x0,
                        y0,
                        solid,
                        blank,
                        bleed: [0; 4],
                    };
                    let src = format!("{entry} cell {t} (col {}, row {})", t % 16, t / 16);
                    register(&mut reg, (page, t, colors), cref, src)?;
                }
            }
        }

        for (j, t) in m.tiles.iter().enumerate() {
            let entry = format!("tiles[{j}]");
            let page = check_page(t.page, &entry)?;
            let tile = u8::try_from(t.tile).map_err(|_| PackError::Tile {
                entry: entry.clone(),
                got: t.tile,
            })?;
            let sheet_idx = match &t.sheet {
                SheetRef::Index(k) => usize::try_from(*k)
                    .ok()
                    .filter(|k| *k < entries.len())
                    .ok_or_else(|| PackError::SheetRef {
                        entry: entry.clone(),
                        msg: format!(
                            "sheet index {k} out of range ({} sheets listed)",
                            entries.len()
                        ),
                    })?,
                SheetRef::File(f) => {
                    let want = normalize_rel_path(f);
                    entries
                        .iter()
                        .position(|(_, p, _)| Some(p) == want.as_ref())
                        .ok_or_else(|| PackError::SheetRef {
                            entry: entry.clone(),
                            msg: format!("sheet \"{f}\" is not listed in sheets[]"),
                        })?
                }
            };
            let (img_idx, ref path, sheet_colors) = entries[sheet_idx];
            let colors = match t.colors.as_deref() {
                Some(c) => parse_colors(Some(c), &entry)?,
                None => sheet_colors,
            };
            let img = &images[img_idx];
            let fits =
                t.x.checked_add(u64::from(cell))
                    .is_some_and(|e| e <= u64::from(img.width))
                    && t.y
                        .checked_add(u64::from(cell))
                        .is_some_and(|e| e <= u64::from(img.height));
            if !fits {
                return Err(PackError::CellOutOfRange {
                    entry,
                    file: path.clone(),
                    x: t.x,
                    y: t.y,
                    cell,
                    sheet: (img.width, img.height),
                });
            }
            let (x0, y0) = (t.x as u32, t.y as u32);
            let bleed = parse_bleed(t.bleed.as_deref(), &entry)?;
            let [bl, bt, br, bb] = bleed.map(|v| u32::from(v) * scale);
            if bl > x0 || bt > y0 || x0 + cell + br > img.width || y0 + cell + bb > img.height {
                return Err(PackError::Bleed {
                    entry,
                    msg: format!(
                        "{bleed:?} around the cell at x={x0}, y={y0} does not fit inside \"{path}\" \
                         ({}x{} px)",
                        img.width, img.height
                    ),
                });
            }
            let (solid, blank) = scan_cell(img, x0, y0, cell);
            let cref = CellRef {
                sheet: img_idx as u16,
                x0,
                y0,
                solid,
                blank,
                bleed,
            };
            let src = format!("{entry} (\"{path}\" x={x0}, y={y0})");
            register(&mut reg, (page, tile, colors), cref, src)?;
        }

        let mut layers = Vec::with_capacity(m.layers.len());
        for (i, l) in m.layers.iter().enumerate() {
            let entry = format!("layers[{i}] (\"{}\")", l.file);
            let path = normalize_rel_path(&l.file).ok_or_else(|| PackError::Path {
                entry: entry.clone(),
                path: l.file.clone(),
            })?;
            let bad = |msg: String| PackError::Layer {
                entry: entry.clone(),
                msg,
            };
            let depth = match l.depth.as_deref() {
                None | Some("back") => LayerDepth::Back,
                Some("front") => LayerDepth::Front,
                Some(other) => {
                    return Err(bad(format!(
                        "unknown depth \"{other}\" (expected \"back\" or \"front\")"
                    )))
                }
            };
            let scroll = match l.scroll {
                None => 100,
                Some(v) => u8::try_from(v)
                    .ok()
                    .filter(|v| *v <= 100)
                    .ok_or_else(|| bad(format!("scroll {v} out of range (0..=100)")))?,
            };
            let coord = |v: i64, name: &str| {
                i32::try_from(v)
                    .ok()
                    .filter(|v| (-65_536..=65_536).contains(v))
                    .ok_or_else(|| bad(format!("{name} {v} out of range")))
            };
            let byte = |v: Option<u64>, name: &str| {
                v.map(|v| {
                    u8::try_from(v)
                        .map_err(|_| bad(format!("when.{name} {v} out of range (0..=255)")))
                })
                .transpose()
            };
            let when = l.when.unwrap_or_default();
            if depth == LayerDepth::Back && !l.over_tiles.is_empty() {
                return Err(bad("\"over_tiles\" needs depth \"front\"".to_string()));
            }
            let mut over_tiles = Vec::with_capacity(l.over_tiles.len());
            for o in &l.over_tiles {
                let page = check_page(o.page, &entry)?;
                let tile = u8::try_from(o.tile).map_err(|_| PackError::Tile {
                    entry: entry.clone(),
                    got: o.tile,
                })?;
                over_tiles.push((page, tile));
            }
            over_tiles.sort_unstable();
            over_tiles.dedup();
            let img_idx = if let Some(&k) = file_to_image.get(&path) {
                k
            } else {
                if images.len() >= usize::from(u16::MAX) {
                    return Err(bad("too many distinct image files".to_string()));
                }
                let bytes = read(&path).map_err(|reason| PackError::MissingFile {
                    file: path.clone(),
                    reason,
                })?;
                let mut img = decode_png_rgba(&bytes).map_err(|err| PackError::Png {
                    file: path.clone(),
                    err,
                })?;
                threshold_alpha(&mut img, alpha_threshold);
                images.push(img);
                image_files.push(path.clone());
                file_to_image.insert(path.clone(), images.len() - 1);
                images.len() - 1
            };
            layers.push(Layer {
                sheet: img_idx as u16,
                depth,
                x: coord(l.x, "x")?,
                y: coord(l.y, "y")?,
                scroll,
                repeat_x: l.repeat_x,
                world: byte(when.world, "world")?,
                region: byte(when.region, "region")?,
                scene: byte(when.scene, "scene")?,
                over_tiles,
            });
        }

        let mut index = vec![TileSlot::default(); CHR_PAGES * TILES_PER_PAGE];
        let (mut tile_count, mut variant_count) = (0, 0);
        let mut max_bleed_v = 0u8;
        // BTreeMap order: None before Some, variants ascending -> already sorted.
        for ((page, tile, colors), (cref, _)) in reg {
            max_bleed_v = max_bleed_v.max(cref.bleed[1]).max(cref.bleed[3]);
            let slot = &mut index[usize::from(page) * TILES_PER_PAGE + usize::from(tile)];
            match colors {
                None => {
                    slot.default = Some(cref);
                    tile_count += 1;
                }
                Some(c) => {
                    slot.variants.push((c, cref));
                    variant_count += 1;
                }
            }
        }

        Ok(Self {
            name: m.name.clone(),
            author: m.author.clone(),
            scale,
            alpha_threshold,
            sheets: images,
            sheet_files: image_files,
            index,
            palette,
            groups: m.groups.clone(),
            tile_count,
            variant_count,
            sprite_alpha_art,
            layers,
            max_bleed_v,
        })
    }

    /// `"sprite_alpha": "art"`: replaced sprites take their silhouette from
    /// the cell's alpha, and cell `bleed` is drawn.
    #[must_use]
    pub fn sprite_alpha_art(&self) -> bool {
        self.sprite_alpha_art
    }

    /// Scene layers in drawing order.
    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Largest top or bottom `bleed` of any cell, in NES pixels.
    #[must_use]
    pub fn max_bleed_v(&self) -> u8 {
        self.max_bleed_v
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn author(&self) -> Option<&str> {
        self.author.as_deref()
    }

    /// HD pixels per NES pixel.
    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Cell edge in pixels (`8 * scale`).
    #[must_use]
    pub fn cell_size(&self) -> u32 {
        8 * self.scale
    }

    #[must_use]
    pub fn alpha_threshold(&self) -> u8 {
        self.alpha_threshold
    }

    /// Master palette override, when the pack sets one.
    #[must_use]
    pub fn palette(&self) -> Option<&MasterPalette> {
        self.palette.as_ref()
    }

    /// Raw `groups` object (artist notes), when present.
    #[must_use]
    pub fn groups(&self) -> Option<&serde_json::Value> {
        self.groups.as_ref()
    }

    /// Decoded sheets (alpha already thresholded to 0/255).
    #[must_use]
    pub fn sheets(&self) -> &[RgbaImage] {
        &self.sheets
    }

    /// Normalised file path of sheet `i`.
    #[must_use]
    pub fn sheet_file(&self, i: usize) -> Option<&str> {
        self.sheet_files.get(i).map(String::as_str)
    }

    /// Registered palette-independent cells.
    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.tile_count
    }

    /// Registered palette-specific cells.
    #[must_use]
    pub fn variant_count(&self) -> usize {
        self.variant_count
    }

    /// `(defaults, variants)` registered on one CHR page.
    #[must_use]
    pub fn page_counts(&self, page: u8) -> (usize, usize) {
        let start = usize::from(page) * TILES_PER_PAGE;
        self.index
            .get(start..start + TILES_PER_PAGE)
            .map_or((0, 0), |slots| {
                slots.iter().fold((0, 0), |(d, v), s| {
                    (d + usize::from(s.default.is_some()), v + s.variants.len())
                })
            })
    }

    /// Resolve a tile drawn with sub-palette colours `colors` (entries 1..=3):
    /// the exact variant first, then the default, else `None` (use CHR art).
    #[inline]
    #[must_use]
    pub fn lookup(&self, page: u8, tile: u8, colors: [u8; 3]) -> Option<CellRef> {
        let slot = self
            .index
            .get(usize::from(page) * TILES_PER_PAGE + usize::from(tile))?;
        if !slot.variants.is_empty() {
            let key = colors.map(|c| c & 0x3F);
            if let Ok(i) = slot.variants.binary_search_by(|(c, _)| c.cmp(&key)) {
                return Some(slot.variants[i].1);
            }
        }
        slot.default
    }

    /// The palette-independent cell only.
    #[must_use]
    pub fn lookup_default(&self, page: u8, tile: u8) -> Option<CellRef> {
        self.index
            .get(usize::from(page) * TILES_PER_PAGE + usize::from(tile))?
            .default
    }

    /// Pixels of a cell returned by [`HdPack::lookup`].
    ///
    /// Panics if `cell` did not come from this pack.
    #[inline]
    #[must_use]
    pub fn cell_pixels(&self, cell: CellRef) -> CellPixels<'_> {
        CellPixels {
            sheet: &self.sheets[usize::from(cell.sheet)],
            x0: cell.x0,
            y0: cell.y0,
            size: 8 * self.scale,
            bleed_px: cell.bleed.map(|v| u32::from(v) * self.scale),
        }
    }
}

/// `(page, tile, variant colours)`; `None` sorts before every variant.
type RegKey = (u8, u8, Option<[u8; 3]>);
/// Registered cells with a human-readable source for duplicate errors.
type Registry = BTreeMap<RegKey, (CellRef, String)>;

fn register(reg: &mut Registry, key: RegKey, cref: CellRef, src: String) -> Result<(), PackError> {
    if let Some((_, first)) = reg.get(&key) {
        return Err(PackError::DuplicateTile {
            page: key.0,
            tile: key.1,
            colors: key.2,
            first: first.clone(),
            second: src,
        });
    }
    reg.insert(key, (cref, src));
    Ok(())
}

fn check_page(p: u64, entry: &str) -> Result<u8, PackError> {
    u8::try_from(p)
        .ok()
        .filter(|p| usize::from(*p) < CHR_PAGES)
        .ok_or_else(|| PackError::Page {
            entry: entry.to_string(),
            got: p,
        })
}

/// `bleed`: absent = none, else four values 0..=[`MAX_BLEED`].
fn parse_bleed(b: Option<&[u64]>, entry: &str) -> Result<[u8; 4], PackError> {
    let Some(b) = b else { return Ok([0; 4]) };
    let bad = |msg: String| PackError::Bleed {
        entry: entry.to_string(),
        msg,
    };
    if b.len() != 4 {
        return Err(bad(format!(
            "has {} values, expected [left, top, right, bottom]",
            b.len()
        )));
    }
    let mut out = [0u8; 4];
    for (o, &v) in out.iter_mut().zip(b) {
        *o = u8::try_from(v)
            .ok()
            .filter(|v| *v <= MAX_BLEED)
            .ok_or_else(|| bad(format!("value {v} out of range (0..={MAX_BLEED})")))?;
    }
    Ok(out)
}

fn parse_colors(c: Option<&[u64]>, entry: &str) -> Result<Option<[u8; 3]>, PackError> {
    let Some(c) = c else { return Ok(None) };
    if c.len() != 3 {
        return Err(PackError::Color {
            entry: entry.to_string(),
            msg: format!(
                "\"colors\" has {} values; expected 3 NES colour indices",
                c.len()
            ),
        });
    }
    let mut out = [0u8; 3];
    for (o, &v) in out.iter_mut().zip(c) {
        if v > 63 {
            return Err(PackError::Color {
                entry: entry.to_string(),
                msg: format!("colour {v} is not an NES colour index (0..=63)"),
            });
        }
        *o = v as u8;
    }
    Ok(Some(out))
}

fn parse_palette(
    spec: &PaletteSpec,
    read: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<MasterPalette, PackError> {
    match spec {
        PaletteSpec::File(f) => {
            let path = normalize_rel_path(f).ok_or_else(|| PackError::Path {
                entry: "palette".to_string(),
                path: f.clone(),
            })?;
            let bytes = read(&path).map_err(|reason| PackError::MissingFile {
                file: path.clone(),
                reason,
            })?;
            MasterPalette::from_pal_bytes(&bytes).map_err(|m| PackError::Palette {
                msg: format!("{path}: {m}"),
            })
        }
        PaletteSpec::Entries(v) => {
            if v.len() != 64 {
                return Err(PackError::Palette {
                    msg: format!("inline palette has {} entries; expected 64", v.len()),
                });
            }
            let mut pal = [[0u8; 3]; 64];
            for (i, (dst, e)) in pal.iter_mut().zip(v).enumerate() {
                *dst = match e {
                    serde_json::Value::String(s) => {
                        MasterPalette::parse_hex_color(s).map_err(|m| PackError::Palette {
                            msg: format!("entry {i}: {m}"),
                        })?
                    }
                    serde_json::Value::Array(a) if a.len() == 3 => {
                        let mut rgb = [0u8; 3];
                        for (c, x) in rgb.iter_mut().zip(a) {
                            *c =
                                x.as_u64()
                                    .and_then(|n| u8::try_from(n).ok())
                                    .ok_or_else(|| PackError::Palette {
                                        msg: format!(
                                            "entry {i}: channels must be integers 0..=255"
                                        ),
                                    })?;
                        }
                        rgb
                    }
                    _ => {
                        return Err(PackError::Palette {
                            msg: format!("entry {i}: expected \"#RRGGBB\" or [r, g, b]"),
                        })
                    }
                };
            }
            Ok(MasterPalette(pal))
        }
    }
}

/// Normalise a manifest path: `\` -> `/`, drop `.` components; reject empty,
/// absolute, drive-qualified and `..` paths.
#[must_use]
pub fn normalize_rel_path(p: &str) -> Option<String> {
    let s = p.replace('\\', "/");
    if s.starts_with('/') || s.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut parts = Vec::new();
    for c in s.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn threshold_alpha(img: &mut RgbaImage, threshold: u8) {
    for px in img.rgba.chunks_exact_mut(4) {
        px[3] = if px[3] >= threshold { 0xFF } else { 0 };
    }
}

/// `(solid, blank)` for the `cell` square at `(x0, y0)` of a thresholded sheet.
fn scan_cell(img: &RgbaImage, x0: u32, y0: u32, cell: u32) -> (bool, bool) {
    let (mut solid, mut blank) = (true, true);
    let w = img.width as usize;
    for y in y0..y0 + cell {
        let start = (y as usize * w + x0 as usize) * 4;
        for px in img.rgba[start..start + cell as usize * 4].chunks_exact(4) {
            if px[3] == 0 {
                solid = false;
            } else {
                blank = false;
            }
        }
    }
    (solid, blank)
}
