//! IPS patch reader, for bring-your-own sprite patches.
//!
//! z2rs ships no sprite art. Players may pass their own IPS file
//! (`--sprite-ips`); the `cosmetic` module decides how its records map onto
//! the image. This module only parses and applies records to a byte buffer.
//!
//! Format: `PATCH`, then records of a 3-byte big-endian offset and a 2-byte
//! size followed by that many bytes; size 0 means an RLE record (2-byte
//! count, 1 byte value). `EOF` ends the list (an optional 3-byte truncate
//! length after it is ignored).

use crate::RandoError;

/// One IPS record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Offset into the patched file (the file includes its iNES header).
    pub offset: usize,
    /// Bytes to write there.
    pub data: Vec<u8>,
}

/// Parse an IPS patch.
pub fn parse(patch: &[u8]) -> Result<Vec<Record>, RandoError> {
    let bad = |m: &str| RandoError::Ips(m.to_string());
    if patch.len() < 8 || &patch[..5] != b"PATCH" {
        return Err(bad("not an IPS patch (missing PATCH header)"));
    }
    let mut pos = 5;
    let mut out = Vec::new();
    loop {
        let head = patch
            .get(pos..pos + 3)
            .ok_or_else(|| bad("truncated record"))?;
        if head == b"EOF" {
            return Ok(out);
        }
        let offset =
            (usize::from(head[0]) << 16) | (usize::from(head[1]) << 8) | usize::from(head[2]);
        let size_b = patch
            .get(pos + 3..pos + 5)
            .ok_or_else(|| bad("truncated record"))?;
        let size = usize::from(u16::from_be_bytes([size_b[0], size_b[1]]));
        pos += 5;
        if size == 0 {
            let rle = patch
                .get(pos..pos + 3)
                .ok_or_else(|| bad("truncated RLE record"))?;
            let count = usize::from(u16::from_be_bytes([rle[0], rle[1]]));
            out.push(Record {
                offset,
                data: vec![rle[2]; count],
            });
            pos += 3;
        } else {
            let data = patch
                .get(pos..pos + size)
                .ok_or_else(|| bad("truncated data"))?;
            out.push(Record {
                offset,
                data: data.to_vec(),
            });
            pos += size;
        }
    }
}

/// Apply records to `image`; records past its end are an error (the image
/// is never grown).
pub fn apply(image: &mut [u8], records: &[Record]) -> Result<(), RandoError> {
    for r in records {
        let dst = image
            .get_mut(r.offset..r.offset + r.data.len())
            .ok_or_else(|| {
                RandoError::Ips(format!("record at {:#X} is outside the image", r.offset))
            })?;
        dst.copy_from_slice(&r.data);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_rle_records() {
        let mut p = b"PATCH".to_vec();
        p.extend_from_slice(&[0x00, 0x00, 0x02, 0x00, 0x02, 0xAA, 0xBB]);
        p.extend_from_slice(&[0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x03, 0x7F]);
        p.extend_from_slice(b"EOF");
        let recs = parse(&p).unwrap();
        assert_eq!(recs.len(), 2);
        let mut img = vec![0u8; 10];
        apply(&mut img, &recs).unwrap();
        assert_eq!(img, vec![0, 0, 0xAA, 0xBB, 0, 0x7F, 0x7F, 0x7F, 0, 0]);
        let mut small = vec![0u8; 4];
        assert!(apply(&mut small, &recs).is_err());
    }

    #[test]
    fn rejects_bad_patches() {
        assert!(parse(b"NOTAPATCH").is_err());
        assert!(parse(b"PATCH\x00\x00").is_err());
        assert!(parse(b"PATCH\x00\x00\x01\x00\x05\x01EOF").is_err());
    }
}
