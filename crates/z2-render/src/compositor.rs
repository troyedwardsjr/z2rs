//! RGBA compositor: the indexed frame at integer scale, with HD pack art
//! substituted per tile and sprite.
//!
//! ## Why the indexed frame is the ground truth
//!
//! The PPU already composited the NES image into [`IndexedFrame`], and
//! [`z2_ppu::render_wide_indexed`] does the same for widescreen margins. So
//! the base pass here is a plain nearest-neighbour upscale of that image
//! through the master palette: with no pack, scale-1 output equals
//! `indexed_to_rgba` of the frame byte for byte, and the widescreen centre
//! equals the non-wide composite — both by construction, not by re-deriving
//! pixels and hoping they agree.
//!
//! A line is only re-rendered where HD art applies: background slots whose
//! tile the pack replaces are painted from the pack, and then the line's
//! recorded sprites are replayed over them with the hardware mux. Replaying
//! is necessary because HD background art must land *under* the sprites the
//! flat frame already baked on top.
//!
//! ## Rules
//!
//! * HD alpha decides whether an HD pixel covers; **NES opacity decides
//!   priority** — background/sprite ordering, `behind`, and sprite-vs-sprite
//!   claiming follow `z2_ppu::render::render_sprites` exactly, so HD art never
//!   changes what is in front of what. A NES-transparent sprite pixel stays
//!   transparent (silhouette extension is not a v1 feature).
//! * The variant key for a tile is the three opaque NES colour indices of its
//!   palette group on that line: `palette_entry(pal * 4 + 1..=3)` for
//!   background, `0x10 + pal * 4 + 1..=3` for sprites.
//! * Slot geometry follows [`z2_ppu::wide`]: fetch slot `k` covers window
//!   pixels `[8k - fine_x, 8k - fine_x + 8)`. Window pixels `0..256` always
//!   come from `record.tiles[k]`; margins own only `wx < 0` and `wx >= 256`,
//!   so when `fine_x > 0` two record slots straddle a margin boundary: slot
//!   32 spills its first `fine_x` columns into the window and the rest into
//!   the right margin, and slot 0 spills its first `fine_x` columns into the
//!   *left margin*. Those leading columns are still `record.tiles[0]`, not
//!   `margins.left` — exactly what [`z2_ppu::render_wide_indexed`] draws
//!   there — so the HD pass must substitute them from the same tile.
//! * Widescreen right-edge recovery ([`Margins::fill_right_clip`]) applies to
//!   the HD composite too: on a line the ROM masks with its opaque sprite
//!   column at x 248 ([`z2_ppu::wide::right_edge_masked`]), the wide indexed
//!   frame already shows the background there, so the sprite replay leaves
//!   window pixels 248..256 alone and they keep the pack's art for that tile
//!   (or the upscaled original where the pack has none).
//! * Lines the pack cannot safely improve keep the exact NES image: greyscale
//!   lines (the greyscale bit rewrites every index), unrecorded lines, and
//!   mid-line split lines ([`LineRecord::split`] / `pixel_split`), where the
//!   slot-to-pixel mapping does not hold for the whole line.
//! * Integer arithmetic only, no allocation in [`Compositor::compose`], no
//!   hash iteration: output is deterministic.

use z2_ppu::record::{BgTileId, FrameRecord, LineRecord, NO_PAGE, RECORD_TILES_PER_LINE};
use z2_ppu::wide::{
    chr_sub, right_edge_masked, wide_width, MarginFill, Margins, WideFrame, MARGIN_SLOTS,
};
use z2_ppu::{
    IndexedFrame, SpriteRef, HEIGHT, PPUMASK_GRAYSCALE, PPUMASK_SHOW_BG, PPUMASK_SHOW_LEFT_BG,
    PPUMASK_SHOW_LEFT_SPRITES, PPUMASK_SHOW_SPRITES, WIDTH,
};

use crate::pack::HdPack;
use crate::palette::MasterPalette;
use crate::MAX_SCALE;

/// Indexed input: the plain NES frame, or a widescreen frame.
#[derive(Debug, Clone, Copy)]
pub struct IndexedView<'a> {
    /// `width * 240` NES palette indices, row-major.
    pub pixels: &'a [u8],
    /// Pixels per row (256 plus `2 * 8 * margin_tiles`).
    pub width: usize,
}

