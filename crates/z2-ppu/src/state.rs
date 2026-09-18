//! PPU state: nametables, CHR banks, OAM, palette RAM, registers, and the
//! scanline-timed catch-up renderer.
//!
//! [`Ppu`] holds everything the game can observe through `$2000-$2007` and
//! `$4014`, plus the MMC1 mirroring mode. Rendering is *scanline-timed*: the
//! owner tells the model where the beam is ([`Ppu::set_beam`], in PPU
//! scanline/dot derived from the CPU cycle count) before every register
//! access, and the model lazily renders every visible scanline the beam has
//! passed with the register state that was live at that time. That is what
//! makes mid-frame effects come out right without dot-level emulation:
//!
//! * the sprite-0 HUD split (the game spins on `$2002` bit 6, then rewrites
//!   `$2000`/`$2005` — rows below the hit line pick up the new scroll),
//! * `$2006`-driven mid-frame vertical scrolls (the title/intro text crawl),
//! * mid-frame `PPUMASK` blanking and MMC1 CHR bank switches,
//! * and the sprite-0 hit flag itself, which sets when the beam reaches the
//!   first overlapping pixel — so a `BIT $2002 / BVC` spin exits at the
//!   right scanline instead of a frame late.
//!
//! Scroll/address state is the hardware register set (`v`, `t`, fine `x`,
//! write toggle `w`) with the documented `$2000`/`$2005`/`$2006`/`$2007`
//! effects, the per-scanline `v` increments while rendering is enabled, and
//! the pre-render `t → v` copy.
//!
//! Model notes (simplifications are documented, not hidden):
//!
//! * Two physical 1 KiB nametables; the four logical slots `$2000/$2400/`
//!   `$2800/$2C00` fold onto them via [`Mirroring`]. `$3000-$3FFF` mirrors
//!   `$2000-$2FFF`.
//! * Pattern tables are two 4 KiB slots (`$0000-$0FFF`, `$1000-$1FFF`)
//!   loaded from CHR ROM with [`Ppu::load_chr_4k`]. Writes to `$0000-$1FFF`
//!   through `$2007` are ignored (Zelda II is CHR-ROM).
//! * Register writes apply to the *whole* current scanline when they land
//!   before dot 256, and from the next scanline otherwise (no mid-scanline
//!   pixel splits).
//! * `$2007` reads are buffered except for palette reads, which return
//!   immediately (the buffer-update quirk for palette reads is not modelled).

use crate::record::{BgTileId, FrameRecord, LineRecord};
use crate::{IndexedFrame, HEIGHT, WIDTH};

/// PPU control register `$2000` bit: generate NMI at vblank start.
pub const PPUCTRL_NMI: u8 = 0x80;
/// PPU control register `$2000` bit: tall (8x16) sprites.
pub const PPUCTRL_TALL_SPRITES: u8 = 0x20;
/// PPU control register `$2000` bit: background pattern table high (`$1000`).
pub const PPUCTRL_BG_TABLE: u8 = 0x10;
/// PPU control register `$2000` bit: 8x8 sprite pattern table high.
pub const PPUCTRL_SPRITE_TABLE: u8 = 0x08;
/// PPU control register `$2000` bit: `$2007` address step 32 (else 1).
pub const PPUCTRL_STEP_32: u8 = 0x04;

/// PPU mask register `$2001` bit: greyscale (palette index `& $30`).
pub const PPUMASK_GRAYSCALE: u8 = 0x01;
/// PPU mask register `$2001` bit: show background in the leftmost 8 pixels.
pub const PPUMASK_SHOW_LEFT_BG: u8 = 0x02;
/// PPU mask register `$2001` bit: show sprites in the leftmost 8 pixels.
pub const PPUMASK_SHOW_LEFT_SPRITES: u8 = 0x04;
/// PPU mask register `$2001` bit: show background.
pub const PPUMASK_SHOW_BG: u8 = 0x08;
/// PPU mask register `$2001` bit: show sprites.
pub const PPUMASK_SHOW_SPRITES: u8 = 0x10;

/// PPU status register `$2002` bit: vblank started.
pub const PPUSTATUS_VBLANK: u8 = 0x80;
/// PPU status register `$2002` bit: sprite-0 hit.
pub const PPUSTATUS_SPRITE0: u8 = 0x40;
/// PPU status register `$2002` bit: sprite overflow (>8 per scanline).
pub const PPUSTATUS_OVERFLOW: u8 = 0x20;

/// Bytes per physical nametable (960 tile bytes + 64 attribute bytes).
pub const NAMETABLE_LEN: usize = 1024;
/// Offset of the attribute table inside a nametable.
pub const ATTR_OFFSET: usize = 0x3C0;
/// Bytes per CHR bank (one pattern table).
pub const CHR_BANK_LEN: usize = 4096;
/// OAM bytes (64 sprites x 4).
pub const OAM_LEN: usize = 256;
/// Palette RAM bytes (`$3F00-$3F1F`).
pub const PALETTE_LEN: usize = 32;

/// Dots per scanline.
pub const DOTS_PER_LINE: u16 = 341;
/// Scanlines per NTSC frame (pre-render + 240 visible + post-render + 20 vblank).
pub const LINES_PER_FRAME: i16 = 262;
/// Beam line value for the pre-render scanline.
pub const PRERENDER_LINE: i16 = -1;
/// First vblank scanline.
pub const VBLANK_LINE: i16 = 241;
/// Dot on the pre-render line by which `v` has been reloaded from `t`
/// (hardware copies the vertical bits during dots 280-304).
pub const PRERENDER_COPY_DOT: u16 = 304;
/// A register write landing at or after this dot applies from the next line.
pub const LINE_COMMIT_DOT: u16 = 256;
/// Background tile fetches per visible line (2 prefetched + 32 on-line).
pub const BG_FETCHES_PER_LINE: u16 = 34;

/// What kind of register access is about to happen at the reported beam
/// position — decides how much of the current scanline is committed with
/// the *old* state before the access applies (see [`Ppu::set_beam_access`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    /// No state change (e.g. a `$2002` read): nothing to commit.
    Read,
    /// Changes what later background *tile fetches* see (`$2000` pattern
    /// select, `$2006`/`$2007` `v` updates, CHR bank / mirroring switches):
    /// tiles already fetched keep their data, the rest re-fetch.
    Fetch,
    /// Changes the *pixel* path immediately (`$2001` mask, palette RAM):
    /// pixels already output keep their colour.
    Pixel,
    /// Changes sprite state used by the next line's evaluation (OAM
    /// writes / DMA): the current line completes with the old state.
    Line,
}

/// One fetched background tile (pattern row planes + palette selector),
/// plus its identity for the render record. Pixel emission reads only
/// `lo`/`hi`/`pal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct BgTile {
    lo: u8,
    hi: u8,
    pal: u8,
    id: BgTileId,
}

/// Per-scanline background pipeline: the pixels emitted so far, the tiles
/// fetched so far (in fetch order), and the emission cursor.
#[derive(Debug, Clone)]
struct LineBuf {
    pixels: [u8; WIDTH],
    opaque: [bool; WIDTH],
    tiles: [BgTile; BG_FETCHES_PER_LINE as usize],
    /// Pixels emitted (`0..=256`).
    cursor: u16,
    /// Next aligned tile index to fetch (`0..=34`).
    next_fetch: u16,
    /// Line has been started (tiles 0-1 prefetched).
    started: bool,
    /// `v` before the tile-0 prefetch (record: `LineRecord::v_start`).
    v_start: u16,
    /// Fine X at pixel 0 (record: `LineRecord::fine_x`).
    fine_x_start: u8,
    /// Fetch-state change after the line started (record only).
    split: bool,
    /// Pixel-state change after pixels were emitted (record only).
    pixel_split: bool,
}

impl LineBuf {
    fn new() -> Self {
        Self {
            pixels: [0; WIDTH],
            opaque: [false; WIDTH],
            tiles: [BgTile::default(); BG_FETCHES_PER_LINE as usize],
            cursor: 0,
            next_fetch: 0,
            started: false,
            v_start: 0,
            fine_x_start: 0,
            split: false,
            pixel_split: false,
        }
    }

    fn reset(&mut self) {
        self.cursor = 0;
        self.next_fetch = 0;
        self.started = false;
        self.v_start = 0;
        self.fine_x_start = 0;
        self.split = false;
        self.pixel_split = false;
    }
}
/// Sprite-0 hit becomes visible to `$2002` this many dots after the hit pixel.
pub const SPRITE0_HIT_LATENCY: u16 = 2;

/// MMC1 / PPU nametable mirroring: how the four logical 1 KiB slots fold
/// onto the two physical nametables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mirroring {
    /// `0`: one screen, both slots read the first nametable.
    SingleLower,
    /// `1`: one screen, both slots read the second nametable.
    SingleUpper,
    /// `2`: `($2000=$2800)=A, ($2400=$2C00)=B`.
    #[default]
    Vertical,
    /// `3`: `($2000=$2400)=A, ($2800=$2C00)=B`.
    Horizontal,
}

impl Mirroring {
    /// Decode the MMC1 control register's mirroring field (bits 1-0).
    #[must_use]
    pub fn from_mmc1_ctrl(ctrl: u8) -> Self {
        match ctrl & 0x03 {
            0 => Mirroring::SingleLower,
            1 => Mirroring::SingleUpper,
            2 => Mirroring::Vertical,
            _ => Mirroring::Horizontal,
        }
    }

