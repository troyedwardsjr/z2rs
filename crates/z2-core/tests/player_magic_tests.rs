//! ROM-free player magic/items/leveling/death tests + gated snapshots.

#[allow(dead_code)]
#[path = "../src/player_magic.rs"]
mod player_magic;

mod common;

use player_magic::*;

// ---------------------------------------------------------------------------
// Pure-logic checks (ROM-free).
// ---------------------------------------------------------------------------

#[test]
fn spell_costs_match_8d7b() {
    // Row spot-checks against the listing bytes.
    assert_eq!(
        SPELL_COSTS[0],
        [0x40, 0x30, 0x30, 0x20, 0x20, 0x20, 0x20, 0x20]
    );
    assert_eq!(
        SPELL_COSTS[7],
        [0xF0, 0xF0, 0xF0, 0xF0, 0xF0, 0xF0, 0xC8, 0x80]
    );
    assert_eq!(spell_cost(SPELL_THUNDER, 8), 0x80);
    assert_eq!(spell_cost(SPELL_JUMP, 8), 0x10);
    assert_eq!(spell_cost(SPELL_LIFE, 1), 0x8C);
}

#[test]
fn cast_flow_with_quirk() {
    let gate = |meter: u8, cost: u8| {
        cast_gate(CastGate {
            dialog: 0,
            menu: 0,
            lock: 0,
            selector: 2,
            last_cast: 9,
            select_pressed: true,
            learned: true,
            meter,
            cost,
        })
    };
    assert_eq!(gate(0x80, 0x40), CastOutcome::Cast { meter: 0x40 });
    assert_eq!(gate(0x10, 0x40), CastOutcome::None);
    // $8E3B free-cast quirk: exactly $FF underflow still casts.
    assert_eq!(gate(0x3F, 0x40), CastOutcome::CastFree);
    let fx = cast_spell(5, 0x01, 0x00);
    assert_eq!(fx.magic_state, 0x01 | 0x20);
    assert_eq!(fx.last_cast, 6);
    assert_eq!(fx.flash, 0xA0);
    let tick = spell_tick(0x02 | 0x04 | 0x80);
    assert_eq!(tick.jump_fx, 1);
    assert!(tick.life_fired && tick.thunder_fired && !tick.fairy_armed);
}

#[test]
fn per_spell_effects() {
    assert_eq!(jump_spell_fx(), 1);
    assert_eq!(life_spell_fx(0xFF, 0xF0), (0xFB, 0xFF));
    assert_eq!(life_spell_fx(0x04, 0x10), (0x00, 0x40));
    assert_eq!(SHIELD_TINT, 0x16);
    assert_eq!(REFLECT_ON, 0x01);
    assert!(spell_spell_fx(1, 0x14));
    assert!(!spell_spell_fx(0, 0x14));
    assert!(!spell_spell_fx(3, 0x14));
    assert!(!spell_spell_fx(1, 0x10));
    assert_eq!(spell_door_step(0x10), (false, true));
    assert_eq!(spell_door_step(0x0F), (true, true));
    let (ms, n) = spell_spell_clear(0xFF, &[1, 0, 1, 0, 0, 0], &[false; 6]);
    assert_eq!((ms, n), (0xBF, 2));
    let (tm, td, hits) = thunder_spell_fx(0xFF, &[1, 1, 0, 0, 0, 0], &[true, false]);
    assert_eq!((tm, td, hits), (0x7F, 0x00, 1));
    assert_eq!(THUNDER_POWER, 0x32);
}

#[test]
fn meter_exp_ticks_match_d3e9() {
    assert_eq!(meter_cap(0), 0xFF);
    assert_eq!(meter_cap(8), 0xFF);
    assert_eq!(meter_cap(4), 0x7F);
    // Alternating refill adds 2/frame until cap.
    let (m, p, moved) = magic_regen_tick(0x00, 0x03, meter_cap(4), 0);
    assert_eq!((m, p, moved), (0x02, 0x02, true));
    let (m2, p2, _) = magic_regen_tick(0x7E, 0x03, meter_cap(4), 0);
    assert_eq!((m2, p2), (0x7F, 0x00));
    // Exp drip + loss.
    assert_eq!(exp_trickle(0, 0, 0, 20), (0, 10, 0, 10));
    assert_eq!(exp_trickle(0, 0xFF, 0, 10), (1, 9, 0, 0));
    assert_eq!(exp_loss_tick(1, 0, 2), (0, 0xFF, 1));
    assert_eq!(exp_loss_tick(0, 0, 2), (0, 0, 2));
}

