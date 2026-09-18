//! Corpus-mint integration tests.
//!
//! * Snapshot codec v1↔v2 compatibility (no ROM): hand-crafted v1 bytes
//!   decode with an empty blob; re-encoding a decoded v1 is byte-stable;
//!   attaching a blob bumps to v2 and round-trips.
//! * `$12`-stall lag-hypothesis check against the live oracle (ROM-gated,
//!   movie-gated): the armed detector agrees with an independently recorded
//!   `$12` trace, is deterministic across two oracles, and pins the measured
//!   early-movie stall count.
//!
//! ROM via `Z2_ROM` (read-only); movies from the out-of-tree corpus dir
//! (read-only); tests never write outside the test process heap.
//!
//! ```sh
//! Z2_ROM=/path/to/zelda2.nes Z2_CORPUS_MOVIES=/path/to/movies cargo test -p z2-verify --test corpus_mint_tests
//! ```

mod common;

use std::path::PathBuf;

use z2_verify::oracle::{Oracle, Port, TetanesOracle};
use z2_verify::snapshot::{
    BlobLayoutV1, Snapshot, MAX_ORACLE_BLOB_LEN, RAM_SIZE, SNAPSHOT_MAGIC, SNAPSHOT_VERSION,
    SNAPSHOT_VERSION_V1, WRAM_SIZE,
};

fn tiny_snapshot(label: &str) -> Snapshot {
    Snapshot::new(
        vec![0xAA; RAM_SIZE],
        vec![0xBB; WRAM_SIZE],
        vec![1, 2, 3],
        vec![4, 5],
        vec![6],
        vec![0x01, 0x00, 0x80],
        label,
        "test.fm2",
        1234,
        1200,
    )
    .unwrap()
}

// --- local codec helpers (mirror the LE layout to craft v1 bytes) ---

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
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
fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let m = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ m;
        }
    }
    !crc
}