    /// Map a logical slot (`0=$2000 .. 3=$2C00`) to a physical nametable.
    #[must_use]
    pub fn physical(self, logical: usize) -> usize {
        match self {
            Mirroring::SingleLower => 0,
            Mirroring::SingleUpper => 1,
            Mirroring::Vertical => logical & 0x01,
            Mirroring::Horizontal => (logical >> 1) & 0x01,
        }
    }
}

/// How many sprites may cover one scanline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpriteLimit {
    /// Hardware-faithful: only the first 8 sprites in OAM order render per
    /// scanline; extras drop out and set the overflow flag. Default: Zelda II
    /// was tested against this.
    #[default]
    Faithful8,
    /// Render every sprite on the scanline (no dropout, no overflow flag).
    /// Provided to *measure* whether the game relies on flicker.
    Unlimited,
}

/// One decoded OAM entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OamEntry {
    /// Raw Y byte: the sprite's top edge is `y.wrapping_add(1)`.
    pub y: u8,
    /// Tile index (8x16: bit 0 picks the pattern table).
    pub tile: u8,
    /// Attribute: `VHP-----pp` (flip-v, flip-h, behind-bg, palette 0-3).
    pub attr: u8,
    /// Left edge x.
    pub x: u8,
}

/// CHR bank load failure from [`Ppu::load_chr_4k`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChrError {
    /// Pattern-table slot must be 0 (`$0000`) or 1 (`$1000`).
    BadSlot { slot: usize },
    /// A 4 KiB bank needs exactly 4096 bytes.
    BadLength { got: usize },
}

impl core::fmt::Display for ChrError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ChrError::BadSlot { slot } => write!(f, "CHR slot {slot} out of range (want 0 or 1)"),
            ChrError::BadLength { got } => {
                write!(f, "CHR bank is {got} bytes, want {CHR_BANK_LEN}")
            }
        }
    }
}

/// One traced PPU event (diagnostics; see [`Ppu::set_trace`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpuEvent {
    /// Beam line at the event (`-1` pre-render, `241+` vblank).
    pub line: i16,
    /// Beam dot at the event.
    pub dot: u16,
    /// What happened.
    pub kind: PpuEventKind,
}

/// Traced event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpuEventKind {
    /// `$2000` write (value).
    Ctrl(u8),
    /// `$2001` write (value).
    Mask(u8),
    /// `$2002` read (value returned).
    Status(u8),
    /// `$2005` write (value, was-first-write).
    Scroll(u8, bool),
    /// `$2006` write (value, was-first-write); `v` after the write.
    Addr(u8, bool, u16),
    /// `$2007` read; `v` before the access.
    DataRead(u16),
    /// `$2007` write (value); `v` before the access.
    DataWrite(u8, u16),
    /// Sprite-0 hit first observed (line, x).
    Hit(u8, u8),
    /// Visible frame started (pre-render `t -> v` reload done); `v`.
    FrameStart(u16),
    /// Frame finished (`finish_frame`).
    FrameEnd,
    /// Mirroring / CHR page change from the mapper (`mmc1 ctrl`, page0, page1).
    Mapper(u8, u8, u8),
}

