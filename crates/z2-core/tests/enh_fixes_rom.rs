//! Engine-fix enhancements (`z2_core::enh::fixes`): ROM-gated runs that
//! reproduce each original bug and show the fixed behaviour.
//!
//! Scenarios start from save states taken while replaying the any% corpus
//! movie (a town house conversation at frame 1590, a sideview room at 5600
//! and the same room just before its right exit at 6440), then poke RAM to
//! set up the case. Self-skips unless `Z2_ROM` names a file and the movie
//! is in `Z2_CORPUS_MOVIES` (default the out-of-tree corpus). No ROM bytes
//! or derived data are stored in the tree.

#![cfg(feature = "interp")]

mod common;

use std::sync::OnceLock;

use z2_core::enh::{Enhancements, FixesOpts};
use z2_core::game::{Game, BTN_A, BTN_LEFT, BTN_RIGHT, BTN_START, BTN_UP};
use z2_core::state::GameState;

/// Start + Up: cancels a level-up window.
const BTN_START_UP: u8 = BTN_START | BTN_UP;

/// Frames the snapshots are taken at.
const TALK_FRAME: usize = 1590;
const ROOM_FRAME: usize = 5600;
const EXIT_FRAME: usize = 6440;

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

fn anypct_track(test: &str) -> Option<Vec<u8>> {
    let dir = common::var_present("Z2_CORPUS_MOVIES")
        .unwrap_or_else(|| "/Volumes/HolyDrive/dev/z2-corpus/movies".to_string());
    let path = common::file_present(&std::path::Path::new(&dir).join("anypct.bk2"), test)?;
    let zip = std::fs::read(&path).ok()?;
    let movie = z2_verify::movie_bk2::parse_bk2_zip(&zip).expect("parse anypct.bk2");
    Some(movie.pad1_track())
}

fn game_with(rom: &[u8], fixes: FixesOpts, default_traps: bool) -> Game {
    let mut g = Game::from_ines(rom).expect("ROM");
    if default_traps {
        register_default_groups(&mut g);
    }
    g.set_enhancements(Enhancements {
        fixes,
        ..Enhancements::default()
    });
    g.reset();
    g
}

struct Fixture {
    rom: Vec<u8>,
    track: Vec<u8>,
    talk: GameState,
    room: GameState,
    exit: GameState,
}

/// ROM, movie and the three snapshots (replayed once per test binary).
fn fixture(test: &str) -> Option<&'static Fixture> {
    static FIX: OnceLock<Option<Fixture>> = OnceLock::new();
    FIX.get_or_init(|| {
        let rom = common::rom_bytes(test)?;
        let track = anypct_track(test)?;
        let mut g = game_with(&rom, FixesOpts::default(), true);
        let mut snaps = Vec::new();
        for (f, &p) in track.iter().take(EXIT_FRAME).enumerate() {
            if f == TALK_FRAME || f == ROOM_FRAME {
                snaps.push(g.save_state());
            }
            g.step(p);
        }
        let exit = g.save_state();
        let room = snaps.pop()?;
        let talk = snaps.pop()?;
        Some(Fixture {
            rom,
            track,
            talk,
            room,
            exit,
        })
    })
    .as_ref()
}

fn from_state(fx: &Fixture, s: &GameState, fixes: FixesOpts) -> Game {
    let mut g = game_with(&fx.rom, fixes, true);
    g.load_state(s);
    g
}

fn only(f: impl FnOnce(&mut FixesOpts)) -> FixesOpts {
    let mut o = FixesOpts::default();
    f(&mut o);
    o
}

fn set_xp(g: &mut Game, xp: u16) {
    g.ram[0x0775] = (xp >> 8) as u8;
    g.ram[0x0776] = xp as u8;
}

fn xp(g: &Game) -> u16 {
    u16::from(g.ram[0x0775]) << 8 | u16::from(g.ram[0x0776])
}

fn xp_to_next(g: &mut Game) {
    let (hi, lo) = (g.ram[0x0770], g.ram[0x0771]);
    g.ram[0x0775] = hi;
    g.ram[0x0776] = lo;
}

// ------------------------------------------------------------ level-up

/// Replay from `snap` with movie input for 30 frames, then a Start+Up tap
/// every 16 frames (closes a level-up window by cancelling); XP is set to
/// the level cost at frame `base + at`. Returns, per frame, (mode, dialog).
fn levelup_run(fx: &Fixture, snap: &GameState, base: usize, at: usize, fix: bool) -> Vec<(u8, u8)> {
    let mut g = from_state(fx, snap, only(|o| o.levelup_softlocks = fix));
    let mut out = Vec::new();
    for f in base..base + 1500 {
        if f == base + at {
            xp_to_next(&mut g);
        }
        let p = if f < base + 30 {
            fx.track[f]
        } else if f % 16 == 0 {
            BTN_START_UP
        } else {
            0
        };
        g.step(p);
        out.push((g.ram[0x0736], g.ram[0x074C]));
    }
    out
}

