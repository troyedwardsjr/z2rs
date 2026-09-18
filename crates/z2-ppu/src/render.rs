//! Per-scanline software renderer + indexed-frame diff.
//!
//! [`render_line`] draws one visible scanline from the live register state
//! (`v`/fine `x`, `PPUCTRL`, `PPUMASK`, OAM, palette): background fetch
//! (nametable + attribute + pattern, walking `v` horizontally with the
//! hardware nametable wrap), both sprite sizes, hardware-faithful 8-sprite
//! dropout and priority, and sprite-0 hit detection. The scanline-timed
//! driver in [`crate::state`] calls it lazily as the beam advances;
//! [`render_frame`] is the whole-frame convenience. [`diff_indexed`] plus
//! [`write_diff_ppm`] show exactly which pixels mismatch when a test fails.
//!
//! Model notes:
//!
//! * One scanline renders background first, then sprites in OAM order so
//!   lower indices win (hardware priority). Sprite-vs-sprite occupancy is
//!   claimed even by behind-background pixels, matching the hardware mux.
//! * Vertical position comes straight from `v` (coarse/fine Y, nametable
//!   bit); rows 30-31 fetch attribute bytes as tiles exactly like hardware.
//! * Sprite overflow sets `$2002` bit 5 whenever more than 8 sprites cover a
//!   scanline in [`SpriteLimit::Faithful8`] mode (the exact hardware race is
//!   not modelled — the flag is what the game can observe).
//! * Sprite-0 hit needs both layers enabled, an opaque sprite-0 pixel over an
//!   opaque background pixel, `x != 255`, and no left-8 clipping hiding that
//!   column — the observable hardware rule. The hit *x* is reported so the
//!   state model can time the flag against the beam.

use crate::palette::indexed_to_rgba;
use crate::record::SpriteRef;
use crate::state::{Ppu, SpriteLimit, ATTR_OFFSET};
use crate::{HEIGHT, WIDTH};

/// Indexed framebuffer: one NES palette index (`$00-$3F`) per pixel.
pub type IndexedFrame = [u8; WIDTH * HEIGHT];

/// How many mismatch coordinates [`FrameDiff`] keeps (the count is exact).
pub const MAX_DIFF_COORDS: usize = 32;

/// Mismatch summary from [`diff_indexed`].
#[derive(Debug, Clone)]
pub struct FrameDiff {
    /// Total mismatching pixels.
    pub count: usize,
    /// First [`MAX_DIFF_COORDS`] mismatches as `(x, y)`.
    pub first: [(u16, u16); MAX_DIFF_COORDS],
    /// How many of `first` are filled.
    pub first_len: usize,
}

impl FrameDiff {
    /// True when the frames are identical.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.count == 0
    }
}

/// Compare two indexed frames: exact mismatch count plus the first
/// [`MAX_DIFF_COORDS`] coordinates for failure inspection.
#[must_use]
pub fn diff_indexed(a: &IndexedFrame, b: &IndexedFrame) -> FrameDiff {
    let mut diff = FrameDiff {
        count: 0,
        first: [(0, 0); MAX_DIFF_COORDS],
        first_len: 0,
    };
    for (i, (&pa, &pb)) in a.iter().zip(b.iter()).enumerate() {
        if pa != pb {
            if diff.first_len < MAX_DIFF_COORDS {
                diff.first[diff.first_len] = ((i % WIDTH) as u16, (i / WIDTH) as u16);
                diff.first_len += 1;
            }
            diff.count += 1;
        }
    }
    diff
}

/// Write a binary PPM (`P6`) overlay: matching pixels use
/// [`indexed_to_rgba`] on frame `a`, mismatching pixels are magenta
/// `(255, 0, 255)`. Dependency-free (hand-rolled header, no image crate) so
/// failing golden tests can dump inspection images anywhere, including wasm
/// harnesses that forward the bytes.
pub fn write_diff_ppm(a: &IndexedFrame, b: &IndexedFrame, out: &mut Vec<u8>) {
    out.extend_from_slice(format!("P6\n{WIDTH} {HEIGHT}\n255\n").as_bytes());
    out.reserve(WIDTH * HEIGHT * 3);
    for (&pa, &pb) in a.iter().zip(b.iter()) {
        if pa == pb {
            let [r, g, bl, _] = indexed_to_rgba(pa);
            out.extend_from_slice(&[r, g, bl]);
        } else {
            out.extend_from_slice(&[255, 0, 255]);
        }
    }
}

