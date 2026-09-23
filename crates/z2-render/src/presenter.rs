//! One object a frontend drives each frame: owns the widescreen indexed
//! buffer, the margin description, the pack and the RGBA output.
//!
//! Plain data in, RGBA out — no dependency on `z2-core`, so the wasm build
//! stays as light as the native one. Per frame the frontend hands over the
//! indexed frame and the PPU's render record; for widescreen it also fills
//! [`Presenter::margins_mut`] with tile identities first (the decoder lives
//! in `z2-core`, which the frontend already has).
//!
//! ```no_run
//! # use z2_render::presenter::{Presenter, PresentConfig};
//! let mut p = Presenter::new(PresentConfig { scale: 2, ..PresentConfig::default() }).unwrap();
//! // texture sizing:
//! let (w, h) = (p.width(), p.height());
//! # let frame: &z2_ppu::IndexedFrame = unimplemented!();
//! # let record: &z2_ppu::record::FrameRecord = unimplemented!();
//! # let chr: &[u8] = &[];
//! let rgba: &[u8] = p.present(frame, record, chr).unwrap();
//! # let _ = (w, h, rgba);
//! ```

use z2_ppu::record::FrameRecord;
use z2_ppu::wide::{render_wide_indexed, wide_width, Margins, WideFrame};
use z2_ppu::{IndexedFrame, HEIGHT};

use crate::compositor::{ComposeError, ComposeInput, Compositor, IndexedView};
use crate::pack::{HdPack, SceneView};
use crate::palette::MasterPalette;

/// Presenter configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentConfig {
    /// Requested output multiplier 1..=8.
    pub scale: u32,
    /// Widescreen margin per side in 8-px tiles (0 = off, max 16).
    pub margin_tiles: u8,
    /// Paint the NES window's clipped left 8 columns from the fetched tiles
    /// (widescreen only; removes the black seam on the overworld).
    pub fill_left_clip: bool,
    /// Paint the NES window's right 8 columns from the fetched tiles on lines
    /// the ROM masks with its opaque sprite column (widescreen only; removes
    /// the matching seam on the right). See
    /// [`z2_ppu::wide::right_edge_masked`].
    pub fill_right_clip: bool,
}

impl Default for PresentConfig {
    fn default() -> Self {
        Self {
            scale: 1,
            margin_tiles: 0,
            fill_left_clip: false,
            fill_right_clip: false,
        }
    }
}

/// Owns every per-frame buffer and composes one frame per call.
#[derive(Debug)]
pub struct Presenter {
    cfg: PresentConfig,
    comp: Compositor,
    /// Indexed widescreen buffer (unused when `margin_tiles == 0`).
    wide: WideFrame,
    margins: Margins,
    pack: Option<HdPack>,
    palette: MasterPalette,
    scene: Option<SceneView>,
    rgba: Vec<u8>,
}

impl Presenter {
    /// Build a presenter for `cfg`.
    ///
    /// # Errors
    /// [`ComposeError::BadScale`] when `cfg.scale` is outside 1..=8.
    pub fn new(cfg: PresentConfig) -> Result<Self, ComposeError> {
        let comp = Compositor::with_margins(cfg.scale, cfg.margin_tiles)?;
        let mut margins = Margins::new(cfg.margin_tiles);
        margins.fill_left_clip = cfg.fill_left_clip;
        margins.fill_right_clip = cfg.fill_right_clip;
        let mut me = Self {
            rgba: vec![0; comp.rgba_len()],
            wide: WideFrame::new(cfg.margin_tiles),
            comp,
            margins,
            pack: None,
            palette: MasterPalette::NES,
            scene: None,
            cfg,
        };
        me.resize_buffers();
        Ok(me)
    }

    /// Replace the configuration, keeping the pack and palette.
    ///
    /// # Errors
    /// [`ComposeError::BadScale`], or [`ComposeError::ScaleMismatch`] when the
    /// loaded pack cannot be sampled at the new scale.
    pub fn set_config(&mut self, cfg: PresentConfig) -> Result<(), ComposeError> {
        if let Some(pack) = &self.pack {
            if !Compositor::pack_supports_scale(pack, effective_scale(cfg.scale, Some(pack))) {
                return Err(ComposeError::ScaleMismatch {
                    pack: pack.scale(),
                    output: cfg.scale,
                });
            }
        }
        self.cfg = cfg;
        self.margins.set_tiles(cfg.margin_tiles);
        self.margins.fill_left_clip = cfg.fill_left_clip;
        self.margins.fill_right_clip = cfg.fill_right_clip;
        self.comp
            .reconfigure(self.effective_scale(), cfg.margin_tiles)?;
        self.resize_buffers();
        Ok(())
    }

