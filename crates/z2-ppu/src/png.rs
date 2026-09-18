//! Minimal std-only PNG encoder for indexed frames (diagnostics only).
//!
//! Pixels go through the display-only NES master palette
//! ([`crate::palette::indexed_to_rgba`]) — never used for verification.
//! The zlib stream uses stored (uncompressed) blocks so no deflate
//! dependency is needed; output is a spec-valid PNG (magic + IHDR + IDAT
//! + IEND with correct CRC32/Adler32).

use crate::palette::indexed_to_rgba;
use crate::{IndexedFrame, HEIGHT, WIDTH};

/// Encode an indexed 256x240 frame as an 8-bit true-colour PNG.
#[must_use]
pub fn encode_indexed_png(indexed: &IndexedFrame) -> Vec<u8> {
    encode_indexed_png_wh(WIDTH, HEIGHT, indexed)
}

/// Encode a `width x height` indexed image (row-major NES palette indices,
/// e.g. a [`crate::wide::WideFrame`]) as an 8-bit true-colour PNG.
///
/// # Panics
///
/// When `indexed.len() != width * height`, or a dimension is zero or does
/// not fit in `u32`.
#[must_use]
pub fn encode_indexed_png_wh(width: usize, height: usize, indexed: &[u8]) -> Vec<u8> {
    assert!(width > 0 && height > 0, "PNG dimensions must be non-zero");
    assert_eq!(
        indexed.len(),
        width * height,
        "indexed length != width*height"
    );
    let w32 = u32::try_from(width).expect("width fits u32");
    let h32 = u32::try_from(height).expect("height fits u32");
    // Raw scanlines: filter byte 0 + RGB triplets.
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for row in indexed.chunks_exact(width) {
        raw.push(0u8);
        for &px in row {
            let [r, g, b, _] = indexed_to_rgba(px);
            raw.extend_from_slice(&[r, g, b]);
        }
    }
    // zlib wrapper around stored blocks (max 65535 bytes each).
    let mut zlib = vec![0x78u8, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(if i + 1 == blocks.len() { 0x01 } else { 0x00 });
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = Vec::with_capacity(8 + 25 + zlib.len() + 12);
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w32.to_be_bytes());
    ihdr.extend_from_slice(&h32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit truecolour
    png_chunk(b"IHDR", &ihdr, &mut out);
    png_chunk(b"IDAT", &zlib, &mut out);
    png_chunk(b"IEND", &[], &mut out);
    out
}

fn png_chunk(tag: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut c = Vec::with_capacity(4 + data.len());
    c.extend_from_slice(tag);
    c.extend_from_slice(data);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// IEEE CRC32 (table-driven).
fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, slot) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Adler-32 for the zlib trailer.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_magic_and_ihdr() {
        let frame = [0x0Fu8; WIDTH * HEIGHT];
        let png = encode_indexed_png(&frame);
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes([png[16], png[17], png[18], png[19]]),
            256
        );
        assert_eq!(
            u32::from_be_bytes([png[20], png[21], png[22], png[23]]),
            240
        );
    }

    #[test]
    fn png_wh_writes_given_dimensions() {
        let img = vec![0x16u8; 432 * 240];
        let png = encode_indexed_png_wh(432, 240, &img);
        assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes([png[16], png[17], png[18], png[19]]),
            432
        );
        assert_eq!(
            u32::from_be_bytes([png[20], png[21], png[22], png[23]]),
            240
        );
        let frame = [0x0Fu8; WIDTH * HEIGHT];
        assert_eq!(
            encode_indexed_png(&frame),
            encode_indexed_png_wh(WIDTH, HEIGHT, &frame),
            "wrapper is the 256x240 case"
        );
    }

    #[test]
    #[should_panic(expected = "indexed length")]
    fn png_wh_rejects_wrong_length() {
        let _ = encode_indexed_png_wh(10, 10, &[0u8; 99]);
    }

    #[test]
    fn crc32_matches_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
