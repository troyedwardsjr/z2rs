//! Quality-of-life enhancements (`z2_core::enh::qol`) in real runs.
//!
//! ROM-gated (self-skip unless `Z2_ROM` names a file) and movie-gated (the
//! corpus movies from `Z2_CORPUS_MOVIES`, else `$Z2_CORPUS/movies`, else
//! the out-of-tree default; self-skip when absent). Each test reaches a
//! state by replaying a TAS (the any% movie walks through Rauru and onto
//! the West overworld; the 100% movie picks up a Life Doll at frame
//! ~3001), then drives inputs or sets up RAM the way the game would (force
//! the die routine, enter a palace slot) and compares the original game
//! with the option on.

#![cfg(feature = "interp")]

mod common;

use std::sync::OnceLock;

use z2_core::enh::{qol, ContinueFrom, Enhancements, QolOpts};
use z2_core::game::Game;
use z2_core::state::GameState;

// ------------------------------------------------------------ helpers

const START: u8 = 1 << z2_core::game::INPUT_START;
const RIGHT: u8 = 1 << z2_core::game::INPUT_RIGHT;

const LIVES: usize = 0x0700;
const REGION: usize = 0x0706;
const WORLD: usize = 0x0707;
const MODE: usize = 0x0736;
const AREA: usize = 0x0748;
const STAGE: usize = 0x076C;
const SCENE: usize = 0x0561;
const TOWN: usize = 0x056B;
const PAGE: usize = 0x003B;
const LINK_X: usize = 0x004D;
const FACING: usize = 0x009F;
const MAGIC: usize = 0x0773;
const MAGIC_CTR: usize = 0x0783;
const SPELLS: usize = 0x077B;
const XP_HI: usize = 0x0775;
const XP_LO: usize = 0x0776;
const MENU_SEL: usize = 0x0488;

/// Parapa Palace (West, slot `$34`).
const PALACE1_SLOT: u8 = 0x34;

fn register_default_groups(game: &mut Game) {
    z2_core::bank7_traps::register_bank7_traps(game);
    z2_core::sideview_traps::register_sideview_traps(game);
    z2_core::sideview_traps::register_overworld_traps(game);
    z2_core::player_traps::register_player_traps(game);
    z2_core::enemy_traps::register_enemy_traps(game);
    z2_core::town_traps::register_town_traps(game);
    z2_core::palace_traps::register_palace_traps(game);
    z2_core::title_traps::register_title_traps(game);
    z2_core::boot_traps::register_boot_traps(game);
}

fn rom(test: &str) -> Option<&'static [u8]> {
    static ROM: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    ROM.get_or_init(|| common::rom_bytes(test)).as_deref()
}

fn movie_track(name: &str, test: &str) -> Option<Vec<u8>> {
    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .or_else(|| common::var_present("Z2_CORPUS").map(|c| format!("{c}/movies")))
        .unwrap_or_else(|| "/Volumes/HolyDrive/dev/z2-corpus/movies".to_string());
    let path = common::file_present(&std::path::Path::new(&dir).join(name), test)?;
    let bytes = std::fs::read(path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&bytes).expect("parse bk2");
    Some(movie.pad1_track())
}

fn qol_game(rom: &[u8], q: QolOpts) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    register_default_groups(&mut g);
    g.set_enhancements(Enhancements {
        qol: q,
        ..Enhancements::default()
    });
    g.reset();
    g
}

/// Recording options for the shared snapshots: the area-entry hook records
/// towns and palaces, nothing else acts before a game over.
fn recording() -> QolOpts {
    QolOpts {
        continue_from: ContinueFrom::Both,
        ..QolOpts::default()
    }
}

/// The any% movie replayed to `frames` with [`recording`] on, as a
/// snapshot (Rauru is entered at frame ~895 and left at ~2435).
fn anypct_at(frames: usize, test: &str) -> Option<GameState> {
    let rom = rom(test)?;
    let track = movie_track("anypct.bk2", test)?;
    let mut g = qol_game(rom, recording());
    for &p in track.iter().take(frames) {
        g.step(p);
    }
    Some(g.save_state())
}

/// Snapshot on the West overworld after visiting Rauru (frame 2600).
fn overworld(test: &str) -> Option<&'static GameState> {
    static S: OnceLock<Option<GameState>> = OnceLock::new();
    S.get_or_init(|| anypct_at(2600, test)).as_ref()
}

/// Snapshot just before Link walks onto Rauru's tile (frame 893).
fn before_rauru(test: &str) -> Option<&'static GameState> {
    static S: OnceLock<Option<GameState>> = OnceLock::new();
    S.get_or_init(|| anypct_at(893, test)).as_ref()
}

