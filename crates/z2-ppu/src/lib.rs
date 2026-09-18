//! `z2-ppu`: software PPU model, including the sprite-0 HUD split.
//!
//! The model renders exactly what the cartridge shows from the game's own
//! PPU writes, with no dot-level emulation: a per-scanline renderer over
//! nametables, attributes, CHR banks, OAM and palette RAM, driven
//! *scanline-timed*: the
//! owner reports the beam position derived from the CPU clock before every
//! register access ([`Ppu::set_beam`]) and the model renders lazily, so the
//! status-bar HUD split, `$2006` mid-frame scrolls and the sprite-0 hit
//! timing all fall out of the register writes themselves.
//!
//! Frames are **indexed**: each pixel is an NES palette index (`$00-$3F`),
//! so golden tests compare pixels without any display transform. RGBA
//! conversion ([`palette::indexed_to_rgba`]) is display-only.
//!
//! ## Wiring (for the frontends and the verifier)
//!
//! Per emulated frame, main must:
//!
//! 1. Copy game state into the [`Ppu`]: nametables (`nt_write` / `set_tile`
//!    / `set_attr_quad`, or `$2006`/`$2007` via `write_addr`/`write_data`),
//!    CHR pages (`load_chr_4k` from `assets.bin` when the MMC1 CHR banks
//!    change), OAM (`oam_dma` from `$4014` or `set_oam_entry`), palette RAM
//!    (`set_palette`), scroll (`write_scroll` x2), `PPUCTRL`/`PPUMASK`
//!    (`write_ctrl`/`write_mask`), and mirroring (`set_mirroring_mmc1`).
//! 2. Keep the beam current with [`Ppu::set_beam`] before each access (the
//!    `z2-core` binding derives scanline/dot from the CPU cycle count).
//! 3. Call [`Ppu::finish_frame`] at vblank for the indexed framebuffer (or
//!    [`Ppu::render_frame`] for a frame-granular render); `$2002` reads
//!    answer sprite-0 hit / vblank against the beam.
//! 4. For verification, serve oracle frames through
//!    [`golden::OracleFrameSource`] and diff with [`golden::verify_against_oracle`].
//!
//! ## Render record and widescreen (opt-in)
//!
//! [`Ppu::set_record`] makes the renderer keep a [`FrameRecord`] beside the
//! indexed frame: per line the fetched background tile identities, scroll,
//! `PPUCTRL`/`PPUMASK`, CHR pages, palette RAM and the chosen sprites. It
//! never changes pixels. [`render_wide_indexed`] composes a wider indexed
//! image whose centre 256 columns are the frame verbatim and whose margins
//! are painted from tile identities ([`Margins`]) supplied by a provider.

pub mod golden;
pub mod palette;
pub mod png;
pub mod record;
pub mod render;
pub mod state;
pub mod wide;

pub use golden::{
    sample_indices, verify_against_oracle, GoldenMismatch, GoldenReport, OracleFrameSource,
    GOLDEN_EVERY_NTH_MOVIE_FRAME,
};
pub use palette::{indexed_to_rgba, NES_PALETTE_RGB};
pub use png::{encode_indexed_png, encode_indexed_png_wh};
pub use record::{BgTileId, FrameRecord, LineRecord, SpriteRef, NO_PAGE, RECORD_TILES_PER_LINE};
pub use render::{
    diff_indexed, probe_sprite0_hit, render_frame, render_line, write_diff_ppm, FrameDiff,
    IndexedFrame, LineFlags, MAX_DIFF_COORDS,
};
pub use state::{
    AccessKind, ChrError, Mirroring, OamEntry, Ppu, PpuEvent, PpuEventKind, SpriteLimit,
    ATTR_OFFSET, BG_FETCHES_PER_LINE, CHR_BANK_LEN, DOTS_PER_LINE, LINES_PER_FRAME,
    LINE_COMMIT_DOT, NAMETABLE_LEN, OAM_LEN, PALETTE_LEN, PPUCTRL_BG_TABLE, PPUCTRL_NMI,
    PPUCTRL_SPRITE_TABLE, PPUCTRL_STEP_32, PPUCTRL_TALL_SPRITES, PPUMASK_GRAYSCALE,
    PPUMASK_SHOW_BG, PPUMASK_SHOW_LEFT_BG, PPUMASK_SHOW_LEFT_SPRITES, PPUMASK_SHOW_SPRITES,
    PPUSTATUS_OVERFLOW, PPUSTATUS_SPRITE0, PPUSTATUS_VBLANK, PRERENDER_COPY_DOT, PRERENDER_LINE,
    SPRITE0_HIT_LATENCY, VBLANK_LINE,
};
pub use wide::{
    chr_row, chr_sub, preset_tiles, render_wide_indexed, right_edge_masked, wide_width, MarginFill,
    MarginLine, Margins, WideFrame, MARGIN_SLOTS, MAX_MARGIN_TILES,
};

/// Visible frame width in pixels.
pub const WIDTH: usize = 256;
/// Visible frame height in pixels.
pub const HEIGHT: usize = 240;
/// Indexed-framebuffer length in bytes (`WIDTH * HEIGHT`).
pub const FRAME_LEN: usize = WIDTH * HEIGHT;
