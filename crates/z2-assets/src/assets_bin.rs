//! `assets.bin` container codec.
//!
//! Layout (all integers little-endian, deterministic — fixed table order,
//! no timestamps, no hashes-as-keys):
//!
//! ```text
//! 0x00  magic        "Z2AS" (4 bytes)
//! 0x04  version      u16 (= FORMAT_VERSION)
//! 0x06  flags        u16 (= 0; reserved)
//! 0x08  body_crc32   u32 (CRC32 of the source ROM body)
//! 0x0C  body_sha1    [u8; 20] (SHA1 of the source ROM body)
//! 0x20  count        u16 (number of section records)
//! 0x22  records      count x { id u16, rom_off u32, len u32, data_off u32 }
//! ...   blobs        concatenated section bytes in record order
//! ```
//!
//! Sections may overlap in `rom_off` (the PRG bank images intentionally
//! duplicate the fine-grained table bytes); `data_off` blobs must be
//! contiguous and exactly fill the file tail.
//!
//! Core-only (`core` + `alloc`): no `std` paths in this file so the loader
//! stays `no_std`-friendly (alloc only) and `wasm32`-compatible.

extern crate alloc;

use alloc::vec::Vec;

use crate::extract::RawSection;
use crate::extract_tables::SECTION_TABLE;
use crate::rom::EXPECTED_BODY_LEN;

/// Container magic.
pub const MAGIC: [u8; 4] = *b"Z2AS";
/// Container format version written by this codec.
pub const FORMAT_VERSION: u16 = 1;
/// Size of the fixed header (magic + version + flags + crc + sha + count).
pub const HEADER_LEN: usize = 34;
/// Size of one section record.
pub const RECORD_LEN: usize = 14;

/// Encode extracted sections into an `assets.bin` image.
pub fn encode(sections: &[RawSection<'_>], body_crc32: u32, body_sha1: [u8; 20]) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(HEADER_LEN + RECORD_LEN * sections.len() + total_len(sections));
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // flags, reserved
    out.extend_from_slice(&body_crc32.to_le_bytes());
    out.extend_from_slice(&body_sha1);
    out.extend_from_slice(&(sections.len() as u16).to_le_bytes());

    let mut data_off = (HEADER_LEN + RECORD_LEN * sections.len()) as u32;
    for s in sections {
        out.extend_from_slice(&s.def.id.to_le_bytes());
        out.extend_from_slice(&s.def.file_off.to_le_bytes());
        out.extend_from_slice(&(s.bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&data_off.to_le_bytes());
        data_off += s.bytes.len() as u32;
    }
    for s in sections {
        out.extend_from_slice(s.bytes);
    }
    out
}

fn total_len(sections: &[RawSection<'_>]) -> usize {
    sections.iter().map(|s| s.bytes.len()).sum()
}

/// A decoded `assets.bin` image borrowing its backing bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assets<'a> {
    bytes: &'a [u8],
    body_crc32: u32,
    body_sha1: [u8; 20],
    count: usize,
}

impl<'a> Assets<'a> {
    /// CRC32 of the source ROM body recorded at extraction time.
    #[must_use]
    pub fn body_crc32(&self) -> u32 {
        self.body_crc32
    }

    /// SHA1 of the source ROM body recorded at extraction time.
    #[must_use]
    pub fn body_sha1(&self) -> [u8; 20] {
        self.body_sha1
    }

    /// Number of section records.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.count
    }

    /// Raw record header `(id, rom_off, len)` for every section, in order.
    #[must_use]
    pub fn records(&self) -> Vec<(u16, u32, u32)> {
        let mut v = Vec::with_capacity(self.count);
        for i in 0..self.count {
            let o = HEADER_LEN + RECORD_LEN * i;
            let id = u16::from_le_bytes([self.bytes[o], self.bytes[o + 1]]);
            let rom_off = u32::from_le_bytes([
                self.bytes[o + 2],
                self.bytes[o + 3],
                self.bytes[o + 4],
                self.bytes[o + 5],
            ]);
            let len = u32::from_le_bytes([
                self.bytes[o + 6],
                self.bytes[o + 7],
                self.bytes[o + 8],
                self.bytes[o + 9],
            ]);
            v.push((id, rom_off, len));
        }
        v
    }

    /// Fetch a section's bytes by id.
    #[must_use]
    pub fn get(&self, id: u16) -> Option<&'a [u8]> {
        for i in 0..self.count {
            let o = HEADER_LEN + RECORD_LEN * i;
            let rid = u16::from_le_bytes([self.bytes[o], self.bytes[o + 1]]);
            if rid != id {
                continue;
            }
            let len = u32::from_le_bytes([
                self.bytes[o + 6],
                self.bytes[o + 7],
                self.bytes[o + 8],
                self.bytes[o + 9],
            ]) as usize;
            let data_off = u32::from_le_bytes([
                self.bytes[o + 10],
                self.bytes[o + 11],
                self.bytes[o + 12],
                self.bytes[o + 13],
            ]) as usize;
            return self.bytes.get(data_off..data_off.saturating_add(len));
        }
        None
    }

    /// One 4 KiB CHR page (`0..32`) from the CHR ROM section.
    #[must_use]
    pub fn chr_page(&self, page: usize) -> Option<&'a [u8]> {
        let chr = self.get(crate::extract_tables::section_id::CHR_ROM)?;
        chr.get(page.checked_mul(4096)?..page.checked_mul(4096)?.checked_add(4096)?)
    }
}

