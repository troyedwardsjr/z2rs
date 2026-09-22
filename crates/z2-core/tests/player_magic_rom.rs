//! ROM-gated end-to-end magic tests: casting and meter refills.
//!
//! The spell path is the one big gameplay system the movie corpus never
//! touches (no TAS in `z2-corpus` learns a spell, so `$077B-$0782` stay
//! zero through all of `anypct` / `hundred-percent` / `warpless`), which
//! left "Select casts the selected spell" and "a pending refill drains
//! into the meter" unpinned. These tests drive the real interpreter with
//! the full hybrid trap set — exactly what the app registers — from a
//! fresh cartridge to side-view gameplay, then exercise:
//!
//! * `Spell_Casting_Routine` (bank 0 `$8DC3`) through `$F5 & $20`: the
//!   meter pays [`spell_cost`], the effect bit lands in `$076F` and the
//!   `$074A` latch blocks an immediate re-cast until the pause pane
//!   clears it (`$A14A`);
//! * the pause/spell menu: Start opens it, Down walks `$0749`, Start
//!   closes it, and the spell picked there is the one that casts;
//! * spell *acquisition* from a wise man, reached by scene warp;
//! * the Life spell (`$8E5D`) and the `$D3F9` refill tick that drains
//!   `$070C`/`$070D` into `$0773`/`$0774`, two units per servicing frame
//!   on alternating frame parities.
//!
//! Skips (never fails) without `$Z2_ROM`, like every other gated test.

#![cfg(feature = "interp")]

mod common;

use z2_core::game::Game;
use z2_core::player_magic::{spell_cost, SPELL_BITS};

// Input bytes on the shared contract (bit0 = A … bit7 = Right).
const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;
const DOWN: u8 = 0x20;
const RIGHT: u8 = 0x80;
const B: u8 = 0x02;

/// Register the same trap groups the native app and `xtask verify` do.
fn register_all(g: &mut Game) {
    z2_core::bank7_traps::register_bank7_traps(g);
    z2_core::sideview_traps::register_sideview_traps(g);
    z2_core::sideview_traps::register_overworld_traps(g);
    z2_core::player_traps::register_player_traps(g);
    z2_core::enemy_traps::register_enemy_traps(g);
    z2_core::town_traps::register_town_traps(g);
    z2_core::palace_traps::register_palace_traps(g);
    z2_core::title_traps::register_title_traps(g);
    z2_core::boot_traps::register_boot_traps(g);
}

fn step_n(g: &mut Game, n: usize, input: u8) {
    for _ in 0..n {
        g.step(input);
    }
}

/// Fresh cartridge → named slot 0 → side-view gameplay in North Castle.
/// The frame counts mirror `menu_flow_tests::start_on_named_slot_…`.
fn to_gameplay(test: &str) -> Option<Game> {
    let raw = common::rom_bytes(test)?;
    let mut g = Game::from_ines(&raw).ok()?;
    register_all(&mut g);
    g.reset();
    step_n(&mut g, 30, 0x00); // title intro
    step_n(&mut g, 5, START); // → file select
    step_n(&mut g, 20, 0x00);
    step_n(&mut g, 5, START); // → register
    step_n(&mut g, 20, 0x00);
    for _ in 0..8 {
        step_n(&mut g, 2, A); // name: eight times 'A'
        step_n(&mut g, 8, 0x00);
    }
    for _ in 0..3 {
        step_n(&mut g, 2, SELECT); // cursor → END
        step_n(&mut g, 8, 0x00);
    }
    step_n(&mut g, 5, START); // commit → file select
    step_n(&mut g, 30, 0x00);
    step_n(&mut g, 5, START); // load slot 0
    step_n(&mut g, 60, 0x00);
    step_n(&mut g, 1100, 0x00); // lives screen → gameplay
    assert_eq!(g.ram[0x0736], 0x0B, "side-view gameplay mode");
    Some(g)
}

/// Learn every spell at magic level `level` with a full meter.
fn arm_magic(g: &mut Game, level: u8) {
    for a in 0x077B..0x0783 {
        g.ram[a] = 1;
    }
    g.ram[0x0778] = level; // magic level
    g.ram[0x0783] = 8; // magic containers
    g.ram[0x0773] = 0xFF; // full meter
}