/// Hand-craft a v1 encoding (no trailing blob) for `label`.
fn craft_v1_bytes(label: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(SNAPSHOT_MAGIC);
    put_u16(&mut out, SNAPSHOT_VERSION_V1);
    put_u16(&mut out, 0); // Uninit
    put_bytes(&mut out, &[0xAA; RAM_SIZE]);
    put_bytes(&mut out, &[0xBB; WRAM_SIZE]);
    put_bytes(&mut out, &[1, 2, 3]);
    put_bytes(&mut out, &[4, 5]);
    put_bytes(&mut out, &[6]);
    put_bytes(&mut out, &[0x01, 0x00, 0x80]);
    put_str(&mut out, label);
    put_str(&mut out, "test.fm2");
    put_u64(&mut out, 1234);
    put_u64(&mut out, 1200);
    let crc = crc32_ieee(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

#[test]
fn v1_bytes_decode_with_empty_blob() {
    let bytes = craft_v1_bytes("boot-title");
    let snap = Snapshot::decode(&bytes).unwrap();
    assert_eq!(snap.version, SNAPSHOT_VERSION_V1);
    assert_eq!(snap.state_source, BlobLayoutV1::Uninit);
    assert!(snap.oracle_blob.is_empty());
    assert_eq!(snap.label, "boot-title");
    assert_eq!(snap.frame_raw, 1234);
    assert_eq!(snap.ram.len(), RAM_SIZE);
    assert_eq!(snap.wram.len(), WRAM_SIZE);
}

#[test]
fn v1_reencode_is_byte_stable() {
    let bytes = craft_v1_bytes("game-start");
    let snap = Snapshot::decode(&bytes).unwrap();
    assert_eq!(snap.encode().unwrap(), bytes);
}

#[test]
fn v1_upgrades_to_v2_with_blob() {
    let bytes = craft_v1_bytes("ending");
    let mut snap = Snapshot::decode(&bytes).unwrap();
    snap.version = SNAPSHOT_VERSION;
    snap.state_source = BlobLayoutV1::OracleV1;
    let snap = snap.with_oracle_blob(vec![0x5A; 1024]).unwrap();
    let enc = snap.encode().unwrap();
    assert!(enc.len() > bytes.len());
    let back = Snapshot::decode(&enc).unwrap();
    assert_eq!(back.version, SNAPSHOT_VERSION);
    assert_eq!(back.oracle_blob, vec![0x5A; 1024]);
    assert_eq!(back.label, "ending");
}

#[test]
fn v1_with_blob_encodes_as_error() {
    // A v1 version tag with a non-empty blob is a caller bug: the encoder
    // refuses instead of silently writing unreadable-by-v1 bytes.
    let mut snap = tiny_snapshot("boot-title");
    snap.version = SNAPSHOT_VERSION_V1;
    snap.oracle_blob = vec![1, 2, 3];
    assert!(snap.encode().is_err());
}

#[test]
fn v2_roundtrips_with_blob() {
    let snap = tiny_snapshot("palace1-enter")
        .with_oracle_blob(vec![0xC3; 4096])
        .unwrap();
    let bytes = snap.encode().unwrap();
    let back = Snapshot::decode(&bytes).unwrap();
    assert_eq!(snap, back);
    // Deterministic encoding.
    assert_eq!(snap.encode().unwrap(), bytes);
}

#[test]
fn oversize_blob_rejected() {
    let big = vec![0u8; MAX_ORACLE_BLOB_LEN + 1];
    assert!(tiny_snapshot("boot-title").with_oracle_blob(big).is_err());
}

#[test]
fn bad_version_rejected() {
    let mut bytes = tiny_snapshot("boot-title").encode().unwrap();
    // Patch the version field (offset 8) to something unknown.
    bytes[8] = 0x09;
    bytes[9] = 0x00;
    assert!(Snapshot::decode(&bytes).is_err());
}

// ---------------------------------------------------------------------------
// `$12`-stall lag hypothesis vs the live oracle (ROM- + movie-gated).
// ---------------------------------------------------------------------------

/// Pinned ROM path, or `None` (skip) when `Z2_ROM` is unset, empty, or does
/// not name an existing file.
fn rom_path() -> Option<PathBuf> {
    common::rom_path("z2-verify corpus mint test")
}

/// Corpus movie dir: `$Z2_CORPUS_MOVIES`, else the canonical out-of-tree
/// default. Returns `None` (skip) when the movie file is absent.
fn movie_path(name: &str) -> Option<PathBuf> {
    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .unwrap_or_else(|| "/Volumes/Holy Drive/dev/z2-corpus/movies".to_string());
    common::file_present(
        &PathBuf::from(dir).join(name),
        "z2-verify corpus movie test",
    )
}

fn load_track(path: &PathBuf) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("read movie");
    let name = path.to_string_lossy();
    if name.ends_with(".bk2") {
        z2_verify::movie_bk2::parse_bk2_zip(&bytes)
            .expect("parse bk2")
            .pad1_track()
    } else {
        let text = String::from_utf8(bytes).expect("fm2 utf8");
        z2_verify::movie_fm2::parse_fm2(&text)
            .expect("parse fm2")
            .pad1_track()
    }
}

/// Armed `$12`-stall detector over a recorded counter trace (mirrors the
/// minter's `LagDetector`: pre-first-tick frames are boot, not lag).
fn stalls_from_trace(trace: &[u8]) -> Vec<u32> {
    let mut skipped = Vec::new();
    let mut prev: Option<u8> = None;
    let mut armed = false;
    for (f, &ctr) in trace.iter().enumerate() {
        match prev {
            None => {
                if ctr != 0 {
                    armed = true;
                }
            }
            Some(p) => {
                if !armed {
                    if ctr == p {
                        prev = Some(ctr);
                        continue;
                    }
                    armed = true;
                }
                if ctr == p {
                    skipped.push(f as u32);
                }
            }
        }
        prev = Some(ctr);
    }
    skipped
}

#[test]
fn f12_stall_signal_matches_trace_and_is_deterministic() {
    let Some(rom) = rom_path() else { return };
    let Some(movie) = movie_path("warp-glitch.bk2") else {
        return;
    };
    let track = load_track(&movie);
    // First 1200 raw frames: boot + title + attract entry.
    let n = 1200usize.min(track.len());

    // Oracle A: record the $12 trace.
    let mut a = TetanesOracle::load_rom(&rom).expect("pinned ROM must load");
    let mut trace: Vec<u8> = Vec::with_capacity(n);
    for &input in &track[..n] {
        a.step(input);
        trace.push(a.ram()[0x12]);
    }
    let skipped = stalls_from_trace(&trace);

    // Oracle B: same inputs must produce the identical trace (the
    // determinism the lag map's re-mint stability rests on).
    let mut b = TetanesOracle::load_rom(&rom).expect("pinned ROM must load");
    for (f, &input) in track[..n].iter().enumerate() {
        b.step(input);
        assert_eq!(b.ram()[0x12], trace[f], "oracle $12 diverged at frame {f}");
    }

    // Pinned measurement (Sep 2026, tetanes-core 0.15, pinned ROM, oracle
    // passing opposing d-pad bits through — see `TetanesOracle::deck_config`):
    // 84 armed stalls in the first 1200 warp-glitch frames — the
    // boot-transition hiccups (f=13-15, counter stuck at $06), the
    // title/file-select transitions, and the real gameplay lag frames the
    // Left+Right glitch route produces once the movie actually plays
    // (before the d-pad change the oracle dropped Left, the movie desynced
    // into the attract loop and only 11 stalls showed). The counter still
    // advances on >90% of frames, so the signal is live, not degenerate.
    eprintln!("f12 stalls in first {n} frames: {}", skipped.len());
    assert_eq!(skipped.len(), 84, "stalls: {skipped:?}");
    assert!(!skipped.is_empty());
    assert!((skipped.len() as f64) < (n as f64) * 0.1);

    // Hypothesis verdict, documented: `$12` non-advance DOES fire on
    // real replay frames (signal live), but the full-movie probe shows it
    // also fires on idle/menu/wedged states (10,630/19,946 stalls with a
    // ~10k-frame terminal freeze on the attract loop), so it cannot
    // distinguish gameplay lag from idle. The minter therefore keeps
    // `input_history` raw and ships the stall map as advisory bookkeeping.
    // This test pins the mechanism (detector == trace stalls); the
    // interpretation lives in `xtask corpus` docs.
    assert!(skipped.contains(&13));
}
