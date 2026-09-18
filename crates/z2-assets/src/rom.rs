//! ROM hash gate.
//!
//! The original Zelda II NES ROM is **never** committed to this repository
//! (see `LEGAL.md`). Every tool that needs ROM bytes obtains them through
//! [`open`], which reads the user-supplied file named by the `Z2_ROM`
//! environment variable and verifies it against the known-good No-Intro
//! USA dump before returning a single byte.
//!
//! Verification procedure:
//! 1. Read the whole file.
//! 2. If it starts with the 16-byte iNES header (`NES\x1a`), strip it.
//!    Both headered (262160 bytes) and headerless (262144 bytes) copies are
//!    accepted as long as the body matches.
//! 3. Check the body length (262144 bytes), CRC32, and SHA1.
//! 4. Anything else is rejected with an error that prints the expected hashes.
//!
//! Hash notes:
//! - The body (header-stripped) hashes are the canonical No-Intro USA values.
//! - An early project note quoted headered-file CRC32 `E3C788B0`, but the actual local
//!   reference ROM hashes to headered CRC32 `861C3FE6`. Both constants are
//!   recorded here for forensics; the gate itself checks **body** hashes, so
//!   either header variant passes as long as the body is correct.
//!
//! This module is intentionally dependency-free (`std` only) so it builds
//! for both host and `wasm32-unknown-unknown` without extra crates.

use std::fmt;
use std::fs;
use std::path::Path;

/// Environment variable naming the user-supplied ROM file.
pub const ROM_ENV_VAR: &str = "Z2_ROM";

/// Magic bytes at the start of a headered (iNES) ROM file.
pub const INES_MAGIC: [u8; 4] = *b"NES\x1a";

/// Length of the iNES header in bytes.
pub const INES_HEADER_LEN: usize = 16;

/// Expected body length in bytes: 128 KiB PRG + 128 KiB CHR (SNROM).
pub const EXPECTED_BODY_LEN: usize = 262144;

/// Expected CRC32 (IEEE) of the header-stripped body: No-Intro USA.
pub const EXPECTED_BODY_CRC32: u32 = 0xBA32_2865;

/// Expected SHA1 (hex) of the header-stripped body: No-Intro USA.
pub const EXPECTED_BODY_SHA1_HEX: &str = "11333adb723a5975e0ecca3aee8f4747aa8d2d26";

/// Headered-file CRC32 actually observed on the local reference ROM.
pub const OBSERVED_HEADERED_CRC32: u32 = 0x861C_3FE6;

/// Headered-file CRC32 quoted in an early project note; never observed locally.
/// Kept for forensics — the gate checks body hashes, not this value.
pub const CLAIMED_HEADERED_CRC32: u32 = 0xE3C7_88B0;

/// SHA1 (hex) of the known-good *headered* reference file (16-byte iNES
/// header + body). Used by the pre-commit hook; not part of the gate itself.
pub const OBSERVED_HEADERED_SHA1_HEX: &str = "08fa60f23e477752fdd850b1fd4171c6ca025127";

/// Failure to locate, read, or verify the user-supplied ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RomError {
    /// `Z2_ROM` is not set.
    MissingEnvVar,
    /// The file could not be read.
    Io(String),
    /// The body has an unexpected length.
    UnexpectedLength { expected: usize, actual: usize },
    /// A hash check failed.
    HashMismatch {
        what: &'static str,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for RomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RomError::MissingEnvVar => write!(
                f,
                "Z2_ROM is not set: point it at your own Zelda II (USA) ROM dump, e.g. \
                 Z2_ROM=/path/to/zelda2.nes. The ROM is never committed to this repo (see LEGAL.md)."
            ),
            RomError::Io(msg) => write!(f, "could not read ROM file: {msg}"),
            RomError::UnexpectedLength { expected, actual } => write!(
                f,
                "ROM has unexpected size: got {actual} bytes, want a {expected}-byte body \
                 (headerless) or a {}-byte file (16-byte iNES header + body). \
                 Expected No-Intro USA dump: body CRC32 {:08X}, body SHA1 {EXPECTED_BODY_SHA1_HEX}.",
                expected + INES_HEADER_LEN,
                EXPECTED_BODY_CRC32
            ),
            RomError::HashMismatch { what, expected, actual } => write!(
                f,
                "ROM {what} mismatch: got {actual}, want {expected}. \
                 This does not look like Zelda II (USA, No-Intro). Expected body \
                 CRC32 {:08X} and body SHA1 {EXPECTED_BODY_SHA1_HEX} \
                 (verify against the header-stripped image; a 16-byte iNES header \
                 is stripped automatically when present).",
                EXPECTED_BODY_CRC32
            ),
        }
    }
}

impl std::error::Error for RomError {}

/// Open the user-supplied ROM named by `Z2_ROM`, verify it, and return the
/// header-stripped body bytes.
///
/// # Errors
///
/// Returns [`RomError::MissingEnvVar`] when `Z2_ROM` is unset, [`RomError::Io`]
/// when the file cannot be read, or a length/hash error identifying the
/// expected No-Intro USA hashes when the file is not the right ROM.
pub fn open() -> Result<Vec<u8>, RomError> {
    let path = std::env::var(ROM_ENV_VAR).map_err(|_| RomError::MissingEnvVar)?;
    open_at(Path::new(&path))
}

