//! Local co-op (`--coop-local` feature path) end-to-end regression, headless.
//!
//! Exercises the same construction and two-pad stepping primitives the
//! windowed local-co-op loop uses ([`app::emu_from_rom_file_with`] with
//! `Features { coop: true }` — the table [`app::resolve_coop`] feeds — and
//! [`app::step_frames2`]) without opening a window or an audio device.
//!
//! Notes on what IS observable:
//! * cooperation state itself is emulator-side (`Game::coop_status()`);
//!   there is no P2 fact in `GameFacts` (facts cover player 1 only), so P2
//!   assertions go through `coop_status()` and the pad-2 RAM bytes
//!   `$F6/$F8` (echoed by `facts.input.p2_*`).
//! * `$F8` holds pad 2's debounced *held* levels, MSB-first (bit 7 = A …
//!   bit 0 = Right), so held-Right reads as `$F8 & 1 == 1`.
//!
//! All tests skip silently without `$Z2_ROM`.

mod common;

use z2_core::facts::Game as FactsGame;
use z2_core::ram::Ram;
use z2_native::app::{self, Features};

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const RIGHT: u8 = 0x80;
const MODE_SIDEVIEW: u8 = 0x0B;
const MODE: usize = 0x0736;

fn coop_emu(test: &str) -> Option<app::Emu> {
    let path = common::rom_path(test)?;
    let feats = Features {
        coop: true,
        wide_gameplay: None,
        record: false,
        margin_sprites: false,
    };
    let (emu, _body) = app::emu_from_rom_file_with(&path, 44_100, feats).expect("build ROM emu");
    Some(emu)
}

fn step2(emu: &mut app::Emu, frames: usize, p1: u8, p2: u8) {
    app::step_frames2(emu, &vec![(p1, p2); frames], None);
}

/// The settled title → file-select → name → load edge sequence from
/// `save_flow.rs`, driven on pad 1 with pad 2 idle.
fn enter_named_save(emu: &mut app::Emu) {
    step2(emu, 30, 0, 0);
    step2(emu, 5, START, 0);
    step2(emu, 20, 0, 0);
    step2(emu, 5, START, 0);
    step2(emu, 20, 0, 0);
    for _ in 0..8 {
        step2(emu, 2, A, 0);
        step2(emu, 8, 0, 0);
    }
    for _ in 0..3 {
        step2(emu, 2, SELECT, 0);
        step2(emu, 8, 0, 0);
    }
    step2(emu, 5, START, 0);
    step2(emu, 30, 0, 0);
}

/// Drive the menu flow into loaded side-view gameplay with P1 only, and
/// assert the co-op run got there healthy with P2 anchored beside P1.
fn reach_coop_gameplay(emu: &mut app::Emu) {
    enter_named_save(emu);
    step2(emu, 5, START, 0);
    step2(emu, 60, 0, 0);
    step2(emu, 110, 0, 0);

    assert_eq!(emu.game.exec_errors, 0, "co-op menu/load path wedged");
    assert_eq!(
        emu.game.ram[MODE], MODE_SIDEVIEW,
        "co-op run should reach side-view gameplay"
    );

    let st = emu
        .game
        .coop_status()
        .expect("co-op enabled at construction");
    assert!(st.active, "P2 is live in the side-view area");
    assert!(st.p2_alive, "P2 spawned alive next to P1");
    assert!(st.p2_deaths == 0, "idle P2 should not have died: {st:?}");
}

/// Run `body` on a thread with a much larger stack: the co-op P2 slices
/// nest extra interpreter frames around each side-view frame, which blows
/// the default 8 MiB test-thread stack in debug builds. Wrapper so each
/// `#[test]` fn stays one line.
fn tall_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(128 * 1024 * 1024)
        .spawn(body)
        .expect("spawn tall-stack test thread")
        .join()
        .expect("tall-stack thread panicked");
}

#[test]
fn coop_boot_reaches_sideview_gameplay_with_p2_idle() {
    tall_stack(|| {
        let Some(mut emu) = coop_emu("native_coop_boot_sideview_p2_idle") else {
            return;
        };

        // Co-op comes in at construction (the `--coop-local` feature path);
        // the runtime toggle through apply_features must agree too.
        assert!(emu.game.coop_status().is_some(), "co-op on at construction");
        let path = common::rom_path("native_coop_boot_sideview_p2_idle")
            .expect("ROM was readable a moment ago");
        let plain = app::emu_from_rom_file(&path, 44_100).expect("plain build");
        assert!(plain.game.coop_status().is_none(), "default build is solo");
        let mut toggled = plain;
        app::apply_features(
            &mut toggled,
            Features {
                coop: true,
                wide_gameplay: None,
                record: false,
                margin_sprites: false,
            },
        );
        assert!(
            toggled.game.coop_status().is_some(),
            "apply_features enables co-op"
        );

        reach_coop_gameplay(&mut emu);

        // Pad 2 has been idle the whole run: the latched pad-2 bytes and the
        // facts echo must both say so, while the run keeps stepping.
        for _ in 0..3 {
            step2(&mut emu, 100, 0, 0);
            assert_eq!(emu.game.exec_errors, 0, "co-op gameplay wedged");
            assert_eq!(emu.game.ram[MODE], MODE_SIDEVIEW);
            assert_eq!(emu.game.ram[0x00F8], 0, "pad-2 held byte stays idle");
            assert_eq!(emu.game.ram[0x00F6], 0, "pad-2 edge byte stays idle");
            let facts = facts_of(&emu);
            assert_eq!(facts.input.p2_held, 0, "facts echo pad 2 idle");
            assert_eq!(facts.input.p2_pressed, 0);
            let st = emu.game.coop_status().expect("co-op stays on");
            assert!(st.active && st.p2_alive, "long run keeps P2 alive: {st:?}");
        }
    });
}