impl<'a> IndexedView<'a> {
    /// The 256x240 NES frame.
    #[must_use]
    pub fn frame(frame: &'a IndexedFrame) -> Self {
        Self {
            pixels: frame.as_slice(),
            width: WIDTH,
        }
    }

    /// A widescreen indexed frame from [`z2_ppu::render_wide_indexed`].
    #[must_use]
    pub fn wide(wide: &'a WideFrame) -> Self {
        Self {
            pixels: &wide.pixels,
            width: wide.width,
        }
    }

    /// Margin width per side in pixels.
    #[must_use]
    pub fn margin_px(&self) -> usize {
        self.width.saturating_sub(WIDTH) / 2
    }
}

/// Everything one [`Compositor::compose`] call reads.
#[derive(Debug, Clone, Copy)]
pub struct ComposeInput<'a> {
    /// The composited indexed image (frame or wide frame).
    pub indexed: IndexedView<'a>,
    /// Record of the same frame.
    pub record: &'a FrameRecord,
    /// Full CHR image (32 x 4 KiB); missing bytes read as zero planes.
    pub chr_rom: &'a [u8],
    /// HD art, when a pack is loaded.
    pub pack: Option<&'a HdPack>,
    /// Margin tile identities; required for HD art in the margins.
    pub margins: Option<&'a Margins>,
    /// Display palette for indexed pixels.
    pub palette: &'a MasterPalette,
}

/// Compose-time failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeError {
    /// Scale outside 1..=8.
    BadScale(u32),
    /// `out` is not [`Compositor::rgba_len`] bytes.
    BadOutLen { want: usize, got: usize },
    /// The indexed source is not `width * 240` pixels.
    BadSourceLen { want: usize, got: usize },
    /// The source row width does not match the configured margins.
    SourceWidth { want: usize, got: usize },
    /// The pack's cells cannot be point-sampled at this output scale.
    ScaleMismatch { pack: u32, output: u32 },
}

impl core::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ComposeError::BadScale(s) => write!(f, "scale {s} out of range (1..={MAX_SCALE})"),
            ComposeError::BadOutLen { want, got } => {
                write!(f, "output buffer is {got} bytes, expected {want}")
            }
            ComposeError::BadSourceLen { want, got } => {
                write!(f, "indexed source is {got} pixels, expected {want}")
            }
            ComposeError::SourceWidth { want, got } => write!(
                f,
                "indexed source is {got} px wide, expected {want} for the configured margins"
            ),
            ComposeError::ScaleMismatch { pack, output } => write!(
                f,
                "output scale {output} does not divide pack scale {pack}: pick a scale that \
                 divides it (or the pack's own scale)"
            ),
        }
    }
}

impl std::error::Error for ComposeError {}

/// Which horizontal band of the output a background slot paints into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    /// Window pixels `0..256`: tiles from the record.
    Centre,
    /// Window pixels `< 0`: `margins.left` for slots `k < 0`, and
    /// `record.tiles[0]` for the `fine_x` leading columns of slot 0.
    Left,
    /// Window pixels `>= 256`: `margins.right`.
    Right,
}

/// RGBA compositor at a fixed integer scale and margin width.
#[derive(Debug, Clone)]
pub struct Compositor {
    scale: u32,
    margin_tiles: u8,
    src_width: usize,
    out_w: usize,
    out_h: usize,
    /// Scratch: NES background colour index per window pixel.
    bg_index: Vec<u8>,
    /// Scratch: NES background opacity per window pixel (sprite priority).
    bg_opaque: Vec<bool>,
    /// Scratch: sprite pixel claims per window pixel.
    claimed: Vec<bool>,
}

impl Compositor {
    /// Compositor at `scale` with no margins.
    ///
    /// # Errors
    /// [`ComposeError::BadScale`] outside 1..=8.
    pub fn new(scale: u32) -> Result<Self, ComposeError> {
        Self::with_margins(scale, 0)
    }

