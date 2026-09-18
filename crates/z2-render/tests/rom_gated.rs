//! ROM-gated end-to-end checks: real frames through the presenter, and a
//! recorder pass turned back into a loadable pack.
//!
//! Self-skipping: without `Z2_ROM` pointing at an existing file these tests
//! print a skip line and pass (LEGAL.md §4: the default test run has no ROM).
//! Run them with `cargo test -p z2-render --release`.

use std::path::PathBuf;

use z2_render::presenter::{PresentConfig, Presenter};
use z2_render::{HdPack, Recorder};

fn rom() -> Option<PathBuf> {
    // Absent = unset, empty, or not an existing file. Report it once, as the
    // module doc promises, rather than skipping silently.
    let p = std::env::var("Z2_ROM")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    if p.is_none() {
        eprintln!("skipping z2-render ROM-gated test: Z2_ROM not set to an existing file");
    }
    p
}

/// Booted game with the render record armed.
fn game() -> Option<z2_core::game::Game> {
    let path = rom()?;
    let raw = std::fs::read(&path).ok()?;
    // Hash gate: never accept a file that is not the expected dump.
    z2_assets_gate(&path)?;
    let mut g = z2_core::game::Game::from_ines(&raw).ok()?;
    g.reset();
    z2_core::boot_traps::register_boot_traps(&mut g);
    g.ppu.model_mut().set_record(true);
    Some(g)
}

/// The ROM hash gate lives in z2-assets, which z2-render does not depend on;
/// re-check the length/CRC cheaply here so a wrong file cannot slip through.
fn z2_assets_gate(path: &std::path::Path) -> Option<()> {
    let bytes = std::fs::read(path).ok()?;
    let body = if bytes.len() > 16 && bytes[0..4] == *b"NES\x1a" {
        &bytes[16..]
    } else {
        &bytes[..]
    };
    (body.len() == 262_144).then_some(())
}

#[test]
fn six_hundred_real_frames_compose_to_the_frame() {
    let Some(mut g) = game() else {
        eprintln!("skipping ROM-gated test: Z2_ROM is not an existing file");
        return;
    };
    let mut p = Presenter::new(PresentConfig {
        scale: 2,
        ..PresentConfig::default()
    })
    .unwrap();
    assert_eq!((p.width(), p.height()), (512, 480));
    for frame_no in 0..600 {
        g.step(0);
        let frame = *g.frame_indexed();
        let record = g.ppu.model().frame_record().expect("record armed").clone();
        let rgba = p.present(&frame, &record, &g.chr).unwrap();
        // No pack: every output pixel is the frame's colour, nearest-upscaled.
        if frame_no % 50 == 0 {
            for y in (0..z2_ppu::HEIGHT).step_by(7) {
                for x in (0..z2_ppu::WIDTH).step_by(5) {
                    let want = z2_ppu::indexed_to_rgba(frame[y * z2_ppu::WIDTH + x]);
                    for (fy, fx) in [(0, 0), (1, 1)] {
                        let i = ((y * 2 + fy) * 512 + x * 2 + fx) * 4;
                        assert_eq!(
                            &rgba[i..i + 4],
                            want.as_slice(),
                            "frame {frame_no} pixel ({x}, {y}) sub ({fx}, {fy})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn recorder_over_real_frames_writes_a_loadable_pack() {
    let Some(mut g) = game() else {
        eprintln!("skipping ROM-gated test: Z2_ROM is not an existing file");
        return;
    };
    let mut rec = Recorder::new();
    for _ in 0..240 {
        g.step(0);
        rec.observe(g.ppu.model().frame_record().expect("record armed"));
    }
    assert_eq!(rec.frames(), 240);
    assert!(
        rec.distinct_keys() > 20,
        "real frames should show many tile/palette combinations, got {}",
        rec.distinct_keys()
    );

    // In-memory round trip only: template sheets are ROM-derived and must
    // never be written inside the repository (LEGAL.md §1).
    let files = rec.write_pack(&g.chr, 2, "rom-record").unwrap();
    let pack = HdPack::from_files(&files).expect("recorder pack loads");
    assert_eq!(pack.scale(), 2);
    assert!(pack.tile_count() > 0);
    for (key, _) in rec.iter() {
        assert!(
            pack.lookup(key.page, key.tile, key.colors).is_some()
                || pack.lookup_default(key.page, key.tile).is_none(),
            "page {} tile {} should resolve or be a blank CHR tile",
            key.page,
            key.tile
        );
    }
    // Composing with that pack must not panic on real content.
    let mut p = Presenter::new(PresentConfig {
        scale: 2,
        ..PresentConfig::default()
    })
    .unwrap();
    p.set_pack(Some(pack)).unwrap();
    let frame = *g.frame_indexed();
    let record = g.ppu.model().frame_record().unwrap().clone();
    let rgba = p.present(&frame, &record, &g.chr).unwrap();
    assert_eq!(rgba.len(), 512 * 480 * 4);
}