/// Open the pause pane, walk the selector to `spell`, close it again.
/// Leaves `$074A` cleared, so the next Select press casts.
fn select_spell(g: &mut Game, spell: u8) {
    g.step(START);
    step_n(g, 25, 0x00);
    for _ in 0..16 {
        if g.ram[0x0749] == spell {
            break;
        }
        g.step(DOWN);
        g.step(0x00);
    }
    assert_eq!(g.ram[0x0749], spell, "selector must reach spell {spell}");
    g.step(START);
    step_n(g, 25, 0x00);
    assert_eq!(g.ram[0x0524], 0x00, "pause pane closed");
    assert_eq!(g.ram[0x074A], 0x00, "$A14A cleared the re-cast latch");
}

#[test]
fn select_casts_every_learned_spell_and_pays_the_table_cost() {
    let Some(mut g) = to_gameplay("select_casts_every_learned_spell") else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    let level = 8u8;
    // Some spells move Link out of plain side-view control (Fairy turns him
    // into one), so each spell starts from the same snapshot.
    let base = g.save_state();
    for spell in 0..8u8 {
        g.load_state(&base);
        arm_magic(&mut g, level);
        select_spell(&mut g, spell);
        g.ram[0x076F] = 0x00; // clear the magic state between casts
        g.step(SELECT);
        let cost = spell_cost(usize::from(spell), level);
        assert_eq!(
            g.ram[0x0773],
            0xFF - cost,
            "spell {spell} must pay ${cost:02X} out of the meter"
        );
        assert_eq!(
            g.ram[0x074A],
            spell + 1,
            "spell {spell} must latch $074A = selector + 1"
        );
        // Life / Spell / Thunder clear their own bit on the same frame's
        // tick, so only assert the bit for the ones that persist.
        if !matches!(spell, 2 | 6 | 7) {
            assert_eq!(
                g.ram[0x076F] & SPELL_BITS[usize::from(spell)],
                SPELL_BITS[usize::from(spell)],
                "spell {spell} must set its $076F effect bit"
            );
        }
    }
}

#[test]
fn recast_needs_the_pause_pane_and_an_unlearned_spell_never_casts() {
    let Some(mut g) = to_gameplay("recast_needs_the_pause_pane") else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    arm_magic(&mut g, 8);
    select_spell(&mut g, 0); // Shield
    g.step(SELECT);
    let after_first = g.ram[0x0773];
    assert_eq!(after_first, 0xFF - spell_cost(0, 8), "first cast pays");
    // Holding/repeating Select cannot re-cast: `$8DD1` compares the
    // selector + 1 against the `$074A` latch.
    for _ in 0..8 {
        g.step(SELECT);
        g.step(0x00);
    }
    assert_eq!(g.ram[0x0773], after_first, "no re-cast without the pane");
    // Re-opening the pane clears `$074A` and the same spell casts again.
    select_spell(&mut g, 0);
    g.step(SELECT);
    assert_eq!(
        g.ram[0x0773],
        after_first - spell_cost(0, 8),
        "second cast after re-opening the pane"
    );
    // An unlearned spell is refused at `$8DDF` (meter untouched). The pane
    // itself never parks on one, so point the selector there by hand after
    // closing it on a spell Link does know.
    select_spell(&mut g, 1); // Jump (learned) leaves $074A clear
    let before = g.ram[0x0773];
    g.ram[0x077B] = 0x00; // forget Shield
    g.ram[0x0749] = 0x00; // …and select it anyway
    g.step(SELECT);
    assert_eq!(g.ram[0x0773], before, "unlearned spell must not cast");
    assert_eq!(g.ram[0x074A], 0x00, "and must not latch $074A");
}

