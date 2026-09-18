//! ROM-free enemy core tests: lifecycle, projectiles, drops/exp,
//! sword/shield/spell gates + determinism fuzz.

#[allow(dead_code)]
#[path = "../src/enemy.rs"]
mod enemy;

use enemy::*;

// ---------------------------------------------------------------------------
// Lifecycle: spawn → update → stun → die → award exp.
// ---------------------------------------------------------------------------

#[test]
fn slot_lifecycle_phases() {
    assert_eq!(phase_of(0, 0), Phase::Free);
    assert_eq!(phase_of(1, 0), Phase::Live);
    assert_eq!(phase_of(1, 0x30), Phase::Stunned);
    assert_eq!(phase_of(2, 0), Phase::Dying);
    // Stun gate + decay ($DA02 / $EF11).
    assert!(stun_stops(0x30));
    assert!(!stun_stops(0));
    assert_eq!(stun_tick(0x30), 0x2F);
    assert_eq!(stun_tick(0), 0);
    // Remove + kill-all ($DD47 / $E18F with the INC-after-remove quirk).
    assert_eq!(remove_enemy(), 0);
    let out = kill_all([1, 0, 2, 0, 0, 1]);
    assert_eq!(out, [1, 0, 1, 0, 0, 1]);
    assert_eq!(kill_all([0, 0, 0, 0, 0, 0]), [0, 0, 0, 0, 0, 0]);
    // Frozen gate ($D6C1): $A8 & $10.
    assert!(link_collision_gate(0x10));
    assert!(!link_collision_gate(0x00));
    // Dispatch key ($D6CA): id << 1.
    assert_eq!(every_frame_dispatch(0x04), 0x08);
}

#[test]
fn facing_and_movement_match_dc91_deb8() {
    // Link right of enemy (BPL) → 1; left (BMI) → 2.
    assert_eq!(facing_toward_link(0x90, 0x01, 0x10, 0x01).0, 1);
    assert_eq!(facing_toward_link(0x10, 0x01, 0x90, 0x01).0, 2);
    // Wall flip ($E8EB): facing ^ 3, negate speed.
    assert_eq!(flip_on_wall(1, 0x08), (2, 0xF8));
    assert_eq!(flip_on_wall(2, 0xF8), (1, 0x08));
    // Horizontal integrate advances (subpixel first, then whole px);
    // gravity adds +2 to vspeed ($DEBE).
    let (x, _, sub) = simple_horizontal(0x10, 0x01, 0x00, 0x08);
    assert_ne!((x, sub), (0x10, 0x00));
    let (x2, _, _) = simple_horizontal(0x10, 0x01, 0x00, 0x18);
    assert_eq!(x2, 0x11);
    let (y, vs, _) = gravity_step(0x80, 0x10, 0x00);
    assert_eq!(vs, 0x12);
    let _ = y;
    // Regen-bit clear ($DD34).
    assert_eq!(regen_clear(false, false, 0x84), (0x84, true));
    assert_eq!(regen_clear(true, false, 0x84), (0x04, true));
}

// ---------------------------------------------------------------------------
// Exp + drops ($DDEC / $E880 / $E891 / $E870 / $5DF-$5E0 / $51B).
// ---------------------------------------------------------------------------

#[test]
fn exp_award_adds_with_carry() {
    // No carry.
    assert_eq!(exp_award(0, 0x05, 0x00, 0x00, 0x00), (0x00, 0x05));
    // Low-byte carry ripples to high.
    assert_eq!(exp_award(0, 0xFF, 0x01, 0x00, 0x01), (0x02, 0x00));
    // Boss marker ($6E1D == $FF) → key path.
    assert!(is_boss_rank(0xFF));
    assert!(!is_boss_rank(0x07));
    assert_eq!(boss_key_drop(), (0x80, 0x40, 0x08));
}