#[test]
fn leveling_matches_chart() {
    assert_eq!(next_level_for(STAT_ATTACK, 1), 0x0014);
    assert_eq!(next_level_for(STAT_ATTACK, 5), 0x012C);
    assert_eq!(next_level_for(STAT_MAGIC, 1), 0x0064);
    assert_eq!(next_level_for(STAT_LIFE, 8), 0xFFFF);
    assert!(level_ready(0x0100, 0x0100));
    assert!(!level_ready(0x00FF, 0x0100));
    let (nl, next) = level_up_choice(STAT_LIFE, 3);
    assert_eq!(nl, 4);
    assert_eq!(next, next_level_for(STAT_LIFE, 4));
    assert_eq!(level_up_choice(STAT_ATTACK, 8).0, 8);
}

#[test]
fn pause_selector_menu() {
    assert_eq!(pause_step(MENU_CLOSED, true, 0), MENU_PAUSED);
    assert_eq!(pause_step(MENU_PAUSED, true, 0), MENU_UNPAUSE);
    assert_eq!(pause_step(MENU_UNPAUSE, false, 0), MENU_CLOSED);
    assert_eq!(pause_step(MENU_CLOSED, true, 1), MENU_CLOSED);
    assert_eq!(selector_step(0, true, false, 8), 7);
    assert_eq!(selector_step(7, false, true, 8), 0);
    assert_eq!(selector_step(3, false, false, 8), 3);
}

#[test]
fn pickups_containers_items() {
    assert_eq!(item_pickup(0x03, 4, false), Pickup::Inventory { slot: 3 });
    assert_eq!(
        item_pickup(0x08, 4, false),
        Pickup::Key { unlock_boss: false }
    );
    assert!(matches!(
        item_pickup(0x0E, 4, false),
        Pickup::Container { magic: true, .. }
    ));
    assert!(matches!(
        item_pickup(0x0F, 4, false),
        Pickup::Container { magic: false, .. }
    ));
    assert_eq!(item_pickup(0x12, 4, false), Pickup::Doll);
    assert_eq!(
        item_pickup(0x13, 4, false),
        Pickup::Flag {
            byte: 0x9C,
            bit: 0x20
        }
    );
    // Red jar scales with containers (ctr << 4).
    assert_eq!(
        item_pickup(0x11, 4, false),
        Pickup::Jar {
            magic_add: 0,
            life_add: 0x40
        }
    );
    let (nc, kasuto, pend) = container_pickup(6, 2, true);
    assert_eq!((nc, kasuto, pend), (7, true, 0x20));
    let p = item_passive([1, 0, 1, 1, 1, 1, 1, 1]);
    assert!(p.candle_lit && p.raft_float && p.boots_stride && p.magic_key);
    assert!(!p.glove_break);
}

#[test]
fn death_lives_continue() {
    assert_eq!(death_check(0x01, 0x00), Some((STATE_DIE, 0x01)));
    assert_eq!(death_check(0x01, 0x05), None);
    assert!(lives_screen(3));
    assert!(!lives_screen(0));
    assert_eq!(continue_flow(7), (3, 8, STATE_INGAME));
    let r = lives_reset();
    assert_eq!((r.y, r.screen_x, r.anim, r.fairy), (0, 0, 0, 0));
}

// ---------------------------------------------------------------------------
// Gated snapshots (same harness contract as the movement suite).
// ---------------------------------------------------------------------------

// Placeholder: the gate-satisfied path below is an unconditional panic
// because the oracle diff for player-magic snapshots is not wired yet.
// Parked as ignored so that a developer *with* a ROM and a real corpus is
// not shown a false red; it stays visible in the ignored list until that diff
// lands. The body is unchanged: a gated test must skip when its
// prerequisites are absent, and must not fail merely because they are
// present.
#[test]
#[ignore = "the oracle diff for player-magic snapshots is not wired yet"]
fn player_magic_snapshots_gated() {
    let rom = common::rom_path("player_magic_snapshots_gated").is_some();
    let has_corpus = common::corpus_snapshots("player_magic_snapshots_gated").is_some();
    if !(rom && has_corpus) {
        eprintln!(
            "skipping player_magic_snapshots_gated: need Z2_ROM (present={rom}) + corpus snapshots (present={has_corpus})"
        );
        return;
    }
    panic!("corpus present but oracle diff not wired");
}
