//! ROM-free encounter tests + 10k-frame determinism fuzz.
//!
//! Compiles standalone (`rustc --edition 2021 --test`) and under cargo.
//! Oracle frame-for-frame comparison is documented as the gated remainder
//! (needs `z2-verify` oracle + `Game` trap wiring by main); what runs here
//! without an oracle: same-seed determinism + timer/table cross-checks vs
//! the disassembly constants.

#[path = "../src/overworld_encounter.rs"]
mod overworld_encounter;

use overworld_encounter::*;

// ---------------------------------------------------------------------------
// Table / gate unit tests (always run).
// ---------------------------------------------------------------------------

#[test]
fn spawn_tables_match_disassembly_bytes() {
    assert_eq!(TERRAIN_BY_GROUP, [0x00, 0x05, 0x04, 0x06, 0x07, 0x08, 0x0A]);
    assert_eq!(WAVE_RELOAD, [0x00, 0x20, 0x18, 0x18, 0x20, 0x09, 0x03]);
    assert_eq!(COUNT_SEED, [0x00, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00]);
    assert_eq!(DEMON_LIFE, [0x00, 0x0A, 0x0A, 0x18, 0x18, 0x30, 0x30]);
    assert_eq!(
        SPAWN_OFFSETS,
        [0x58, 0x76, 0x98, 0x7A, 0x38, 0x74, 0xB8, 0x7C]
    );
    assert_eq!(WAVE_TIMER_ON_EXIT, 8);
    assert_eq!(WAVE_TICK_FRAMES, 21);
}

#[test]
fn type_index_walk_matches_threshold_scan() {
    // rng >= top threshold of the group row stays at the row top.
    assert_eq!(type_index_for_rng(1, 0xF0), 0b1111);
    // rng = 0 walks to the bottom (flat[0] = 0 stops the walk at y=0).
    assert_eq!(type_index_for_rng(6, 0x00), 0b0011);
    // Mid value: group 1 row [00,60,B0,D0] from y=3: 0x70 < D0 → y=2;
    // 0x70 < B0 → y=1; 0x70 >= 0x60 → stop → ((1)<<2)|3 = 7.
    assert_eq!(type_index_for_rng(1, 0x70), 7);
    // Result always indexes DEMON_TYPES.
    for g in 1..=6 {
        for rng in [0x00u8, 0x01, 0x59, 0x60, 0xAF, 0xD0, 0xFF] {
            assert!(type_index_for_rng(g, rng) < DEMON_TYPES.len());
        }
    }
}

#[test]
fn spawn_event_fills_free_slots_deterministically() {
    let mk = || Demons::empty();
    let a = {
        let mut d = mk();
        spawn_event(&mut d, 2, 0x02, 0x70, 0x40, 0x80, 5, 0)
    };
    let mut d2 = mk();
    let b = spawn_event(&mut d2, 2, 0x02, 0x70, 0x40, 0x80, 5, 0);
    assert_eq!(a, b, "same seed must give identical spawn");
    // Wave timer reloads from the terrain table while stepping…
    assert_eq!(a.wave_timer, WAVE_RELOAD[2]);
    // …but is preserved when idle ($26 == 0).
    let mut d3 = mk();
    let c = spawn_event(&mut d3, 2, 0x02, 0x70, 0x40, 0x80, 0, 11);
    assert_eq!(c.wave_timer, 11);
    // One of the four attempts is skipped (counter == alt_rng & 3).
    assert_eq!(a.filled_count, 3);
    // Occupied slots are never overwritten.
    let mut d4 = mk();
    d4.kind[7] = 1;
    d4.timer[7] = 5;
    let before = d4.clone();
    spawn_event(&mut d4, 2, 0x02, 0x70, 0x40, 0x80, 5, 0);
    assert_eq!((d4.kind[7], d4.timer[7]), (before.kind[7], before.timer[7]));
    // New slots carry group life + valid types + scroll-biased positions.
    for s in 0..DEMON_SLOTS {
        if d2.kind[s] != 0 {
            assert_eq!(d2.timer[s], DEMON_LIFE[2]);
            assert!((1..=3).contains(&d2.kind[s]));
        }
    }
}

