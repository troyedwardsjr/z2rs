//! Snapshot (save-state) codec for the web frontend.
//!
//! Two formats:
//!
//! * **`Z2WEB01`** — the tab's own save-state format, emitted by
//!   [`WebSnapshot::encode`] and accepted by [`WebSnapshot::decode`]. It
//!   carries the live [`z2_core::game::Game`] memory images (`ram`, `wram`).
//!   The interpreter's 6502 register file, mapper shift state, PPU/APU
//!   models and the frame counter are **not** captured (the engine exposes
//!   no state import — see `tools/xtask/src/verify.rs`: `--dut game` with
//!   `--snapshot` is rejected for the same reason), so a restored state
//!   resumes deterministically only from the next reset boundary. The JS
//!   side persists these blobs in IndexedDB save slots (see `site/app.js`).
//! * **`Z2SNAP01`** — the `z2-verify` labelled-snapshot corpus format
//!   (magic `Z2SNAP01`, little-endian fields, trailing CRC32).
//!   [`import_corpus_snapshot`] is a decode-only interop reader for it: it
//!   extracts the `ram`/`wram` images (and `frame_logic`) so a QA snapshot
//!   minted by the corpus pipeline can be opened in-tab. It is a field
//!   reader, not a reimplementation of `z2_verify::snapshot` (labels are
//!   not validated, opaque PPU/APU/mapper blobs and the input history are
//!   skipped). `z2-web` does not depend on `z2-verify` so the browser
//!   bundle stays oracle-free.
//!
//! ROM bytes are never stored in either format (the tab re-verifies the
//! dropped ROM through the hash gate on every load).

use std::fmt;

/// Magic opening every `Z2WEB01` blob.
pub const WEB_MAGIC: &[u8; 8] = b"Z2WEB001";
/// Current `Z2WEB01` encoding version.
pub const WEB_VERSION: u16 = 1;

/// NES internal RAM (`$0000-$07FF`).
pub const RAM_SIZE: usize = 2048;
/// Battery-backed WRAM (`$6000-$7FFF`).
pub const WRAM_SIZE: usize = 8192;

/// Magic opening every `Z2SNAP01` corpus blob.
pub const CORPUS_MAGIC: &[u8; 8] = b"Z2SNAP01";

/// Codec failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotError(pub String);

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "snapshot: {}", self.0)
    }
}

impl std::error::Error for SnapshotError {}

fn fail(msg: impl Into<String>) -> SnapshotError {
    SnapshotError(msg.into())
}

/// Tab save-state: live memory images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSnapshot {
    /// Frames executed when the snapshot was taken (informational: the
    /// engine frame counter itself is not rewound on restore).
    pub frame_count: u64,
    /// 2 KiB NES RAM image.
    pub ram: Vec<u8>,
    /// 8 KiB battery WRAM image.
    pub wram: Vec<u8>,
}

impl WebSnapshot {
    /// Build, checking fixed sizes.
    pub fn new(frame_count: u64, ram: &[u8], wram: &[u8]) -> Result<Self, SnapshotError> {
        if ram.len() != RAM_SIZE {
            return Err(fail(format!("ram is {} bytes, want {RAM_SIZE}", ram.len())));
        }
        if wram.len() != WRAM_SIZE {
            return Err(fail(format!(
                "wram is {} bytes, want {WRAM_SIZE}",
                wram.len()
            )));
        }
        Ok(Self {
            frame_count,
            ram: ram.to_vec(),
            wram: wram.to_vec(),
        })
    }