#[test]
fn life_spell_and_pending_refills_reach_the_meters() {
    let Some(mut g) = to_gameplay("life_spell_and_pending_refills") else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    arm_magic(&mut g, 8);
    g.ram[0x0784] = 8; // heart containers
    g.ram[0x0774] = 0x10; // hurt
    select_spell(&mut g, 2); // Life
    g.step(SELECT);
    // `$30` queued at `$8E69`, minus the unit the same frame's refill tick
    // may already have drained.
    assert!(
        (0x2F..=0x30).contains(&g.ram[0x070D]),
        "Life queues $30 into the pending-life byte ($8E65), saw ${:02X}",
        g.ram[0x070D]
    );
    assert_eq!(
        g.ram[0x076F] & SPELL_BITS[2],
        0,
        "Life clears its own bit on the tick that fires it"
    );
    // `$D3F9` services one meter per frame (`$12 & 1`), +2 units a time.
    let hp_before = g.ram[0x0774];
    let pending_before = g.ram[0x070D];
    step_n(&mut g, 20, 0x00);
    assert_eq!(
        g.ram[0x0774],
        hp_before + 20,
        "ten servicing frames add 2 life units each"
    );
    assert_eq!(
        g.ram[0x070D],
        pending_before - 10,
        "and drain one pending unit each"
    );
    // The magic side of the same tick drains on the other parity.
    g.ram[0x0773] = 0x10;
    g.ram[0x070C] = 0x20;
    step_n(&mut g, 20, 0x00);
    assert_eq!(g.ram[0x0773], 0x10 + 20, "pending magic reaches $0773");
    assert_eq!(g.ram[0x070C], 0x20 - 10, "pending magic drains");
    // A refill stops at the container cap ((containers << 5) - 1).
    g.ram[0x0774] = 0xF0;
    g.ram[0x070D] = 0x40;
    step_n(&mut g, 40, 0x00);
    assert_eq!(g.ram[0x0774], 0xFF, "life clamps at the 8-container cap");
    assert_eq!(g.ram[0x070D], 0x00, "and the pending byte is dropped");
}

/// Spell *acquisition* — the step before any of the above, and the one the
/// movie corpus never reaches (`$077B-$0782` stay zero through every TAS).
///
/// Scene-warps into a wise man's basement through the game's own loader
/// (README.md: region `$0706`, world `$0707`, key area
/// `$0748`, town `$056B`, scene `$0561`, page `$075C`, facing `$0701`,
/// then mode `$0736 = 0`), walks Link into the old man and lets the
/// dialog run. The whole talk is bank-3 ROM code in the interpreter — the
/// shipped trap table has no entry below `$C000` — so this pins that the
/// grant (`$B531`: `$077B,y = 1`) and the first-spell selector jump
/// (`$B548`: `STY $0749`) both land.
#[test]
fn wise_man_grants_the_spell_and_sets_the_selector() {
    let Some(mut g) = to_gameplay("wise_man_grants_the_spell") else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // The grant needs `$0783 >= spell + 1` ($B526).
    g.ram[0x0783] = 8;
    let base = g.save_state();
    // (town code, basement scene, spell index) — Rauru teaches Shield,
    // Ruto teaches Jump.
    for (town, scene, spell) in [(0u8, 0x24u8, 0usize), (1, 0x25, 1)] {
        g.load_state(&base);
        g.ram[0x0706] = 0; // West Hyrule
        g.ram[0x0707] = 1; // town world
        g.ram[0x0748] = 0x2C; // town key area
        g.ram[0x056B] = town;
        g.ram[0x0561] = scene; // wise-man basements are $24+
        g.ram[0x075C] = 0;
        g.ram[0x0701] = 0;
        g.ram[0x0736] = 0;
        step_n(&mut g, 160, 0x00); // settle the warp
        assert_eq!(g.ram[0x0736], 0x0B, "warp lands in side view");
        assert_eq!(g.ram[0x077B + spell], 0x00, "spell starts unlearned");
        // Walk into the wise man; the talk starts on contact and runs
        // itself (B only advances the text).
        for _ in 0..3 {
            step_n(&mut g, 120, RIGHT);
            for _ in 0..60 {
                g.step(B);
                g.step(0x00);
            }
        }
        assert_eq!(
            g.ram[0x077B + spell],
            0x01,
            "wise man in town {town} must grant spell {spell}"
        );
        assert_eq!(
            g.ram[0x0749], spell as u8,
            "the first spell learned also moves the selector ($B548)"
        );
    }
}
