//! No-ROM window state at the state-machine level (grey-window fix).
//!
//! No window can open in this environment, so these are headless proxies:
//! they exercise the exact pure helpers the event loop is wired to
//! (`no_rom_title`, `window_title`, `WindowUiState`, `classify_dropped_file`,
//! `emu_from_rom_file` rejection, usage text) without touching winit/pixels.

use std::path::Path;

use z2_native::app::{self, DroppedFileKind, WindowUiState, NATIVE_USAGE, NO_ROM_TITLE};

#[test]
fn no_rom_title_is_exact_and_shown_while_cartless() {
    assert_eq!(
        NO_ROM_TITLE,
        "z2rs — no ROM: drop a .nes file or restart with --rom PATH"
    );
    assert_eq!(app::no_rom_title(), NO_ROM_TITLE);
    assert_eq!(
        app::window_title(60.0, 0, false, false, false),
        NO_ROM_TITLE
    );
    // Paused or fast-forward, the cartless window still says no-ROM.
    assert_eq!(app::window_title(60.0, 0, true, true, false), NO_ROM_TITLE);
    // With a ROM the meter line returns.
    let t = app::window_title(59.9, 2, false, false, true);
    assert!(t.starts_with("z2rs — "));
    assert!(!t.contains("no ROM"));
}

#[test]
fn drop_to_load_transition_is_synthetic_but_exact() {
    // Launch cartless: running behind the no-ROM title.
    let mut ui = WindowUiState::new(false);
    assert!(!ui.has_rom);
    assert!(!ui.paused);
    assert_eq!(ui.title(60.0, 0, false), NO_ROM_TITLE);
    // A rejected drop leaves the state untouched …
    let dir = std::env::temp_dir().join(format!("z2-norom-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let bad = dir.join("bad.nes");
    std::fs::write(&bad, b"not a rom").unwrap();
    assert!(app::emu_from_rom_file(&bad, 44100).is_err());
    assert!(!ui.has_rom && !ui.paused);
    assert_eq!(ui.title(60.0, 0, false), NO_ROM_TITLE);
    // … while an accepted drop (modelled by the same `on_rom_loaded` call
    // the event loop makes) restores the normal title and unpauses.
    ui.on_rom_loaded();
    assert!(ui.has_rom && !ui.paused);
    let t = ui.title(60.0, 0, false);
    assert!(t.starts_with("z2rs — "));
    assert!(!t.contains("no ROM"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drop_routing_sends_movies_to_demo_and_rest_to_rom() {
    assert_eq!(
        app::classify_dropped_file(Path::new("run.fm2")),
        DroppedFileKind::Movie
    );
    assert_eq!(
        app::classify_dropped_file(Path::new("run.BK2")),
        DroppedFileKind::Movie
    );
    assert_eq!(
        app::classify_dropped_file(Path::new("zelda2.nes")),
        DroppedFileKind::Rom
    );
}

#[test]
fn ever_focused_gate_blocks_open_unfocused_pause() {
    let mut ui = WindowUiState::new(true);
    ui.on_focus(false, true);
    assert!(!ui.paused, "macOS open-unfocused must not instant-pause");
    ui.on_focus(true, true);
    ui.paused = false;
    ui.on_focus(false, true);
    assert!(ui.paused, "backgrounding after focus pauses with pref on");
    let mut ui = WindowUiState::new(true);
    ui.on_focus(true, false);
    ui.paused = false;
    ui.on_focus(false, false);
    assert!(!ui.paused, "pref off never auto-pauses");
}

#[test]
fn usage_text_documents_rom_recovery() {
    for needle in ["--rom", ".nes", "drop", "--headless", "--movie"] {
        assert!(
            NATIVE_USAGE.contains(needle),
            "usage must mention {needle}:\n{NATIVE_USAGE}"
        );
    }
    let err = app::parse_native_args(&["z2-native".to_string(), "--help".to_string()])
        .expect_err("--help prints usage");
    assert!(err.contains("usage:"));
    assert!(err.contains("--rom"));
}