    /// Encode to the canonical `Z2WEB01` binary form
    /// (magic + version + frame + blobs + CRC32).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 2 + 8 + 4 + RAM_SIZE + 4 + WRAM_SIZE + 4);
        out.extend_from_slice(WEB_MAGIC);
        out.extend_from_slice(&WEB_VERSION.to_le_bytes());
        out.extend_from_slice(&self.frame_count.to_le_bytes());
        out.extend_from_slice(&(self.ram.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.ram);
        out.extend_from_slice(&(self.wram.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.wram);
        let crc = crc32_ieee(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Decode + verify magic, version, sizes and CRC32.
    pub fn decode(b: &[u8]) -> Result<Self, SnapshotError> {
        if b.len() < 8 + 4 {
            return Err(fail("too short"));
        }
        let (body, tail) = b.split_at(b.len() - 4);
        let want = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        if crc32_ieee(body) != want {
            return Err(fail("CRC32 mismatch"));
        }
        let mut c = Cursor { b: body, off: 0 };
        if c.take(8, "magic")? != WEB_MAGIC {
            return Err(fail("bad magic"));
        }
        let version = c.u16("version")?;
        if version != WEB_VERSION {
            return Err(fail(format!("unsupported version {version}")));
        }
        let frame_count = c.u64("frame_count")?;
        let ram = c.bytes("ram", RAM_SIZE)?;
        let wram = c.bytes("wram", WRAM_SIZE)?;
        if ram.len() != RAM_SIZE || wram.len() != WRAM_SIZE {
            return Err(fail("bad ram/wram size"));
        }
        if c.off != body.len() {
            return Err(fail("trailing bytes"));
        }
        Ok(Self {
            frame_count,
            ram,
            wram,
        })
    }
}

/// `ram`/`wram` images imported from a `Z2SNAP01` corpus snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusState {
    /// Logic (non-lag) frame index at mint time.
    pub frame_logic: u64,
    /// 2 KiB NES RAM image.
    pub ram: Vec<u8>,
    /// 8 KiB battery WRAM image.
    pub wram: Vec<u8>,
}

/// Decode-only interop reader for `Z2SNAP01` corpus blobs: verifies magic,
/// version and CRC32, then extracts `ram`/`wram`/`frame_logic`, skipping
/// the opaque PPU/APU/mapper blobs, the input history and the label
/// strings (see the module docs).
pub fn import_corpus_snapshot(b: &[u8]) -> Result<CorpusState, SnapshotError> {
    if b.len() < 8 + 4 {
        return Err(fail("too short"));
    }
    let (body, tail) = b.split_at(b.len() - 4);
    let want = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    if crc32_ieee(body) != want {
        return Err(fail("CRC32 mismatch"));
    }
    let mut c = Cursor { b: body, off: 0 };
    if c.take(8, "magic")? != CORPUS_MAGIC {
        return Err(fail("bad magic (not a Z2SNAP01 corpus snapshot)"));
    }
    let version = c.u16("version")?;
    if version != 1 {
        return Err(fail(format!("unsupported corpus version {version}")));
    }
    let _layout = c.u16("layout")?;
    let ram = c.bytes("ram", 64 * 1024)?;
    let wram = c.bytes("wram", 64 * 1024)?;
    if ram.len() != RAM_SIZE || wram.len() != WRAM_SIZE {
        return Err(fail("bad ram/wram size"));
    }
    // Skip opaque blobs + history (bounded like z2-verify's own limits).
    let _ppu = c.bytes("ppu", 64 * 1024)?;
    let _apu = c.bytes("apu", 64 * 1024)?;
    let _mapper = c.bytes("mapper", 64 * 1024)?;
    let _history = c.bytes("input_history", 4 * 1024 * 1024)?;
    let _label = c.string("label")?;
    let _source_movie = c.string("source_movie")?;
    let _frame_raw = c.u64("frame_raw")?;
    let frame_logic = c.u64("frame_logic")?;
    if c.off != body.len() {
        return Err(fail("trailing bytes"));
    }
    Ok(CorpusState {
        frame_logic,
        ram,
        wram,
    })
}

// --- little-endian cursor (mirrors z2-verify's snapshot codec) ---

struct Cursor<'a> {
    b: &'a [u8],
    off: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, what: &str) -> Result<&'a [u8], SnapshotError> {
        let end = self.off + n;
        if end > self.b.len() {
            return Err(fail(format!("truncated snapshot in {what}")));
        }
        let s = &self.b[self.off..end];
        self.off = end;
        Ok(s)
    }
    fn u16(&mut self, what: &str) -> Result<u16, SnapshotError> {
        let s = self.take(2, what)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self, what: &str) -> Result<u32, SnapshotError> {
        let s = self.take(4, what)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self, what: &str) -> Result<u64, SnapshotError> {
        let s = self.take(8, what)?;
        Ok(u64::from_le_bytes([
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
        ]))
    }
    fn bytes(&mut self, what: &str, limit: usize) -> Result<Vec<u8>, SnapshotError> {
        let n = self.u32(what)? as usize;
        if n > limit {
            return Err(fail(format!("{what} too large: {n}")));
        }
        Ok(self.take(n, what)?.to_vec())
    }
    fn string(&mut self, what: &str) -> Result<String, SnapshotError> {
        let b = self.bytes(what, 4096)?;
        String::from_utf8(b).map_err(|_| fail(format!("{what} not UTF-8")))
    }
}

