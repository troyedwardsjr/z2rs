//! Frontend feature wiring: flag parsing, feature resolution, the ROM-drop
//! rebuild, texture sizing, and the save-state/netplay interlock.
//!
//! Headless by construction: nothing here opens a window or an audio device.
//! The two design critiques both flagged "start with no ROM, then drop one"
//! as the path that silently loses features, so that is pinned here.

use z2_native::app::{self, CoopMode, Display, DisplaySettings, Features};
use z2_native::config::{KeyBindings, NativeConfig};
use z2_native::netplay;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

// ---------------------------------------------------------------- flag parsing

#[test]
fn every_new_flag_parses() {
    let a = app::parse_native_args(&argv(&[
        "z2-native",
        "--rom",
        "/tmp/z2.nes",
        "--widescreen",
        "16:9",
        "--coop-local",
        "--p2-pad",
        "1",
        "--hd-pack",
        "/tmp/pack",
        "--hd-scale",
        "2",
        "--hd-record",
        "/tmp/rec",
        "--fill-left-clip",
        "off",
        "--fill-right-clip",
        "off",
        "--margin-sprites",
        "off",
    ]))
    .expect("parses");
    assert_eq!(a.rom.as_deref(), Some("/tmp/z2.nes"));
    assert_eq!(a.widescreen.as_deref(), Some("16:9"));
    assert_eq!(a.coop, CoopMode::Local);
    assert_eq!(a.p2_pad, Some(1));
    assert_eq!(a.hd_pack.as_deref(), Some("/tmp/pack"));
    assert_eq!(a.hd_scale, Some(2));
    assert_eq!(a.hd_record.as_deref(), Some("/tmp/rec"));
    assert_eq!(a.fill_left_clip, Some(false));
    assert_eq!(a.fill_right_clip, Some(false));
    assert_eq!(a.margin_sprites, Some(false));

    let h = app::parse_native_args(&argv(&[
        "z2-native",
        "--coop-host",
        "my-room_1",
        "--signal",
        "ws://example.test:3536",
        "--net-delay",
        "4",
    ]))
    .expect("parses");
    assert_eq!(h.coop, CoopMode::Host("my-room_1".to_string()));
    assert!(h.coop.is_online());
    assert_eq!(h.coop.room(), Some("my-room_1"));
    assert_eq!(h.signal.as_deref(), Some("ws://example.test:3536"));
    assert_eq!(h.net_delay, Some(4));

    let j = app::parse_native_args(&argv(&["z2-native", "--coop-join", "r"])).expect("parses");
    assert_eq!(j.coop, CoopMode::Join("r".to_string()));
}

#[test]
fn unknown_flags_still_error_with_usage() {
    for bad in [
        vec!["z2-native", "--nope"],
        vec!["z2-native", "--widescreen"],         // missing value
        vec!["z2-native", "--widescreen", "21:9"], // unknown preset
        vec!["z2-native", "--widescreen", "17"],   // out of range
        vec!["z2-native", "--net-delay", "99"],    // above MAX_DELAY
        vec!["z2-native", "--net-delay", "abc"],
        vec!["z2-native", "--p2-pad", "left"],
        vec!["z2-native", "--hd-scale", "0"],
        vec!["z2-native", "--hd-scale", "9"],
        vec!["z2-native", "--hd-scale", "big"],
        vec!["z2-native", "--hd-pack"], // missing value
        vec!["z2-native", "--fill-left-clip", "yes"],
        vec!["z2-native", "--fill-right-clip", "yes"],
        vec!["z2-native", "--margin-sprites", "yes"],
        vec!["z2-native", "--coop-host", "bad room"], // space is not allowed
        vec!["z2-native", "--coop-host", ""],         // empty room
        vec!["z2-native", "--signal", "http://x"],    // not ws://
        vec!["z2-native", "--coop-local", "--coop-join", "r"], // two modes
    ] {
        let err = app::parse_native_args(&argv(&bad)).expect_err(&format!("must reject {bad:?}"));
        assert!(
            err.contains("usage:"),
            "error for {bad:?} must carry the usage text: {err}"
        );
    }
}