/// Snapshot inside Rauru (frame 960, sideview running).
fn in_rauru(test: &str) -> Option<&'static GameState> {
    static S: OnceLock<Option<GameState>> = OnceLock::new();
    S.get_or_init(|| anypct_at(960, test)).as_ref()
}

fn from_state(rom: &[u8], s: &GameState, q: QolOpts) -> Game {
    let mut g = qol_game(rom, q);
    g.load_state(s);
    g
}

fn step_n(g: &mut Game, pad: u8, n: usize) {
    for _ in 0..n {
        g.step(pad);
    }
}

fn step_until(g: &mut Game, max: usize, mut done: impl FnMut(&Game) -> bool) -> bool {
    for _ in 0..max {
        if done(g) {
            return true;
        }
        g.step(0);
    }
    done(g)
}

fn xp(g: &Game) -> u16 {
    u16::from_be_bytes([g.ram[XP_HI], g.ram[XP_LO]])
}

fn set_xp(g: &mut Game, v: u16) {
    let [h, l] = v.to_be_bytes();
    g.ram[XP_HI] = h;
    g.ram[XP_LO] = l;
}

/// Run the die routine on the last life (`$076C = 2`, the game's own
/// "die" request), wait for the game-over CONTINUE/SAVE screen, and return
/// once it is up.
fn die_to_gameover(g: &mut Game) {
    g.ram[LIVES] = 1;
    g.ram[STAGE] = 2;
    wait_gameover_screen(g);
}

fn wait_gameover_screen(g: &mut Game) {
    // Stage 2, mode 9 is `LCA85` (the CONTINUE/SAVE choice).
    let up = step_until(g, 3000, |g| g.ram[STAGE] == 2 && g.ram[MODE] == 9);
    assert!(up, "game-over screen never came up");
    g.ram[MENU_SEL] = 0; // cursor on CONTINUE
}

/// Press Start on CONTINUE and wait until the restarted area is running.
fn press_continue(g: &mut Game) {
    step_n(g, START, 2);
    step_n(g, 0, 2);
    assert_eq!(g.ram[STAGE], 1, "continue restarts the game");
    // Let the loader chain finish (towns/palaces reach sideview mode $0B,
    // the North Palace restart runs its own intro modes).
    step_n(g, 0, 240);
}

// ------------------------------------------------------------ continue_from

#[test]
fn continue_from_last_town_restarts_in_rauru() {
    let t = "continue_from_last_town_restarts_in_rauru";
    let (Some(rom), Some(s)) = (rom(t), overworld(t)) else {
        return;
    };
    let rec = from_state(rom, s, recording()).enh_state;
    assert!(rec.continue_valid, "Rauru was recorded on entry");
    assert_eq!(
        rec.continue_loc[0],
        qol::pack_loc(0, 0x2D),
        "West, slot $2D"
    );

    let mut og = from_state(rom, s, QolOpts::default());
    die_to_gameover(&mut og);
    press_continue(&mut og);
    assert_eq!(og.ram[WORLD], 0, "original: North Palace (overworld world)");
    assert_eq!(og.ram[AREA], 0, "original: key area 0 is the North Palace");

    for mode in [ContinueFrom::LastTown, ContinueFrom::Both] {
        let mut g = from_state(
            rom,
            s,
            QolOpts {
                continue_from: mode,
                ..QolOpts::default()
            },
        );
        die_to_gameover(&mut g);
        press_continue(&mut g);
        assert_eq!(g.ram[WORLD], 1, "{mode:?}: a West town");
        assert_eq!(g.ram[TOWN], 0, "{mode:?}: Rauru");
        assert_eq!(g.ram[SCENE], 2, "{mode:?}: Rauru's scene");
        assert_eq!(g.ram[MODE], 0x0B, "{mode:?}: sideview running");
        assert_eq!(g.ram[LIVES], 3);
        // Entered like a walk-in: the original right-end start.
        assert_eq!(g.ram[PAGE], 3);
    }

    // Palace-only mode leaves an overworld game over original.
    let mut d = from_state(
        rom,
        s,
        QolOpts {
            continue_from: ContinueFrom::DungeonEntrance,
            ..QolOpts::default()
        },
    );
    die_to_gameover(&mut d);
    press_continue(&mut d);
    assert_eq!((d.ram[WORLD], d.ram[AREA]), (0, 0));
}

/// Walk into Parapa Palace from the overworld (the game's own mode-6 area
/// entry with `$0748` on the palace slot), then a few steps right.
fn enter_palace1(g: &mut Game) {
    g.ram[AREA] = PALACE1_SLOT;
    g.ram[MODE] = 6;
    let ok = step_until(g, 600, |g| g.ram[WORLD] == 3 && g.ram[MODE] == 0x0B);
    assert!(ok, "palace never loaded");
    step_n(g, 0, 4);
    step_n(g, RIGHT, 90);
    assert!(
        g.ram[PAGE] > 0 || g.ram[LINK_X] > 0x40,
        "Link walked into the palace"
    );
}