/// Software PPU state with scanline-timed rendering.
///
/// Construct with [`Ppu::new`], load CHR with [`Ppu::load_chr_4k`], drive the
/// CPU-visible registers with the `write_*` / `read_*` methods, keep
/// [`Ppu::set_beam`] current, and collect each frame with
/// [`Ppu::finish_frame`] (or the whole-frame convenience [`Ppu::render_frame`]).
#[derive(Debug, Clone)]
pub struct Ppu {
    nt: [[u8; NAMETABLE_LEN]; 2],
    mirroring: Mirroring,
    chr: [[u8; CHR_BANK_LEN]; 2],
    /// Asset CHR page (0-31) backing each slot, `u8::MAX` when unset.
    chr_page_no: [u8; 2],
    oam: [u8; OAM_LEN],
    oam_addr: u8,
    palette_ram: [u8; PALETTE_LEN],
    ppuctrl: u8,
    ppumask: u8,
    ppustatus: u8,
    /// Current VRAM address / scroll position (`yyy NN YYYYY XXXXX`).
    v: u16,
    /// Temporary VRAM address (top-left tile of the frame).
    t: u16,
    /// Fine X scroll (0-7).
    fine_x: u8,
    /// Shared `$2005`/`$2006` first/second-write toggle (`false` = next write
    /// is the first). Cleared by [`Ppu::read_status`].
    w: bool,
    ppu_data_buf: u8,
    sprite_limit: SpriteLimit,
    // -- scanline-timed rendering ------------------------------------------
    /// Frame under construction (boxed: 60 KiB must not live on the stack
    /// of every `Ppu`/`Game` temporary).
    frame_buf: Box<IndexedFrame>,
    /// Next visible scanline to render: `-1` = pre-render still pending
    /// (`v` not yet reloaded), `0..=239` next line, `240` = frame complete.
    next_line: i16,
    /// Beam position last reported by the owner (`-1` pre-render, `0..=239`
    /// visible, `240` post-render, `241..=260` vblank).
    beam_line: i16,
    beam_dot: u16,
    /// First sprite-0 hit of this frame: `(scanline, x)`. Cleared at the
    /// pre-render line ([`Ppu::begin_frame`]).
    hit: Option<(u8, u8)>,
    /// Memoised dry-run hit probe for the not-yet-rendered current line.
    probe_cache: Option<(u8, Option<u8>)>,
    /// Event trace (empty unless enabled; diagnostics only).
    trace: Vec<PpuEvent>,
    trace_on: bool,
    /// Background pipeline of the line being rendered.
    line: LineBuf,
    /// Opt-in render record of the frame in progress (`None` = off).
    record: Option<FrameRecord>,
    /// Record of the last frame completed by `finish_frame`.
    record_done: Option<FrameRecord>,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    /// Power-on state: zeroed memories, vertical mirroring, faithful
    /// 8-sprite limit, beam parked in vblank.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nt: [[0; NAMETABLE_LEN]; 2],
            mirroring: Mirroring::Vertical,
            chr: [[0; CHR_BANK_LEN]; 2],
            chr_page_no: [u8::MAX; 2],
            oam: [0xFF; OAM_LEN],
            oam_addr: 0,
            palette_ram: [0; PALETTE_LEN],
            ppuctrl: 0,
            ppumask: 0,
            ppustatus: 0,
            v: 0,
            t: 0,
            fine_x: 0,
            w: false,
            ppu_data_buf: 0,
            sprite_limit: SpriteLimit::Faithful8,
            frame_buf: Box::new([0; WIDTH * HEIGHT]),
            next_line: PRERENDER_LINE,
            beam_line: VBLANK_LINE,
            beam_dot: 0,
            hit: None,
            probe_cache: None,
            trace: Vec::new(),
            trace_on: false,
            line: LineBuf::new(),
            record: None,
            record_done: None,
        }
    }

    // -- tracing ----------------------------------------------------------------

    /// Enable/disable the event trace (diagnostics; off by default).
    pub fn set_trace(&mut self, on: bool) {
        self.trace_on = on;
        if !on {
            self.trace.clear();
        }
    }

    /// Take the recorded events (clears the buffer).
    pub fn take_trace(&mut self) -> Vec<PpuEvent> {
        std::mem::take(&mut self.trace)
    }

    // -- render record ------------------------------------------------------------

    /// Enable/disable the per-frame render record (off by default). Turning
    /// it on allocates the two record buffers once; turning it off drops
    /// them. Never affects pixels, sprite-0 hit or overflow. Lines rendered
    /// before enabling stay unrecorded (`LineRecord::valid == false`).
    pub fn set_record(&mut self, on: bool) {
        if on {
            if self.record.is_none() {
                self.record = Some(FrameRecord::new());
            }
            if self.record_done.is_none() {
                self.record_done = Some(FrameRecord::new());
            }
        } else {
            self.record = None;
            self.record_done = None;
        }
    }

    /// Whether the render record is on.
    #[must_use]
    pub fn record_enabled(&self) -> bool {
        self.record.is_some()
    }

    /// Record of the last frame completed by [`Ppu::finish_frame`] (matches
    /// the frame it returned). `None` when the record is off; an all-invalid
    /// record before the first completed frame.
    #[must_use]
    pub fn frame_record(&self) -> Option<&FrameRecord> {
        self.record_done.as_ref()
    }

    /// Record of the frame in progress (lines committed so far).
    #[must_use]
    pub fn record_in_progress(&self) -> Option<&FrameRecord> {
        self.record.as_ref()
    }

    /// A fetch-affecting change landed: flag the line if it already started.
    fn note_fetch_change(&mut self) {
        if self.line.started {
            self.line.split = true;
        }
    }

    /// A pixel-affecting change landed: flag the line if pixels were emitted.
    fn note_pixel_change(&mut self) {
        if self.line.started && self.line.cursor > 0 {
            self.line.pixel_split = true;
        }
    }

    fn trace_event(&mut self, kind: PpuEventKind) {
        if self.trace_on {
            self.trace.push(PpuEvent {
                line: self.beam_line,
                dot: self.beam_dot,
                kind,
            });
        }
    }

    // -- frame cadence --------------------------------------------------------

    /// Pre-render equivalent: clear vblank, sprite-0 hit and overflow.
    pub fn begin_frame(&mut self) {
        self.ppustatus &= !(PPUSTATUS_VBLANK | PPUSTATUS_SPRITE0 | PPUSTATUS_OVERFLOW);
        self.hit = None;
        self.probe_cache = None;
    }

    /// Vblank start: set the vblank flag (NMI delivery itself is main's job;
    /// see [`Ppu::nmi_enabled`]).
    pub fn end_frame(&mut self) {
        self.ppustatus |= PPUSTATUS_VBLANK;
    }

    /// Report the beam position (`line` `-1` = pre-render, `0..=239`
    /// visible, `240..=260` post-render/vblank; `dot` `0..=340`) and render
    /// every visible scanline the beam has passed since the last call.
    ///
    /// Equivalent to [`Ppu::set_beam_access`] with [`AccessKind::Read`]:
    /// lines before the beam line are committed, and the beam line itself
    /// once the beam is past [`LINE_COMMIT_DOT`].
    pub fn set_beam(&mut self, line: i16, dot: u16) {
        self.set_beam_access(line, dot, AccessKind::Read);
    }

    /// Report the beam position ahead of a register access of `kind`.
    ///
    /// Besides committing the lines the beam has passed, the current line
    /// is rendered *up to the point the access can affect*: for
    /// [`AccessKind::Fetch`] every background tile whose fetch started
    /// before `dot` is emitted with the old state (the hardware fetch
    /// pipeline runs two tiles ahead of the pixels), for
    /// [`AccessKind::Pixel`] the pixels up to `dot`, for
    /// [`AccessKind::Line`] the whole line. The remainder renders later
    /// with the state after the access.
    pub fn set_beam_access(&mut self, line: i16, dot: u16, kind: AccessKind) {
        self.beam_line = line.clamp(PRERENDER_LINE, LINES_PER_FRAME - 2);
        self.beam_dot = dot.min(DOTS_PER_LINE - 1);
        self.catch_up(kind);
    }

    /// Current beam `(line, dot)` as last reported.
    #[must_use]
    pub fn beam(&self) -> (i16, u16) {
        (self.beam_line, self.beam_dot)
    }

    /// Render everything the beam has passed, then the partial current line
    /// as far as `kind` requires.
    fn catch_up(&mut self, kind: AccessKind) {
        if self.next_line == PRERENDER_LINE {
            let past_prerender = (self.beam_line == PRERENDER_LINE
                && self.beam_dot >= PRERENDER_COPY_DOT)
                || (self.beam_line >= 0 && self.beam_line < HEIGHT as i16);
            if !past_prerender {
                return;
            }
            self.start_visible_frame();
        }
        if self.next_line >= HEIGHT as i16 {
            return;
        }
        let upto: i16 = if self.beam_line < 0 {
            0
        } else if self.beam_dot >= LINE_COMMIT_DOT {
            self.beam_line + 1
        } else {
            self.beam_line
        };
        let upto = upto.min(HEIGHT as i16);
        while self.next_line < upto {
            self.finish_line();
        }
        if self.next_line == self.beam_line
            && self.beam_line < HEIGHT as i16
            && self.beam_dot < LINE_COMMIT_DOT
        {
            let x_to: u16 = match kind {
                AccessKind::Read => return,
                AccessKind::Fetch => {
                    // Tiles whose fetch started before `dot`: aligned tile
                    // `k` is fetched during dots `8(k-2)+1 ..= 8(k-2)+8`.
                    let k_eff = (self.beam_dot + 6) / 8 + 2;
                    (8 * k_eff).saturating_sub(u16::from(self.fine_x))
                }
                AccessKind::Pixel => self.beam_dot,
                AccessKind::Line => WIDTH as u16,
            };
            self.emit_bg_until(x_to.min(WIDTH as u16));
        }
    }

    /// Pre-render line: reload `v` from `t` when rendering is enabled, then
    /// arm line 0.
    fn start_visible_frame(&mut self) {
        if self.rendering_enabled() {
            self.v = self.t;
        }
        self.next_line = 0;
        self.line.reset();
        self.probe_cache = None;
        if let Some(r) = self.record.as_mut() {
            if r.lines_done != 0 || !r.sprites.is_empty() {
                r.clear();
            }
        }
        let v = self.v;
        self.trace_event(PpuEventKind::FrameStart(v));
    }

    /// Fetch one background tile at `v` (nametable byte, attribute
    /// quadrant, pattern row) and advance `v`'s coarse X like the hardware
    /// fetch does. With rendering disabled nothing is fetched (blank tile,
    /// `v` untouched).
    fn fetch_bg_tile(&self, v: &mut u16) -> BgTile {
        if !self.rendering_enabled() {
            return BgTile::default();
        }
        let fine_y = ((*v >> 12) & 0x07) as u8;
        let coarse_y = ((*v >> 5) & 0x1F) as u8;
        let coarse_x = (*v & 0x1F) as u8;
        let logical = ((*v >> 10) & 0x03) as u8;
        let tile = self.nt_byte(logical, usize::from(coarse_y) * 32 + usize::from(coarse_x));
        let attr = self.nt_byte(
            logical,
            ATTR_OFFSET + usize::from(coarse_y >> 2) * 8 + usize::from(coarse_x >> 2),
        );
        let qx = (coarse_x >> 1) & 1;
        let qy = (coarse_y >> 1) & 1;
        let pal = (attr >> (((qy << 1) | qx) * 2)) & 0x03;
        let table = self.bg_table();
        let out = BgTile {
            lo: self.chr_byte(table, tile, fine_y, false),
            hi: self.chr_byte(table, tile, fine_y, true),
            pal,
            id: BgTileId {
                page: self.chr_page_no[table & 1],
                tile,
                pal,
                fine_y,
                nt: logical,
                coarse_x,
                coarse_y,
                fetched: true,
            },
        };
        // Coarse X increment with nametable wrap (hardware dot 8k+8).
        if *v & 0x001F == 0x001F {
            *v &= !0x001F;
            *v ^= 0x0400;
        } else {
            *v += 1;
        }
        out
    }

    /// Emit background pixels `[cursor, x_to)` of the current line into the
    /// line buffer, fetching tiles in order as they are needed.
    fn emit_bg_until(&mut self, x_to: u16) {
        let x_to = x_to.min(WIDTH as u16);
        if !self.line.started {
            self.line.started = true;
            self.line.cursor = 0;
            self.line.next_fetch = 0;
            self.line.v_start = self.v;
            self.line.fine_x_start = self.fine_x;
            // Tiles 0-1 were prefetched at the end of the previous line.
            let mut v = self.v;
            for k in 0..2usize {
                let t = self.fetch_bg_tile(&mut v);
                self.line.tiles[k] = t;
            }
            self.v = v;
            self.line.next_fetch = 2;
        }
        if self.line.cursor >= x_to {
            return;
        }
        let show_bg = self.show_bg();
        let show_left = self.show_left8_bg();
        let universal = self.apply_gray(self.backdrop_entry());
        let fine_x = u16::from(self.fine_x);
        let mut v = self.v;
        let mut x = self.line.cursor;
        while x < x_to {
            let k = (x + fine_x) >> 3;
            while self.line.next_fetch <= k && self.line.next_fetch < BG_FETCHES_PER_LINE {
                let t = self.fetch_bg_tile(&mut v);
                self.line.tiles[self.line.next_fetch as usize] = t;
                self.line.next_fetch += 1;
            }
            let tile = self.line.tiles[(k as usize).min(BG_FETCHES_PER_LINE as usize - 1)];
            let sub_x = ((x + fine_x) & 7) as u8;
            let bit = 7 - sub_x;
            let sub = ((tile.lo >> bit) & 1) | (((tile.hi >> bit) & 1) << 1);
            let xi = x as usize;
            if !show_bg || (x < 8 && !show_left) || sub == 0 {
                self.line.pixels[xi] = universal;
                self.line.opaque[xi] = false;
            } else {
                let index = self.palette_entry(usize::from(tile.pal) * 4 + usize::from(sub));
                self.line.pixels[xi] = self.apply_gray(index);
                self.line.opaque[xi] = true;
            }
            x += 1;
        }
        self.v = v;
        self.line.cursor = x_to;
    }

    /// Complete the current line: emit the remaining background, overlay
    /// sprites, commit to the frame, then apply the end-of-line `v`
    /// increments (dot 256 vertical increment, dot 257 horizontal reload).
    fn finish_line(&mut self) {
        let line = self.next_line as u8;
        self.emit_bg_until(WIDTH as u16);
        // Remaining hardware fetches of the line (tiles up to 33) still
        // advance `v` even though their pixels never show.
        let mut v = self.v;
        while self.line.next_fetch < BG_FETCHES_PER_LINE {
            // Kept in the line buffer for the record only; no pixel reads it.
            let t = self.fetch_bg_tile(&mut v);
            self.line.tiles[self.line.next_fetch as usize] = t;
            self.line.next_fetch += 1;
        }
        self.v = v;
        let mut out = self.line.pixels;
        let opaque = self.line.opaque;
        // Taken out of `self` so the sprite pass can borrow `self` immutably
        // while pushing into the record's sprite list.
        let mut rec = self.record.take();
        let sprite_start = rec.as_ref().map_or(0, |r| r.sprites.len());
        let flags = if self.show_sprites() {
            crate::render::render_sprites(
                self,
                line,
                &mut out,
                &opaque,
                true,
                rec.as_mut().map(|r| &mut r.sprites),
            )
        } else {
            crate::render::LineFlags::default()
        };
        let start = usize::from(line) * WIDTH;
        self.frame_buf[start..start + WIDTH].copy_from_slice(&out);
        if let Some(r) = rec.as_mut() {
            self.record_line(r, line, sprite_start, flags.overflow);
        }
        self.record = rec;
        if let Some(x) = flags.hit_x {
            if self.hit.is_none() {
                self.hit = Some((line, x));
                self.trace_event(PpuEventKind::Hit(line, x));
            }
        }
        if flags.overflow {
            self.ppustatus |= PPUSTATUS_OVERFLOW;
        }
        if self.rendering_enabled() {
            self.inc_y();
            self.copy_h();
        }
        self.next_line += 1;
        self.line.reset();
        self.probe_cache = None;
    }

    /// Commit line `line` into the record `r` (state snapshot at commit).
    fn record_line(&self, r: &mut FrameRecord, line: u8, sprite_start: usize, overflow: bool) {
        let l: &mut LineRecord = &mut r.lines[usize::from(line)];
        l.valid = true;
        l.v_start = self.line.v_start;
        l.fine_x = self.line.fine_x_start;
        l.fine_x_end = self.fine_x;
        l.ctrl = self.ppuctrl;
        l.mask = self.ppumask;
        l.chr_page = self.chr_page_no;
        l.palette = self.palette_ram;
        l.backdrop = self.apply_gray(self.backdrop_entry());
        l.split = self.line.split;
        l.pixel_split = self.line.pixel_split;
        for (dst, t) in l.tiles.iter_mut().zip(self.line.tiles.iter()) {
            *dst = t.id;
        }
        let count = r.sprites.len().saturating_sub(sprite_start);
        l.sprite_start = sprite_start.min(usize::from(u16::MAX)) as u16;
        l.sprite_len = count.min(usize::from(u8::MAX)) as u8;
        l.sprite_overflow = overflow;
        r.lines_done = r.lines_done.saturating_add(1);
    }

    /// Backdrop colour for pixels with no opaque background: palette entry
    /// 0, or — with rendering disabled and `v` pointing into palette RAM —
    /// the entry `v` selects (the hardware "background palette hijack",
    /// visible as a solid colour while a game blanks the screen after a
    /// palette upload).
    #[must_use]
    pub fn backdrop_entry(&self) -> u8 {
        if !self.rendering_enabled() && (self.v & 0x3F00) == 0x3F00 {
            self.palette_entry((self.v & 0x1F) as usize)
        } else {
            self.palette_entry(0)
        }
    }

    /// Greyscale bit: indices collapse to the `$x0` column.
    pub(crate) fn apply_gray(&self, index: u8) -> u8 {
        if self.grayscale() {
            index & 0x30
        } else {
            index
        }
    }

    /// Dry-run background opacity for the *rest* of the current line from
    /// the live fetch state (pixels already emitted keep their opacity).
    /// Used by the sprite-0 probe; mutates nothing.
    pub(crate) fn probe_bg_opaque(&self) -> [bool; WIDTH] {
        let mut opaque = [false; WIDTH];
        let mut v = self.v;
        let mut next_fetch = self.line.next_fetch;
        let mut tiles = self.line.tiles;
        let started = self.line.started;
        let cursor = if started { self.line.cursor } else { 0 };
        opaque[..cursor as usize].copy_from_slice(&self.line.opaque[..cursor as usize]);
        if !started {
            for slot in tiles.iter_mut().take(2) {
                *slot = self.fetch_bg_tile(&mut v);
            }
            next_fetch = 2;
        }
        if !self.show_bg() {
            return opaque;
        }
        let show_left = self.show_left8_bg();
        let fine_x = u16::from(self.fine_x);
        for x in cursor..WIDTH as u16 {
            let k = (x + fine_x) >> 3;
            while next_fetch <= k && next_fetch < BG_FETCHES_PER_LINE {
                tiles[next_fetch as usize] = self.fetch_bg_tile(&mut v);
                next_fetch += 1;
            }
            let tile = tiles[(k as usize).min(BG_FETCHES_PER_LINE as usize - 1)];
            let bit = 7 - ((x + fine_x) & 7) as u8;
            let sub = ((tile.lo >> bit) & 1) | (((tile.hi >> bit) & 1) << 1);
            opaque[x as usize] = sub != 0 && (x >= 8 || show_left);
        }
        opaque
    }

    /// Complete the frame: render every remaining visible scanline with the
    /// current state and return the finished framebuffer. The next
    /// [`Ppu::set_beam`] past the pre-render line starts a new frame.
    /// Does not touch the vblank flag (see [`Ppu::end_frame`]).
    pub fn finish_frame(&mut self) -> IndexedFrame {
        if self.next_line == PRERENDER_LINE {
            self.start_visible_frame();
        }
        while self.next_line < HEIGHT as i16 {
            self.finish_line();
        }
        self.next_line = PRERENDER_LINE;
        self.beam_line = VBLANK_LINE;
        self.beam_dot = 0;
        self.probe_cache = None;
        if self.record.is_some() {
            std::mem::swap(&mut self.record, &mut self.record_done);
            if let Some(r) = self.record.as_mut() {
                r.clear();
            }
        }
        self.trace_event(PpuEventKind::FrameEnd);
        *self.frame_buf
    }

    /// Whole-frame convenience: finish the frame with the current state and
    /// raise vblank (tests / frame-granular callers). Equivalent to a frame
    /// during which no register changed.
    pub fn render_frame(&mut self) -> IndexedFrame {
        let f = self.finish_frame();
        self.end_frame();
        f
    }

    /// Number of visible scanlines already rendered for the frame in
    /// progress (0 when the pre-render copy is still pending).
    #[must_use]
    pub fn lines_rendered(&self) -> u16 {
        self.next_line.max(0) as u16
    }

    /// Whether the beam is on a rendering scanline (pre-render or visible)
    /// with rendering enabled — the condition for the `$2007` "increment
    /// both" quirk.
    fn beam_in_render(&self) -> bool {
        self.rendering_enabled()
            && self.beam_line >= PRERENDER_LINE
            && self.beam_line < HEIGHT as i16
    }

    // -- loopy register helpers ---------------------------------------------------

    /// Current VRAM address / scroll register `v`.
    #[must_use]
    pub fn v(&self) -> u16 {
        self.v
    }

    /// Temporary VRAM address `t`.
    #[must_use]
    pub fn t(&self) -> u16 {
        self.t
    }

    /// Fine X scroll (0-7).
    #[must_use]
    pub fn fine_x(&self) -> u8 {
        self.fine_x
    }

    /// Diagnostics: overwrite `v` (dry-run line renders from a chosen
    /// scroll position).
    pub fn debug_set_v(&mut self, v: u16) {
        self.v = v & 0x7FFF;
    }

    /// Vertical increment of `v` (hardware dot-256 behaviour).
    fn inc_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v += 0x1000;
        } else {
            self.v &= !0x7000;
            let mut y = (self.v >> 5) & 0x1F;
            if y == 29 {
                y = 0;
                self.v ^= 0x0800;
            } else if y == 31 {
                y = 0;
            } else {
                y += 1;
            }
            self.v = (self.v & !0x03E0) | (y << 5);
        }
    }

    /// Coarse-X increment of `v` with nametable wrap.
    fn inc_coarse_x(&mut self) {
        if self.v & 0x001F == 0x001F {
            self.v &= !0x001F;
            self.v ^= 0x0400;
        } else {
            self.v += 1;
        }
    }

    /// Horizontal reload of `v` from `t` (hardware dot-257 behaviour).
    fn copy_h(&mut self) {
        self.v = (self.v & !0x041F) | (self.t & 0x041F);
    }

    /// Whether background or sprites are enabled (`PPUMASK` bits 3-4).
    #[must_use]
    pub fn rendering_enabled(&self) -> bool {
        self.ppumask & (PPUMASK_SHOW_BG | PPUMASK_SHOW_SPRITES) != 0
    }

    /// True when `PPUCTRL` bit 7 allows an NMI at vblank start.
    #[must_use]
    pub fn nmi_enabled(&self) -> bool {
        self.ppuctrl & PPUCTRL_NMI != 0
    }

    /// True when `PPUCTRL` bit 5 selects 8x16 sprites (else 8x8).
    #[must_use]
    pub fn sprite_tall(&self) -> bool {
        self.ppuctrl & PPUCTRL_TALL_SPRITES != 0
    }

    /// Sprite pixel height (8 or 16).
    #[must_use]
    pub fn sprite_height(&self) -> u16 {
        if self.sprite_tall() {
            16
        } else {
            8
        }
    }

    /// Background pattern-table base (`0=$0000`, `1=$1000`, `PPUCTRL` bit 4).
    #[must_use]
    pub fn bg_table(&self) -> usize {
        usize::from(self.ppuctrl & PPUCTRL_BG_TABLE != 0)
    }

    /// 8x8 sprite pattern-table base (`PPUCTRL` bit 3; ignored in 8x16 mode,
    /// where OAM bit 0 picks the table).
    #[must_use]
    pub fn sprite_table(&self) -> usize {
        usize::from(self.ppuctrl & PPUCTRL_SPRITE_TABLE != 0)
    }

    /// Base nametable from `PPUCTRL` bits 1-0.
    #[must_use]
    pub fn base_nametable(&self) -> u8 {
        self.ppuctrl & 0x03
    }

    /// `$2007` address step: 32 when `PPUCTRL` bit 2 set, else 1.
    #[must_use]
    pub fn vram_step(&self) -> u16 {
        if self.ppuctrl & PPUCTRL_STEP_32 != 0 {
            32
        } else {
            1
        }
    }

    /// Whether backgrounds / sprites render at all (`PPUMASK` bits 3-4).
    #[must_use]
    pub fn show_bg(&self) -> bool {
        self.ppumask & PPUMASK_SHOW_BG != 0
    }
    /// Whether sprites render at all.
    #[must_use]
    pub fn show_sprites(&self) -> bool {
        self.ppumask & PPUMASK_SHOW_SPRITES != 0
    }
    /// Whether the leftmost 8 background pixels render.
    #[must_use]
    pub fn show_left8_bg(&self) -> bool {
        self.ppumask & PPUMASK_SHOW_LEFT_BG != 0
    }
    /// Whether the leftmost 8 sprite pixels render.
    #[must_use]
    pub fn show_left8_sprites(&self) -> bool {
        self.ppumask & PPUMASK_SHOW_LEFT_SPRITES != 0
    }
    /// Greyscale bit (`PPUMASK` bit 0): rendered indices are `& $30`.
    #[must_use]
    pub fn grayscale(&self) -> bool {
        self.ppumask & PPUMASK_GRAYSCALE != 0
    }
    /// Emphasis bits (`PPUMASK` bits 7-5). Display-only; the indexed frame is
    /// unaffected.
    #[must_use]
    pub fn emphasis(&self) -> u8 {
        (self.ppumask >> 5) & 0x07
    }

    // -- mirroring ---------------------------------------------------------

    /// Set the MMC1 mirroring mode directly.
    pub fn set_mirroring(&mut self, mirroring: Mirroring) {
        self.mirroring = mirroring;
        self.probe_cache = None;
        self.note_fetch_change();
    }

    /// Record a mapper change in the trace (diagnostics).
    pub fn trace_mapper(&mut self, ctrl: u8, page0: u8, page1: u8) {
        self.trace_event(PpuEventKind::Mapper(ctrl, page0, page1));
    }

    /// Set mirroring from an MMC1 control-register value (bits 1-0).
    pub fn set_mirroring_mmc1(&mut self, ctrl: u8) {
        self.set_mirroring(Mirroring::from_mmc1_ctrl(ctrl));
    }

    /// Current mirroring mode.
    #[must_use]
    pub fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    // -- nametables --------------------------------------------------------

    /// Read a byte from `$2000-$3FFF` through the mirroring map (the address
    /// is masked to 12 bits, so `$3000-$3FFF` mirrors work).
    #[must_use]
    pub fn nt_read(&self, addr: u16) -> u8 {
        let off = (addr as usize) & 0x0FFF;
        let (phys, inner) = (self.mirroring.physical(off / 0x400), off % 0x400);
        self.nt[phys][inner]
    }

    /// Write a byte through the mirroring map (see [`Ppu::nt_read`]).
    pub fn nt_write(&mut self, addr: u16, value: u8) {
        let off = (addr as usize) & 0x0FFF;
        let (phys, inner) = (self.mirroring.physical(off / 0x400), off % 0x400);
        self.nt[phys][inner] = value;
        self.probe_cache = None;
        self.note_fetch_change();
    }

    /// Byte from a *logical* slot (`0=$2000 .. 3=$2C00`) at 0-1023.
    #[must_use]
    pub(crate) fn nt_byte(&self, logical: u8, offset: usize) -> u8 {
        let phys = self.mirroring.physical((logical & 0x03) as usize);
        self.nt[phys][offset & 0x03FF]
    }

    /// Set one background tile in a logical nametable (`col`/`row` 0-31/0-29).
    pub fn set_tile(&mut self, logical_nt: u8, col: u8, row: u8, tile: u8) {
        let addr =
            0x2000 + (u16::from(logical_nt & 0x03) << 10) + u16::from(row) * 32 + u16::from(col);
        self.nt_write(addr, tile);
    }

    /// Set one 2-bit palette selector in an attribute byte: `attr_col` /
    /// `attr_row` 0-7 pick the byte, `qx` / `qy` pick the 16x16 quadrant.
    pub fn set_attr_quad(
        &mut self,
        logical_nt: u8,
        attr_col: u8,
        attr_row: u8,
        qx: u8,
        qy: u8,
        pal: u8,
    ) {
        let addr = 0x2000
            + (u16::from(logical_nt & 0x03) << 10)
            + ATTR_OFFSET as u16
            + u16::from(attr_row & 0x07) * 8
            + u16::from(attr_col & 0x07);
        let shift = (((qy & 1) << 1) | (qx & 1)) * 2;
        let cur = self.nt_read(addr) & !(0x03 << shift);
        self.nt_write(addr, cur | ((pal & 0x03) << shift));
    }

    // -- CHR ---------------------------------------------------------------

    /// Load one 4 KiB CHR page from the extracted assets into pattern-table
    /// `slot` (`0=$0000-$0FFF`, `1=$1000-$1FFF`). `asset_page` (0-31) records
    /// which CHR page this came from for provenance.
    pub fn load_chr_4k(
        &mut self,
        slot: usize,
        asset_page: u8,
        data: &[u8],
    ) -> Result<(), ChrError> {
        if slot > 1 {
            return Err(ChrError::BadSlot { slot });
        }
        if data.len() != CHR_BANK_LEN {
            return Err(ChrError::BadLength { got: data.len() });
        }
        self.chr[slot].copy_from_slice(data);
        self.chr_page_no[slot] = asset_page;
        self.probe_cache = None;
        self.note_fetch_change();
        Ok(())
    }

    /// Asset CHR page backing `slot`, or `u8::MAX` when unset.
    #[must_use]
    pub fn chr_page_no(&self, slot: usize) -> u8 {
        self.chr_page_no[slot & 1]
    }

    /// Borrow one loaded 4 KiB pattern-table slot (diagnostics / oracle diffs).
    #[must_use]
    pub fn chr_slot(&self, slot: usize) -> &[u8; CHR_BANK_LEN] {
        &self.chr[slot & 1]
    }

    /// Raw pattern-table byte: `table` 0/1, `tile` 0-255, `row` 0-7,
    /// `hi` selects bit plane 1.
    #[must_use]
    pub(crate) fn chr_byte(&self, table: usize, tile: u8, row: u8, hi: bool) -> u8 {
        let base = (tile as usize) * 16 + (row as usize & 0x07);
        self.chr[table & 1][base + if hi { 8 } else { 0 }]
    }

    // -- OAM -----------------------------------------------------------------

    /// Decode OAM entry `i` (0-63, masked).
    #[must_use]
    pub fn oam_entry(&self, i: usize) -> OamEntry {
        let o = &self.oam[(i & 63) * 4..];
        OamEntry {
            y: o[0],
            tile: o[1],
            attr: o[2],
            x: o[3],
        }
    }

    /// Store OAM entry `i` (0-63, masked).
    pub fn set_oam_entry(&mut self, i: usize, e: OamEntry) {
        let o = &mut self.oam[(i & 63) * 4..][..4];
        o[0] = e.y;
        o[1] = e.tile;
        o[2] = e.attr;
        o[3] = e.x;
        self.probe_cache = None;
    }

    /// `$4014` DMA: copy a full 256-byte page into OAM.
    pub fn oam_dma(&mut self, page: &[u8; OAM_LEN]) {
        self.oam.copy_from_slice(page);
        self.probe_cache = None;
    }

    /// Raw OAM bytes (e.g. for oracle comparison).
    #[must_use]
    pub fn oam(&self) -> &[u8; OAM_LEN] {
        &self.oam
    }

    /// `$2003`: set the OAM address.
    pub fn write_oam_addr(&mut self, addr: u8) {
        self.oam_addr = addr;
    }

    /// `$2004`: write one OAM byte, auto-incrementing the address.
    pub fn write_oam_data(&mut self, value: u8) {
        self.oam[self.oam_addr as usize] = value;
        self.oam_addr = self.oam_addr.wrapping_add(1);
        self.probe_cache = None;
    }

    // -- palette RAM ---------------------------------------------------------

    /// Fold a `$3F00-$3FFF` address to a 0-31 palette-RAM index, applying the
    /// `$3F10/$14/$18/$1C -> $3F00/$04/$08/$0C` mirrors.
    const fn palette_index(addr: u16) -> usize {
        let mut i = (addr as usize) & 0x1F;
        if i & 0x13 == 0x10 {
            i -= 0x10;
        }
        i
    }

    /// Write palette RAM (value masked to 6 bits, as on hardware). The
    /// four backdrop entries are shared between the background and sprite
    /// halves, so both raw cells are kept equal (the raw 32-byte view then
    /// reads like the oracle's `$3F00-$3F1F` peek).
    pub fn palette_write(&mut self, addr: u16, value: u8) {
        let i = Self::palette_index(addr);
        self.palette_ram[i] = value & 0x3F;
        if i & 0x03 == 0 {
            self.palette_ram[i | 0x10] = value & 0x3F;
        }
        self.probe_cache = None;
        self.note_pixel_change();
    }

    /// Read palette RAM through the mirror map.
    #[must_use]
    pub fn palette_read(&self, addr: u16) -> u8 {
        self.palette_ram[Self::palette_index(addr)]
    }

    /// Raw palette entry 0-31 with mirror folding (for the renderer/tests).
    #[must_use]
    pub fn palette_entry(&self, i: usize) -> u8 {
        let mut idx = i & 0x1F;
        if idx & 0x13 == 0x10 {
            idx -= 0x10;
        }
        self.palette_ram[idx]
    }

    /// Set palette entry 0-31 (masked to 6 bits, mirrors folded).
    pub fn set_palette(&mut self, i: usize, value: u8) {
        let at = 0x3F00 + (i as u16 & 0x1F);
        self.palette_write(at, value);
    }

    /// Raw palette RAM (e.g. for oracle comparison).
    #[must_use]
    pub fn palette(&self) -> &[u8; PALETTE_LEN] {
        &self.palette_ram
    }

    // -- scroll / address latch / data port ----------------------------------

    /// `$2005`: first write sets coarse/fine X in `t`/`x`, second write sets
    /// coarse/fine Y in `t`.
    pub fn write_scroll(&mut self, value: u8) {
        let first = !self.w;
        self.trace_event(PpuEventKind::Scroll(value, first));
        if !self.w {
            self.t = (self.t & !0x001F) | u16::from(value >> 3);
            self.fine_x = value & 0x07;
        } else {
            self.t =
                (self.t & !0x73E0) | (u16::from(value & 0x07) << 12) | (u16::from(value >> 3) << 5);
        }
        self.w = !self.w;
        self.probe_cache = None;
        self.note_fetch_change();
    }

    /// Scroll `(x, y)` implied by `t`/fine `x` (what the last `$2005` pair
    /// and `$2000` base bits selected; diagnostics/tests).
    #[must_use]
    pub fn scroll(&self) -> (u8, u8) {
        let x = (((self.t & 0x1F) << 3) as u8) | self.fine_x;
        let y = ((((self.t >> 5) & 0x1F) << 3) | ((self.t >> 12) & 0x07)) as u8;
        (x, y)
    }

    /// `$2006`: first write loads the high 6 bits of `t` (bit 14 cleared),
    /// second loads the low byte and copies `t` into `v`.
    pub fn write_addr(&mut self, value: u8) {
        if !self.w {
            self.t = (self.t & 0x00FF) | (u16::from(value & 0x3F) << 8);
        } else {
            self.t = (self.t & 0xFF00) | u16::from(value);
            self.v = self.t;
        }
        let first = !self.w;
        self.w = !self.w;
        self.probe_cache = None;
        self.note_fetch_change();
        let v = self.v;
        self.trace_event(PpuEventKind::Addr(value, first, v));
    }

    /// Current VRAM address `v` (for tests/wiring).
    #[must_use]
    pub fn vram_addr(&self) -> u16 {
        self.v & 0x3FFF
    }

    /// Post-access `v` step: `+1`/`+32` outside rendering, the hardware
    /// "increment coarse X and Y" glitch while the beam is rendering.
    fn step_vram_addr(&mut self) {
        if self.beam_in_render() {
            self.inc_coarse_x();
            self.inc_y();
        } else {
            self.v = (self.v + self.vram_step()) & 0x7FFF;
        }
        self.probe_cache = None;
    }

    /// `$2007`: write through the VRAM address, then step it.
    pub fn write_data(&mut self, value: u8) {
        let v0 = self.v;
        self.trace_event(PpuEventKind::DataWrite(value, v0));
        match self.v & 0x3FFF {
            0x0000..=0x1FFF => { /* CHR-ROM: ignored */ }
            0x2000..=0x3EFF => self.nt_write(self.v & 0x3FFF, value),
            _ => self.palette_write(self.v & 0x3FFF, value),
        }
        self.step_vram_addr();
    }

    /// `$2007`: buffered read (palette reads return immediately).
    #[must_use]
    pub fn read_data(&mut self) -> u8 {
        let addr = self.v & 0x3FFF;
        self.trace_event(PpuEventKind::DataRead(addr));
        let out = match addr {
            0x0000..=0x1FFF => {
                let b = self.ppu_data_buf;
                let table = usize::from(addr >= 0x1000);
                self.ppu_data_buf = self.chr[table][(addr as usize) & 0x0FFF];
                b
            }
            0x2000..=0x3EFF => {
                let b = self.ppu_data_buf;
                self.ppu_data_buf = self.nt_read(addr);
                b
            }
            _ => self.palette_read(addr),
        };
        self.step_vram_addr();
        out
    }

    // -- control / mask / status ----------------------------------------------

    /// `$2000`: full control register; bits 1-0 also land in `t` bits 11-10.
    pub fn write_ctrl(&mut self, value: u8) {
        self.trace_event(PpuEventKind::Ctrl(value));
        self.ppuctrl = value;
        self.t = (self.t & !0x0C00) | (u16::from(value & 0x03) << 10);
        self.probe_cache = None;
        self.note_fetch_change();
    }

    /// `$2000` value (for tests/wiring).
    #[must_use]
    pub fn ctrl(&self) -> u8 {
        self.ppuctrl
    }

    /// `$2001`: mask + emphasis bits.
    pub fn write_mask(&mut self, value: u8) {
        self.trace_event(PpuEventKind::Mask(value));
        self.ppumask = value;
        self.probe_cache = None;
        // `$2001` gates both the pixel path and (via rendering-enabled) fetches.
        self.note_pixel_change();
        self.note_fetch_change();
    }

    /// `$2001` value.
    #[must_use]
    pub fn mask(&self) -> u8 {
        self.ppumask
    }

    /// Sprite-0 hit as the beam sees it now: a hit already rendered (or on
    /// the current line, once the beam is past its pixel) — the dry-run
    /// probe renders the current line ahead of time when needed.
    fn hit_now(&mut self) -> bool {
        if let Some((l, x)) = self.hit {
            let l = i16::from(l);
            return l < self.beam_line
                || (l == self.beam_line && self.beam_dot >= u16::from(x) + SPRITE0_HIT_LATENCY)
                || self.beam_line >= HEIGHT as i16;
        }
        if self.beam_line < 0 || self.beam_line >= HEIGHT as i16 {
            return false;
        }
        if self.next_line != self.beam_line {
            // Line already committed (beam past dot 256) or pre-render pending.
            return false;
        }
        let line = self.beam_line as u8;
        let probe = match self.probe_cache {
            Some((l, r)) if l == line => r,
            _ => {
                let r = crate::render::probe_sprite0_hit(self, line);
                self.probe_cache = Some((line, r));
                r
            }
        };
        match probe {
            Some(x) if self.beam_dot >= u16::from(x) + SPRITE0_HIT_LATENCY => {
                self.hit = Some((line, x));
                true
            }
            _ => false,
        }
    }

    /// `$2002`: read status (bits 7-5 only; lower bits are open bus on
    /// hardware and read back 0 here). Side effects: clears vblank and resets
    /// the `$2005`/`$2006` toggle, like hardware.
    pub fn read_status(&mut self) -> u8 {
        let hit = self.hit_now();
        let mut out = self.ppustatus & (PPUSTATUS_VBLANK | PPUSTATUS_OVERFLOW);
        if hit {
            out |= PPUSTATUS_SPRITE0;
        }
        self.ppustatus &= !PPUSTATUS_VBLANK;
        self.w = false;
        self.trace_event(PpuEventKind::Status(out));
        out
    }

    /// Raw status byte including flag state (for tests): sprite-0 reflects
    /// the beam-relative answer without the read side effects.
    #[must_use]
    pub fn status(&self) -> u8 {
        let mut s = self.ppustatus & (PPUSTATUS_VBLANK | PPUSTATUS_OVERFLOW);
        if self.sprite0_hit() {
            s |= PPUSTATUS_SPRITE0;
        }
        s
    }

    /// True when the vblank flag is set.
    #[must_use]
    pub fn vblank(&self) -> bool {
        self.ppustatus & PPUSTATUS_VBLANK != 0
    }

    /// True when the sprite-0 hit flag is set for the beam position (no
    /// probe: a hit on the not-yet-rendered current line only becomes
    /// visible through [`Ppu::read_status`]).
    #[must_use]
    pub fn sprite0_hit(&self) -> bool {
        match self.hit {
            Some((l, x)) => {
                let l = i16::from(l);
                l < self.beam_line
                    || (l == self.beam_line && self.beam_dot >= u16::from(x) + SPRITE0_HIT_LATENCY)
                    || self.beam_line >= HEIGHT as i16
            }
            None => false,
        }
    }

    /// Scanline and x of the frame's first sprite-0 hit, if rendered so far.
    #[must_use]
    pub fn sprite0_hit_at(&self) -> Option<(u8, u8)> {
        self.hit
    }

    /// True when the sprite-overflow flag is set.
    #[must_use]
    pub fn sprite_overflow(&self) -> bool {
        self.ppustatus & PPUSTATUS_OVERFLOW != 0
    }

    // -- misc ------------------------------------------------------------------

    /// Per-scanline sprite policy (default [`SpriteLimit::Faithful8`]).
    #[must_use]
    pub fn sprite_limit(&self) -> SpriteLimit {
        self.sprite_limit
    }

    /// Change the per-scanline sprite policy.
    pub fn set_sprite_limit(&mut self, limit: SpriteLimit) {
        self.sprite_limit = limit;
    }

    // -- save states ------------------------------------------------------------

    /// Copy every piece of emulation state from `src` into `self` without
    /// allocating (save-state snapshots and rollback restores).
    ///
    /// Copied: memories (nametables, both loaded CHR slots and their page
    /// numbers, OAM, palette RAM), registers and latches (`v`/`t`/fine X/`w`,
    /// read buffer, OAM address, status), mirroring, sprite limit, the frame
    /// under construction, the beam and next-line cursors, the sprite-0 hit
    /// and its probe memo, and the current line's background pipeline.
    ///
    /// Not copied: the event trace and its enable flag (diagnostics), and
    /// the render record (presentation for widescreen / HD consumers; it
    /// never feeds back into rendering). The destination keeps its own record
    /// enablement; its record buffers are cleared, so a frontend presents the
    /// record only after the next completed frame.
    pub fn copy_state_from(&mut self, src: &Ppu) {
        self.nt = src.nt;
        self.mirroring = src.mirroring;
        self.chr = src.chr;
        self.chr_page_no = src.chr_page_no;
        self.oam = src.oam;
        self.oam_addr = src.oam_addr;
        self.palette_ram = src.palette_ram;
        self.ppuctrl = src.ppuctrl;
        self.ppumask = src.ppumask;
        self.ppustatus = src.ppustatus;
        self.v = src.v;
        self.t = src.t;
        self.fine_x = src.fine_x;
        self.w = src.w;
        self.ppu_data_buf = src.ppu_data_buf;
        self.sprite_limit = src.sprite_limit;
        *self.frame_buf = *src.frame_buf;
        self.next_line = src.next_line;
        self.beam_line = src.beam_line;
        self.beam_dot = src.beam_dot;
        self.hit = src.hit;
        self.probe_cache = src.probe_cache;
        self.line.clone_from(&src.line);
        if let Some(r) = self.record.as_mut() {
            r.clear();
        }
        if let Some(r) = self.record_done.as_mut() {
            r.clear();
        }
    }

    /// Serialized size of [`Ppu::write_state`] in bytes.
    pub const STATE_BYTES: usize = 2 * NAMETABLE_LEN
        + 1
        + 2 * CHR_BANK_LEN
        + 2
        + OAM_LEN
        + 1
        + PALETTE_LEN
        + 3
        + 4
        + 1
        + 1
        + 1
        + 1
        + WIDTH * HEIGHT
        + 2
        + 2
        + 2
        + 3
        + 4
        + LINE_STATE_BYTES;

    /// Append the state [`Ppu::copy_state_from`] copies to `out`
    /// (little-endian, fixed length [`Ppu::STATE_BYTES`]).
    pub fn write_state(&self, out: &mut Vec<u8>) {
        let start = out.len();
        for t in &self.nt {
            out.extend_from_slice(t);
        }
        out.push(self.mirroring as u8);
        for c in &self.chr {
            out.extend_from_slice(c);
        }
        out.extend_from_slice(&self.chr_page_no);
        out.extend_from_slice(&self.oam);
        out.push(self.oam_addr);
        out.extend_from_slice(&self.palette_ram);
        out.extend_from_slice(&[self.ppuctrl, self.ppumask, self.ppustatus]);
        out.extend_from_slice(&self.v.to_le_bytes());
        out.extend_from_slice(&self.t.to_le_bytes());
        out.push(self.fine_x);
        out.push(u8::from(self.w));
        out.push(self.ppu_data_buf);
        out.push(match self.sprite_limit {
            SpriteLimit::Faithful8 => 0,
            SpriteLimit::Unlimited => 1,
        });
        out.extend_from_slice(&self.frame_buf[..]);
        out.extend_from_slice(&self.next_line.to_le_bytes());
        out.extend_from_slice(&self.beam_line.to_le_bytes());
        out.extend_from_slice(&self.beam_dot.to_le_bytes());
        let (hs, hl, hx) = match self.hit {
            Some((l, x)) => (1, l, x),
            None => (0, 0, 0),
        };
        out.extend_from_slice(&[hs, hl, hx]);
        let pc = match self.probe_cache {
            None => [0, 0, 0, 0],
            Some((l, None)) => [1, l, 0, 0],
            Some((l, Some(x))) => [1, l, 1, x],
        };
        out.extend_from_slice(&pc);
        let lb = &self.line;
        for (p, o) in lb.pixels.iter().zip(lb.opaque.iter()) {
            out.push(*p);
            out.push(u8::from(*o));
        }
        for t in &lb.tiles {
            let id = &t.id;
            out.extend_from_slice(&[
                t.lo,
                t.hi,
                t.pal,
                id.page,
                id.tile,
                id.pal,
                id.fine_y,
                id.nt,
                id.coarse_x,
                id.coarse_y,
                u8::from(id.fetched),
            ]);
        }
        out.extend_from_slice(&lb.cursor.to_le_bytes());
        out.extend_from_slice(&lb.next_fetch.to_le_bytes());
        out.push(u8::from(lb.started));
        out.extend_from_slice(&lb.v_start.to_le_bytes());
        out.push(lb.fine_x_start);
        out.push(u8::from(lb.split));
        out.push(u8::from(lb.pixel_split));
        debug_assert_eq!(out.len() - start, Self::STATE_BYTES);
    }

    /// Inverse of [`Ppu::write_state`]: `bytes` must be exactly
    /// [`Ppu::STATE_BYTES`] long. On error `self` is left unchanged. The
    /// trace and render record are handled as in [`Ppu::copy_state_from`].
    pub fn read_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        if bytes.len() != Self::STATE_BYTES {
            return Err(StateError::Length {
                got: bytes.len(),
                want: Self::STATE_BYTES,
            });
        }
        let mut r = Reader(bytes);
        let mut n = Ppu::new();
        for t in &mut n.nt {
            t.copy_from_slice(r.take(NAMETABLE_LEN));
        }
        n.mirroring = Mirroring::from_mmc1_ctrl(r.u8());
        for c in &mut n.chr {
            c.copy_from_slice(r.take(CHR_BANK_LEN));
        }
        n.chr_page_no.copy_from_slice(r.take(2));
        n.oam.copy_from_slice(r.take(OAM_LEN));
        n.oam_addr = r.u8();
        n.palette_ram.copy_from_slice(r.take(PALETTE_LEN));
        n.ppuctrl = r.u8();
        n.ppumask = r.u8();
        n.ppustatus = r.u8();
        n.v = r.u16();
        n.t = r.u16();
        n.fine_x = r.u8();
        n.w = r.bool()?;
        n.ppu_data_buf = r.u8();
        n.sprite_limit = match r.u8() {
            0 => SpriteLimit::Faithful8,
            1 => SpriteLimit::Unlimited,
            _ => return Err(StateError::Invalid("sprite limit")),
        };
        n.frame_buf.copy_from_slice(r.take(WIDTH * HEIGHT));
        n.next_line = r.u16() as i16;
        n.beam_line = r.u16() as i16;
        n.beam_dot = r.u16();
        let (hs, hl, hx) = (r.bool()?, r.u8(), r.u8());
        n.hit = hs.then_some((hl, hx));
        let (ps, pl, pxs, px) = (r.bool()?, r.u8(), r.bool()?, r.u8());
        n.probe_cache = ps.then_some((pl, pxs.then_some(px)));
        for i in 0..WIDTH {
            n.line.pixels[i] = r.u8();
            n.line.opaque[i] = r.bool()?;
        }
        for t in &mut n.line.tiles {
            t.lo = r.u8();
            t.hi = r.u8();
            t.pal = r.u8();
            t.id.page = r.u8();
            t.id.tile = r.u8();
            t.id.pal = r.u8();
            t.id.fine_y = r.u8();
            t.id.nt = r.u8();
            t.id.coarse_x = r.u8();
            t.id.coarse_y = r.u8();
            t.id.fetched = r.bool()?;
        }
        n.line.cursor = r.u16();
        n.line.next_fetch = r.u16();
        n.line.started = r.bool()?;
        n.line.v_start = r.u16();
        n.line.fine_x_start = r.u8();
        n.line.split = r.bool()?;
        n.line.pixel_split = r.bool()?;
        self.copy_state_from(&n);
        Ok(())
    }
}

