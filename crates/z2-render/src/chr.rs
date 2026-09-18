//! CHR page and tile decoding, and page-sheet painting.
//!
//! The cartridge CHR image is 32 pages of 4 KiB; a page is 256 tiles of 16
//! bytes (8 bytes low bit-plane, 8 bytes high bit-plane, MSB = leftmost
//! pixel). A page sheet lays the page's tiles out on a 16x16 grid: tile `t`
//! sits at column `t % 16`, row `t / 16`, each cell `8 * scale` pixels
//! square. 8x16 sprites are two tiles `t` and `t + 1`, so on a sheet they are
//! two horizontally adjacent cells (top half left, bottom half right).

use crate::png_io::RgbaImage;

/// Bytes per CHR page.
pub const CHR_PAGE_LEN: usize = 4096;
/// Pages in the Zelda II CHR image (128 KiB).
pub const CHR_PAGES: usize = 32;
/// Tiles per page.
pub const TILES_PER_PAGE: usize = 256;

/// Borrow page `page` from a CHR image, if it is fully present.
#[must_use]
pub fn chr_page(chr_rom: &[u8], page: u8) -> Option<&[u8]> {
    let start = page as usize * CHR_PAGE_LEN;
    chr_rom.get(start..start + CHR_PAGE_LEN)
}

/// Decode tile `tile` of a 4 KiB page into 64 row-major sub-palette indices (0-3).
///
/// Panics when `page_bytes` is shorter than [`CHR_PAGE_LEN`].
#[must_use]
pub fn tile_subpixels(page_bytes: &[u8], tile: u8) -> [u8; 64] {
    let base = tile as usize * 16;
    let lo = &page_bytes[base..base + 8];
    let hi = &page_bytes[base + 8..base + 16];
    let mut out = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            let bit = 7 - x;
            out[y * 8 + x] = ((lo[y] >> bit) & 1) | (((hi[y] >> bit) & 1) << 1);
        }
    }
    out
}

/// Nearest-upscale one tile into `8 * scale` square RGBA: sub-palette 0 is
/// transparent, 1..=3 take `rgb[sub - 1]` at alpha 255.
#[must_use]
pub fn upscale_tile_rgba(sub: &[u8; 64], rgb: [[u8; 3]; 3], scale: u32) -> Vec<u8> {
    let s = scale as usize;
    let cell = 8 * s;
    let mut out = vec![0u8; cell * cell * 4];
    for py in 0..cell {
        for px in 0..cell {
            let v = sub[(py / s) * 8 + px / s];
            if v != 0 {
                let [r, g, b] = rgb[(v - 1) as usize];
                let i = (py * cell + px) * 4;
                out[i..i + 4].copy_from_slice(&[r, g, b, 0xFF]);
            }
        }
    }
    out
}

/// Paint a `128 * scale` square page sheet from one CHR page.
///
/// `rgb_for_tile(t)` supplies the three opaque colours for tile `t`, or
/// `None` to leave the cell fully transparent (not part of the sheet).
///
/// Panics when `page_bytes` is shorter than [`CHR_PAGE_LEN`].
pub fn paint_page_sheet(
    page_bytes: &[u8],
    scale: u32,
    mut rgb_for_tile: impl FnMut(u8) -> Option<[[u8; 3]; 3]>,
) -> RgbaImage {
    let cell = 8 * scale;
    let mut img = RgbaImage::new(16 * cell, 16 * cell);
    let width = img.width as usize;
    for t in 0..=255u8 {
        let Some(rgb) = rgb_for_tile(t) else {
            continue;
        };
        let pixels = upscale_tile_rgba(&tile_subpixels(page_bytes, t), rgb, scale);
        let x0 = (t as usize % 16) * cell as usize;
        let y0 = (t as usize / 16) * cell as usize;
        let c = cell as usize;
        for row in 0..c {
            let dst = ((y0 + row) * width + x0) * 4;
            img.rgba[dst..dst + c * 4].copy_from_slice(&pixels[row * c * 4..(row + 1) * c * 4]);
        }
    }
    img
}
