//! `z2-render`: the display layer on top of the indexed PPU frame.
//!
//! This crate owns everything about *how a frame is shown* that is not part
//! of reconstruction parity: HD graphics packs, template sheets rendered from
//! the user's CHR, and the recorder that collects the tile/palette
//! combinations a play session actually drew. `cargo xtask hdpack --help`
//! covers the artist workflow; [`pack`] documents the `pack.json` format.
//!
//! Layout:
//! - [`compositor`]: indexed frame + render record -> RGBA at integer scale,
//!   with HD art substituted per tile and sprite.
//! - [`presenter`]: the one object a frontend drives each frame (owns the
//!   widescreen, margin and RGBA buffers).
//! - [`png_io`]: PNG decode (any colour type / bit depth -> RGBA8) and encode.
//! - [`palette`]: the 64-entry NES master palette and `.pal` parsing.
//! - [`chr`]: CHR page / tile decoding and page-sheet painting.
//! - [`pack`]: the `pack.json` v1 schema, validation and the resolved
//!   [`HdPack`] lookup tables.
//! - [`template`]: full-page template sheets straight from CHR pages.
//! - [`recorder`]: seen-tile bookkeeping and palette-specific template packs.
//! - `fs` (native only): directory load / write helpers and the git work
//!   tree guard used by every tool that writes ROM-derived output.
//!
//! The core API never touches the filesystem: packs load from a reader
//! closure or an in-memory file list, so the web frontend can hand in files
//! from a directory picker. Nothing here iterates a `HashMap`; every lookup
//! structure is a dense `Vec` or a `BTreeMap`, so output is deterministic.
//!
//! Legal: template sheets are rendered from ROM CHR bytes. They are written
//! only to a user-chosen directory outside the repository (LEGAL.md §1), every
//! template PNG carries the [`template::TEMPLATE_TEXT_KEY`] tEXt marker the
//! pre-commit hook looks for, and tests use synthetic CHR only.

#![forbid(unsafe_code)]

pub mod chr;
pub mod compositor;
#[cfg(not(target_arch = "wasm32"))]
pub mod fs;
pub mod pack;
pub mod palette;
pub mod png_io;
pub mod presenter;
pub mod recorder;
pub mod template;

pub use chr::{paint_page_sheet, tile_subpixels, CHR_PAGES, CHR_PAGE_LEN, TILES_PER_PAGE};
pub use compositor::{
    bg_colors, sprite_colors, ComposeError, ComposeInput, Compositor, IndexedView,
};
pub use pack::{
    CellPixels, CellRef, HdPack, Layer, LayerDepth, LayerEntry, LayerWhen, OverTile, PackError,
    PackManifest, PaletteSpec, SceneView, SheetEntry, SheetRef, TileEntry, MAX_BLEED, PACK_FORMAT,
    PACK_MANIFEST, PACK_VERSION,
};
pub use palette::MasterPalette;
pub use png_io::{decode_png_rgba, encode_png_rgba, PngError, RgbaImage, MAX_SHEET_DIM};
pub use presenter::{PresentConfig, Presenter};
pub use recorder::{Recorder, SeenAs, SeenKey};
pub use template::{template_from_chr, TemplateError, TemplateOptions, TemplateOutput};

/// Largest supported pack scale (HD pixels per NES pixel).
pub const MAX_SCALE: u32 = 8;