/// Per-line flags reported by [`render_line`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineFlags {
    /// First x where sprite 0 hit an opaque background pixel on this line.
    pub hit_x: Option<u8>,
    /// More than 8 sprites covered the line (faithful mode only).
    pub overflow: bool,
}

/// Render one visible scanline into `out` (256 indexed pixels) from the
/// live state as a whole-line dry run (no fetch pipeline state consumed);
/// pure with respect to the PPU (flags are returned, not set). Used by
/// tests and diagnostics — the frame renderer itself streams tiles through
/// [`Ppu::set_beam_access`].
pub fn render_line(ppu: &Ppu, line: u8, out: &mut [u8; WIDTH]) -> LineFlags {
    let universal = apply_gray(ppu, ppu.backdrop_entry());
    let mut bg_opaque = [false; WIDTH];

    if ppu.show_bg() {
        render_background(ppu, out, &mut bg_opaque);
    } else {
        out.fill(universal);
    }

    if ppu.show_sprites() {
        render_sprites(ppu, line, out, &bg_opaque, true, None)
    } else {
        LineFlags::default()
    }
}

/// Dry-run sprite-0 probe for `line` with the current state: the x of the
/// first sprite-0/background overlap, or `None`. Used by the state model to
/// answer `$2002` reads on the not-yet-rendered current line (pixels the
/// line pipeline already emitted keep their opacity).
#[must_use]
pub fn probe_sprite0_hit(ppu: &Ppu, line: u8) -> Option<u8> {
    if !ppu.show_bg() || !ppu.show_sprites() {
        return None;
    }
    let mut out = [0u8; WIDTH];
    let bg_opaque = ppu.probe_bg_opaque();
    render_sprites(ppu, line, &mut out, &bg_opaque, false, None).hit_x
}

/// Whole-frame convenience: finish the frame with the current state and
/// raise vblank (see [`Ppu::render_frame`]).
pub fn render_frame(ppu: &mut Ppu) -> IndexedFrame {
    ppu.render_frame()
}

/// Apply the greyscale bit: indices collapse to the `$x0` column.
fn apply_gray(ppu: &Ppu, index: u8) -> u8 {
    if ppu.grayscale() {
        index & 0x30
    } else {
        index
    }
}

/// Background pass: walk `v` across the line (coarse X wraps into the
/// neighbouring nametable), fetch tile/attribute/pattern per pixel.
fn render_background(ppu: &Ppu, out: &mut [u8; WIDTH], bg_opaque: &mut [bool; WIDTH]) {
    let v = ppu.v();
    let table = ppu.bg_table();
    let show_left = ppu.show_left8_bg();
    let universal = apply_gray(ppu, ppu.backdrop_entry());
    let fine_y = ((v >> 12) & 0x07) as u8;
    let coarse_y = ((v >> 5) & 0x1F) as u8;
    let nt_y = ((v >> 11) & 0x01) as u8;
    let mut coarse_x = (v & 0x1F) as u8;
    let mut nt_x = ((v >> 10) & 0x01) as u8;
    let mut fx = ppu.fine_x();

    // Attribute row/quadrant are constant along the line.
    let attr_row = coarse_y >> 2;
    let qy = (coarse_y >> 1) & 1;

    let mut x = 0usize;
    while x < WIDTH {
        let logical = (nt_y << 1) | nt_x;
        let tile = ppu.nt_byte(logical, usize::from(coarse_y) * 32 + usize::from(coarse_x));
        let attr = ppu.nt_byte(
            logical,
            ATTR_OFFSET + usize::from(attr_row) * 8 + usize::from(coarse_x >> 2),
        );
        let qx = (coarse_x >> 1) & 1;
        let pal = (attr >> (((qy << 1) | qx) * 2)) & 0x03;
        let lo = ppu.chr_byte(table, tile, fine_y, false);
        let hi = ppu.chr_byte(table, tile, fine_y, true);
        // Emit the remaining pixels of this tile.
        while fx < 8 && x < WIDTH {
            let bit = 7 - fx;
            let sub = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            if (x < 8 && !show_left) || sub == 0 {
                out[x] = universal;
            } else {
                let index = ppu.palette_entry(usize::from(pal) * 4 + usize::from(sub));
                out[x] = apply_gray(ppu, index);
                bg_opaque[x] = true;
            }
            fx += 1;
            x += 1;
        }
        fx = 0;
        if coarse_x == 31 {
            coarse_x = 0;
            nt_x ^= 1;
        } else {
            coarse_x += 1;
        }
    }
}