/// Softlock 1: the window raised on the exit frame freezes the room exit
/// (mode `$10`) forever; fixed, the exit completes and the window opens in
/// the next room, then closes.
#[test]
fn rom_levelup_on_room_exit_frame() {
    let Some(fx) = fixture("rom_levelup_on_room_exit_frame") else {
        return;
    };
    // The movie leaves the room on frame 6456 (16 frames after the snapshot).
    let og = levelup_run(fx, &fx.exit, EXIT_FRAME, 16, false);
    assert_eq!(og[16], (0x10, 1), "window raised on the exit frame");
    assert!(
        og[16..].iter().all(|&s| s == (0x10, 1)),
        "original: exit mode stuck with the window up"
    );
    let fixed = levelup_run(fx, &fx.exit, EXIT_FRAME, 16, true);
    assert_eq!(fixed[16], (0x10, 0), "window deferred");
    let back = fixed.iter().position(|&(m, d)| m == 0x0B && d == 1);
    let back = back.expect("window opens in the next room");
    assert!(back > 16);
    assert_eq!(fixed.last(), Some(&(0x0B, 0)), "and closes");
}

/// Softlock 2: XP reaching the level cost late in a conversation turns the
/// open dialogue (`$074C = 2`) into a level-up window that never closes;
/// fixed, the conversation ends first and the window then opens and closes.
#[test]
fn rom_levelup_during_dialogue() {
    let Some(fx) = fixture("rom_levelup_during_dialogue") else {
        return;
    };
    // The movie opens a conversation at frame 1594; frame 1608 is late in it.
    let og = levelup_run(fx, &fx.talk, TALK_FRAME, 18, false);
    assert_eq!(og[17], (0x0B, 2), "talking");
    assert!(og[18..].iter().all(|&s| s == (0x0B, 1)), "original: stuck");
    let fixed = levelup_run(fx, &fx.talk, TALK_FRAME, 18, true);
    assert_eq!(fixed[18], (0x0B, 2), "dialogue kept");
    let opened = fixed
        .iter()
        .position(|&(_, d)| d == 1)
        .expect("window opens");
    assert!(fixed[18..opened].iter().all(|&(_, d)| d == 2));
    assert_eq!(fixed.last(), Some(&(0x0B, 0)), "and closes");
}

// ------------------------------------------------------------ sideview room

/// Link standing still on the ground in the 5600 room (no input until he
/// has landed and stopped).
fn settled(fx: &Fixture, fixes: FixesOpts) -> Game {
    let mut g = from_state(fx, &fx.room, fixes);
    let mut still = 0;
    for _ in 0..300 {
        g.step(0);
        if g.ram[0x0479] == 0 && g.ram[0x70] == 0 && g.ram[0xA7] & 0x04 != 0 {
            still += 1;
            if still >= 4 {
                return g;
            }
        } else {
            still = 0;
        }
    }
    panic!("Link never settled");
}

/// Run `run` frames from standing in direction `dir`, then press A (still
/// holding `dir`): whether the jump took the high row (`$057D` is still
/// `$FC` after its first frame; the low row has already dropped to `$FD`).
fn run_up_high_jump(fx: &Fixture, fixes: FixesOpts, dir: u8, run: usize) -> bool {
    let mut g = settled(fx, fixes);
    for _ in 0..run {
        g.step(dir);
    }
    for _ in 0..4 {
        g.step(BTN_A | dir);
        if g.ram[0x0479] != 0 {
            return g.ram[0x057D] == 0xFC;
        }
    }
    panic!("no jump after a {run}-frame run-up");
}

/// The original reaches the high jump with a shorter run-up to the right
/// on some frames (stale carry in the speed test); fixed, the run-up needed
/// is the same both ways.
#[test]
fn rom_jump_direction_balance() {
    let Some(fx) = fixture("rom_jump_direction_balance") else {
        return;
    };
    let fixed = only(|o| o.jump_direction_balance = true);
    let mut og_right_only = 0;
    for run in 12..=24 {
        let (r, l) = (
            run_up_high_jump(fx, FixesOpts::default(), BTN_RIGHT, run),
            run_up_high_jump(fx, FixesOpts::default(), BTN_LEFT, run),
        );
        let (fr, fl) = (
            run_up_high_jump(fx, fixed, BTN_RIGHT, run),
            run_up_high_jump(fx, fixed, BTN_LEFT, run),
        );
        eprintln!("run-up {run}: original right {r} left {l} | fixed right {fr} left {fl}");
        assert!(!l || r, "original never favours left");
        if r && !l {
            og_right_only += 1;
        }
        assert_eq!(fr, fl, "fixed: same result both ways after {run} frames");
    }
    assert!(og_right_only > 0, "original: some run-up favours right");
}

