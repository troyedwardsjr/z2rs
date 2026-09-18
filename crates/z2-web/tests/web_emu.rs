//! Integration tests for `z2-web`.
//!
//! ROM-free tests run everywhere. The `with_real_rom_*` tests need the
//! user-supplied ROM (`Z2_ROM` env, never committed) and skip otherwise —
//! same convention as the rest of the workspace.

use z2_web::WebEmu;

#[test]
fn fresh_instance_reports_no_rom_as_json() {
    let emu = WebEmu::new();
    assert!(!emu.rom_loaded());
    assert_eq!(emu.rom_crc32_hex(), "");
    assert_eq!(emu.asset_section_count(), 0);
    let state: serde_json::Value = serde_json::from_str(&emu.state()).unwrap();
    assert_eq!(state["romLoaded"], false);
    assert_eq!(state["frame"], 0);
    assert!(!emu.version().is_empty());
}

#[test]
fn rom_gate_rejects_garbage() {
    let mut emu = WebEmu::new();
    assert!(emu.load_rom(b"definitely not a NES ROM").is_err());
    assert!(emu.load_rom(&vec![0u8; 262144]).is_err());
    assert!(!emu.rom_loaded());
    assert!(emu.step_frame(0).is_err());
    assert!(emu.facts().is_err());
    assert!(emu.snapshot().is_err());
    assert!(emu.screenshot().is_err());
}

#[test]
fn restore_rejects_garbage_and_wrong_sizes() {
    let mut emu = WebEmu::new();
    assert!(emu.restore(b"junk").is_err());
    assert!(emu.load_sram(&[0u8; 100]).is_err());
    // No ROM either way: restore must fail, not panic.
    assert!(emu.restore(&vec![0u8; 1000]).is_err());
}

#[test]
fn load_movie_validates_text() {
    let mut emu = WebEmu::new();
    // Strict .fm2: bad glyph errors with a message (no browser needed).
    assert!(emu.load_movie("|0|...X....|\n").is_err());
    // Header-only text parses as an empty track (warnings-free, 0 frames).
    let report: serde_json::Value =
        serde_json::from_str(&emu.load_movie("version 3\npalFlag 0\n").unwrap()).unwrap();
    assert_eq!(report["frames"], 0);
    assert_eq!(emu.movie_len(), 0);
}

/// Full boot + step + facts with the real ROM. Self-skips without `Z2_ROM`.
#[test]
fn with_real_rom_boots_steps_and_facts() {
    // Absent = unset, empty, or not an existing file: public CI used to
    // export `Z2_ROM=""`, which an `is_err()` guard reads as "ROM present".
    let path = match std::env::var("Z2_ROM") {
        Ok(p) if std::path::Path::new(&p).is_file() => p,
        _ => {
            eprintln!(
                "skipping with_real_rom_boots_steps_and_facts: Z2_ROM not set to an existing file"
            );
            return;
        }
    };
    let file = std::fs::read(&path).expect("read Z2_ROM");
    let mut emu = WebEmu::new();
    emu.load_rom(&file).expect("real ROM passes the gate");
    assert!(emu.rom_loaded());
    assert_eq!(emu.rom_crc32_hex(), "BA322865");
    assert_eq!(
        emu.rom_sha1_hex(),
        "11333adb723a5975e0ecca3aee8f4747aa8d2d26"
    );
    assert!(emu.asset_section_count() > 0);
    // CHR_ROM section resolves (assets.bin id 0x0100).
    assert!(!emu
        .asset_section(0x0100)
        .expect("CHR section present")
        .is_empty());

    emu.step_frames(0, 30).expect("steps");
    assert_eq!(emu.frame_count(), 30.0);
    let facts: serde_json::Value =
        serde_json::from_str(&emu.facts().expect("facts")).expect("facts is valid JSON");
    assert!(facts.get("link").is_some());
    assert!(facts.get("mode").is_some());

    emu.render_frame().expect("renders");
    assert_eq!(emu.frame_rgba().len(), 256 * 240 * 4);

    // Snapshot roundtrip preserves the memory images. Note: the engine
    // frame counter is not rewound on restore (no core state import), so
    // the re-taken blob carries the newer counter — ram/wram match exactly.
    let blob = emu.snapshot().expect("snapshot");
    emu.step_frames(1, 5).expect("steps on");
    emu.restore(&blob).expect("restore");
    let blob2 = emu.snapshot().expect("snapshot again");
    assert_ne!(blob, blob2, "frame counter advanced past the snapshot");
    emu.restore(&blob2).expect("restore again");
    assert_eq!(blob2, emu.snapshot().expect("snapshot stable"));

    // SRAM roundtrip is an exact 8 KiB image.
    let sram = emu.sram_bytes().expect("sram");
    assert_eq!(sram.len(), 8192);
    emu.load_sram(&sram).expect("sram reload");
}