#[test]
fn net_mode_flag_selects_rollback_or_lockstep() {
    use z2_native::config::NetMode;
    let a = app::parse_native_args(&argv(&["z2-native", "--coop-host", "r"])).expect("parses");
    assert_eq!(
        a.net_mode, None,
        "unset: the config decides (default rollback)"
    );
    assert_eq!(NativeConfig::default().netplay.mode, NetMode::Rollback);
    for (word, mode) in [
        ("rollback", NetMode::Rollback),
        ("lockstep", NetMode::Lockstep),
    ] {
        let a = app::parse_native_args(&argv(&["z2-native", "--net-mode", word])).expect("parses");
        assert_eq!(a.net_mode, Some(mode));
    }
    // Lockstep keeps its 0-8 range; rollback accepts 0-3.
    let a = app::parse_native_args(&argv(&[
        "z2-native",
        "--net-mode",
        "lockstep",
        "--net-delay",
        "6",
    ]))
    .expect("lockstep delay 6 parses");
    assert_eq!(a.net_delay, Some(6));
    for bad in [
        vec!["z2-native", "--net-mode", "ggpo"],
        vec!["z2-native", "--net-mode"],
        vec!["z2-native", "--net-mode", "rollback", "--net-delay", "4"],
    ] {
        let err = app::parse_native_args(&argv(&bad)).expect_err(&format!("must reject {bad:?}"));
        assert!(err.contains("usage:"), "{err}");
    }
}

#[test]
fn netplay_and_movie_playback_are_mutually_exclusive() {
    let err = app::parse_native_args(&argv(&[
        "z2-native",
        "--coop-host",
        "r",
        "--movie",
        "m.bk2",
    ]))
    .expect_err("a movie would override the confirmed pads");
    assert!(err.contains("mutually exclusive"), "{err}");
}

#[test]
fn help_still_surfaces_usage_and_documents_the_new_flags() {
    let err = app::parse_native_args(&argv(&["z2-native", "--help"])).expect_err("help");
    for needle in [
        "usage:",
        "--rom",
        ".nes",
        "drop",
        "--headless",
        "--movie",
        "--widescreen",
        "--coop-local",
        "--coop-host",
        "--coop-join",
        "--signal",
        "--net-delay",
        "--net-mode",
        "rollback|lockstep",
        "--p2-pad",
        "--hd-pack",
        "--hd-scale",
        "--hd-record",
        "--fill-left-clip",
        "--fill-right-clip",
        "--margin-sprites",
    ] {
        assert!(err.contains(needle), "usage must mention {needle}");
    }
}

// ------------------------------------------------------------ feature resolve

#[test]
fn cli_overrides_config_and_config_supplies_the_default() {
    let mut cfg = NativeConfig {
        widescreen: "16:10".into(),
        coop_local: true,
        ..NativeConfig::default()
    };
    // No CLI flags: the config decides.
    let none = app::parse_native_args(&argv(&["z2-native"])).unwrap();
    let d = app::resolve_display(&none, &cfg).unwrap();
    assert_eq!(d.wide_tiles, 8, "config 16:10 = 8 tiles/side");
    let f = app::resolve_features(&none, &cfg).unwrap();
    assert!(f.coop, "config coop_local honoured");
    assert!(f.record, "widescreen needs the render record");
    assert!(
        f.margin_sprites,
        "widescreen draws margin sprites by default"
    );

    // Margin sprites: config key, CLI override, and never without margins.
    let off = NativeConfig {
        widescreen_margin_sprites: false,
        ..cfg.clone()
    };
    assert!(!app::resolve_features(&none, &off).unwrap().margin_sprites);
    let on = app::parse_native_args(&argv(&["z2-native", "--margin-sprites", "on"])).unwrap();
    assert!(app::resolve_features(&on, &off).unwrap().margin_sprites);
    let narrow = app::parse_native_args(&argv(&[
        "z2-native",
        "--widescreen",
        "off",
        "--margin-sprites",
        "on",
    ]))
    .unwrap();
    assert!(!app::resolve_features(&narrow, &cfg).unwrap().margin_sprites);
    assert_eq!(
        f.wide_gameplay,
        Some(8),
        "wide gameplay follows widescreen by default"
    );

    // CLI wins over the config for widescreen.
    let cli = app::parse_native_args(&argv(&["z2-native", "--widescreen", "off"])).unwrap();
    assert_eq!(app::resolve_display(&cli, &cfg).unwrap().wide_tiles, 0);
    assert!(
        !app::resolve_features(&cli, &cfg).unwrap().record,
        "no widescreen, no pack: the record stays off"
    );
    assert_eq!(
        app::resolve_features(&cli, &cfg).unwrap().wide_gameplay,
        None,
        "no widescreen, no wide gameplay"
    );
    let off = app::parse_native_args(&argv(&["z2-native", "--wide-gameplay", "off"])).unwrap();
    assert_eq!(
        app::resolve_features(&off, &cfg).unwrap().wide_gameplay,
        None
    );

    // And for the HD keys.
    let hd = NativeConfig {
        hd_scale: 4,
        ..NativeConfig::default()
    };
    assert_eq!(app::resolve_display(&none, &hd).unwrap().scale, 4);
    let cli2 = app::parse_native_args(&argv(&["z2-native", "--hd-scale", "2"])).unwrap();
    assert_eq!(app::resolve_display(&cli2, &hd).unwrap().scale, 2);
    // A nonsense scale in the CONFIG degrades instead of refusing to start.
    let bad = NativeConfig {
        hd_scale: 999,
        ..NativeConfig::default()
    };
    assert_eq!(app::resolve_display(&none, &bad).unwrap().scale, 8);

    // Any online mode implies two Links even with coop_local off in config.
    cfg.coop_local = false;
    let host = app::parse_native_args(&argv(&["z2-native", "--coop-host", "r"])).unwrap();
    assert!(
        app::resolve_features(&host, &cfg).unwrap().coop,
        "a session must never run with a second pad the ROM ignores"
    );
}

