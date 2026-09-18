//! Lockstep first-divergence tests.
//!
//! ROM-free: every test here runs the deterministic [`StubPort`](z2_verify::oracle::StubPort)
//! double against itself, so `cargo test -p z2-verify` covers the comparator
//! with no `Z2_ROM` set. The acceptance probes:
//!
//! * identical stubs replay a movie-length track with zero divergence;
//! * a deliberately corrupted byte is reported at the right frame + address.

use z2_verify::lockstep::{Lockstep, Region};
use z2_verify::oracle::{Oracle, Port, StubPort};

/// Deterministic pseudo-random track (fixed-seed xorshift32).
fn synth_inputs(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x9E37_79B9;
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
fn stub_vs_stub_movie_length_zero_divergence() {
    let mut o = StubPort::new();
    let mut d = StubPort::new();
    // 6000 frames = 100 s of NTSC input (debug builds step the ~72 KiB
    // stub state slowly; the ROM-gated oracle test covers 5000 emulated
    // frames on top of this).
    let inputs = synth_inputs(6000);
    Lockstep::run(&mut o, &mut d, &inputs, 6000).expect("identical stubs must agree");
}

#[test]
fn stub_state_save_load_keeps_lockstep() {
    let mut o = StubPort::new();
    let inputs = synth_inputs(200);
    for &i in &inputs[..100] {
        o.step(i);
    }
    let blob = o.save_state();
    let mut d = StubPort::new();
    d.load_state(&blob).expect("stub state must restore");
    Lockstep::run(&mut o, &mut d, &inputs[100..], 100).expect("restored stub must agree");
}

#[test]
fn corrupted_ram_byte_reported_at_frame_zero_and_exact_addr() {
    let mut o = StubPort::new();
    let mut d = StubPort::new();
    d.corrupt_ram(0x456, 0xA5);
    let err = Lockstep::run(&mut o, &mut d, &synth_inputs(64), 64).unwrap_err();
    assert_eq!(err.frame, 0, "corruption is visible on the first compare");
    assert_eq!(err.region, Region::MainRam);
    assert_eq!(err.addr, 0x456);
    assert_eq!(err.actual, d.ram()[0x456]);
    assert_eq!(err.expected, o.ram()[0x456]);
    assert_eq!(err.actual.wrapping_sub(err.expected), 0xA5);
    assert_eq!(err.last_writer, None);
}

#[test]
fn each_region_reports_its_own_addr_space() {
    // (corrupt, region, addr)
    type Case = (fn(&mut StubPort), Region, u32);
    let cases: &[Case] = &[
        (
            |d| d.corrupt_ram(0x0010, 0x01),
            Region::ZeroPageStack,
            0x0010,
        ),
        (|d| d.corrupt_ram(0x07FF, 0x02), Region::MainRam, 0x07FF),
        (
            |d| d.corrupt_wram(0x1FFF, 0x03),
            Region::Wram,
            0x6000 + 0x1FFF,
        ),
        (|d| d.corrupt_oam(255, 0x04), Region::Oam, 255),
        (|d| d.corrupt_palette(0, 0x05), Region::Palette, 0x3F00),
        // Not the last pixel: column 255 (and row 0) are the oracle's
        // mis-rendered overscan edges and are skipped by default.
        (
            |d| d.corrupt_frame(256 * 240 - 2, 0x06),
            Region::Frame,
            (256 * 240 - 2) as u32,
        ),
    ];
    for (i, (corrupt, region, addr)) in cases.iter().enumerate() {
        let mut o = StubPort::new();
        let mut d = StubPort::new();
        corrupt(&mut d);
        let err = Lockstep::run(&mut o, &mut d, &[], 4).unwrap_err();
        assert_eq!(err.region, *region, "case {i}");
        assert_eq!(err.addr, *addr, "case {i}");
        assert_eq!(err.frame, 0, "case {i}");
    }
}

#[test]
fn mid_run_corruption_reports_relative_frame() {
    let mut o = StubPort::new();
    let mut d = StubPort::new();
    let pre = synth_inputs(50);
    for &i in &pre {
        o.step(i);
        d.step(i);
    }
    // Fault injected at absolute frame 50; the tail run starts its own
    // frame count, so the report is relative (absolute = 50 + relative).
    d.corrupt_wram(0x00FF, 0xBE);
    let tail = synth_inputs(50);
    let err = Lockstep::run(&mut o, &mut d, &tail, 50).unwrap_err();
    assert_eq!(err.frame, 0);
    assert_eq!(err.region, Region::Wram);
    assert_eq!(err.addr, 0x6000 + 0x00FF);
}

#[test]
fn frame_edge_mismatches_are_skipped_unless_strict() {
    use z2_verify::lockstep::CompareOpts;
    use z2_verify::oracle::StubPort;
    // Column 255 and row 0 only: default compare is clean, strict reports.
    for idx in [255usize, 100 * 256 + 255, 17] {
        let mut o = StubPort::new();
        let mut d = StubPort::new();
        d.corrupt_frame(idx, 0x09);
        assert!(
            Lockstep::run(&mut o, &mut d, &[], 2).is_ok(),
            "edge pixel {idx} skipped by default"
        );
        let mut o = StubPort::new();
        let mut d = StubPort::new();
        d.corrupt_frame(idx, 0x09);
        let opts = CompareOpts {
            strict_edges: true,
            ..CompareOpts::default()
        };
        let err = Lockstep::run_with(&mut o, &mut d, &[], 2, opts).unwrap_err();
        assert_eq!(err.region, Region::Frame);
        assert_eq!(err.addr, idx as u32);
    }
}