    /// Load (or clear) the HD pack.
    ///
    /// The output scale becomes the requested scale when it divides the
    /// pack's scale (HD cells are then point-sampled), otherwise the pack's
    /// own scale — so a 4x pack always displays correctly even when the
    /// frontend asked for 3x. Check [`Presenter::effective_scale`] and
    /// re-read [`Presenter::width`]/[`Presenter::height`] afterwards.
    ///
    /// # Errors
    /// [`ComposeError::BadScale`] when the resulting scale is out of range.
    pub fn set_pack(&mut self, pack: Option<HdPack>) -> Result<(), ComposeError> {
        self.pack = pack;
        self.comp
            .reconfigure(self.effective_scale(), self.cfg.margin_tiles)?;
        self.resize_buffers();
        Ok(())
    }

    /// The loaded pack, if any.
    #[must_use]
    pub fn pack(&self) -> Option<&HdPack> {
        self.pack.as_ref()
    }

    /// Override the display palette (a pack's own palette wins when set).
    pub fn set_palette(&mut self, palette: MasterPalette) {
        self.palette = palette;
    }

    /// Tell the pack's layers which scene is on screen and where the camera
    /// is, before [`Presenter::present`]. `None` (the default) hides every
    /// layer that names a scene. Only a pack with `layers` reads it.
    pub fn set_scene(&mut self, scene: Option<SceneView>) {
        self.scene = scene;
    }

    /// Whether the loaded pack has layers, so [`Presenter::set_scene`] matters.
    #[must_use]
    pub fn needs_scene(&self) -> bool {
        self.pack.as_ref().is_some_and(|p| !p.layers().is_empty())
    }

    /// Configuration in force.
    #[must_use]
    pub fn config(&self) -> PresentConfig {
        self.cfg
    }

    /// Output multiplier actually used (see [`Presenter::set_pack`]).
    #[must_use]
    pub fn effective_scale(&self) -> u32 {
        effective_scale(self.cfg.scale, self.pack.as_ref())
    }

    /// Output width in pixels (size your texture with this).
    #[must_use]
    pub fn width(&self) -> usize {
        self.comp.width()
    }

    /// Output height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.comp.height()
    }

    /// True when widescreen margins are enabled.
    #[must_use]
    pub fn is_wide(&self) -> bool {
        self.cfg.margin_tiles > 0
    }

    /// The record must be armed (`Ppu::set_record(true)`) whenever this is
    /// true: widescreen margins and HD art both need tile identities.
    #[must_use]
    pub fn needs_record(&self) -> bool {
        self.is_wide() || self.pack.is_some()
    }

    /// Margin tile identities to fill before [`Presenter::present`] in
    /// widescreen mode (one entry per visible line).
    pub fn margins_mut(&mut self) -> &mut Margins {
        &mut self.margins
    }

    /// The margin description.
    #[must_use]
    pub fn margins(&self) -> &Margins {
        &self.margins
    }

    /// Last composed frame (row-major RGBA8, `width * height * 4` bytes).
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Compose one frame and return the RGBA buffer.
    ///
    /// `chr_rom` is the full CHR image (`Game::chr`); it is only read when a
    /// pack or margins need pattern data, so `&[]` is fine for the plain
    /// path.
    ///
    /// # Errors
    /// [`ComposeError`] on a size or scale mismatch.
    pub fn present(
        &mut self,
        frame: &IndexedFrame,
        record: &FrameRecord,
        chr_rom: &[u8],
    ) -> Result<&[u8], ComposeError> {
        let palette = self
            .pack
            .as_ref()
            .and_then(HdPack::palette)
            .copied()
            .unwrap_or(self.palette);
        let indexed = if self.is_wide() {
            render_wide_indexed(frame, record, &self.margins, chr_rom, &mut self.wide);
            IndexedView::wide(&self.wide)
        } else {
            IndexedView::frame(frame)
        };
        self.comp.compose(
            ComposeInput {
                indexed,
                record,
                chr_rom,
                pack: self.pack.as_ref(),
                margins: if self.is_wide() {
                    Some(&self.margins)
                } else {
                    None
                },
                palette: &palette,
                scene: self.scene,
            },
            &mut self.rgba,
        )?;
        Ok(&self.rgba)
    }

    fn resize_buffers(&mut self) {
        let want = self.comp.rgba_len();
        if self.rgba.len() != want {
            self.rgba.resize(want, 0);
        }
        self.wide.resize(self.cfg.margin_tiles);
        debug_assert_eq!(
            self.comp.width(),
            wide_width(self.cfg.margin_tiles) * self.effective_scale() as usize
        );
        debug_assert_eq!(self.comp.height(), HEIGHT * self.effective_scale() as usize);
    }
}

fn effective_scale(requested: u32, pack: Option<&HdPack>) -> u32 {
    match pack {
        Some(p) if requested == 0 || !p.scale().is_multiple_of(requested) => p.scale(),
        _ => requested,
    }
}