    /// Compositor at `scale` with `margin_tiles` 8-px tiles per side.
    ///
    /// # Errors
    /// [`ComposeError::BadScale`] outside 1..=8.
    pub fn with_margins(scale: u32, margin_tiles: u8) -> Result<Self, ComposeError> {
        let mut me = Self {
            scale: 1,
            margin_tiles: 0,
            src_width: WIDTH,
            out_w: WIDTH,
            out_h: HEIGHT,
            bg_index: vec![0; WIDTH],
            bg_opaque: vec![false; WIDTH],
            claimed: vec![false; WIDTH],
        };
        me.reconfigure(scale, margin_tiles)?;
        Ok(me)
    }

    /// Change scale and margin width. Scratch buffers are size-stable, so
    /// this never allocates.
    ///
    /// # Errors
    /// [`ComposeError::BadScale`] outside 1..=8.
    pub fn reconfigure(&mut self, scale: u32, margin_tiles: u8) -> Result<(), ComposeError> {
        if scale == 0 || scale > MAX_SCALE {
            return Err(ComposeError::BadScale(scale));
        }
        self.scale = scale;
        self.margin_tiles = margin_tiles;
        self.src_width = wide_width(margin_tiles);
        self.out_w = self.src_width * scale as usize;
        self.out_h = HEIGHT * scale as usize;
        Ok(())
    }

    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }

    #[must_use]
    pub fn margin_tiles(&self) -> u8 {
        self.margin_tiles
    }

    /// Output width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.out_w
    }

    /// Output height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.out_h
    }

    /// Bytes one composed frame needs.
    #[must_use]
    pub fn rgba_len(&self) -> usize {
        self.out_w * self.out_h * 4
    }

    /// Whether `pack`'s cells can be point-sampled at output `scale`.
    #[must_use]
    pub fn pack_supports_scale(pack: &HdPack, scale: u32) -> bool {
        scale > 0 && pack.scale().is_multiple_of(scale)
    }

    /// Compose one frame into `out` (row-major RGBA8, [`Compositor::rgba_len`] bytes).
    ///
    /// # Errors
    /// [`ComposeError`] on a buffer, source-size or scale mismatch.
    pub fn compose(&mut self, input: ComposeInput<'_>, out: &mut [u8]) -> Result<(), ComposeError> {
        if out.len() != self.rgba_len() {
            return Err(ComposeError::BadOutLen {
                want: self.rgba_len(),
                got: out.len(),
            });
        }
        if input.indexed.width != self.src_width {
            return Err(ComposeError::SourceWidth {
                want: self.src_width,
                got: input.indexed.width,
            });
        }
        if input.indexed.pixels.len() != self.src_width * HEIGHT {
            return Err(ComposeError::BadSourceLen {
                want: self.src_width * HEIGHT,
                got: input.indexed.pixels.len(),
            });
        }
        let step = match input.pack {
            Some(p) if !Self::pack_supports_scale(p, self.scale) => {
                return Err(ComposeError::ScaleMismatch {
                    pack: p.scale(),
                    output: self.scale,
                })
            }
            Some(p) => p.scale() / self.scale,
            None => 1,
        };

        for y in 0..HEIGHT {
            self.upscale_line(&input, y, out);
            let Some(pack) = input.pack else { continue };
            let rec = input.record.line(y);
            if !Self::line_uses_hd(rec) {
                continue;
            }
            let hd_bg = self.paint_hd_background(rec, &input, pack, step, y, out);
            self.replay_sprites(rec, &input, pack, step, y, hd_bg, out);
        }
        Ok(())
    }

    /// Nearest-upscale one indexed line into its `scale` output rows.
    fn upscale_line(&self, input: &ComposeInput<'_>, y: usize, out: &mut [u8]) {
        let n = self.scale as usize;
        let src = &input.indexed.pixels[y * self.src_width..(y + 1) * self.src_width];
        let row_bytes = self.out_w * 4;
        let first = y * n;
        let (head, tail) = out.split_at_mut((first + 1) * row_bytes);
        let row0 = &mut head[first * row_bytes..];
        for (x, &idx) in src.iter().enumerate() {
            let rgba = input.palette.rgba(idx);
            for px in row0[x * n * 4..(x * n + n) * 4].chunks_exact_mut(4) {
                px.copy_from_slice(&rgba);
            }
        }
        for r in 0..n.saturating_sub(1) {
            tail[r * row_bytes..(r + 1) * row_bytes].copy_from_slice(row0);
        }
    }

    /// Can HD art be applied to this line at all?
    fn line_uses_hd(rec: &LineRecord) -> bool {
        rec.valid && rec.mask & PPUMASK_GRAYSCALE == 0 && !rec.split && !rec.pixel_split
    }

    /// Tile identity of window pixel `wx`'s slot (window pixels only).
    fn window_tile(rec: &LineRecord, wx: i32) -> (BgTileId, u8) {
        let fine_x = i32::from(rec.fine_x & 7);
        let k = (wx + fine_x).div_euclid(8);
        let sub_x = (wx + fine_x).rem_euclid(8) as u8;
        let id = if (0..RECORD_TILES_PER_LINE as i32).contains(&k) {
            rec.tiles[k as usize]
        } else {
            BgTileId::NONE
        };
        (id, sub_x)
    }

    /// Paint every background slot the pack replaces. Returns true when any
    /// pixel was painted (the line's sprites then need replaying).
    fn paint_hd_background(
        &mut self,
        rec: &LineRecord,
        input: &ComposeInput<'_>,
        pack: &HdPack,
        step: u32,
        y: usize,
        out: &mut [u8],
    ) -> bool {
        if rec.mask & PPUMASK_SHOW_BG == 0 {
            return false;
        }
        let tiles = i32::from(self.margin_tiles);
        let mut painted = false;
        // Centre: slots 0..34 clipped to window pixels 0..256.
        for k in 0..RECORD_TILES_PER_LINE as i32 {
            painted |= self.paint_slot(rec, input, pack, step, y, k, Region::Centre, out);
        }
        if tiles > 0 && input.margins.is_some() {
            // `..=0` because slot 0's leading `fine_x` columns land at
            // `wx < 0`, in the left margin band.
            for k in -(MARGIN_SLOTS as i32)..=0 {
                painted |= self.paint_slot(rec, input, pack, step, y, k, Region::Left, out);
            }
            for k in 32..32 + MARGIN_SLOTS as i32 {
                painted |= self.paint_slot(rec, input, pack, step, y, k, Region::Right, out);
            }
        }
        painted
    }

    /// Paint one slot's HD cell into one region. Returns whether it painted.
    #[allow(clippy::too_many_arguments)]
    fn paint_slot(
        &self,
        rec: &LineRecord,
        input: &ComposeInput<'_>,
        pack: &HdPack,
        step: u32,
        y: usize,
        k: i32,
        region: Region,
        out: &mut [u8],
    ) -> bool {
        let record_slot = |k: i32| {
            if (0..RECORD_TILES_PER_LINE as i32).contains(&k) {
                rec.tiles[k as usize]
            } else {
                BgTileId::NONE
            }
        };
        let id = match region {
            Region::Centre => record_slot(k),
            Region::Left | Region::Right => {
                let Some(margins) = input.margins else {
                    return false;
                };
                let ml = &margins.lines[y];
                if ml.fill != MarginFill::Tiles {
                    return false;
                }
                match region {
                    // Slot 0 straddles the left boundary when `fine_x != 0`;
                    // its margin-side columns still come from the record,
                    // as in `render_wide_indexed`.
                    Region::Left if k >= 0 => Some(record_slot(k)),
                    Region::Left => ml.left.get((-1 - k) as usize).copied(),
                    _ => ml.right.get((k - 32) as usize).copied(),
                }
                .unwrap_or(BgTileId::NONE)
            }
        };
        if !id.fetched || id.page == NO_PAGE {
            return false;
        }
        let Some(cell) = pack.lookup(id.page, id.tile, bg_colors(rec, id.pal)) else {
            return false;
        };

        let n = self.scale as usize;
        let left_px = (self.src_width.saturating_sub(WIDTH) / 2) as i32;
        let fine_x = i32::from(rec.fine_x & 7);
        let show_left_bg = rec.mask & PPUMASK_SHOW_LEFT_BG != 0;
        let fill_left_clip = input.margins.is_some_and(|m| m.fill_left_clip);
        let backdrop = input.palette.rgba(rec.backdrop);
        let cellpx = pack.cell_pixels(cell);
        let ps = pack.scale();
        let mut painted = false;

        for px in 0..8i32 {
            let wx = 8 * k + px - fine_x;
            // Each region paints only its own pixels, so slot 32 can serve
            // both the window's tail and the right margin.
            let in_region = match region {
                Region::Centre => (0..WIDTH as i32).contains(&wx),
                Region::Left => (-left_px..0).contains(&wx),
                Region::Right => (WIDTH as i32..WIDTH as i32 + left_px).contains(&wx),
            };
            if !in_region {
                continue;
            }
            // Hardware clips the left 8 background pixels to the backdrop.
            if region == Region::Centre && wx < 8 && !show_left_bg && !fill_left_clip {
                continue;
            }
            let ox = (wx + left_px) as usize;
            for fy in 0..n {
                let hd_y = u32::from(id.fine_y & 7) * ps + (fy as u32) * step;
                let row = cellpx.row(hd_y);
                let oy = y * n + fy;
                for fx in 0..n {
                    let sidx = ((px as u32 * ps + fx as u32 * step) as usize) * 4;
                    let dst = (oy * self.out_w + ox * n + fx) * 4;
                    if row[sidx + 3] != 0 {
                        out[dst..dst + 4].copy_from_slice(&[
                            row[sidx],
                            row[sidx + 1],
                            row[sidx + 2],
                            0xFF,
                        ]);
                    } else {
                        // Transparent HD background == transparent NES
                        // background: the line's backdrop colour.
                        out[dst..dst + 4].copy_from_slice(&backdrop);
                    }
                }
            }
            painted = true;
        }
        painted
    }

    /// Replay the line's recorded sprites over the (possibly HD) background.
    ///
    /// Needed when HD background art was painted (it would otherwise cover
    /// sprites the flat frame already had on top) or when a sprite itself has
    /// HD art. The mux is `render_sprites`': OAM order claims pixels, and a
    /// `behind` sprite loses to opaque background.
    #[allow(clippy::too_many_arguments)]
    fn replay_sprites(
        &mut self,
        rec: &LineRecord,
        input: &ComposeInput<'_>,
        pack: &HdPack,
        step: u32,
        y: usize,
        hd_bg: bool,
        out: &mut [u8],
    ) {
        if rec.mask & PPUMASK_SHOW_SPRITES == 0 {
            return;
        }
        let sprites = input.record.sprites_on(y);
        if sprites.is_empty() {
            return;
        }
        // Widescreen right-edge recovery: when the line is covered by the
        // ROM's edge-mask sprite column and the fill is on, the wide indexed
        // frame already shows the background there, so the replay must not
        // put the mask (or anything else) back over those columns. Same gate
        // as `z2_ppu::wide::render_wide_indexed`, via the same helper.
        let right_fill = input.margins.is_some_and(|m| m.fill_right_clip)
            && right_edge_masked(input.record, y, input.chr_rom);
        let any_hd_sprite = sprites.iter().any(|s| {
            s.page != NO_PAGE
                && pack
                    .lookup(s.page, s.tile, sprite_colors(rec, s.pal))
                    .is_some()
        });
        if !hd_bg && !any_hd_sprite {
            return;
        }

        self.resolve_background_layer(rec, input, y);
        let n = self.scale as usize;
        let margin_px = self.src_width.saturating_sub(WIDTH) / 2;
        let show_left_spr = rec.mask & PPUMASK_SHOW_LEFT_SPRITES != 0;
        for c in self.claimed.iter_mut() {
            *c = false;
        }
        for s in sprites {
            let hd = if s.page == NO_PAGE {
                None
            } else {
                pack.lookup(s.page, s.tile, sprite_colors(rec, s.pal))
            };
            let id = sprite_tile_id(s);
            for dx in 0..8usize {
                let x = usize::from(s.x) + dx;
                if x >= WIDTH {
                    break;
                }
                if x < 8 && !show_left_spr {
                    continue;
                }
                if right_fill && x >= WIDTH - 8 {
                    continue; // recovered background owns these columns
                }
                let col = if s.flip_h { 7 - dx } else { dx };
                let sub = chr_sub(input.chr_rom, id, col as u8).unwrap_or(0);
                if sub == 0 {
                    continue; // NES-transparent: no coverage, no claim.
                }
                if self.claimed[x] {
                    continue;
                }
                self.claimed[x] = true;
                if s.behind && self.bg_opaque[x] {
                    continue;
                }
                let ox = x + margin_px;
                match hd {
                    None => {
                        let rgba = input.palette.rgba(
                            rec.palette_entry(0x10 + usize::from(s.pal & 3) * 4 + usize::from(sub)),
                        );
                        self.fill_block(ox, y, &rgba, out);
                    }
                    Some(cell) => {
                        let cellpx = pack.cell_pixels(cell);
                        let ps = pack.scale();
                        let bg = input.palette.rgba(self.bg_index[x]);
                        for fy in 0..n {
                            let sub_row = fy as u32 * step;
                            let hd_y = u32::from(s.fine_row & 7) * ps
                                + if s.flip_v { ps - 1 - sub_row } else { sub_row };
                            let row = cellpx.row(hd_y);
                            let oy = y * n + fy;
                            for fx in 0..n {
                                let sub_col = fx as u32 * step;
                                let mirrored = if s.flip_h { ps - 1 - sub_col } else { sub_col };
                                let sidx = ((col as u32 * ps + mirrored) as usize) * 4;
                                let dst = (oy * self.out_w + ox * n + fx) * 4;
                                if row[sidx + 3] != 0 {
                                    out[dst..dst + 4].copy_from_slice(&[
                                        row[sidx],
                                        row[sidx + 1],
                                        row[sidx + 2],
                                        0xFF,
                                    ]);
                                } else {
                                    // Hole in HD sprite art shows the background.
                                    out[dst..dst + 4].copy_from_slice(&bg);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// NES background colour index and opacity per window pixel, exactly as
    /// `render_background` computes them (needed for sprite priority and for
    /// the colour under a hole in HD sprite art).
    fn resolve_background_layer(&mut self, rec: &LineRecord, input: &ComposeInput<'_>, y: usize) {
        let show_bg = rec.mask & PPUMASK_SHOW_BG != 0;
        let show_left_bg = rec.mask & PPUMASK_SHOW_LEFT_BG != 0;
        for x in 0..WIDTH {
            let (idx, opaque) = if !show_bg {
                (rec.backdrop, false)
            } else {
                let (id, sub_x) = Self::window_tile(rec, x as i32);
                match chr_sub(input.chr_rom, id, sub_x) {
                    None | Some(0) => (rec.backdrop, false),
                    Some(sub) if x < 8 && !show_left_bg => {
                        let _ = sub;
                        (rec.backdrop, false)
                    }
                    Some(sub) => (
                        rec.palette_entry(usize::from(id.pal & 3) * 4 + usize::from(sub)),
                        true,
                    ),
                }
            };
            self.bg_index[x] = idx;
            self.bg_opaque[x] = opaque;
        }
        let _ = y;
    }

    /// Fill one NES pixel's `scale x scale` output block with `rgba`.
    fn fill_block(&self, ox: usize, y: usize, rgba: &[u8; 4], out: &mut [u8]) {
        let n = self.scale as usize;
        for fy in 0..n {
            let oy = y * n + fy;
            for fx in 0..n {
                let dst = (oy * self.out_w + ox * n + fx) * 4;
                out[dst..dst + 4].copy_from_slice(rgba);
            }
        }
    }
}

/// Pattern identity of a recorded sprite row (for [`chr_sub`]).
fn sprite_tile_id(s: &SpriteRef) -> BgTileId {
    BgTileId {
        page: s.page,
        tile: s.tile,
        pal: s.pal,
        fine_y: s.fine_row & 7,
        fetched: true,
        ..BgTileId::NONE
    }
}

/// Variant key for a background tile: its palette group's 3 opaque colours.
#[must_use]
pub fn bg_colors(rec: &LineRecord, pal: u8) -> [u8; 3] {
    let base = usize::from(pal & 3) * 4;
    [
        rec.palette_entry(base + 1),
        rec.palette_entry(base + 2),
        rec.palette_entry(base + 3),
    ]
}

/// Variant key for a sprite: its palette group's 3 opaque colours.
#[must_use]
pub fn sprite_colors(rec: &LineRecord, pal: u8) -> [u8; 3] {
    let base = 0x10 + usize::from(pal & 3) * 4;
    [
        rec.palette_entry(base + 1),
        rec.palette_entry(base + 2),
        rec.palette_entry(base + 3),
    ]
}
