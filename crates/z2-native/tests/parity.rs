//! Integration: headless/windowed share one `Game` step path.
//!
//! Headless-CI constraint: nothing here opens a window or an audio device —
//! only the pure shared primitives (`app::new_emu`, `app::step_frames`,
//! `headless::shared_step_selftest`, config/input/audio math).

mod common;

use z2_native::{app, audio, headless};

#[test]
fn headless_and_windowed_step_the_same_game_frames() {
    // Both funnels step real `Game::step` frames through `app::step_frames`.
    let (frames, depth) = headless::shared_step_selftest(30, 44100);
    assert_eq!(frames, 30);

    let mut emu = app::new_emu(44100);
    let ring = audio::SharedAudio::new(44100);
    let n = app::step_frames(&mut emu, &[0u8; 30], Some(&ring));
    assert_eq!(n, 30);
    assert_eq!(emu.game.frame_count(), frames);
    assert_eq!(ring.depth(), depth);
    // 30 frames ≈ 30*735 ±30 jitter samples of PCM.
    assert!((ring.depth() as u64).abs_diff(30 * 735) <= 30);
}

#[test]
fn headless_smoke_stays_windowless_and_gated() {
    // Smoke runs without a display, ROM, or oracle …
    let report =
        headless::run_headless(&headless::HeadlessArgs::default()).expect("smoke runs windowless");
    assert_eq!(report.frames_run, 0);
    // … and real runs are UNGATED since the standalone Game path landed:
    // 60 blank frames step the synthetic emu with no window, ROM, or oracle.
    let gated = headless::HeadlessArgs {
        frames: 60,
        ..Default::default()
    };
    let report = headless::run_headless(&gated).expect("headless frames run windowless");
    assert_eq!(report.frames_run, 60);
}

#[test]
fn loop_audio_plumbing_self_test_without_rom() {
    // Acceptance: loop + audio plumbing with a synthetic-frame self-test.
    let pcm = audio::synthetic_apu_frame(44100);
    assert!((pcm.len() as i64 - 735).abs() <= 1);
    let ring = audio::SharedAudio::new(44100);
    let mut emu = app::new_emu(44100);
    app::step_frames(&mut emu, &[0u8; 60], Some(&ring));
    assert_eq!(emu.game.frame_count(), 60);
    assert_eq!(ring.underruns(), 0, "push-only self-test never starves");
}

#[test]
fn movie_and_snapshot_helpers_stay_offline() {
    // Unknown extensions error without touching hardware.
    assert!(app::load_movie_track(std::path::Path::new("/tmp/x.xyz")).is_err());
    // Snapshot encode/decode round-trips through the live Game type.
    let emu = app::new_emu(44100);
    let snap = app::snapshot_from_game(&emu.game, Vec::new()).expect("snap");
    let bytes = snap.encode().expect("encode");
    let back = z2_verify::snapshot::Snapshot::decode(&bytes).expect("decode");
    assert_eq!(snap, back);
}

/// ROM-gated: with a cartridge the emulator boots (reset vector), draws
/// the title (non-uniform framebuffer), and the interpreter-run sound
/// engine audibly drives the synth. Skips without `Z2_ROM`.
#[test]
fn rom_boot_renders_title_and_audible_audio() {
    let Some(rom) = common::rom_path("rom_boot_renders_title_and_audible_audio") else {
        return;
    };
    let body = std::fs::read(&rom).expect("read ROM");
    let body = z2_assets::rom::strip_ines_header(&body).to_vec();
    let mut emu = app::emu_from_rom_body(&body, 44100).expect("build emu");
    // Reset vector taken (not address-0 BRK soup).
    assert_eq!(emu.game.cpu.pc, 0xFF70, "CPU starts at reset vector");
    // Native ROM construction must install the hybrid trap registries before
    // reset/stepping; an empty table falls back to the very slow interpreter.
    assert!(
        !emu.game.traps.is_empty(),
        "native ROM path must register traps"
    );
    for addr in [0xFF9D, 0xC33C, 0xC2CA] {
        assert!(
            emu.game.traps.get(addr).is_some(),
            "missing native trap ${addr:04X}"
        );
    }
    let ring = audio::SharedAudio::new(44100);
    app::step_frames(&mut emu, &vec![0u8; 600], Some(&ring));
    assert_eq!(emu.game.frame_count(), 600);
    // Title drawn: framebuffer is not uniform.
    let frame = emu.game.frame_indexed();
    let first = frame[0];
    assert!(
        frame.iter().any(|&b| b != first),
        "framebuffer must show drawn PPU output by frame 600"
    );
    // Audible: drain the ring and measure energy (title music runs).
    let mut pcm = Vec::new();
    ring.consume(ring.depth(), &mut pcm);
    assert!(!pcm.is_empty(), "PCM was produced");
    let energy: i64 = pcm.iter().map(|&s| (s as i64).abs()).sum();
    assert!(
        energy > pcm.len() as i64,
        "PCM must carry signal, not digital silence (energy {energy} over {} samples)",
        pcm.len()
    );
    assert_eq!(emu.apu.frames_rendered(), 600);
}