/// Shield box left edge (`$00`) for facing `$9F` with Link's screen X
/// `$CC = $60`, through the ROM routine (`call_asm`) or the trap table
/// (`fire_trap`).
fn shield_left_edge(g: &mut Game, facing: u8, via_trap: bool) -> u8 {
    g.ram[0x9F] = facing;
    g.ram[0xCC] = 0x60;
    g.ram[0x17] = 1;
    if via_trap {
        let _ = g.fire_trap(0xE9D8);
    } else {
        g.call_asm(0xE9D8);
    }
    assert_eq!(g.ram[0x02], 5, "box width");
    g.ram[0x00]
}

/// The original right-facing shield box is 2 px further from Link's centre
/// (`$CC + 16`) than the left-facing one; fixed, they mirror.
#[test]
fn rom_shield_hitbox_symmetry() {
    let Some(fx) = fixture("rom_shield_hitbox_symmetry") else {
        return;
    };
    let centre = 0x60 + 16;
    // (gap past the centre facing right, gap before it facing left)
    let gaps = |right: u8, left: u8| (i32::from(right) - centre, centre - (i32::from(left) + 5));
    let mut rom = from_state(fx, &fx.room, FixesOpts::default());
    let og = gaps(
        shield_left_edge(&mut rom, 1, false),
        shield_left_edge(&mut rom, 2, false),
    );
    let port = gaps(
        shield_left_edge(&mut rom, 1, true),
        shield_left_edge(&mut rom, 2, true),
    );
    assert_eq!(og, (6, 4), "original ROM routine");
    assert_eq!(port, og, "default port agrees");
    let mut g = from_state(fx, &fx.room, only(|o| o.shield_hitbox_symmetry = true));
    let fixed = gaps(
        shield_left_edge(&mut g, 1, true),
        shield_left_edge(&mut g, 2, true),
    );
    assert_eq!(fixed, (4, 4), "fixed: mirrored");
    // Without the default traps the hook wraps the interpreted ROM routine.
    let mut bare = game_with(&fx.rom, only(|o| o.shield_hitbox_symmetry = true), false);
    bare.load_state(&fx.room);
    let fixed_rom = gaps(
        shield_left_edge(&mut bare, 1, true),
        shield_left_edge(&mut bare, 2, true),
    );
    assert_eq!(fixed_rom, (4, 4));
}

/// Level-RAM (WRAM) index of the cell a Link probe at world X offset `dx`
/// (row offset 0) reads, from the ROM's page tables.
fn level_cell(g: &Game, dx: u8) -> usize {
    let bank7 = |a: u16| g.prg[7 * 0x4000 + usize::from(a & 0x3FFF)];
    let world = (u16::from(g.ram[0x3B]) << 8 | u16::from(g.ram[0x4D])) + u16::from(dx);
    let page = world >> 8;
    let base = u16::from(bank7(0xEAE0 + page)) | u16::from(bank7(0xEAE4 + page)) << 8;
    let ptr = base + ((world & 0xFF) >> 4);
    usize::from(ptr + u16::from(g.ram[0x29] & 0xF0)) - 0x6000
}

/// A step-on breakable tile under Link's left foot (x + 12) but not under
/// his foot centre (x + 15): the original leaves it; fixed, it shatters.
#[test]
fn rom_crumble_both_feet() {
    let Some(fx) = fixture("rom_crumble_both_feet") else {
        return;
    };
    let run = |fix: bool| {
        let mut g = settled(fx, only(|o| o.crumble_both_feet = fix));
        // Put the foot centre 1 px into its column so the left foot is in
        // the column to its left.
        g.ram[0x4D] = (g.ram[0x4D] & 0xF0) | 0x01;
        let (foot, centre) = (level_cell(&g, 0x0C), level_cell(&g, 0x0F));
        assert_eq!(foot + 1, centre, "left foot one column left");
        let bank = usize::from(g.ram[0x0769]);
        let breakable = g.prg[bank * 0x4000 + 0x051F];
        assert_ne!(g.wram[centre], breakable);
        g.wram[foot] = breakable;
        let slots_before = g.ram[0x041A..0x041F].to_vec();
        g.step(0);
        let slots_after = g.ram[0x041A..0x041F].to_vec();
        (g.wram[foot], breakable, slots_before != slots_after)
    };
    let (tile, breakable, debris) = run(false);
    assert_eq!(tile, breakable, "original: only the foot centre counts");
    assert!(!debris);
    let (tile, _, debris) = run(true);
    assert_eq!(tile, 0x8F, "fixed: shattered");
    assert!(debris, "debris slot claimed");
}

