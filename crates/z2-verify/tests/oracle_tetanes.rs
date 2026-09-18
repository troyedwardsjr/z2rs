//! Oracle integration tests.
//!
//! ROM-gated tests skip gracefully when `Z2_ROM` is unset, following the
//! `z2-assets` roundtrip pattern (`let Some(..) else { return }`). Run the
//! full set with:
//!
//! ```sh
//! Z2_ROM=/Volumes/Holy\ Drive/dev/z2rs/rom/zelda2.nes cargo test -p z2-verify
//! ```
//!
//! The ROM file is read-only and never copied.

mod common;

use std::path::PathBuf;

use z2_verify::lockstep::Lockstep;
use z2_verify::oracle::{Oracle, Port, StubPort, TetanesOracle};

/// Pinned-ROM path, or `None` (skip) when `Z2_ROM` is unset, empty, or does
/// not name an existing file.
fn rom_path() -> Option<PathBuf> {
    common::rom_path("z2-verify oracle test")
}

/// Deterministic movie-length-ish input track (fixed-seed xorshift32).
fn synth_inputs(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xFF) as u8
        })
        .collect()
}

#[test]
fn stub_load_rom_rejects_garbage_path_without_rom() {
    // No ROM needed: a missing file must fail the gate, never yield a stub.
    let err = StubPort::load_rom(std::path::Path::new("/nonexistent/zelda2.nes"));
    assert!(err.is_err());
}

#[test]
fn stub_load_rom_passes_gate_with_pinned_rom() {
    let Some(path) = rom_path() else { return };
    let stub = StubPort::load_rom(&path).expect("pinned ROM must pass the gate");
    assert_eq!(stub.frame_no(), 0);
}

#[test]
fn tetanes_rejects_garbage_path_without_rom() {
    let err = TetanesOracle::load_rom(std::path::Path::new("/nonexistent/zelda2.nes"));
    assert!(err.is_err());
}

#[test]
fn tetanes_powers_on_to_documented_all_zeros() {
    let Some(path) = rom_path() else { return };
    let o = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    // Documented power-on pattern (see oracle.rs): every observable byte $00.
    assert!(o.ram().iter().all(|&b| b == 0), "cpu ram boots $00");
    assert!(o.wram().iter().all(|&b| b == 0), "wram boots $00");
    assert!(o.oam().iter().all(|&b| b == 0), "oam boots $00");
    assert!(o.palette().iter().all(|&b| b == 0), "palette boots $00");
    assert!(
        o.frame_indexed().iter().all(|&b| b == 0),
        "framebuffer boots $00"
    );
}

#[test]
fn oracle_vs_oracle_replays_movie_length_with_zero_divergence() {
    let Some(path) = rom_path() else { return };
    let mut a = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    let mut b = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    // Movie-length-ish: 5000 frames of pseudo-random input (~83 s of NTSC).
    let inputs = synth_inputs(5000);
    Lockstep::run(&mut a, &mut b, &inputs, 5000).expect("two oracles must agree");
}

#[test]
fn save_state_resumes_identically() {
    let Some(path) = rom_path() else { return };
    let inputs = synth_inputs(240);
    let mut a = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    for &i in &inputs[..120] {
        a.step(i);
    }
    let saved = a.save_state();
    assert!(!saved.is_empty());
    eprintln!("oracle state blob: {} bytes", saved.len());

    // Twin loads the same ROM, restores the blob, replays the same tail.
    let mut b = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    b.load_state(&saved).expect("state must restore");
    for &i in &inputs[120..180] {
        a.step(i);
        b.step(i);
    }
    // Lockstep over the tail must agree (exercises load_state, not just
    // byte equality of the caches).
    Lockstep::run(&mut a, &mut b, &inputs[180..240], 60).expect("resumed oracle must agree");
}

#[test]
fn load_state_rejects_garbage() {
    let Some(path) = rom_path() else { return };
    let mut a = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    assert!(a.load_state(&[]).is_err());
    assert!(a.load_state(&[0u8; 64]).is_err());
}

#[test]
fn perf_reports_speed_with_regression_floor() {
    let Some(path) = rom_path() else { return };
    let mut o = TetanesOracle::load_rom(&path).expect("pinned ROM must load");
    // Warm up (first frames pay for lazy init), then time the run.
    for _ in 0..60 {
        o.step(0);
    }
    let frames = 1200usize;
    let t0 = std::time::Instant::now();
    for _ in 0..frames {
        o.step(0);
    }
    let secs = t0.elapsed().as_secs_f64();
    let fps = frames as f64 / secs;
    let mult = fps / 60.0;
    eprintln!("oracle perf: {fps:.0} fps = {mult:.1}x realtime ({frames} frames, no input)");
    // Regression floor, NOT the acceptance line: the target is 20x
    // native. Measured Sep 2026 on an M4 Max under heavy load (~10):
    // 15.5-17.0x boot/title, 18.4-18.7x post-menu in release; 2.4x in debug.
    // Bottleneck is 99.3% inside tetanes `clock_frame` (PPU render); the
    // oracle wrapper copies cost ~7 us/frame (0.7%). Re-run on a quiet box
    // for a representative number.
    #[cfg(debug_assertions)]
    assert!(
        mult >= 1.0,
        "oracle slower than realtime in debug: {mult:.1}x"
    );
    #[cfg(not(debug_assertions))]
    assert!(
        mult >= 12.0,
        "oracle perf regressed: {mult:.1}x (floor 12x)"
    );
}