/// CRC32 (IEEE 802.3) — same bitwise implementation as the ROM gate so the
/// codec stays dependency-free and `wasm32`-clean.
pub fn crc32_ieee(data: &[u8]) -> u32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> WebSnapshot {
        WebSnapshot::new(1234, &vec![0xAA; RAM_SIZE], &vec![0xBB; WRAM_SIZE]).unwrap()
    }

    #[test]
    fn roundtrip() {
        let s = tiny();
        let bytes = s.encode();
        assert_eq!(&bytes[..8], WEB_MAGIC);
        assert_eq!(WebSnapshot::decode(&bytes).unwrap(), s);
    }

    #[test]
    fn rejects_wrong_sizes() {
        assert!(WebSnapshot::new(0, &[0; 100], &vec![0; WRAM_SIZE]).is_err());
        assert!(WebSnapshot::new(0, &vec![0; RAM_SIZE], &[0; 100]).is_err());
    }

    #[test]
    fn decode_rejects_corruption() {
        let mut bytes = tiny().encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(WebSnapshot::decode(&bytes).is_err());
        assert!(WebSnapshot::decode(&bytes[..10]).is_err());
    }

    /// Build a minimal `Z2SNAP01` blob by hand (same field order as
    /// `z2-verify`'s encoder) and check the interop reader extracts it.
    #[test]
    fn corpus_import_reads_hand_built_blob() {
        fn put_u16(out: &mut Vec<u8>, v: u16) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        fn put_u32(out: &mut Vec<u8>, v: u32) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
            put_u32(out, b.len() as u32);
            out.extend_from_slice(b);
        }
        fn put_str(out: &mut Vec<u8>, s: &str) {
            put_u32(out, s.len() as u32);
            out.extend_from_slice(s.as_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(CORPUS_MAGIC);
        put_u16(&mut out, 1); // version
        put_u16(&mut out, 0); // layout Uninit
        put_bytes(&mut out, &vec![0x11; RAM_SIZE]);
        put_bytes(&mut out, &vec![0x22; WRAM_SIZE]);
        put_bytes(&mut out, &[1, 2, 3]); // ppu
        put_bytes(&mut out, &[4, 5]); // apu
        put_bytes(&mut out, &[6]); // mapper
        put_bytes(&mut out, &[0x01, 0x00]); // history
        put_str(&mut out, "boot-title");
        put_str(&mut out, "test.fm2");
        out.extend_from_slice(&1234u64.to_le_bytes()); // frame_raw
        out.extend_from_slice(&1200u64.to_le_bytes()); // frame_logic
        let crc = crc32_ieee(&out);
        out.extend_from_slice(&crc.to_le_bytes());

        let got = import_corpus_snapshot(&out).unwrap();
        assert_eq!(got.frame_logic, 1200);
        assert_eq!(got.ram, vec![0x11; RAM_SIZE]);
        assert_eq!(got.wram, vec![0x22; WRAM_SIZE]);
        // The tab codec must not mistake a corpus blob for its own.
        assert!(WebSnapshot::decode(&out).is_err());
    }
}
