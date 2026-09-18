//! ROM-free player physics/combat/fairy tests + gated snapshots + fuzz.

#[allow(dead_code)]
#[path = "../src/player.rs"]
mod player;

mod common;

use player::*;

// ---------------------------------------------------------------------------
// Pure-logic checks (ROM-free).
// ---------------------------------------------------------------------------

#[test]
fn walk_accel_friction_clamp() {
    // Accelerate right to max.
    let mut v = 0u8;
    for _ in 0..64 {
        v = walk_step_default(v, BTN_RIGHT, false);
    }
    assert_eq!(v, WALK_MAX);
    // Friction returns to 0.
    for _ in 0..64 {
        v = walk_step_default(v, 0, false);
    }
    assert_eq!(v, 0);
    // Mid-air control is weaker: one frame from 0.
    assert_eq!(walk_step_default(0, BTN_RIGHT, true), AIR_ACCEL);
    assert_eq!(walk_step_default(0, BTN_RIGHT, false), WALK_ACCEL);
    // Facing updates.
    assert_eq!(facing_step(FACING_RIGHT, BTN_LEFT), FACING_LEFT);
    assert_eq!(facing_step(FACING_LEFT, BTN_LEFT | BTN_RIGHT), FACING_LEFT);
}

#[test]
fn jump_duck_thrust_gates() {
    let (m, v) = jump_init(0, true, false);
    assert_eq!((m, v), (2, JUMP_VELOCITY));
    let (m2, v2) = jump_init(0, true, true);
    assert_eq!((m2, v2), (2, JUMP_VELOCITY_SPELL));
    assert_eq!(jump_init(1, true, false).0, 1);
    assert_eq!(duck_state(true, 0, 1), (true, false, false));
    assert_eq!(duck_state(true, 0, 0), (true, true, false));
    assert_eq!(duck_state(true, 1, 1), (false, false, false));
    let t = thrust_avail(THRUST_DOWN, 1, BTN_DOWN);
    assert!(t.down && !t.up);
}

#[test]
fn hitboxes_overlap_paths() {
    // code43/code44/code45 box builders + idem overlap.
    let shield = shield_box(0x80, 0x90, 0x04, 0x0C);
    assert_eq!((shield.x, shield.w, shield.h), (0x89, SHIELD_W, 0x0C));
    let sword = sword_box(0x90, 0x80, true, false);
    assert_eq!(sword.w, SWORD_W);
    let body = body_box(0x80, 0x90, true, 0x11);
    assert_eq!((body.w, body.h), (BODY_W, BODY_H));
    assert!(boxes_overlap(sword, sword));
    let far = HitBox {
        x: 0x00,
        y: 0x00,
        w: 0x02,
        h: 0x02,
    };
    assert!(!boxes_overlap(sword, far));
    // Touching edges do not overlap.
    let a = HitBox {
        x: 0x10,
        y: 0x10,
        w: 0x05,
        h: 0x05,
    };
    let b = HitBox {
        x: 0x15,
        y: 0x10,
        w: 0x05,
        h: 0x05,
    };
    assert!(!boxes_overlap(a, b));
}