// --------------------------------------------------------------- texture sizing

#[test]
fn present_size_follows_the_widescreen_preset_and_scale() {
    assert_eq!(app::present_size(0), (256, 240));
    assert_eq!(app::present_size(8), (384, 240), "16:10");
    assert_eq!(app::present_size(11), (432, 240), "16:9");
    assert_eq!(app::present_size(16), (512, 240), "widest");
    assert_eq!(app::present_size_scaled(0, 2), (512, 480));
    assert_eq!(app::present_size_scaled(11, 4), (1728, 960));
    // …and the presenter agrees, which is what the texture is sized from.
    for tiles in [0u8, 8, 11, 16] {
        for scale in [1u32, 2, 4] {
            let d = Display::new(DisplaySettings {
                wide_tiles: tiles,
                scale,
                fill_left_clip: true,
                fill_right_clip: true,
                pack_dir: None,
                record_dir: None,
                margin_sprites: false,
            })
            .expect("no pack");
            assert_eq!(d.size(), app::present_size_scaled(tiles, scale));
        }
    }
}

#[test]
fn window_never_opens_smaller_than_its_texture() {
    // pixels 0.15 clamps its integer scale to >= 1, so a window smaller than
    // the texture CROPS. Whatever the monitor, the logical size must cover it.
    for tiles in [0u8, 8, 11, 16] {
        let (tw, th) = app::present_size(tiles);
        for monitor in [
            None,
            Some((1280, 800)),
            Some((3840, 2160)),
            Some((640, 480)),
        ] {
            let (lw, lh) = app::initial_window_size(tw, th, monitor);
            assert!(
                lw >= f64::from(tw) && lh >= f64::from(th),
                "tiles={tiles} monitor={monitor:?} gave {lw}x{lh} for a {tw}x{th} texture"
            );
            // Integer multiple, so pixels scales exactly with no half pixels.
            assert_eq!(lw % f64::from(tw), 0.0, "non-integer scale {lw}/{tw}");
        }
    }
    // A roomy display gets 3x; a small one steps down rather than cropping.
    assert_eq!(
        app::initial_window_size(256, 240, Some((3840, 2160))),
        (768.0, 720.0)
    );
    let (w, _) = app::initial_window_size(432, 240, Some((1280, 800)));
    assert!(w <= 1280.0, "16:9 must fit a 1280-wide display, got {w}");
}

// ------------------------------------------------------- no-ROM start, any flags