/// Open, verify, and return the header-stripped body of the ROM file at `path`.
///
/// A leading 16-byte iNES header (magic `NES\x1a`) is stripped when present;
/// otherwise the file must already be the bare 262144-byte body.
pub fn open_at(path: &Path) -> Result<Vec<u8>, RomError> {
    let file = fs::read(path).map_err(|e| RomError::Io(format!("{}: {e}", path.display())))?;
    let body = strip_ines_header(&file);
    verify_body(body)?;
    Ok(body.to_vec())
}

/// Strip the 16-byte iNES header when the magic `NES\x1a` is present;
/// otherwise return the input unchanged.
#[must_use]
pub fn strip_ines_header(file_bytes: &[u8]) -> &[u8] {
    if file_bytes.len() >= INES_HEADER_LEN && file_bytes[0..4] == INES_MAGIC {
        &file_bytes[INES_HEADER_LEN..]
    } else {
        file_bytes
    }
}

/// Verify a header-stripped body against the expected length, CRC32, and SHA1.
pub fn verify_body(body: &[u8]) -> Result<(), RomError> {
    if body.len() != EXPECTED_BODY_LEN {
        return Err(RomError::UnexpectedLength {
            expected: EXPECTED_BODY_LEN,
            actual: body.len(),
        });
    }
    let crc = crc32_ieee(body);
    if crc != EXPECTED_BODY_CRC32 {
        return Err(RomError::HashMismatch {
            what: "CRC32",
            expected: format!("{:08X}", EXPECTED_BODY_CRC32),
            actual: format!("{crc:08X}"),
        });
    }
    let sha = sha1_hex(body);
    if sha != EXPECTED_BODY_SHA1_HEX {
        return Err(RomError::HashMismatch {
            what: "SHA1",
            expected: EXPECTED_BODY_SHA1_HEX.to_string(),
            actual: sha,
        });
    }
    Ok(())
}

/// CRC32 (IEEE 802.3, polynomial 0xEDB88320) over `data`.
#[must_use]
pub fn crc32_ieee(data: &[u8]) -> u32 {
    // Bitwise (table-free) implementation: no lookup table to keep in sync,
    // plenty fast for a 256 KiB image, and dependency-free for wasm builds.
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = crc & 1;
            crc >>= 1;
            if mask != 0 {
                crc ^= 0xEDB8_8320;
            }
        }
    }
    !crc
}

/// SHA1 digest of `data` (FIPS 180-4), dependency-free for wasm builds.
#[must_use]
pub fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// Lowercase hex SHA1 of `data`.
#[must_use]
pub fn sha1_hex(data: &[u8]) -> String {
    let digest = sha1_digest(data);
    let mut s = String::with_capacity(40);
    for b in digest {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('?'));
        s.push(char::from_digit(u32::from(b & 0xF), 16).unwrap_or('?'));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vector() {
        // Standard check value for CRC32/ISO-HDLC.
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0x0000_0000);
    }

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn strip_header_only_with_magic() {
        let mut headered = vec![0u8; INES_HEADER_LEN + 8];
        headered[0..4].copy_from_slice(b"NES\x1a");
        assert_eq!(strip_ines_header(&headered).len(), 8);
        let bare = vec![0xAAu8; 100];
        assert_eq!(strip_ines_header(&bare), bare.as_slice());
        // Too short to hold a header: left alone.
        assert_eq!(strip_ines_header(b"NES"), b"NES");
    }

    #[test]
    fn wrong_body_fails_with_expected_hashes() {
        let body = vec![0u8; EXPECTED_BODY_LEN];
        let err = verify_body(&body).expect_err("zeroed body must not verify");
        let msg = err.to_string();
        assert!(
            msg.contains("BA322865"),
            "message prints expected CRC32: {msg}"
        );
        assert!(
            msg.contains(EXPECTED_BODY_SHA1_HEX),
            "message prints expected SHA1: {msg}"
        );
    }

    #[test]
    fn wrong_length_fails() {
        let err = verify_body(b"too short").expect_err("short body must not verify");
        assert!(matches!(err, RomError::UnexpectedLength { .. }));
        assert!(err.to_string().contains("BA322865"));
    }

    #[test]
    fn open_at_rejects_non_rom_file() {
        let path = std::env::temp_dir().join("z2rs-rom-gate-test.bin");
        std::fs::write(&path, b"not a rom").unwrap();
        let err = open_at(&path).expect_err("junk file must not verify");
        assert!(matches!(err, RomError::UnexpectedLength { .. }));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_at_strips_header_before_verifying() {
        // Headered junk must fail on the *body* hash/length path, proving the
        // 16-byte header was stripped (17 junk bytes stay 17, not 33).
        let mut file = Vec::from(&b"NES\x1a............"[..]);
        file.extend_from_slice(b"0123456789abcdefg");
        let path = std::env::temp_dir().join("z2rs-rom-gate-header-test.bin");
        std::fs::write(&path, &file).unwrap();
        let err = open_at(&path).expect_err("headered junk must not verify");
        match err {
            RomError::UnexpectedLength { actual, .. } => assert_eq!(actual, 17),
            other => panic!("expected length error, got {other}"),
        }
        std::fs::remove_file(&path).ok();
    }
}