thread_local! {
    static BLOCK_TESTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn count_block_test(g: &mut Game) {
    BLOCK_TESTS.with(|c| c.set(c.get() + 1));
    z2_core::enh::call_original(g, 0xE1DD);
}

/// Calls of the sword-vs-breakable-block test over 20 frames with the
/// immunity timer held at 2.
fn block_tests_with_timer_2(fx: &Fixture, fix: bool) -> u32 {
    let mut g = settled(fx, only(|o| o.iframe_update_skip = fix));
    z2_core::enh::hook(
        &mut g,
        "test_count_block",
        Some(7),
        0xE1DD,
        count_block_test,
        None,
    );
    BLOCK_TESTS.with(|c| c.set(0));
    for _ in 0..20 {
        g.ram[0x0518] = 2;
        g.step(0);
    }
    BLOCK_TESTS.with(std::cell::Cell::get)
}

/// With the immunity timer at 1-2 the original runs the block test only on
/// every other frame; fixed, every frame.
#[test]
fn rom_iframe_update_skip() {
    let Some(fx) = fixture("rom_iframe_update_skip") else {
        return;
    };
    assert_eq!(block_tests_with_timer_2(fx, false), 10);
    assert_eq!(block_tests_with_timer_2(fx, true), 20);
}

/// XP after one frame, and after 20 more, with exactly 10 XP pending
/// (from 0).
fn xp_after_ten_pending(fx: &Fixture, fix: bool) -> (u16, u16) {
    let mut g = settled(fx, only(|o| o.xp_drain_fix = fix));
    set_xp(&mut g, 0);
    g.ram[0x0755] = 0;
    g.ram[0x0756] = 10;
    g.step(0);
    let first = xp(&g);
    for _ in 0..20 {
        g.step(0);
    }
    (first, xp(&g))
}

/// A drain of 10 hits Link at 3 XP; 8 XP are gained once XP is 0. Returns
/// (drain left when XP hit 0, final XP).
fn drain_run(fx: &Fixture, fix: bool) -> (u8, u16) {
    let mut g = settled(fx, only(|o| o.xp_drain_fix = fix));
    set_xp(&mut g, 3);
    g.ram[0x05E8] = 10;
    let mut left = None;
    for _ in 0..60 {
        // Immune, so no hit rewrites the drain.
        g.ram[0x0518] = 0x30;
        if left.is_none() && xp(&g) == 0 {
            left = Some(g.ram[0x05E8]);
            g.ram[0x0756] = 8;
        }
        g.step(0);
    }
    (left.expect("XP drained to 0"), xp(&g))
}

/// Exactly 10 pending XP: the original adds all 10 at once; fixed, one at
/// a time. A drain that empties XP: the original keeps the rest and takes
/// it out of the next gain; fixed, the rest is dropped.
#[test]
fn rom_xp_drain_fix() {
    let Some(fx) = fixture("rom_xp_drain_fix") else {
        return;
    };
    assert_eq!(xp_after_ten_pending(fx, false), (10, 10));
    assert_eq!(xp_after_ten_pending(fx, true), (1, 10));
    let (left, og) = drain_run(fx, false);
    eprintln!("original: drain left at 0 XP {left}, final XP {og}");
    assert!(left > 0, "original: drain left over at 0 XP");
    assert!(og < 8, "original: the leftover drain ate gained XP");
    let (_, fixed) = drain_run(fx, true);
    assert_eq!(fixed, 8, "fixed: the gain is kept");
}

/// Every fix on over a stretch of the movie, then everything off again:
/// patches and hooks are undone.
#[test]
fn rom_all_fixes_run_and_undo() {
    let Some(fx) = fixture("rom_all_fixes_run_and_undo") else {
        return;
    };
    let all = FixesOpts {
        levelup_softlocks: true,
        iframe_update_skip: true,
        jump_direction_balance: true,
        shield_hitbox_symmetry: true,
        crumble_both_feet: true,
        xp_drain_fix: true,
    };
    let mut g = from_state(fx, &fx.room, all);
    let off = game_with(&fx.rom, FixesOpts::default(), true);
    assert_ne!(g.prg, off.prg, "patched");
    assert_eq!(g.traps.len(), off.traps.len() + 2, "two new trap slots");
    for &p in &fx.track[ROOM_FRAME..ROOM_FRAME + 600] {
        g.step(p);
    }
    g.set_enhancements(Enhancements::default());
    assert_eq!(g.prg, off.prg, "patches undone");
    assert_eq!(g.traps.len(), off.traps.len(), "hooks undone");
}