/// The failure both critiques predicted: a no-ROM launch under a feature flag.
/// Whatever the flags, the app must build, size its texture from the presenter
/// and compose a frame of exactly that size without panicking — before any
/// frame has been stepped and after.
#[test]
fn no_rom_start_is_consistent_under_every_feature_combination() {
    for tiles in [0u8, 8, 11, 16] {
        for scale in [1u32, 2, 3] {
            for coop in [false, true] {
                let settings = DisplaySettings {
                    wide_tiles: tiles,
                    scale,
                    fill_left_clip: tiles > 0,
                    fill_right_clip: tiles > 0,
                    pack_dir: None,
                    record_dir: None,
                    margin_sprites: false,
                };
                let what = format!("tiles={tiles} scale={scale} coop={coop}");
                let feats = settings.features(coop);
                assert_eq!(feats.coop, coop);
                assert_eq!(feats.record, tiles > 0, "{what}: record arming");
                let mut emu = app::new_emu_with(44_100, feats);
                assert_eq!(emu.game.record_enabled(), feats.record, "{what}");
                // Co-op is deliberately not enabled without a cartridge: there
                // is no ROM code for the co-op wrappers to override.
                assert!(emu.game.coop_status().is_none(), "{what}");

                let mut display = Display::new(settings).expect("no pack cannot fail");
                let (w, h) = display.size();
                assert_eq!((w, h), app::present_size_scaled(tiles, scale), "{what}");
                let rgba = display.present(&emu.game).expect("power-on present");
                assert_eq!(rgba.len() as u32, w * h * 4, "{what}");
                assert!(rgba.chunks_exact(4).all(|p| p[3] == 0xFF), "{what}: opaque");
                // And after real frames through the shared stepping primitive.
                app::step_frames(&mut emu, &[0u8; 4], None);
                let rgba = display.present(&emu.game).expect("stepped present");
                assert_eq!(rgba.len() as u32, w * h * 4, "{what}");
            }
        }
    }
}

/// A `--coop-local` no-ROM start must also survive the two-pad step path.
#[test]
fn no_rom_two_pad_stepping_and_present_is_safe() {
    let settings = DisplaySettings {
        wide_tiles: 11,
        scale: 2,
        fill_left_clip: true,
        fill_right_clip: true,
        pack_dir: None,
        record_dir: None,
        margin_sprites: false,
    };
    let mut emu = app::new_emu_with(44_100, settings.features(true));
    let mut display = Display::new(settings).expect("no pack");
    app::step_frames2(&mut emu, &[(0xFF, 0xFF); 4], None);
    let (w, h) = display.size();
    assert_eq!(
        display.present(&emu.game).expect("present").len() as u32,
        w * h * 4
    );
}

// ---------------------------------------------------------- ROM-drop rebuild

/// A ROM built with features carries them; this is the shape the `DroppedFile`
/// handler uses, so a drop cannot silently lose widescreen or co-op.
#[test]
fn rom_build_applies_features_and_reports_a_trapset() {
    let Some(path) = app::env_path_if_usable("Z2_ROM") else {
        eprintln!("SKIP rom_build_applies_features_and_reports_a_trapset: Z2_ROM unset");
        return;
    };
    let feats = Features {
        coop: true,
        wide_gameplay: None,
        record: true,
        margin_sprites: false,
    };
    let (emu, body) = app::emu_from_rom_file_with(&path, 44_100, feats).expect("build");
    assert_eq!(
        body.len(),
        z2_assets::rom::EXPECTED_BODY_LEN,
        "body kept for netplay"
    );
    assert!(
        emu.game.record_enabled(),
        "widescreen needs the render record"
    );
    assert!(emu.game.coop_status().is_some(), "co-op enabled");
    assert_ne!(emu.trapset_id, 0, "trap-set identity captured");

    // The default build keeps every feature off, and the trap-set identity is
    // the SAME either way: it is captured before co-op registration, so the two
    // peers of a session agree regardless of who enabled co-op first.
    let plain = app::emu_from_rom_body(&body, 44_100).expect("plain build");
    assert!(!plain.game.record_enabled());
    assert!(plain.game.coop_status().is_none());
    assert_eq!(
        plain.trapset_id, emu.trapset_id,
        "co-op must not change the trap-set identity peers compare"
    );
}

// ------------------------------------------------------------- HD pack wiring