#[test]
fn ai_tick_zeroes_velocities_off_tick_and_chases_on_tick() {
    let mut d = Demons::empty();
    d.kind[0] = 1;
    d.timer[0] = 10;
    d.vx[0] = 5;
    // Off-tick frames leave everything alone.
    ai_tick(&mut d, 0x03, 0x84, 0x00, 0x00, 0x00, 0x00);
    assert_eq!(d.vx[0], 5);
    // Tick frame zeroes then re-derives. Demon vertically aligned with the
    // chase anchor (vtest = $70−y+scroll+$10 < $20) and left of Link on
    // screen steps right (+1, toward Link).
    d.y[0] = 0x70;
    d.x[0] = 0x80;
    ai_tick(&mut d, 0x40, 0x84, 0x00, 0x00, 0xAA, 0x00);
    assert_eq!((d.vx[0], d.vy[0]), (1, 0), "demon left of Link steps right");
    // Random-walk branch (frame < $40): sign from bit7, axis from bit2.
    let mut r = Demons::empty();
    r.kind[1] = 2;
    r.timer[1] = 9;
    r.y[1] = 50;
    r.x[1] = 60;
    ai_tick(&mut r, 0x10, 0x84, 0x00, 0x00, 0x00, 0x00); // +1, Y axis
    assert_eq!((r.vy[1], r.y[1]), (1, 51));
    ai_tick(&mut r, 0x20, 0x84, 0x00, 0x00, 0x04, 0x80); // -1, X axis
    assert_eq!((r.vx[1], r.x[1]), (-1, 59));
    // Fairy never chases even at high frames.
    let mut f = Demons::empty();
    f.kind[2] = 3;
    f.timer[2] = 9;
    f.y[2] = 0x00;
    f.x[2] = 0x80;
    ai_tick(&mut f, 0x40, 0x84, 0x00, 0x00, 0x00, 0x00);
    assert_eq!((f.vx[2], f.vy[2]), (0, 1), "fairy random-walks");
}

#[test]
fn integrate_and_expire_match_l8336_l840b() {
    let mut d = Demons::empty();
    d.kind[0] = 1;
    d.timer[0] = 3;
    d.vy[0] = -1;
    d.vx[0] = 2;
    d.y[0] = 10;
    d.x[0] = 10;
    integrate(&mut d);
    assert_eq!((d.y[0], d.x[0]), (9, 12));
    d.timer[0] = 0;
    integrate(&mut d);
    assert_eq!(d.kind[0], 0, "expired timer clears the slot");
}

#[test]
fn exit_reset_matches_le179_plus_overworld4() {
    let mut d = Demons::empty();
    d.kind[0] = 2;
    d.timer[0] = 4;
    assert_eq!(exit_reset(&mut d), 8);
    assert_eq!(d, Demons::empty());
}

// ---------------------------------------------------------------------------
// 10k-frame determinism fuzz from 3 snapshot-like starts (oracle-free).
// ---------------------------------------------------------------------------

/// Minimal LCG (same recurrence family as typical harnesses; NOT the NES
/// LFSR — determinism source only, never compared to hardware).
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 16) as u8
    }
}

struct FuzzStart {
    demons: Demons,
    frame: u8,
    link_scr_x: u8,
    scroll: (u8, u8),
}

fn fuzz_trajectory(seed: u32, start: &FuzzStart, frames: usize) -> (Demons, u8, u64) {
    let mut rng = Lcg(seed);
    let mut d = start.demons.clone();
    let mut frame = start.frame;
    let mut wave = WAVE_TIMER_ON_EXIT;
    let mut tally: u8 = 1;
    let mut checksum: u64 = 0;
    for _ in 0..frames {
        frame = frame.wrapping_add(1);
        let r = rng.next();
        let rx = rng.next();
        ai_tick(
            &mut d,
            frame,
            start.link_scr_x,
            start.scroll.0,
            start.scroll.1,
            r,
            rx,
        );
        integrate(&mut d);
        // Wave + life timers tick on the 21-frame sweep.
        if frame.is_multiple_of(WAVE_TICK_FRAMES) {
            wave = dec_timer(wave);
            for s in 0..DEMON_SLOTS {
                d.timer[s] = dec_timer(d.timer[s]);
            }
        }
        // A step commits about every 16 frames while moving (L86AF/$7D).
        if frame.is_multiple_of(16) {
            tally = tally.wrapping_add(1);
        }
        // overworld1 runs every frame: spawn on grass when the gate opens.
        if spawn_attempt(1, 0x40, tally, wave) {
            if let Some(g) = spawn_group_for_terrain(0x05) {
                let out = spawn_event(
                    &mut d,
                    g,
                    rng.next(),
                    rng.next(),
                    start.scroll.0,
                    start.scroll.1,
                    tally,
                    wave,
                );
                wave = out.wave_timer;
            }
        }
        // Fold observable state (timers must stay within table bounds).
        for s in 0..DEMON_SLOTS {
            checksum = checksum
                .wrapping_mul(31)
                .wrapping_add(d.y[s] as u64)
                .wrapping_add((d.x[s] as u64) << 8)
                .wrapping_add((d.kind[s] as u64) << 16)
                .wrapping_add((d.timer[s] as u64) << 24);
        }
        checksum = checksum.wrapping_add(wave as u64);
    }
    (d, wave, checksum)
}