#[test]
fn continue_from_dungeon_entrance_restarts_at_the_palace() {
    let t = "continue_from_dungeon_entrance_restarts_at_the_palace";
    let (Some(rom), Some(s)) = (rom(t), overworld(t)) else {
        return;
    };
    let cases = [
        (ContinueFrom::Og, 0u8),
        (ContinueFrom::DungeonEntrance, 3),
        (ContinueFrom::Both, 3),
        (ContinueFrom::LastTown, 1),
    ];
    for (mode, world) in cases {
        let mut g = from_state(
            rom,
            s,
            QolOpts {
                continue_from: mode,
                ..QolOpts::default()
            },
        );
        enter_palace1(&mut g);
        die_to_gameover(&mut g);
        press_continue(&mut g);
        assert_eq!(g.ram[WORLD], world, "{mode:?}");
        if world == 3 {
            assert_eq!(g.ram[AREA], PALACE1_SLOT, "{mode:?}: same palace");
            assert_eq!(g.ram[REGION], 0);
            assert_eq!(g.ram[SCENE], 0, "{mode:?}: entrance scene");
            assert_eq!(g.ram[PAGE], 0, "{mode:?}: left end");
            assert_eq!(g.ram[MODE], 0x0B);
        }
    }
}

// ------------------------------------------------------------ XP

#[test]
fn gameover_keeps_a_share_of_xp() {
    let t = "gameover_keeps_a_share_of_xp";
    let (Some(rom), Some(s)) = (rom(t), overworld(t)) else {
        return;
    };
    for (pct, kept) in [(0u8, 0u16), (25, 100), (100, 400)] {
        let mut g = from_state(
            rom,
            s,
            QolOpts {
                gameover_keep_xp_pct: pct,
                ..QolOpts::default()
            },
        );
        set_xp(&mut g, 400);
        die_to_gameover(&mut g);
        assert_eq!(xp(&g), kept, "{pct}%: after the death routine");
        press_continue(&mut g);
        assert_eq!(xp(&g), kept, "{pct}%: after CONTINUE");
    }
}

// ------------------------------------------------------------ dolls

#[test]
fn lives_from_dolls_after_the_first_doll() {
    let t = "lives_from_dolls_after_the_first_doll";
    let Some(rom) = rom(t) else { return };
    let Some(track) = movie_track("hundred-percent.bk2", t) else {
        return;
    };
    let on = QolOpts {
        lives_from_dolls: true,
        ..QolOpts::default()
    };
    let mut results = Vec::new();
    for q in [QolOpts::default(), on] {
        let mut g = qol_game(rom, q);
        // The 100% route grabs a Life Doll near frame 3001 (lives 3 -> 4).
        for &p in track.iter().take(3100) {
            g.step(p);
        }
        assert_eq!(g.ram[LIVES], 4, "doll picked up");
        if q.lives_from_dolls {
            assert_eq!(qol::dolls_collected(&g), 1);
        }
        die_to_gameover(&mut g);
        press_continue(&mut g);
        results.push(g.ram[LIVES]);
    }
    assert_eq!(results, vec![3, 4], "original 3 lives; 3 + 1 doll");
}

// ------------------------------------------------------------ town side

fn rauru_start(rom: &[u8], s: &GameState, q: QolOpts, facing: Option<u8>) -> (u8, u8, u8) {
    let t = "enter_town_from_side_starts_at_the_left_end";
    let track = movie_track("anypct.bk2", t).expect("movie");
    let mut g = from_state(rom, s, q);
    if let Some(f) = facing {
        g.ram[0x0562] = f;
    }
    for &p in track.iter().take(945).skip(893) {
        g.step(p);
    }
    assert_eq!((g.ram[WORLD], g.ram[MODE]), (1, 0x0B), "in Rauru");
    (g.ram[PAGE], g.ram[LINK_X], g.ram[FACING])
}

#[test]
fn enter_town_from_side_starts_at_the_left_end() {
    let t = "enter_town_from_side_starts_at_the_left_end";
    let (Some(rom), Some(s)) = (rom(t), before_rauru(t)) else {
        return;
    };
    if movie_track("anypct.bk2", t).is_none() {
        return;
    }
    let on = QolOpts {
        enter_town_from_side: true,
        ..QolOpts::default()
    };
    // The movie walks down onto the tile: original right end, facing left.
    let og = rauru_start(rom, s, QolOpts::default(), None);
    assert_eq!(og.0, 3);
    assert!(og.1 >= 0xC0, "right end x {:#x}", og.1);
    assert_eq!(og.2, 2);
    assert_eq!(rauru_start(rom, s, on, None), og, "down: unchanged");
    // Approaching while moving right (overworld facing $01).
    assert_eq!(
        rauru_start(rom, s, QolOpts::default(), Some(1)),
        og,
        "original ignores the approach side"
    );
    let (page, x, facing) = rauru_start(rom, s, on, Some(1));
    assert_eq!(page, 0, "left end page");
    assert!(x < 0x40, "left end x {x:#x}");
    assert_eq!(facing, 1, "facing right, into town");
    assert_eq!(
        rauru_start(rom, s, on, Some(2)),
        og,
        "moving left: right end"
    );
}