/// `assets.bin` decode failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// File too short to hold the fixed header.
    TruncatedHeader,
    /// First four bytes are not `Z2AS`.
    BadMagic,
    /// Unsupported format version.
    UnsupportedVersion { found: u16 },
    /// A record runs past the end of the file.
    TruncatedRecords,
    /// Section blobs are not contiguous / do not fill the file tail.
    BadBlobLayout,
    /// A blob lies outside the file.
    BlobOutOfBounds,
    /// Trailing bytes after the last blob.
    TrailingBytes,
    /// Declared section count exceeds the table the game knows.
    TooManySections { found: usize },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::TruncatedHeader => write!(f, "assets.bin too short for header"),
            DecodeError::BadMagic => write!(f, "assets.bin has bad magic (want Z2AS)"),
            DecodeError::UnsupportedVersion { found } => {
                write!(
                    f,
                    "unsupported assets.bin version {found} (want {FORMAT_VERSION})"
                )
            }
            DecodeError::TruncatedRecords => write!(f, "assets.bin section records truncated"),
            DecodeError::BadBlobLayout => {
                write!(
                    f,
                    "assets.bin blobs are not contiguous from the record table"
                )
            }
            DecodeError::BlobOutOfBounds => write!(f, "assets.bin blob outside file bounds"),
            DecodeError::TrailingBytes => write!(f, "assets.bin has trailing bytes"),
            DecodeError::TooManySections { found } => {
                write!(f, "assets.bin declares {found} sections (table is smaller)")
            }
        }
    }
}

fn read_u16(bytes: &[u8], off: usize) -> Option<u16> {
    bytes
        .get(off..off.saturating_add(2))
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_le_bytes)
}

fn read_u32(bytes: &[u8], off: usize) -> Option<u32> {
    bytes
        .get(off..off.saturating_add(4))
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes)
}

/// Decode and fully validate an `assets.bin` image.
pub fn decode(bytes: &[u8]) -> Result<Assets<'_>, DecodeError> {
    if bytes.len() < HEADER_LEN {
        return Err(DecodeError::TruncatedHeader);
    }
    if bytes[0..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = read_u16(bytes, 4).ok_or(DecodeError::TruncatedHeader)?;
    if version != FORMAT_VERSION {
        return Err(DecodeError::UnsupportedVersion { found: version });
    }
    let body_crc32 = read_u32(bytes, 8).ok_or(DecodeError::TruncatedHeader)?;
    let mut body_sha1 = [0u8; 20];
    body_sha1.copy_from_slice(bytes.get(12..32).ok_or(DecodeError::TruncatedHeader)?);
    let count = read_u16(bytes, 32).ok_or(DecodeError::TruncatedHeader)? as usize;
    if count > SECTION_TABLE.len() + 64 {
        return Err(DecodeError::TooManySections { found: count });
    }

    let records_end = HEADER_LEN + RECORD_LEN * count;
    if bytes.len() < records_end {
        return Err(DecodeError::TruncatedRecords);
    }

    // Blobs must be contiguous from the end of the record table and exactly
    // fill the file; each blob must fit the source ROM body model.
    let mut cursor = records_end;
    for i in 0..count {
        let o = HEADER_LEN + RECORD_LEN * i;
        let _id = read_u16(bytes, o).ok_or(DecodeError::TruncatedRecords)?;
        let rom_off = read_u32(bytes, o + 2).ok_or(DecodeError::TruncatedRecords)?;
        let len = read_u32(bytes, o + 6).ok_or(DecodeError::TruncatedRecords)? as usize;
        let data_off = read_u32(bytes, o + 10).ok_or(DecodeError::TruncatedRecords)? as usize;
        if data_off != cursor {
            return Err(DecodeError::BadBlobLayout);
        }
        let end = data_off.saturating_add(len);
        if end > bytes.len() || end < data_off {
            return Err(DecodeError::BlobOutOfBounds);
        }
        // Provenance sanity: the record must fit a 262160-byte headered image.
        if rom_off.saturating_add(len as u32) as usize
            > EXPECTED_BODY_LEN + crate::rom::INES_HEADER_LEN
        {
            return Err(DecodeError::BlobOutOfBounds);
        }
        cursor = end;
    }
    if cursor != bytes.len() {
        return Err(DecodeError::TrailingBytes);
    }

    Ok(Assets {
        bytes,
        body_crc32,
        body_sha1,
        count,
    })
}