#[test]
fn sword_gate_and_stab_quirks() {
    let live = SwordGate {
        retracted: false,
        target_immune: false,
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
    // Jackhammer down-thrust + crouch-thrust anims + up-stab hover.
    assert_eq!(downstab_bounce(ANIM_DOWN_STAB, 0), Some(0xFE));
    assert_eq!(downstab_bounce(ANIM_STAB, 0), None);
    assert_eq!(upstab_hover(ANIM_UP_STAB, 0, 0x70, 0x60), Some(0x00));
    assert_eq!(upstab_hover(ANIM_UP_STAB, 0, 0x50, 0x60), None);
}

#[test]
fn damage_recoil_shield_paths() {
    // Shield spell halves table damage.
    assert_eq!(damage_to_link(0, 1, false), 0x10);
    assert_eq!(damage_to_link(0, 1, true), 0x08);
    // Borrow kills.
    assert_eq!(apply_damage(0x05, 0x10), (0x00, true));
    assert_eq!(apply_damage(0x20, 0x10), (0x10, false));
    // Recoil suppressed when facing matches collision bits.
    assert_eq!(recoil_select(0, 1, 0), None);
    assert!(recoil_select(0, 0, 1).is_some());
    // Daira+ pierce without reflect; reflect always shields.
    assert!(!shield_blocks(false, 0x17, true));
    assert!(shield_blocks(true, 0x17, true));
    assert!(shield_blocks(false, 0x05, true));
    assert!(!shield_blocks(false, 0x05, false));
}

#[test]
fn fairy_form_paths() {
    assert_eq!(fairy_cast(0, 0x1F), 0);
    assert_eq!(fairy_cast(0, 0x20), 8);
    assert_eq!(fairy_revert(8, 0), 0);
    assert_eq!(fairy_revert(8, 0xFF), 8);
    let (y0, x0) = fairy_step(0x60, 0x70, 0x00);
    let (y1, _) = fairy_step(0x60, 0x70, 0x18);
    assert_eq!((y0, x0), (0x60 + 8 + FAIRY_FLOAT[0], 0x70 + 0x0C));
    assert_ne!(y0, y1);
    assert_eq!(fairy_flicker(0), 1);
    assert_eq!(fairy_flicker(0x20), (0x20 >> 1) & 3);
}

#[test]
fn headless_step_smoke() {
    // Walk right 30 frames on flat floor: X advances, still grounded.
    let mut st = PlayerState::synthetic(0);
    for _ in 0..30 {
        st.coll = COLL_BELOW;
        st.step(BTN_RIGHT, 0);
    }
    assert!(st.x_lo != 0x80 || st.x_hi != 0x01);
    assert_eq!(st.midair, 0);
    // Jump then fall: airborne at least once across 120 frames.
    let mut st = PlayerState::synthetic(0);
    let mut saw_air = false;
    st.coll = COLL_BELOW;
    st.step(0, BTN_A);
    for _ in 0..120 {
        st.coll = if st.y >= 0x90 && (st.vspeed as i8) >= 0 {
            COLL_BELOW
        } else {
            0
        };
        st.step(0, 0);
        saw_air |= st.midair != 0;
    }
    assert!(saw_air);
}

// ---------------------------------------------------------------------------
// Gated snapshot harness (ROM/corpus needed for oracle diff — skips cleanly).
// ---------------------------------------------------------------------------

// Placeholder: the gate-satisfied path below is an unconditional panic
// because the oracle diff for player-state snapshots is not wired yet.
// Parked as ignored so that a developer *with* a ROM and a real corpus is
// not shown a false red; it stays visible in the ignored list until that diff
// lands. The body is unchanged: a gated test must skip when its
// prerequisites are absent, and must not fail merely because they are
// present.
#[test]
#[ignore = "the oracle diff for player-state snapshots is not wired yet"]
fn player_snapshots_gated() {
    let rom = common::rom_path("player_snapshots_gated").is_some();
    let has_corpus = common::corpus_snapshots("player_snapshots_gated").is_some();
    if !(rom && has_corpus) {
        eprintln!(
            "skipping player_snapshots_gated: need Z2_ROM (present={rom}) + corpus snapshots (present={has_corpus})"
        );
        return;
    }
    // Harness-ready: oracle diff plugs in here once corpus lands.
    panic!("corpus present but oracle diff not wired");
}

// Placeholder: the gate-satisfied path below is an unconditional panic
// because the movie player-segment diff is not wired yet. Parked as ignored
// so that a developer *with* a ROM and a real corpus is not shown a false
// red; it stays visible in the ignored list until that diff lands. The body is
// unchanged: a gated test must skip when its prerequisites are absent, and
// must not fail merely because they are present.
#[test]
#[ignore = "the movie player-segment diff is not wired yet"]
fn player_movie_segments_gated() {
    // Four movies' player segments (TASVerifier-style inputs); report absence.
    let Some(root) = common::env_dir("Z2_MOVIES", "corpus/movies", "player_movie_segments_gated")
    else {
        return;
    };
    let names = ["movie1", "movie2", "movie3", "movie4"];
    let mut missing = Vec::new();
    for n in names {
        let cand = root.join(format!("{n}.fm2"));
        if !cand.is_file() {
            missing.push(cand.display().to_string());
        }
    }
    if !missing.is_empty() {
        eprintln!(
            "SKIP player movie segments: missing files: {}",
            missing.join(", ")
        );
        return;
    }
    panic!("movies present but segment diff not wired");
}

// ---------------------------------------------------------------------------
// Determinism fuzz: 600 frames x 50 seeds x 5 synthetic snapshots.
// Self-consistency oracle (same seed → identical trajectory).
// ---------------------------------------------------------------------------

#[test]
fn player_fuzz_self_consistency() {
    const FRAMES: usize = 600;
    const SEEDS: u32 = 50;
    let mut checked: u64 = 0;
    for snap in 0..5 {
        let init = PlayerState::synthetic(snap);
        for seed in 0..SEEDS {
            let a = run_trajectory(init, 0x1000 + seed, FRAMES);
            let b = run_trajectory(init, 0x1000 + seed, FRAMES);
            assert_eq!(a, b, "divergence snap={snap} seed={seed}");
            checked += 1;
        }
    }
    eprintln!("player fuzz self-consistency: {checked} trajectories x {FRAMES} frames clean");
}