/// End to end for the HD path, against the real ROM: step real frames through
/// the shared primitive, present them through the same `Display` the window
/// uses with recording on, write the recorded template pack, then load that
/// pack back and present through it. This is the whole artist round trip —
/// play, record, repaint, play again — with the repaint step skipped.
#[test]
fn hd_recording_round_trips_into_a_loadable_pack() {
    let Some(rom) = app::env_path_if_usable("Z2_ROM") else {
        eprintln!("SKIP hd_recording_round_trips_into_a_loadable_pack: Z2_ROM unset");
        return;
    };
    let dir = std::env::temp_dir().join(format!("z2-hdrec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let settings = DisplaySettings {
        wide_tiles: 0,
        scale: 2,
        fill_left_clip: false,
        fill_right_clip: false,
        pack_dir: None,
        record_dir: Some(dir.clone()),
        margin_sprites: false,
    };
    let feats = settings.features(false);
    assert!(feats.record, "recording needs the PPU render record");
    let (mut emu, _body) =
        app::emu_from_rom_file_with(&rom, 44_100, feats).expect("build from ROM");
    let mut display = Display::new(settings).expect("no pack to load yet");
    assert!(display.needs_record());
    // 400 frames is past the copyright screen and into the title, so real
    // tiles are drawn with real palettes.
    for _ in 0..40 {
        app::step_frames(&mut emu, &[0u8; 10], None);
        display.present(&emu.game).expect("present");
    }
    let out = display
        .write_recorded_pack(&emu.game.chr)
        .expect("write the recorded pack")
        .expect("a directory was configured");
    assert_eq!(out, dir);
    assert!(dir.join("pack.json").is_file(), "pack.json written");
    assert!(
        dir.join("TEMPLATE-ROM-DERIVED.txt").is_file(),
        "the ROM-derived marker travels with the sheets"
    );

    // …and the pack it wrote is one the frontend can load and present with.
    let reload = DisplaySettings {
        wide_tiles: 11,
        scale: 2,
        fill_left_clip: true,
        fill_right_clip: true,
        pack_dir: Some(dir.clone()),
        record_dir: None,
        margin_sprites: false,
    };
    let mut hd = Display::new(reload).expect("the recorded pack loads");
    assert_eq!(hd.size(), app::present_size_scaled(11, 2));
    assert_eq!(
        hd.effective_scale(),
        2,
        "scale 2 divides the pack's scale 2"
    );
    let rgba = hd.present(&emu.game).expect("present through the pack");
    assert_eq!(rgba.len() as u32, hd.size().0 * hd.size().1 * 4);
    let _ = std::fs::remove_dir_all(&dir);
}

// --------------------------------------------------------- save-state interlock

#[test]
fn save_states_are_blocked_only_while_a_session_is_live() {
    assert!(netplay::savestate_blocked(false).is_none());
    let why = netplay::savestate_blocked(true).expect("blocked during netplay");
    assert!(why.contains("desync"), "must say why: {why}");
}

#[test]
fn a_transportless_build_refuses_online_flags_clearly() {
    let r = app::parse_native_args(&argv(&["z2-native", "--coop-host", "r"]));
    if netplay::supported() {
        assert!(r.is_ok(), "this build has the transport");
    } else {
        let err = r.expect_err("no transport: must refuse, not pretend");
        assert!(err.contains("netplay"), "{err}");
    }
}

// --------------------------------------------------------------- input mapping

#[test]
fn second_player_keys_are_documented_and_disjoint() {
    let p1 = KeyBindings::default();
    let p2 = KeyBindings::default_p2();
    assert_eq!(p2.bit_for("KeyG"), Some(0));
    assert_eq!(p2.bit_for("KeyF"), Some(1));
    assert_eq!(p2.bit_for("KeyW"), Some(4));
    assert_eq!(p2.bit_for("KeyD"), Some(7));
    for k in p2.map.keys() {
        assert!(!p1.map.contains_key(k), "P2 key {k} collides with P1");
    }
    // The usage text must name the P2 layout or nobody can discover it.
    for needle in ["G=A", "F=B", "W/A/S/D"] {
        assert!(
            app::NATIVE_USAGE.contains(needle),
            "usage must document {needle}"
        );
    }
}

// ------------------------------------------------------------------ env policy

#[test]
fn env_rom_path_counts_as_absent_when_unusable() {
    let var = "Z2_TEST_ROM_PATH_PROBE";
    std::env::remove_var(var);
    assert!(app::env_path_if_usable(var).is_none(), "unset = absent");
    std::env::set_var(var, "");
    assert!(app::env_path_if_usable(var).is_none(), "empty = absent");
    std::env::set_var(var, "/nonexistent-dir-z2rs/none.nes");
    assert!(
        app::env_path_if_usable(var).is_none(),
        "missing file = absent"
    );
    // An existing file is returned as-is.
    let dir = std::env::temp_dir().join(format!("z2-envprobe-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("present.nes");
    std::fs::write(&f, b"x").unwrap();
    std::env::set_var(var, &f);
    assert_eq!(app::env_path_if_usable(var).as_deref(), Some(f.as_path()));
    std::env::remove_var(var);
    let _ = std::fs::remove_dir_all(&dir);
}