fn three_starts() -> [FuzzStart; 3] {
    // Start 0: fresh exit (empty). Start 1: mid-wave mixed types.
    // Start 2: full board incl. fairy.
    let mut mid = Demons::empty();
    mid.kind[0] = 1;
    mid.timer[0] = 0x18;
    mid.y[0] = 0x40;
    mid.x[0] = 0x60;
    mid.kind[3] = 2;
    mid.timer[3] = 0x30;
    mid.y[3] = 0x70;
    mid.x[3] = 0x20;
    let mut full = Demons::empty();
    for s in 0..DEMON_SLOTS {
        full.kind[s] = (s % 3 + 1) as u8;
        full.timer[s] = 0x0A + s as u8;
        full.y[s] = 0x30 + s as u8 * 3;
        full.x[s] = 0x50 + s as u8 * 5;
    }
    [
        FuzzStart {
            demons: Demons::empty(),
            frame: 0,
            link_scr_x: 0x84,
            scroll: (0x10, 0x20),
        },
        FuzzStart {
            demons: mid,
            frame: 0x39,
            link_scr_x: 0x84,
            scroll: (0x90, 0x40),
        },
        FuzzStart {
            demons: full,
            frame: 0x80,
            link_scr_x: 0x7C,
            scroll: (0x00, 0x00),
        },
    ]
}

#[test]
fn ten_k_frame_random_walk_is_deterministic() {
    for (i, start) in three_starts().iter().enumerate() {
        let (d1, w1, c1) = fuzz_trajectory(0xC0FFEE, start, 10_000);
        let (d2, w2, c2) = fuzz_trajectory(0xC0FFEE, start, 10_000);
        assert_eq!(d1, d2, "start {i}: demons must be bit-identical");
        assert_eq!((w1, c1), (w2, c2), "start {i}: timer+hash identical");
        // Timer cross-check vs disassembly constants (no oracle needed):
        // life timers never exceed the max table value, wave ≤ 32.
        assert!(d1.timer.iter().all(|&t| t <= 0x30), "start {i}: life bound");
        assert!(w1 <= 0x20, "start {i}: wave bound");
        // Different seeds diverge (fuzz actually exercises RNG).
        let (_, _, c3) = fuzz_trajectory(0xDEAD, start, 10_000);
        assert_ne!(c1, c3, "start {i}: seed must matter");
    }
}

/// Oracle-gated remainder (recorded, not run): with `z2-verify` + `Game`
/// trap wiring (main), replay each snapshot's `input_history` through the
/// trapped `overworld1`/`L841B` and compare `$0516`/`$050E`/`$82` per frame
/// for the first divergence; warpless-segment acceptance is frame-exact
/// `$0516` equality at every 21-frame tick boundary.
#[test]
fn oracle_trace_comparison_harness_documents_inputs() {
    // States the oracle run must cover (kept in sync with the fuzz starts).
    assert_eq!(three_starts().len(), 3);
    assert_eq!((WAVE_TICK_FRAMES as usize) * 476, 9996); // 10k-frame window ≈ 476 ticks
}

#[test]
fn address_and_code_consts_match_ram_map() {
    assert_eq!(ADDR_WAVE_TIMER, 0x0516);
    assert_eq!(ADDR_ALT_RNG, 0x051C);
    assert_eq!(ADDR_RNG, 0x051B);
    assert_eq!(ADDR_FAIRY_FLAG, 0x0759);
    assert_eq!(ADDR_ENCOUNTER_TYPE, 0x075A);
    assert_eq!(ADDR_AREA_INDEX, 0x0748);
    assert_eq!(ADDR_STEP_TALLY, 0x0026);
    assert_eq!(ADDR_OVERWORLD_INDEX, 0x0706);
    assert_eq!(ADDR_TILE_Y, 0x0073);
    assert_eq!(
        (
            ADDR_MONSTER_TYPE_0,
            ADDR_MONSTER_TYPE_1,
            ADDR_MONSTER_TYPE_2,
            ADDR_MONSTER_TYPE_3
        ),
        (0x0086, 0x0087, 0x0088, 0x0089)
    );
    // 2D and flat threshold views agree row-major.
    for g in 0..6 {
        assert_eq!(&THRESHOLDS_FLAT[g * 4..g * 4 + 4], &THRESHOLDS[g]);
    }
    assert_eq!(
        (
            DemonType::None as u8,
            DemonType::Weak as u8,
            DemonType::Strong as u8
        ),
        (0, 1, 2)
    );
    assert_eq!(
        (EncounterType::Small as u8, EncounterType::Big as u8),
        (1, 2)
    );
}