#[test]
fn drop_groups_and_sixth_kill_rule() {
    assert_eq!(drop_group(0x00), DropGroup::None);
    assert_eq!(drop_group(0x40), DropGroup::Group1);
    assert_eq!(drop_group(0x80), DropGroup::Group2);
    assert_eq!(drop_group(0xC0), DropGroup::Group3);
    // Every 6th kill per size group drops ($E899-$E8A5).
    assert_eq!(drop_counter_tick(5), (0, true));
    assert_eq!(drop_counter_tick(0), (1, false));
    // Table select: weak uses rng&7, strong adds 8 ($E8B8-$E8BC).
    assert_eq!(drop_table_index(0x03, false), 3);
    assert_eq!(drop_table_index(0x03, true), 11);
    // Full death step: no-group → no drop but still dying timer.
    let d = monster_death(0x03, 0x00, 0x10, 2, 0x05);
    assert_eq!(d.drop_index, None);
    assert!(!d.boss_key);
    assert_eq!(d.timer, DEATH_TIMER);
    // Grouped 6th kill fires.
    let d2 = monster_death(0x03, 0x80, 0x10, 5, 0x05);
    assert!(d2.drop_index.is_some());
    // Boss rank → key, no drop roll.
    let b = monster_death(0x03, 0x80, 0xFF, 5, 0x05);
    assert!(b.boss_key);
    assert_eq!(b.drop_index, None);
    // RNG mask helper.
    assert_eq!(rng_sample(0x3F, 0x07), 0x07);
}

// ---------------------------------------------------------------------------
// Projectiles: spawn/move/collide/disintegrate ($DBCE/$DBFB/$E6E8/$F2-$FF).
// ---------------------------------------------------------------------------

#[test]
fn projectile_spawn_scans_for_free_slot() {
    // Full row → None (carry set).
    assert_eq!(spawn_projectile_slot(&[1, 1, 1, 1, 1, 1], 5), None);
    // Top-down scan finds highest free.
    assert_eq!(spawn_projectile_slot(&[0, 0, 0, 0, 0, 1], 5), Some(4));
    // Bubble scan is Y = 3..0 only.
    assert_eq!(spawn_projectile_slot(&[1, 1, 1, 0, 0, 0], 3), Some(3));
    assert_eq!(spawn_projectile_slot(&[1, 1, 1, 1, 0, 0], 3), None);
    assert_eq!(spawn_projectile_slot(&[1, 1, 1, 1, 1, 1], 3), None);
    // Spawn copy stamps flame $04 + enemy pos/facing.
    let p = spawn_projectile_copy(0x40, 0x02, 0x80, 0x01, 0x10);
    assert_eq!((p.kind, p.flag), (0x04, PROJ_ACTIVE));
    assert_eq!((p.x, p.page, p.y, p.facing), (0x40, 0x02, 0x80, 0x01));
    // Bubble init ($DC07): facing 1, yspeed 0.
    assert_eq!(bubble_init(), (0x01, 0x00));
}

#[test]
fn projectile_disintegration_counts_up_and_wraps() {
    // Shield deflect ($E6E8): $7D = 0, $8D = $F2 (BUG-preserved).
    assert_eq!(projectile_disintegrate(), (0x00, 0xF2));
    // $F2-$FF count up; $FF wraps to inactive $00.
    assert_eq!(projectile_tick_flag(0xF2), 0xF3);
    assert_eq!(projectile_tick_flag(0xFE), 0xFF);
    assert_eq!(projectile_tick_flag(0xFF), 0x00);
    // Active flags pass through.
    assert_eq!(projectile_tick_flag(0x01), 0x01);
    assert_eq!(projectile_tick_flag(0x00), 0x00);
    // Liveness: kind 0 never live; active or F2-FF live.
    assert!(!projectile_live(0x01, 0x00));
    assert!(projectile_live(0x01, 0x04));
    assert!(projectile_live(0xF5, 0x04));
    assert!(!projectile_live(0x00, 0x04));
    // Tick: disintegrating slots hold position; live ones integrate.
    let dis = ProjectileSlot {
        y: 0x50,
        x: 0x60,
        page: 0x01,
        facing: 1,
        speed: 0x10,
        kind: 0x04,
        flag: 0xF2,
    };
    let (x, _, _, f) = projectile_tick(dis, 0x00);
    assert_eq!((x, f), (0x60, 0xF3));
}