// ------------------------------------------------------------ wise men

/// Run the wise-man dialog condition (`bank 3 $B518`, `Y` = town / spell
/// index: the container check and the grant) from the ROM bytes, as the
/// NPC talk handler does. The handler's text-box setup is not reproduced,
/// so the dialog request it leaves in `$0524` is dropped again and the
/// frame loop carries on as after the player closed the box.
fn wise_man(g: &mut Game, spell: u8) {
    let saved_bank = g.mmc1.prg;
    let routine = g.ram[0x0524];
    g.mmc1.prg = (g.mmc1.prg & 0xF0) | 3;
    g.cpu.y = spell;
    g.call_asm(0xB518);
    g.mmc1.prg = saved_bank;
    g.ram[0x0524] = routine;
}

#[test]
fn wise_men_without_container_requirement_and_with_mp_refill() {
    let t = "wise_men_without_container_requirement_and_with_mp_refill";
    let (Some(rom), Some(s)) = (rom(t), in_rauru(t)) else {
        return;
    };
    let both = QolOpts {
        no_mp_requirement_for_spells: true,
        wise_men_restore_mp: true,
        ..QolOpts::default()
    };
    // Jump (spell 1) needs 2 magic containers; Link has 1 and an empty
    // meter.
    let mut out = Vec::new();
    for q in [QolOpts::default(), both] {
        let mut g = from_state(rom, s, q);
        assert_eq!(g.ram[WORLD], 1);
        step_n(&mut g, 0, 2); // prime the spell snapshot
        g.ram[MAGIC_CTR] = 1;
        g.ram[MAGIC] = 0;
        assert_eq!(g.ram[SPELLS + 1], 0);
        wise_man(&mut g, 1);
        let learned = g.ram[SPELLS + 1];
        step_n(&mut g, 0, 200);
        out.push((learned, g.ram[MAGIC]));
    }
    assert_eq!(out[0], (0, 0), "original: denied, meter untouched");
    assert_eq!(out[1], (1, 0x1F), "learned with 1 container, MP refilled");

    // MP refill alone, with enough containers for the original check.
    let mut g = from_state(
        rom,
        s,
        QolOpts {
            wise_men_restore_mp: true,
            ..QolOpts::default()
        },
    );
    step_n(&mut g, 0, 2);
    g.ram[MAGIC_CTR] = 4;
    g.ram[MAGIC] = 3;
    wise_man(&mut g, 0); // Shield, Rauru's own spell
    assert_eq!(g.ram[SPELLS], 1);
    step_n(&mut g, 0, 300);
    assert_eq!(g.ram[MAGIC], 0x7F, "4 containers full");
}

// ------------------------------------------------------------ softlock warp

#[test]
fn overworld_softlock_warp_goes_to_the_continue_screen_without_xp_loss() {
    let t = "overworld_softlock_warp_goes_to_the_continue_screen_without_xp_loss";
    let (Some(rom), Some(s)) = (rom(t), overworld(t)) else {
        return;
    };
    let chord = qol::SOFTLOCK_CHORD;
    for on in [false, true] {
        let mut g = from_state(
            rom,
            s,
            QolOpts {
                overworld_softlock_warp: on,
                ..QolOpts::default()
            },
        );
        set_xp(&mut g, 400);
        let ok = step_until(&mut g, 300, |g| g.ram[MODE] == 5);
        assert!(ok, "on the overworld");
        step_n(&mut g, START, 2);
        step_n(&mut g, 0, 20);
        assert_eq!(g.ram[0x0524], 2, "paused");
        step_n(&mut g, chord, 200);
        if !on {
            assert_eq!(g.ram[STAGE], 1, "original: nothing happens");
            assert_eq!(g.ram[0x0524], 2, "still paused");
            continue;
        }
        assert_eq!(g.ram[STAGE], 2, "die routine running");
        wait_gameover_screen(&mut g);
        assert_eq!(xp(&g), 400, "no game-over XP penalty");
        press_continue(&mut g);
        assert_eq!(xp(&g), 400);
        assert_eq!((g.ram[WORLD], g.ram[AREA]), (0, 0), "North Palace");
        assert_eq!(g.ram[LIVES], 3);
    }
}
