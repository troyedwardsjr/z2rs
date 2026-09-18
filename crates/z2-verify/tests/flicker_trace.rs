use std::path::Path;

mod common;

use z2_verify::oracle::{Oracle, Port, TetanesOracle};

fn hash(frame: &[u8; 256 * 240]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in frame {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
#[ignore = "diagnostic trace, not a test: prints frame hashes and writes PPM images to /tmp; run with --ignored and Z2_ROM set"]
fn oracle_title_trace() {
    let Some(path) = common::rom_path("oracle_title_trace") else {
        return;
    };
    let mut oracle = TetanesOracle::load_rom(Path::new(&path)).unwrap();
    for frame in 1..=20 {
        oracle.step(0);
        let visible = oracle
            .frame_indexed()
            .iter()
            .filter(|&&p| p != oracle.frame_indexed()[0])
            .count();
        eprintln!(
            "frame={frame} hash={:016X} visible={} first={:02X}",
            hash(oracle.frame_indexed()),
            visible,
            oracle.frame_indexed()[0]
        );
    }
}
