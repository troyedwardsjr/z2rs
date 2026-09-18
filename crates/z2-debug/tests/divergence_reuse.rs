//! Reuse cross-checks against the real `z2-verify` movie and lockstep types.
//!
//! `z2_verify` is a dev-dependency (host-only): these tests prove the
//! overlay's field-for-field adapters agree with the real lockstep
//! comparator, the real `.fm2` parser and the real Snapshot codec — without
//! pulling `tetanes-core` into the library's wasm builds.

use z2_core::facts::Game;
use z2_core::ram::Ram;
use z2_debug::divergence::{find_first_ram_diff, DivergenceReport};
use z2_debug::transport::{HistoryEntry, HistoryRing, Transport};
use z2_verify::lockstep::Lockstep;
use z2_verify::movie_fm2::parse_fm2;
use z2_verify::oracle::{Port, StubPort};
use z2_verify::snapshot::{Snapshot, RAM_SIZE, WRAM_SIZE};

#[test]
fn adapter_agrees_with_real_lockstep() {
    // Two deterministic stubs, one corrupted byte: the real comparator
    // reports it; the overlay adapter must say the same thing.
    let mut oracle = StubPort::new();
    let mut dut = StubPort::new();
    dut.corrupt_ram(0x456, 0xA5);
    let err = Lockstep::run(&mut oracle, &mut dut, &[0x11; 8], 8).unwrap_err();
    let rep = DivergenceReport::from_lockstep_parts(
        err.frame,
        err.region.as_str(),
        err.addr,
        err.expected,
        err.actual,
        err.last_writer.clone(),
    );
    assert_eq!(rep.frame, 0);
    assert_eq!((rep.region, rep.addr), ("ram", 0x456));
    assert_eq!(
        (rep.expected, rep.actual),
        (oracle.ram()[0x456], dut.ram()[0x456])
    );
    assert_eq!(rep.last_writer, None);
    // Summary carries the same greppable fields as Divergence::to_string.
    for needle in ["frame 0", "ram", "0x0456"] {
        assert!(rep.summary().contains(needle), "{}", rep.summary());
        assert!(err.to_string().contains(needle), "{err}");
    }
    // The overlay-side RAM scan agrees with lockstep on RAM regions.
    let o: &[u8; 2048] = oracle.ram();
    let d: &[u8; 2048] = dut.ram();
    let fast = find_first_ram_diff(o, d, err.frame, None).unwrap();
    assert_eq!(
        (fast.region, fast.addr, fast.expected, fast.actual),
        (rep.region, rep.addr, rep.expected, rep.actual)
    );
}

#[test]
fn zp_fault_sorts_before_ram_in_both() {
    let mut oracle = StubPort::new();
    let mut dut = StubPort::new();
    dut.corrupt_ram(0x700, 0x01);
    dut.corrupt_ram(0x42, 0x7E);
    let err = Lockstep::run(&mut oracle, &mut dut, &[], 2).unwrap_err();
    assert_eq!(err.region.as_str(), "zp+stack");
    let fast = find_first_ram_diff(oracle.ram(), dut.ram(), 0, None).unwrap();
    assert_eq!((fast.region, fast.addr), ("zp+stack", 0x42));
}

#[test]
fn fm2_track_feeds_transport_verbatim() {
    let movie = parse_fm2(
        "version 3\npalFlag 0\nromFilename Zelda II - The Adventure of Link (USA).nes\n\
         |0|........|\n|0|.......A|\n|0|RLDUTSBA|\n",
    )
    .unwrap();
    assert!(movie.warnings().is_empty());
    let track = movie.pad1_track();
    assert_eq!(track, vec![0x00, 0x01, 0xFF]);
    let mut t = Transport::new();
    t.load_track("test.fm2", track);
    assert_eq!(t.len(), 3);
    assert_eq!(t.current_input(), 0x00);
    t.advance();
    assert_eq!(t.current_input(), 0x01);
    t.advance();
    assert_eq!(t.current_input(), 0xFF);
}

#[test]
fn snapshot_codec_feeds_history_ring() {
    // Encode/decode through the real Snapshot codec, then capture the RAM
    // into the overlay history and read identical facts back.
    let mut ram_bytes = vec![0u8; RAM_SIZE];
    ram_bytes[0x774] = 0x06; // link_hp ($0774)
    ram_bytes[0x012] = 0x2A; // frame_counter ($0012)
    let snap = Snapshot::new(
        ram_bytes,
        vec![0u8; WRAM_SIZE],
        vec![],
        vec![],
        vec![],
        vec![0x01, 0x00],
        "boot-title",
        "test.fm2",
        2,
        2,
    )
    .unwrap();
    let bytes = snap.encode().unwrap();
    let back = Snapshot::decode(&bytes).unwrap();
    assert_eq!(back.label, "boot-title");
    let ram = Ram::from_slice(&back.ram).unwrap();
    assert_eq!((ram.link_hp(), ram.frame_counter()), (0x06, 0x2A));
    let mut ring = HistoryRing::new(4);
    ring.push(HistoryEntry::capture(back.frame_raw, &ram));
    let sel = ring.selected().unwrap();
    assert_eq!(sel.frame, 2);
    assert_eq!(sel.facts, Game::new(ram).facts());
}