// ---------------------------------------------------------------------------
// Sword / shield / spells (player.rs hitboxes reused read-only).
// ---------------------------------------------------------------------------

#[test]
fn sword_shield_spell_gates() {
    let live = SwordGate {
        retracted: false,
        untouchable: false,
        slot_live: true,
        enemy_code: 0x05,
        overlaps: true,
        blade: 0,
        fire_immune: false,
        jar_guarded: false,
    };
    assert_eq!(sword_gate(live), SwordOutcome::Hit);
    assert_eq!(
        sword_gate(SwordGate {
            retracted: true,
            ..live
        }),
        SwordOutcome::Miss
    );
    assert_eq!(
        sword_gate(SwordGate {
            enemy_code: 0x13,
            ..live
        }),
        SwordOutcome::Miss
    );
    assert_eq!(
        sword_gate(SwordGate {
            fire_immune: true,
            ..live
        }),
        SwordOutcome::Deflect
    );
    assert_eq!(
        sword_gate(SwordGate { blade: 1, ..live }),
        SwordOutcome::TouchItem
    );
    // Damage: kill vs survive, hover/bounce quirks ($E6F3-$E720).
    let kill = sword_damage_step(0x02, 0x06, 0x05, 0, 0x90, 0x80);
    assert!(kill.dead);
    assert_eq!(kill.hp, 0);
    assert_eq!(kill.stun, SWORD_STUN);
    let hover = sword_damage_step(0x20, 0x06, 0x08, 0, 0x90, 0x80);
    assert_eq!(hover.link_vspeed, Some(0x00));
    let bounce = sword_damage_step(0x20, 0x06, 0x09, 0, 0x90, 0x80);
    assert_eq!(bounce.link_vspeed, Some(0xFE));
    // Shield router ($E558): Daira+ pierce without reflect.
    assert!(!shield_blocks(false, 0x17, true));
    assert!(shield_blocks(true, 0x17, true));
    assert!(shield_blocks(false, 0x05, true));
    assert!(!shield_blocks(false, 0x05, false));
    // Thunder ($32) and Spell-spell revert.
    assert_eq!(thunder_damage_step(0x40).hp, 0x40 - THUNDER_POWER);
    assert!(spell_spell_reverts(1, false));
    assert!(!spell_spell_reverts(1, true));
    assert!(!spell_spell_reverts(0, false));
    // Box overlap sanity (duplicated kernel, $E9F9).
    let a = HitBox {
        x: 0x10,
        y: 0x10,
        w: 0x05,
        h: 0x05,
    };
    assert!(boxes_overlap(a, a));
    let far = HitBox {
        x: 0x80,
        y: 0x80,
        w: 0x02,
        h: 0x02,
    };
    assert!(!boxes_overlap(a, far));
}

// ---------------------------------------------------------------------------
// Determinism fuzz: same seed → identical (acceptance gate, green).
// ---------------------------------------------------------------------------

#[test]
fn enemy_fuzz_self_consistency() {
    const FRAMES: usize = 600;
    const SEEDS: u32 = 30;
    let mut checked: u64 = 0;
    for snap in 0..4 {
        let init = EnemySet::synthetic(snap);
        for seed in 0..SEEDS {
            let a = run_trajectory(init, 0x2000 + seed, FRAMES);
            let b = run_trajectory(init, 0x2000 + seed, FRAMES);
            assert_eq!(a, b, "divergence snap={snap} seed={seed}");
            checked += 1;
        }
    }
    eprintln!("enemy fuzz self-consistency: {checked} trajectories x {FRAMES} frames clean");
}