/// Serialized size of the current line's background pipeline.
const LINE_STATE_BYTES: usize = 2 * WIDTH + 11 * BG_FETCHES_PER_LINE as usize + 2 + 2 + 1 + 2 + 3;

/// Error from [`Ppu::read_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    /// Wrong byte count.
    Length {
        /// Bytes supplied.
        got: usize,
        /// Bytes expected ([`Ppu::STATE_BYTES`]).
        want: usize,
    },
    /// A field held an out-of-range value.
    Invalid(&'static str),
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StateError::Length { got, want } => {
                write!(f, "PPU state is {got} bytes, expected {want}")
            }
            StateError::Invalid(what) => write!(f, "PPU state has an invalid {what}"),
        }
    }
}

impl std::error::Error for StateError {}

/// Length-checked cursor for [`Ppu::read_state`] (the caller checks the total).
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        head
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u16(&mut self) -> u16 {
        let b = self.take(2);
        u16::from_le_bytes([b[0], b[1]])
    }
    fn bool(&mut self) -> Result<bool, StateError> {
        match self.u8() {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StateError::Invalid("flag")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmc1_mirroring_modes_fold_logical_slots() {
        let mut p = Ppu::new();
        p.set_mirroring_mmc1(2);
        assert_eq!(p.mirroring(), Mirroring::Vertical);
        p.set_mirroring_mmc1(3);
        assert_eq!(p.mirroring(), Mirroring::Horizontal);
        p.set_mirroring_mmc1(0);
        assert_eq!(p.mirroring(), Mirroring::SingleLower);
        p.set_mirroring_mmc1(1);
        assert_eq!(p.mirroring(), Mirroring::SingleUpper);
        // Upper control bits are ignored.
        p.set_mirroring_mmc1(0x80 | 2);
        assert_eq!(p.mirroring(), Mirroring::Vertical);
    }

    #[test]
    fn vertical_mirroring_pairs_slots() {
        let mut p = Ppu::new();
        p.set_mirroring(Mirroring::Vertical);
        p.nt_write(0x2000, 0xAA);
        assert_eq!(p.nt_read(0x2800), 0xAA, "$2800 mirrors $2000");
        p.nt_write(0x2400, 0xBB);
        assert_eq!(p.nt_read(0x2C00), 0xBB, "$2C00 mirrors $2400");
        assert_eq!(p.nt_read(0x3000), p.nt_read(0x2000), "$3000 mirrors $2000");
    }

    #[test]
    fn horizontal_mirroring_pairs_slots() {
        let mut p = Ppu::new();
        p.set_mirroring(Mirroring::Horizontal);
        p.nt_write(0x2000, 0x11);
        assert_eq!(p.nt_read(0x2400), 0x11, "$2400 mirrors $2000");
        p.nt_write(0x2800, 0x22);
        assert_eq!(p.nt_read(0x2C00), 0x22, "$2C00 mirrors $2800");
    }

    #[test]
    fn single_screen_modes_share_one_table() {
        let mut p = Ppu::new();
        p.set_mirroring(Mirroring::SingleLower);
        p.nt_write(0x2000, 0x5A);
        for a in [0x2400, 0x2800, 0x2C00] {
            assert_eq!(p.nt_read(a), 0x5A, "single-lower shares table");
        }
        p.set_mirroring(Mirroring::SingleUpper);
        p.nt_write(0x2000, 0xA5);
        for a in [0x2400, 0x2800, 0x2C00] {
            assert_eq!(p.nt_read(a), 0xA5, "single-upper shares table");
        }
    }

    #[test]
    fn palette_mirrors_fold_to_base_entries() {
        let mut p = Ppu::new();
        p.palette_write(0x3F00, 0x0F);
        for mirror in [0x3F10, 0x3F20, 0x3F30] {
            assert_eq!(p.palette_read(mirror), 0x0F, "mirror of $3F00");
        }
        assert_eq!(
            p.palette()[0x10],
            0x0F,
            "raw view keeps the mirror cell equal"
        );
        p.palette_write(0x3F14, 0x21);
        assert_eq!(p.palette_read(0x3F04), 0x21, "$3F14 mirrors $3F04");
        assert_eq!(p.palette_entry(0x14), 0x21, "entry() folds too");
        assert_eq!(p.palette()[0x04], 0x21);
        assert_eq!(p.palette()[0x14], 0x21);
    }

    #[test]
    fn palette_writes_mask_to_six_bits() {
        let mut p = Ppu::new();
        p.palette_write(0x3F01, 0xFF);
        assert_eq!(p.palette_read(0x3F01), 0x3F);
    }

    #[test]
    fn scroll_latch_takes_two_writes_and_status_resets_it() {
        let mut p = Ppu::new();
        p.write_scroll(100);
        p.write_scroll(200);
        assert_eq!(p.scroll(), (100, 200));
        // A third write starts a new pair (x again).
        p.write_scroll(7);
        assert_eq!(p.scroll(), (7, 200));
        p.write_scroll(8);
        assert_eq!(p.scroll(), (7, 8));
        // Reading $2002 resets the latch: next write is x again.
        p.write_scroll(1);
        let _ = p.read_status();
        p.write_scroll(2);
        assert_eq!(p.scroll(), (2, 8));
    }

    #[test]
    fn ctrl_base_nametable_lands_in_t() {
        let mut p = Ppu::new();
        p.write_ctrl(0x02);
        assert_eq!(p.t() & 0x0C00, 0x0800);
        p.write_ctrl(0x01);
        assert_eq!(p.t() & 0x0C00, 0x0400);
    }

    #[test]
    fn status_read_reports_flags_and_clears_vblank() {
        let mut p = Ppu::new();
        p.end_frame();
        assert!(p.vblank());
        let s = p.read_status();
        assert_eq!(s & PPUSTATUS_VBLANK, PPUSTATUS_VBLANK);
        assert!(!p.vblank(), "$2002 read clears vblank");
        // Lower (open-bus) bits read back 0.
        assert_eq!(s & 0x1F, 0);
    }

    #[test]
    fn vram_addr_latch_and_step_modes() {
        let mut p = Ppu::new();
        p.write_ctrl(0x00); // step 1
        p.write_addr(0x21);
        p.write_addr(0x08);
        assert_eq!(p.vram_addr(), 0x2108);
        p.write_data(0x5A);
        assert_eq!(p.vram_addr(), 0x2109, "step-1 advance");
        assert_eq!(p.nt_read(0x2108), 0x5A);

        p.write_ctrl(PPUCTRL_STEP_32);
        p.write_addr(0x20);
        p.write_addr(0x00);
        p.write_data(0xA5);
        assert_eq!(p.vram_addr(), 0x2020, "step-32 advance");
        assert_eq!(p.nt_read(0x2000), 0xA5);
    }

    #[test]
    fn data_port_increments_both_axes_while_rendering() {
        let mut p = Ppu::new();
        p.write_mask(PPUMASK_SHOW_BG);
        // Beam on visible line 10, rendering on: the "increment both" glitch.
        p.set_beam(-1, 340);
        p.set_beam(10, 100);
        p.write_addr(0x20);
        p.write_addr(0x00);
        assert_eq!(p.v(), 0x2000);
        let _ = p.read_data();
        assert_eq!(p.v(), 0x3001, "coarse X +1 and fine Y +1");
        // Same access in vblank steps normally.
        p.finish_frame();
        p.write_addr(0x20);
        p.write_addr(0x00);
        p.set_beam(241, 0);
        let _ = p.read_data();
        assert_eq!(p.v(), 0x2001);
    }

    #[test]
    fn vram_writes_to_chr_rom_are_ignored() {
        let mut p = Ppu::new();
        p.write_addr(0x00);
        p.write_addr(0x10);
        p.write_data(0xFF);
        assert_eq!(p.chr_byte(0, 1, 0, false), 0, "CHR-ROM unchanged");
    }

    #[test]
    fn oam_entry_and_dma_roundtrip() {
        let mut p = Ppu::new();
        let e = OamEntry {
            y: 0x10,
            tile: 0x42,
            attr: 0x83,
            x: 0x77,
        };
        p.set_oam_entry(7, e);
        assert_eq!(p.oam_entry(7), e);
        assert_eq!(p.oam_entry(7 + 64), e, "index masked to 0-63");
        let mut page = [0u8; OAM_LEN];
        page[3] = 0x99;
        p.oam_dma(&page);
        assert_eq!(p.oam()[3], 0x99);
        assert_eq!(p.oam_entry(0).x, 0x99, "entry 0 x from DMA page");
    }

    #[test]
    fn chr_load_validates_slot_and_length() {
        let mut p = Ppu::new();
        let page = [0x5Au8; CHR_BANK_LEN];
        assert!(p.load_chr_4k(0, 3, &page).is_ok());
        assert_eq!(p.chr_page_no(0), 3);
        assert_eq!(
            p.load_chr_4k(2, 0, &page),
            Err(ChrError::BadSlot { slot: 2 })
        );
        assert_eq!(
            p.load_chr_4k(1, 0, &[0u8; 8]),
            Err(ChrError::BadLength { got: 8 })
        );
        assert_eq!(p.chr_page_no(1), u8::MAX, "failed load leaves slot unset");
    }

    #[test]
    fn inc_y_wraps_row_29_into_next_nametable() {
        let mut p = Ppu::new();
        p.v = (7 << 12) | (29 << 5);
        p.inc_y();
        assert_eq!(p.v, 0x0800, "row 29 -> row 0 of the lower nametable");
        p.v = (7 << 12) | (31 << 5);
        p.inc_y();
        assert_eq!(p.v, 0x0000, "row 31 -> row 0, no toggle");
        p.v = 0x0000;
        p.inc_y();
        assert_eq!(p.v, 0x1000, "fine Y +1");
    }

    #[test]
    fn prerender_copies_t_into_v_only_when_rendering() {
        let mut p = Ppu::new();
        p.write_scroll(16);
        p.write_scroll(8);
        p.set_beam(PRERENDER_LINE, 340);
        assert_eq!(p.v(), 0, "rendering off: no reload");
        let _ = p.finish_frame();
        p.write_mask(PPUMASK_SHOW_BG);
        p.set_beam(PRERENDER_LINE, 340);
        assert_eq!(p.v(), p.t(), "rendering on: v reloaded from t");
        assert_eq!(p.lines_rendered(), 0);
        p.set_beam(5, 0);
        assert_eq!(p.lines_rendered(), 5, "lines before the beam render");
        p.set_beam(5, 300);
        assert_eq!(
            p.lines_rendered(),
            6,
            "past dot 256 commits the current line"
        );
    }

    #[test]
    fn post_render_line_never_fetches() {
        let mut p = Ppu::new();
        p.write_mask(PPUMASK_SHOW_BG);
        p.write_scroll(0);
        p.write_scroll(0);
        p.set_beam(-1, 340);
        p.set_beam(200, 0);
        let _ = p.finish_frame();
        let v0 = p.v();
        // An access on line 240 must not run tile fetches (which would
        // advance coarse X / toggle the nametable bit).
        p.set_beam_access(240, 100, AccessKind::Fetch);
        assert_eq!(p.v(), v0);
        p.set_beam_access(240, 250, AccessKind::Pixel);
        assert_eq!(p.v(), v0);
    }

    #[test]
    fn ctrl_bit_helpers() {
        let mut p = Ppu::new();
        p.write_ctrl(PPUCTRL_NMI | PPUCTRL_TALL_SPRITES | PPUCTRL_BG_TABLE | 0x02);
        assert!(p.nmi_enabled());
        assert!(p.sprite_tall());
        assert_eq!(p.sprite_height(), 16);
        assert_eq!(p.bg_table(), 1);
        assert_eq!(p.sprite_table(), 0);
        assert_eq!(p.base_nametable(), 2);
        assert_eq!(p.vram_step(), 1);
        p.write_ctrl(PPUCTRL_STEP_32 | PPUCTRL_SPRITE_TABLE);
        assert_eq!(p.vram_step(), 32);
        assert_eq!(p.sprite_table(), 1);
        assert!(!p.sprite_tall());
    }
}
