//! Display enhancements on real frames (ROM-gated; the corpus test also
//! needs `$Z2_CORPUS`). Every option is checked both ways: off leaves the
//! presented frame byte-identical, on changes it only where it should.

mod common;

use z2_native::app::{self, Display, DisplaySettings, Emu};
use z2_native::display_enh::{self, AudioFx, DisplayEnh, FlashColor};
use z2_native::headless::{self, ParseOutcome};

fn settings(enh: DisplayEnh) -> DisplaySettings {
    DisplaySettings {
        scale: 1,
        display_enh: enh,
        ..DisplaySettings::default()
    }
}

fn present(emu: &Emu, enh: DisplayEnh) -> Vec<u8> {
    Display::new(settings(enh))
        .expect("display")
        .present(&emu.game)
        .expect("present")
        .to_vec()
}

/// Rows (game pixels at scale 1) where two frames differ.
fn diff_rows(a: &[u8], b: &[u8], w: usize) -> Vec<usize> {
    let mut rows: Vec<usize> = a
        .chunks(w * 4)
        .zip(b.chunks(w * 4))
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, _)| i)
        .collect();
    rows.dedup();
    rows
}

#[test]
fn overlays_and_effects_only_change_the_frame_when_on() {
    let Some(rom) = common::rom_path("overlays_and_effects_only_change_the_frame_when_on") else {
        return;
    };
    let mut emu = app::emu_from_rom_file(&rom, 44_100).expect("rom loads");
    app::step_frames(&mut emu, &[0u8; 300], None);
    let base = present(&emu, DisplayEnh::default());
    let w = 256;
    assert_eq!(base.len(), w * 240 * 4);

    // Values set but every switch off: byte-identical.
    let mut idle = DisplayEnh::zalia_preset();
    idle.screen_shake = false;
    idle.flash_color = FlashColor::Og;
    idle.low_hp_beep_reduced = false;
    assert!(!idle.effects_enabled);
    assert_eq!(
        present(&emu, idle),
        base,
        "switched-off values change nothing"
    );
    let zero = DisplayEnh {
        effects_enabled: true,
        ..DisplayEnh::default()
    };
    assert_eq!(present(&emu, zero), base, "effects on at zero strength");

    // Frame counter: only the top text strip changes.
    let fc = DisplayEnh {
        dev_framecount: true,
        ..DisplayEnh::default()
    };
    let shown = present(&emu, fc);
    let rows = diff_rows(&base, &shown, w);
    assert!(!rows.is_empty(), "the frame counter is drawn");
    assert!(rows.iter().all(|&r| r < 8), "only the top strip: {rows:?}");

    // Post effects: the whole picture changes.
    let fx = DisplayEnh {
        effects_enabled: true,
        brightness: 0.2,
        ..DisplayEnh::default()
    };
    let bright = present(&emu, fx);
    assert!(diff_rows(&base, &bright, w).len() > 200);

    // HD scale 2: the overlay scales with the picture.
    let mut s2 = settings(fc);
    s2.scale = 2;
    let mut d2 = Display::new(s2).expect("display");
    let big = d2.present(&emu.game).expect("present").to_vec();
    let mut s2_off = settings(DisplayEnh::default());
    s2_off.scale = 2;
    let big_base = Display::new(s2_off)
        .expect("display")
        .present(&emu.game)
        .expect("present")
        .to_vec();
    let rows = diff_rows(&big_base, &big, 512);
    assert!(!rows.is_empty() && rows.iter().all(|&r| r < 16), "{rows:?}");

    // The headless --dump-present surface takes the same JSON.
    let dir = std::env::temp_dir().join(format!("z2-display-enh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dump = |json: Option<&str>, name: &str| {
        let out = dir.join(name);
        let mut argv: Vec<String> = [
            "--headless",
            "--rom",
            rom.to_str().unwrap(),
            "--frames",
            "300",
            "--dump-present",
            out.to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(j) = json {
            argv.push("--display-enh-json".into());
            argv.push(j.into());
        }
        let ParseOutcome::Run(args) = headless::parse_headless_args(&argv).expect("parses") else {
            panic!("expected a run");
        };
        headless::run_headless(&args).expect("headless run");
        std::fs::read(&out).expect("dump written")
    };
    let plain = dump(None, "plain.png");
    assert_eq!(dump(Some("{}"), "empty.png"), plain, "{{}} is the default");
    assert_ne!(
        dump(Some(r#"{"dev_framecount":true}"#), "fc.png"),
        plain,
        "the overlay reaches the dump"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The anypct movie casts a flashing spell at frame 9020 (backdrop cycles
/// red/green/blue while `$074B` counts down) and walks with the low-HP beep
/// from frame 18 473 (every 48 frames, `$07FF = $40`).
#[test]
fn flash_recolour_and_low_hp_beep_on_the_corpus() {
    let test = "flash_recolour_and_low_hp_beep_on_the_corpus";
    let Some(rom) = common::rom_path(test) else {
        return;
    };
    let Some(corpus) = common::var_present("Z2_CORPUS") else {
        eprintln!("skipping {test}: Z2_CORPUS not set");
        return;
    };
    let Some(movie) = common::file_present(
        &std::path::Path::new(&corpus).join("movies/anypct.bk2"),
        test,
    ) else {
        return;
    };
    let track = app::load_movie_track(&movie).expect("movie parses");
    let mut emu = app::emu_from_rom_file(&rom, 44_100).expect("rom loads");
    for &b in &track[..8_900] {
        emu.game.step(b);
        let _ = emu.game.apu.drain_log();
    }

    // Flash: present every frame through an OG and a "None" display.
    let mut og = Display::new(settings(DisplayEnh::default())).unwrap();
    let mut none = Display::new(settings(DisplayEnh {
        flash_color: FlashColor::None,
        ..DisplayEnh::default()
    }))
    .unwrap();
    let mut red = Display::new(settings(DisplayEnh {
        flash_color: FlashColor::Red,
        ..DisplayEnh::default()
    }))
    .unwrap();
    let mut shake = Display::new(settings(DisplayEnh {
        screen_shake: true,
        ..DisplayEnh::default()
    }))
    .unwrap();
    let mut flash_frames = 0;
    let mut flash_at = None;
    let mut last_plain = Vec::new();
    for (i, &b) in track.iter().enumerate().take(9_070).skip(8_900) {
        emu.game.step(b);
        let _ = emu.game.apu.drain_log();
        let ram = emu.game.ram();
        if flash_at.is_none() && ram[display_enh::RAM_FLASH] != 0 {
            // A decor flash ($A0) with no spell cast (MP unchanged, $074A 0):
            // flashes, but must not shake.
            assert_eq!(ram[display_enh::RAM_FLASH], 0xA0, "frame {i}");
            assert_ne!(ram[display_enh::RAM_LAST_CAST], display_enh::THUNDER_CAST);
            flash_at = Some(i);
        }
        let flashing = ram[display_enh::RAM_FLASH] != 0 && emu.game.palette()[0] != 0x0F;
        let a = og.present(&emu.game).unwrap().to_vec();
        let n = none.present(&emu.game).unwrap().to_vec();
        let r = red.present(&emu.game).unwrap().to_vec();
        let sh = shake.present(&emu.game).unwrap().to_vec();
        assert!(sh == a, "frame {i}: no shake");
        if !flashing {
            last_plain = a.clone();
            assert!(n == a, "frame {i}: no flash, no change");
            assert!(r == a, "frame {i}: no flash, no change");
            continue;
        }
        flash_frames += 1;
        let col = z2_ppu::indexed_to_rgba(emu.game.palette()[0]);
        let count = |px: &[u8]| px.chunks(4).filter(|p| *p == col).count();
        let steady = z2_ppu::indexed_to_rgba(0x0F);
        let count_steady = |px: &[u8]| px.chunks(4).filter(|p| *p == steady).count();
        assert!(
            count(&a) > 10_000,
            "frame {i}: the game flashes the backdrop"
        );
        // Pixels of the flash colour left: about as many as tiles of that
        // colour showed before the flash (12 and 16 are also tile colours).
        assert!(
            count(&n) <= count(&last_plain) + 1_000,
            "frame {i}: None removes the flash ({} left, {} before, {} flashing)",
            count(&n),
            count(&last_plain),
            count(&a)
        );
        assert!(count_steady(&n) > count_steady(&a) + 10_000);
        if emu.game.palette()[0] != 0x16 {
            // Red replaces the green / blue strobe (it pulses red / steady).
            assert!(
                count(&r) <= count(&last_plain) + 1_000,
                "frame {i}: Red replaces the strobe"
            );
        }
    }
    assert_eq!(flash_at, Some(8_985));
    assert!(flash_frames >= 30, "the flash was seen ({flash_frames})");

    // Low-HP beep: feed the same register writes to a plain APU and to an
    // AudioFx with the reduced beep, and compare each beep's power.
    for &b in &track[9_070..18_400] {
        emu.game.step(b);
        let _ = emu.game.apu.drain_log();
    }
    let mut plain = z2_apu::Apu::new(44_100);
    let mut fx_apu = z2_apu::Apu::new(44_100);
    let mut fx = AudioFx::default();
    fx.configure(&DisplayEnh {
        low_hp_beep_reduced: true,
        ..DisplayEnh::default()
    });
    let power = |pcm: &[i16]| {
        let mean = pcm.iter().map(|&s| i64::from(s)).sum::<i64>() / pcm.len().max(1) as i64;
        pcm.iter()
            .map(|&s| (i64::from(s) - mean).pow(2))
            .sum::<i64>()
    };
    let mut beeps: Vec<(i64, i64)> = Vec::new();
    let (mut pa, mut pb) = (Vec::new(), Vec::new());
    let mut prev_beep = false;
    for &b in &track[18_400..18_700] {
        emu.game.step(b);
        let log = emu.game.apu.drain_log();
        for &(a, v) in &log {
            plain.write_reg(a, v);
        }
        let (mut a, mut c) = (Vec::new(), Vec::new());
        plain.audio(&mut a);
        fx.render_frame(&log, emu.game.ram(), &mut fx_apu, &mut c);
        let beep = emu.game.ram()[display_enh::RAM_SFX_PULSE1] == display_enh::LOW_HP_BEEP_ID;
        if beep {
            pa.extend_from_slice(&a);
            pb.extend_from_slice(&c);
        } else if prev_beep {
            beeps.push((power(&pa), power(&pb)));
            pa.clear();
            pb.clear();
        }
        prev_beep = beep;
    }
    // Beeps at 18 473, 18 521, 18 569 and 18 617 (then HP is restored).
    // The music keeps playing on the other channels, so a silenced beep
    // lowers the power rather than zeroing it.
    assert_eq!(beeps.len(), 4, "beeps seen: {beeps:?}");
    for (n, &(a, b)) in beeps.iter().enumerate() {
        if n < usize::from(display_enh::LOW_HP_BEEPS_AUDIBLE) {
            assert_eq!(a, b, "beep {n} still plays");
        } else {
            assert!(b < a - a / 20, "beep {n} is silenced: {b} vs {a}");
        }
    }
}