/// Sprite pass for one scanline: dropout, priority, behind-bg, sprite-0 hit.
/// `draw` false = probe only (no pixel writes). `sink` (render record)
/// receives every sprite chosen for the line, in OAM order, when drawing;
/// it never influences pixels or flags.
pub(crate) fn render_sprites(
    ppu: &Ppu,
    line: u8,
    out: &mut [u8; WIDTH],
    bg_opaque: &[bool; WIDTH],
    draw: bool,
    mut sink: Option<&mut Vec<SpriteRef>>,
) -> LineFlags {
    let mut flags = LineFlags::default();
    let height = ppu.sprite_height();
    let tall = ppu.sprite_tall();
    let table_8x8 = ppu.sprite_table();
    let show_left = ppu.show_left8_sprites();
    let show_bg = ppu.show_bg();
    let show_left_bg = ppu.show_left8_bg();

    // OAM-order evaluation: first 8 covering sprites win in faithful mode.
    let mut on_line = [0usize; 64];
    let mut found = 0usize;
    for i in 0..64 {
        let e = ppu.oam_entry(i);
        // NOTE: `as u16 + 1`, not `wrapping_add(1)`: Y=$FF parks the sprite
        // at top=256 (fully off-screen), it must not wrap to 0.
        let top = e.y as u16 + 1;
        if (line as u16) >= top && (line as u16) < top + height {
            on_line[found] = i;
            found += 1;
        }
    }
    if ppu.sprite_limit() == SpriteLimit::Faithful8 && found > 8 {
        flags.overflow = true;
    }
    let take = match ppu.sprite_limit() {
        SpriteLimit::Faithful8 => found.min(8),
        SpriteLimit::Unlimited => found,
    };

    // Draw front-to-back (OAM order): the first sprite to claim a pixel
    // wins, so lower OAM indices end up on top (hardware priority).
    let mut claimed = [false; WIDTH];
    for &i in &on_line[..take] {
        let e = ppu.oam_entry(i);
        let left = e.x as usize;
        let row_in_sprite = (line as u16) - (e.y as u16 + 1);

        let (table, tile, fine_row) = if tall {
            let r = if e.attr & 0x80 != 0 {
                15 - row_in_sprite
            } else {
                row_in_sprite
            };
            let top_tile = e.tile & 0xFE;
            let t = if r < 8 {
                top_tile
            } else {
                top_tile.wrapping_add(1)
            };
            (usize::from(e.tile & 0x01), t, (r & 7) as u8)
        } else {
            let r = if e.attr & 0x80 != 0 {
                7 - row_in_sprite
            } else {
                row_in_sprite
            };
            (table_8x8, e.tile, r as u8)
        };
        let flip_h = e.attr & 0x40 != 0;
        let behind = e.attr & 0x20 != 0;
        let pal = (e.attr & 0x03) as usize;
        if draw {
            if let Some(s) = sink.as_deref_mut() {
                s.push(SpriteRef {
                    oam_index: i as u8,
                    x: e.x,
                    page: ppu.chr_page_no(table),
                    tile,
                    fine_row,
                    row_in_sprite: row_in_sprite as u8,
                    pal: pal as u8,
                    flip_h,
                    flip_v: e.attr & 0x80 != 0,
                    behind,
                    tall,
                });
            }
        }
        let lo = ppu.chr_byte(table, tile, fine_row, false);
        let hi = ppu.chr_byte(table, tile, fine_row, true);

        for dx in 0..8usize {
            let x = left + dx;
            if x >= WIDTH {
                break;
            }
            if x < 8 && !show_left {
                continue;
            }
            let col = if flip_h { 7 - dx } else { dx };
            let bit = 7 - col as u8;
            let sub = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            if sub == 0 {
                continue;
            }
            if i == 0
                && flags.hit_x.is_none()
                && show_bg
                && bg_opaque[x]
                && x != 255
                && (x >= 8 || (show_left_bg && show_left))
            {
                flags.hit_x = Some(x as u8);
            }
            if !draw {
                continue;
            }
            if claimed[x] {
                continue;
            }
            claimed[x] = true;
            if behind && bg_opaque[x] {
                continue;
            }
            let index = ppu.palette_entry(0x10 + pal * 4 + usize::from(sub));
            out[x] = apply_gray(ppu, index);
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{OamEntry, CHR_BANK_LEN};

    /// PPU with rendering on, blank CHR/NT, universal bg `$0F`, sprite
    /// palette 0 entries distinct.
    fn blank_ppu() -> Ppu {
        let mut p = Ppu::new();
        p.write_mask(0x1E); // show bg+sprites, no left-8 clip
        p.set_palette(0, 0x0F);
        p
    }

    /// Fill one CHR slot with a flat tile: every row `low`/`high`.
    fn flat_chr(tile: u8, low: u8, high: u8) -> [u8; CHR_BANK_LEN] {
        let mut chr = [0u8; CHR_BANK_LEN];
        for row in 0..8 {
            chr[tile as usize * 16 + row] = low;
            chr[tile as usize * 16 + row + 8] = high;
        }
        chr
    }

    #[test]
    fn transparent_background_yields_universal_colour() {
        let mut p = blank_ppu();
        let frame = p.render_frame();
        assert!(frame.iter().all(|&px| px == 0x0F));
        assert!(!p.sprite0_hit());
    }

    #[test]
    fn attribute_quadrants_pick_sub_palettes() {
        let mut p = blank_ppu();
        // Tile 7: every pixel sub-palette index 1 (low plane set).
        p.load_chr_4k(0, 0, &flat_chr(7, 0xFF, 0x00)).unwrap();
        for ty in 0..4u8 {
            for tx in 0..4u8 {
                p.set_tile(0, tx, ty, 7);
            }
        }
        // Attr byte 0: TL=0, TR=1, BL=2, BR=3.
        p.set_attr_quad(0, 0, 0, 0, 0, 0);
        p.set_attr_quad(0, 0, 0, 1, 0, 1);
        p.set_attr_quad(0, 0, 0, 0, 1, 2);
        p.set_attr_quad(0, 0, 0, 1, 1, 3);
        p.set_palette(1, 0x01);
        p.set_palette(5, 0x11);
        p.set_palette(9, 0x21);
        p.set_palette(13, 0x02);

        let frame = p.render_frame();
        let px = |x: usize, y: usize| frame[y * WIDTH + x];
        assert_eq!(px(4, 4), 0x01, "top-left quadrant -> palette 0");
        assert_eq!(px(20, 4), 0x11, "top-right quadrant -> palette 1");
        assert_eq!(px(4, 20), 0x21, "bottom-left quadrant -> palette 2");
        assert_eq!(px(20, 20), 0x02, "bottom-right quadrant -> palette 3");
    }

    #[test]
    fn sprite_8x8_draws_with_palette_and_flip() {
        let mut p = blank_ppu();
        // Tile 1: left half sub=2, right half transparent.
        let mut chr = [0u8; CHR_BANK_LEN];
        for row in 0..8 {
            chr[16 + row] = 0x00; // low plane 0
            chr[16 + row + 8] = 0xF0; // high plane: left 4 px sub=2
        }
        p.load_chr_4k(0, 0, &chr).unwrap();
        p.set_palette(0x12, 0x16);
        p.set_oam_entry(
            1,
            OamEntry {
                y: 19,
                tile: 1,
                attr: 0,
                x: 10,
            },
        );
        let frame = p.render_frame();
        let px = |x: usize, y: usize| frame[y * WIDTH + x];
        assert_eq!(px(10, 20), 0x16);
        assert_eq!(px(13, 27), 0x16);
        assert_eq!(px(14, 20), 0x0F, "transparent half shows bg");
        assert_eq!(px(9, 20), 0x0F, "left of sprite shows bg");

        // Horizontal flip moves the opaque half to the right.
        p.set_oam_entry(
            1,
            OamEntry {
                y: 19,
                tile: 1,
                attr: 0x40,
                x: 10,
            },
        );
        let frame = p.render_frame();
        let px = |x: usize, y: usize| frame[y * WIDTH + x];
        assert_eq!(px(14, 20), 0x16, "flipped: opaque on right");
        assert_eq!(px(10, 20), 0x0F, "flipped: transparent on left");
    }

    #[test]
    fn sprite_8x16_uses_tile_bit_for_table_and_halves() {
        let mut p = blank_ppu();
        p.write_ctrl(crate::state::PPUCTRL_TALL_SPRITES);
        // Tile bit0=1 -> pattern table 1. Top half (tile $20) blank,
        // bottom half (tile $21) opaque sub=1.
        let mut chr1 = [0u8; CHR_BANK_LEN];
        for row in 0..8 {
            chr1[0x21 * 16 + row] = 0xFF;
        }
        p.load_chr_4k(1, 1, &chr1).unwrap();
        p.set_palette(0x11, 0x2A);
        p.set_oam_entry(
            2,
            OamEntry {
                y: 29,
                tile: 0x21,
                attr: 0,
                x: 50,
            },
        );
        let frame = p.render_frame();
        let px = |x: usize, y: usize| frame[y * WIDTH + x];
        assert_eq!(px(50, 30), 0x0F, "top half (tile $20) blank");
        assert_eq!(px(50, 38), 0x2A, "bottom half (tile $21) opaque");
        assert_eq!(px(50, 45), 0x2A);
    }

    #[test]
    fn sprite_vertical_flip_mirrors_rows() {
        let mut p = blank_ppu();
        // Tile 3: only row 0 opaque.
        let mut chr = [0u8; CHR_BANK_LEN];
        chr[3 * 16] = 0xFF;
        p.load_chr_4k(0, 0, &chr).unwrap();
        p.set_palette(0x11, 0x09);
        p.set_oam_entry(
            3,
            OamEntry {
                y: 9,
                tile: 3,
                attr: 0,
                x: 30,
            },
        );
        let f = p.render_frame();
        assert_eq!(f[10 * WIDTH + 30], 0x09, "row 0 opaque unflipped");
        assert_eq!(f[17 * WIDTH + 30], 0x0F);
        p.set_oam_entry(
            3,
            OamEntry {
                y: 9,
                tile: 3,
                attr: 0x80,
                x: 30,
            },
        );
        let f = p.render_frame();
        assert_eq!(f[10 * WIDTH + 30], 0x0F, "flipped: top now blank");
        assert_eq!(f[17 * WIDTH + 30], 0x09, "flipped: bottom opaque");
    }

    #[test]
    fn ninth_sprite_drops_out_and_sets_overflow() {
        let mut p = blank_ppu();
        p.load_chr_4k(0, 0, &flat_chr(5, 0xFF, 0x00)).unwrap();
        p.set_palette(0x11, 0x02); // sprite palette 0, sub 1
        p.write_ctrl(0); // bg table 0, sprite table 0, 8x8
                         // Sprite 0 stays on the scanline (it counts for dropout) but uses
                         // blank tile 0, so it is transparent and cannot set sprite-0 hit.
        p.set_oam_entry(
            0,
            OamEntry {
                y: 49,
                tile: 0,
                attr: 0,
                x: 200,
            },
        );
        for i in 1..9usize {
            p.set_oam_entry(
                i,
                OamEntry {
                    y: 49,
                    tile: 5,
                    attr: 0,
                    x: (10 + (i as u8 - 1) * 8),
                },
            );
        }
        let frame = p.render_frame();
        // 9th sprite (index 8) at x=10+7*8=66 must be dropped.
        assert_eq!(frame[50 * WIDTH + 66], 0x0F, "9th sprite drops out");
        assert_eq!(frame[50 * WIDTH + 10 + 6 * 8], 0x02, "8th sprite draws");
        assert!(p.sprite_overflow(), "overflow flag set");
        assert!(!p.sprite0_hit(), "transparent sprite 0: no hit");

        p.begin_frame();
        p.set_sprite_limit(SpriteLimit::Unlimited);
        let frame = p.render_frame();
        assert_eq!(frame[50 * WIDTH + 66], 0x02, "unlimited draws all");
        assert!(!p.sprite_overflow(), "no overflow in unlimited mode");
    }

    #[test]
    fn lower_oam_index_wins_and_behind_bg_stays_behind() {
        let mut p = blank_ppu();
        p.load_chr_4k(0, 0, &flat_chr(6, 0xFF, 0x00)).unwrap();
        p.set_palette(0x11, 0x0A); // sprite pal 0
        p.set_palette(0x15, 0x0B); // sprite pal 1
                                   // Sprite 1 (idx 1) at x=20 pal 0, sprite 2 at x=22 pal 1.
        p.set_oam_entry(
            1,
            OamEntry {
                y: 59,
                tile: 6,
                attr: 0,
                x: 20,
            },
        );
        p.set_oam_entry(
            2,
            OamEntry {
                y: 59,
                tile: 6,
                attr: 1,
                x: 22,
            },
        );
        // Park sprite 0 off-screen so hit logic stays out of the way.
        p.set_oam_entry(
            0,
            OamEntry {
                y: 0xFF,
                tile: 6,
                attr: 0,
                x: 0,
            },
        );
        let frame = p.render_frame();
        assert_eq!(frame[60 * WIDTH + 23], 0x0A, "lower index wins overlap");

        // Behind-background sprite over opaque bg stays hidden...
        p.load_chr_4k(1, 1, &flat_chr(7, 0xFF, 0x00)).unwrap();
        p.write_ctrl(crate::state::PPUCTRL_BG_TABLE); // bg from table 1
        for tx in 0..32u8 {
            p.set_tile(0, tx, 7, 7);
        }
        p.set_palette(1, 0x1C);
        p.set_oam_entry(
            4,
            OamEntry {
                y: 59,
                tile: 6,
                attr: 0x20,
                x: 100,
            },
        );
        let frame = p.render_frame();
        assert_eq!(frame[60 * WIDTH + 100], 0x1C, "behind sprite hidden by bg");
        // ...but the same sprite in front draws.
        p.set_oam_entry(
            4,
            OamEntry {
                y: 59,
                tile: 6,
                attr: 0,
                x: 100,
            },
        );
        let frame = p.render_frame();
        assert_eq!(frame[60 * WIDTH + 100], 0x0A, "front sprite covers bg");
    }

    #[test]
    fn sprite0_hit_sets_only_on_opaque_overlap() {
        let mut p = blank_ppu();
        p.load_chr_4k(0, 0, &flat_chr(9, 0xFF, 0x00)).unwrap();
        p.set_palette(1, 0x06);
        p.set_palette(0x11, 0x16);
        for tx in 0..32u8 {
            p.set_tile(0, tx, 0, 9);
        }
        // Sprite 0 overlapping opaque bg on scanline 4.
        p.set_oam_entry(
            0,
            OamEntry {
                y: 3,
                tile: 9,
                attr: 0,
                x: 40,
            },
        );
        p.render_frame();
        assert!(p.sprite0_hit(), "opaque overlap sets hit");
        assert_eq!(p.sprite0_hit_at(), Some((4, 40)));

        // Move sprite 0 below the opaque rows -> cleared and stays clear.
        p.begin_frame();
        p.set_oam_entry(
            0,
            OamEntry {
                y: 30,
                tile: 9,
                attr: 0,
                x: 40,
            },
        );
        p.render_frame();
        assert!(!p.sprite0_hit(), "no overlap, no hit");

        // Overlap again but with sprites disabled -> no hit.
        p.begin_frame();
        p.set_oam_entry(
            0,
            OamEntry {
                y: 3,
                tile: 9,
                attr: 0,
                x: 40,
            },
        );
        p.write_mask(0x0A); // bg on, sprites off
        p.render_frame();
        assert!(!p.sprite0_hit(), "hit needs sprites enabled");
        p.write_mask(0x1E);
    }

    #[test]
    fn sprite0_hit_ignores_transparent_sprite_pixels() {
        let mut p = blank_ppu();
        // Bg opaque, sprite 0 fully transparent (blank tile 0).
        p.load_chr_4k(0, 0, &flat_chr(8, 0xFF, 0x00)).unwrap();
        p.set_palette(1, 0x06);
        for tx in 0..32u8 {
            p.set_tile(0, tx, 0, 8);
        }
        p.set_oam_entry(
            0,
            OamEntry {
                y: 3,
                tile: 0,
                attr: 0,
                x: 40,
            },
        );
        p.render_frame();
        assert!(!p.sprite0_hit(), "transparent sprite pixel: no hit");
    }

    #[test]
    fn sprite0_hit_is_timed_against_the_beam() {
        let mut p = blank_ppu();
        p.load_chr_4k(0, 0, &flat_chr(9, 0xFF, 0x00)).unwrap();
        p.set_palette(1, 0x06);
        p.set_palette(0x11, 0x16);
        for tx in 0..32u8 {
            for row in 0..30u8 {
                p.set_tile(0, tx, row, 9);
            }
        }
        // Sprite 0 rows 40..47, x=100.
        p.set_oam_entry(
            0,
            OamEntry {
                y: 39,
                tile: 9,
                attr: 0,
                x: 100,
            },
        );
        p.set_beam(-1, 340);
        p.set_beam(20, 10);
        assert_eq!(p.read_status() & 0x40, 0, "before the sprite line: no hit");
        p.set_beam(40, 50);
        assert_eq!(p.read_status() & 0x40, 0, "on the line, before the pixel");
        p.set_beam(40, 110);
        assert_ne!(p.read_status() & 0x40, 0, "past the pixel: hit visible");
        p.set_beam(200, 0);
        assert_ne!(p.read_status() & 0x40, 0, "sticky for the frame");
        let _ = p.finish_frame();
        assert!(p.sprite0_hit(), "still set through vblank");
        p.begin_frame();
        assert!(!p.sprite0_hit(), "cleared at pre-render");
    }

    #[test]
    fn mid_frame_scroll_write_splits_the_frame() {
        let mut p = blank_ppu();
        // Column 0 -> tile 2 (sub 1), column 2 -> tile 3 (sub 2).
        let mut chr = flat_chr(2, 0xFF, 0x00);
        for row in 0..8 {
            chr[3 * 16 + row] = 0x00;
            chr[3 * 16 + row + 8] = 0xFF;
        }
        p.load_chr_4k(0, 0, &chr).unwrap();
        for row in 0..30u8 {
            p.set_tile(0, 0, row, 2);
            p.set_tile(0, 1, row, 2);
            p.set_tile(0, 2, row, 3);
        }
        p.set_palette(1, 0x01);
        p.set_palette(2, 0x02);
        // Frame starts at scroll 0; at line 100 the game writes scroll x=16.
        p.write_scroll(0);
        p.write_scroll(0);
        p.set_beam(-1, 340);
        p.set_beam(100, 20);
        let _ = p.read_status(); // reset the toggle like the game does
        p.write_scroll(16);
        p.write_scroll(0);
        let frame = p.finish_frame();
        // `$2005` lands in `t`; `v` picks the horizontal bits up at the
        // end of the current line (dot 257 reload), so the split shows from
        // the next line — exactly like hardware.
        assert_eq!(frame[0], 0x01, "top uses scroll 0");
        assert_eq!(frame[100 * WIDTH], 0x01, "write line still old scroll");
        assert_eq!(frame[101 * WIDTH], 0x02, "next line: scroll 16");
        assert_eq!(frame[200 * WIDTH], 0x02);
    }

    #[test]
    fn addr_write_mid_frame_moves_vertical_scroll() {
        let mut p = blank_ppu();
        // Row 0 -> tile 2 (sub 1); row 10 -> tile 3 (sub 2).
        let mut chr = flat_chr(2, 0xFF, 0x00);
        for row in 0..8 {
            chr[3 * 16 + row] = 0x00;
            chr[3 * 16 + row + 8] = 0xFF;
        }
        p.load_chr_4k(0, 0, &chr).unwrap();
        for col in 0..32u8 {
            p.set_tile(0, col, 0, 2);
            p.set_tile(0, col, 10, 3);
        }
        p.set_palette(1, 0x01);
        p.set_palette(2, 0x02);
        p.set_beam(-1, 340);
        p.set_beam(50, 300);
        // $2006 pair pointing at tile row 10 (coarse Y = 10 -> $2140).
        let _ = p.read_status();
        p.write_addr(0x21);
        p.write_addr(0x40);
        let frame = p.finish_frame();
        assert_eq!(frame[0], 0x01, "line 0 shows row 0");
        assert_eq!(frame[50 * WIDTH], 0x0F, "line 50 (row 6) is blank");
        assert_eq!(frame[51 * WIDTH], 0x02, "after the write: row 10 tiles");
    }

    #[test]
    fn blanked_screen_shows_palette_entry_under_v() {
        let mut p = Ppu::new();
        p.set_palette(0, 0x0F);
        p.set_palette(0x13, 0x22);
        p.write_mask(0x00); // rendering off
        p.write_addr(0x3F);
        p.write_addr(0x13); // v = $3F13
        let frame = p.render_frame();
        assert!(frame.iter().all(|&px| px == 0x22), "hijacked backdrop");
        p.write_addr(0x00);
        p.write_addr(0x00);
        let frame = p.render_frame();
        assert!(
            frame.iter().all(|&px| px == 0x0F),
            "v outside palette: entry 0"
        );
    }

    #[test]
    fn grayscale_collapses_palette_indices() {
        let mut p = blank_ppu();
        p.load_chr_4k(0, 0, &flat_chr(4, 0xFF, 0xFF)).unwrap(); // sub 3
        p.set_tile(0, 0, 0, 4);
        p.set_palette(3, 0x27);
        p.write_mask(0x1F); // + greyscale
        let frame = p.render_frame();
        assert_eq!(frame[0], 0x27 & 0x30, "greyscale masks index");
    }

    #[test]
    fn left8_clip_hides_edges() {
        let mut p = blank_ppu();
        p.load_chr_4k(0, 0, &flat_chr(4, 0xFF, 0x00)).unwrap();
        for tx in 0..32u8 {
            p.set_tile(0, tx, 0, 4);
        }
        p.set_palette(1, 0x09);
        p.write_mask(0x08); // bg on, left-8 bg off
        let frame = p.render_frame();
        assert_eq!(frame[0], 0x0F, "left 8 hidden");
        assert_eq!(frame[8], 0x09, "column 8 shows");
    }

    #[test]
    fn horizontal_scroll_wraps_into_next_nametable() {
        let mut p = blank_ppu();
        p.set_mirroring(crate::Mirroring::Vertical);
        let mut chr = flat_chr(2, 0xFF, 0x00);
        for row in 0..8 {
            chr[3 * 16 + row] = 0x00;
            chr[3 * 16 + row + 8] = 0xFF;
        }
        p.load_chr_4k(0, 0, &chr).unwrap();
        for row in 0..30u8 {
            p.set_tile(0, 31, row, 2); // last column of NT0 -> sub 1
            p.set_tile(1, 0, row, 3); // first column of NT1 -> sub 2
        }
        p.set_palette(1, 0x01);
        p.set_palette(2, 0x02);
        p.write_scroll(248);
        p.write_scroll(0);
        let frame = p.render_frame();
        assert_eq!(frame[0], 0x01, "x=0 -> NT0 column 31");
        assert_eq!(frame[8], 0x02, "x=8 -> NT1 column 0");
    }

    #[test]
    fn diff_counts_and_reports_first_coords() {
        let a = [0x0Fu8; WIDTH * HEIGHT];
        let mut b = a;
        b[7 * WIDTH + 5] = 0x01;
        b[100 * WIDTH + 200] = 0x02;
        let d = diff_indexed(&a, &b);
        assert_eq!(d.count, 2);
        assert!(!d.is_clean());
        assert_eq!(d.first_len, 2);
        assert_eq!(d.first[0], (5, 7));
        assert_eq!(d.first[1], (200, 100));
        assert!(diff_indexed(&a, &a).is_clean());
    }

    #[test]
    fn diff_ppm_marks_mismatches_magenta() {
        let a = [0x0Fu8; WIDTH * HEIGHT];
        let mut b = a;
        b[0] = 0x01;
        let mut out = Vec::new();
        write_diff_ppm(&a, &b, &mut out);
        let header = format!("P6\n{WIDTH} {HEIGHT}\n255\n");
        assert!(out.starts_with(header.as_bytes()));
        let body = &out[header.len()..];
        assert_eq!(body.len(), WIDTH * HEIGHT * 3);
        assert_eq!(&body[0..3], &[255, 0, 255], "mismatch -> magenta");
        let [r, g, bl, _] = indexed_to_rgba(0x0F);
        assert_eq!(&body[3..6], &[r, g, bl], "match -> display colour");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::state::CHR_BANK_LEN;

    /// Overworld-style scroll: horizontal mirroring, base NT2, fine x=1,
    /// coarse 31, every column of table B the same opaque tile whose row 0
    /// is `19 29 29 29 19 29 29 29` (sub 0 = colour 1, others colour 2).
    #[test]
    fn right_edge_tiles_render_consistently_across_lines() {
        let mut p = Ppu::new();
        p.set_mirroring(crate::Mirroring::Horizontal);
        p.write_mask(0x1E);
        p.set_palette(0, 0x0F);
        p.set_palette(1, 0x19);
        p.set_palette(2, 0x29);
        // Tile $6D: sub 1 at column 0 and 4, sub 2 elsewhere, every row.
        let mut chr = [0u8; CHR_BANK_LEN];
        for row in 0..8 {
            chr[0x6D * 16 + row] = 0b1000_1000; // low plane: cols 0,4
            chr[0x6D * 16 + row + 8] = 0b0111_0111; // high plane: others
        }
        p.load_chr_4k(1, 1, &chr).unwrap();
        for col in 0..32u8 {
            for row in 0..30u8 {
                p.set_tile(2, col, row, 0x6D);
            }
        }
        p.write_ctrl(0x12); // bg table 1, base NT2
        p.write_scroll(249);
        p.write_scroll(0);
        // Lazy start deep into the frame, like the game's first access.
        p.set_beam(-1, 340);
        p.set_beam(207, 308);
        let frame = p.finish_frame();
        let row = |y: usize| frame[y * WIDTH + 240..y * WIDTH + 256].to_vec();
        let expect = vec![
            0x29, 0x29, 0x29, 0x19, 0x29, 0x29, 0x29,
            0x19, // x 240-247: col 29 sub1-7, col 30 sub0
            0x29, 0x29, 0x29, 0x19, 0x29, 0x29, 0x29,
            0x19, // x 248-255: col 30 sub1-7, col 31 sub0
        ];
        assert_eq!(row(0), expect, "row 0 right edge");
        assert_eq!(row(1), expect, "row 1 right edge");
        assert_eq!(row(100), expect, "row 100 right edge");
        assert_eq!(
            &frame[0..8],
            &[0x29, 0x29, 0x29, 0x19, 0x29, 0x29, 0x29, 0x19],
            "left edge"
        );
    }
}
