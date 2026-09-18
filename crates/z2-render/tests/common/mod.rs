//! Shared helpers for z2-render integration tests. Synthetic data only: no
//! ROM-derived bytes (LEGAL.md §1).
#![allow(dead_code)]

use z2_render::{encode_png_rgba, CHR_PAGE_LEN};

/// Procedural CHR image of `pages` 4 KiB pages; every tile has at least one
/// opaque pixel (top-left), so every tile registers on a page sheet.
pub fn synthetic_chr(pages: usize) -> Vec<u8> {
    let mut v = vec![0u8; pages * CHR_PAGE_LEN];
    let mut s: u32 = 0x1234_5678;
    for b in &mut v {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *b = s as u8;
    }
    for p in 0..pages {
        for t in 0..256 {
            v[p * CHR_PAGE_LEN + t * 16] |= 0x80;
        }
    }
    v
}

/// A `128 * scale` square page sheet painted per cell pixel.
pub fn page_sheet_png(scale: u32, paint: impl Fn(u8, u32, u32) -> [u8; 4]) -> Vec<u8> {
    let cell = 8 * scale;
    let edge = 16 * cell;
    let mut rgba = Vec::with_capacity((edge * edge * 4) as usize);
    for y in 0..edge {
        for x in 0..edge {
            let t = ((y / cell) * 16 + x / cell) as u8;
            rgba.extend_from_slice(&paint(t, x % cell, y % cell));
        }
    }
    encode_png_rgba(edge, edge, &rgba, &[]).unwrap()
}

/// A `w` x `h` PNG filled with one pixel value.
pub fn solid_png(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
    let rgba: Vec<u8> = (0..w * h).flat_map(|_| px).collect();
    encode_png_rgba(w, h, &rgba, &[]).unwrap()
}

/// Build an owned file list.
pub fn files(list: Vec<(&str, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    list.into_iter().map(|(p, b)| (p.to_string(), b)).collect()
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}