#[test]
fn p2_pad_reaches_the_world_and_p1_mode_is_undisturbed() {
    tall_stack(|| {
        let Some(mut emu) = coop_emu("native_coop_p2_pad_reaches_world") else {
            return;
        };
        reach_coop_gameplay(&mut emu);

        let before = emu.game.coop_status().expect("co-op on");
        // Hold Right on pad 2 only: the P2 update slice feeds pad 2's bytes
        // to the Link driver, so P2 should walk while P1's pad stays idle.
        step2(&mut emu, 90, 0, RIGHT);

        assert_eq!(emu.game.exec_errors, 0, "P2-pad run wedged");
        assert_eq!(
            emu.game.ram[MODE], MODE_SIDEVIEW,
            "pad-2 play must not disturb the P1 mode byte"
        );
        // Pad 2's input actually reached the game: the held byte is
        // MSB-first (bit 0 = Right), and pad 1's bytes stayed idle.
        assert_eq!(
            emu.game.ram[0x00F8] & 1,
            1,
            "$F8 bit 0 = Right held on pad 2"
        );
        assert_eq!(emu.game.ram[0x00F7], 0, "pad 1 held byte idle");
        let facts = facts_of(&emu);
        assert_eq!(facts.input.p2_held & 1, 1, "facts echo pad-2 Right");
        assert_eq!(facts.input.p1_held, 0);

        // P2 should have moved (or, worst case, been stopped by the world —
        // death by contact is also a pad-2-driven world event, visible as
        // the respawn timer). An anchor-frozen idle P2 would mean pad 2 is
        // deaf.
        let after = emu.game.coop_status().expect("co-op still on");
        let moved = after.p2_x != before.p2_x
            || after.p2_page != before.p2_page
            || after.p2_screen_x != before.p2_screen_x;
        assert!(
            moved || after.respawn_frames > 0 || after.p2_deaths > before.p2_deaths,
            "P2 did not respond to its own pad: before={before:?} after={after:?}"
        );
    });
}

#[test]
fn coop_dual_pad_script_is_deterministic() {
    tall_stack(|| {
        let Some(path) = common::rom_path("native_coop_dual_pad_deterministic") else {
            return;
        };
        let feats = Features {
            coop: true,
            wide_gameplay: None,
            record: false,
            margin_sprites: false,
        };
        // Shared script: pad-2 idle through the menu flow, then pad 2 walks
        // right while pad 1 idles in gameplay.
        let mut script: Vec<(u8, u8)> = Vec::new();
        let push = |script: &mut Vec<(u8, u8)>, n: usize, p1: u8, p2: u8| {
            for _ in 0..n {
                script.push((p1, p2));
            }
        };
        push(&mut script, 30, 0, 0);
        push(&mut script, 5, START, 0);
        push(&mut script, 20, 0, 0);
        push(&mut script, 5, START, 0);
        push(&mut script, 20, 0, 0);
        for _ in 0..8 {
            push(&mut script, 2, A, 0);
            push(&mut script, 8, 0, 0);
        }
        for _ in 0..3 {
            push(&mut script, 2, SELECT, 0);
            push(&mut script, 8, 0, 0);
        }
        push(&mut script, 5, START, 0);
        push(&mut script, 30, 0, 0);
        // …then the save-load tail (second START, side-view entry, settling).
        push(&mut script, 5, START, 0);
        push(&mut script, 60, 0, 0);
        push(&mut script, 110, 0, 0);
        push(&mut script, 60, 0, RIGHT);
        push(&mut script, 120, 0, 0);

        let run = |tag: &str| -> String {
            let (mut emu, _body) =
                app::emu_from_rom_file_with(&path, 44_100, feats).expect("build fresh emu");
            app::step_frames2(&mut emu, &script, None);
            assert_eq!(emu.game.exec_errors, 0, "{tag}: co-op run wedged");
            assert_eq!(emu.game.ram[MODE], MODE_SIDEVIEW, "{tag}: reached gameplay");
            let st = emu.game.coop_status().expect("{tag}: co-op on");
            assert!(st.active, "{tag}: P2 live: {st:?}");
            facts_json(&emu)
        };
        let a = run("run A");
        let b = run("run B");
        assert_eq!(a, b, "the co-op dual-pad path is not deterministic");
    });
}

fn facts_of(emu: &app::Emu) -> z2_core::facts::GameFacts {
    let ram = Ram::from_slice(&emu.game.ram).expect("2048-byte game RAM");
    FactsGame::new(ram).facts()
}

fn facts_json(emu: &app::Emu) -> String {
    facts_of(emu).to_json()
}
