//! Palace pure-logic tests.
//!
//! ROM-free fixtures throughout; ROM-gated loaders take caller-staged
//! bytes (`item_code_at`).

use z2_core::palace::*;

// ---------------------------------------------------------------------------
// Index + map sets.
// ---------------------------------------------------------------------------

#[test]
fn palace_numbers_split_into_two_sets_of_63() {
    assert_eq!(ROOMS_PER_SET, 63);
    for n in 1..=6u8 {
        let p = palace_of_number(n).expect("1-6 must decode");
        assert_eq!(palace_number(p), n);
    }
    assert_eq!(palace_of_number(0), None);
    assert_eq!(palace_of_number(7), None);
    // 1/2/5 → A (world 3); 3/4/6 → B (world 4).
    for (n, set, world) in [
        (1, MapSet::SetA, 3),
        (2, MapSet::SetA, 3),
        (5, MapSet::SetA, 3),
        (3, MapSet::SetB, 4),
        (4, MapSet::SetB, 4),
        (6, MapSet::SetB, 4),
    ] {
        let p = palace_of_number(n).unwrap();
        assert_eq!(palace_map_set(p), set, "palace {n}");
        assert_eq!(world_of_set(set), world);
    }
    assert_eq!(WORLD_GREAT_PALACE, 5);
}

#[test]
fn palace_quirks_entrance_boss_shared_lava_heights() {
    assert!(entrance_shared());
    assert!(boss_room_shared());
    assert_eq!(lava_pit_height(4), Some(2));
    assert_eq!(lava_pit_height(5), Some(3));
    assert_eq!(lava_pit_height(7), None);
    // Chunk selectors wrap the slot.
    assert_eq!(great_palace_chunk(0), ROM_GREAT_PALACE[0]);
    assert_eq!(great_palace_chunk(4), ROM_GREAT_PALACE[0]);
    assert_eq!(type_b_body(1), ROM_TYPE_B[1]);
    assert_eq!(ROM_ENTRANCE_A.addr, 0x82E5);
    assert_eq!(ROM_BOSS_ROOM_A.addr, 0x831B);
    assert_eq!(ROM_TYPE_A.addr, 0x861F);
    assert_eq!(ROM_THUNDERBIRD_TAB.addr, 0xA34F);
    assert_eq!(ROM_DARKLINK_TAB.addr, 0x97CE);
}

// ---------------------------------------------------------------------------
// Keys + doors.
// ---------------------------------------------------------------------------

#[test]
fn keys_pickup_wraps_and_doors_consume_or_bypass() {
    assert_eq!(key_pickup(3), 4);
    assert_eq!(key_pickup(0xFF), 0x00);
    assert_eq!(locked_door_consume(3, false), Some(2));
    assert_eq!(locked_door_consume(0, false), None);
    assert!(door_blocked(0, false));
    // Magic key bypasses without decrementing.
    assert_eq!(locked_door_consume(0, true), Some(0));
    assert_eq!(locked_door_consume(2, true), Some(2));
    assert!(!door_blocked(0, true));
    assert_eq!(MAX_KEYS, 9);
}

// ---------------------------------------------------------------------------
// Item rooms.
// ---------------------------------------------------------------------------

#[test]
fn palace_item_map_covers_candle_to_cross() {
    let want = [
        (PalaceId::P1, ITEM_CANDLE),
        (PalaceId::P2, ITEM_GLOVE),
        (PalaceId::P3, ITEM_RAFT),
        (PalaceId::P4, ITEM_BOOTS),
        (PalaceId::P5, ITEM_FLUTE),
        (PalaceId::P6, ITEM_CROSS),
    ];
    for (p, code) in want {
        assert_eq!(palace_item(p), code);
        assert!(code < 8, "palace items live in the flag row");
    }
    assert_eq!(ITEM_MAGIC_KEY, 7);
    assert_eq!(ITEM_KEY, 8);
}

#[test]
fn grant_palace_item_sets_flag_bits_only() {
    let mut row = [0u8; 8];
    assert!(grant_palace_item(&mut row, ITEM_RAFT));
    assert_eq!(row[ITEM_RAFT as usize], 0x01);
    // Idempotent; other slots untouched.
    assert!(grant_palace_item(&mut row, ITEM_RAFT));
    assert_eq!(row, [0, 0, 1, 0, 0, 0, 0, 0]);
    // Non-flag codes (key/containers) are not flag writes.
    assert!(!grant_palace_item(&mut row, ITEM_KEY));
    assert!(!grant_palace_item(&mut row, ITEM_HEART_CONTAINER));
    // Short slices fail total (no panic).
    let mut tiny = [0u8; 2];
    assert!(!grant_palace_item(&mut tiny, ITEM_BOOTS));
    assert!(grant_palace_item(&mut tiny, ITEM_GLOVE));
}

#[test]
fn item_code_loader_reads_staged_bytes() {
    let area = [0x00u8, 0x86, 0x04];
    assert_eq!(item_code_at(&area, 1), Some(0x86));
    assert_eq!(item_code_at(&area, 3), None);
    assert_eq!(item_code_at(&[], 0), None);
}

// ---------------------------------------------------------------------------
// Crystals.
// ---------------------------------------------------------------------------

#[test]
fn crystal_place_gate_needs_all_four() {
    assert!(crystal_place_gate(0, 6, true, true));
    assert!(!crystal_place_gate(1, 6, true, true), "decor blocks");
    assert!(!crystal_place_gate(0, 0, true, true), "none left");
    assert!(!crystal_place_gate(0, 6, false, true), "no touch");
    assert!(!crystal_place_gate(0, 6, true, false), "airborne");
}

#[test]
fn crystal_slot_keeps_the_carry_quirk() {
    // region 0 passes through: 0 + code + 1.
    assert_eq!(crystal_slot_index(0, 1), 2);
    // region != 0 adds 2 first: 1 + 2 + code + 1.
    assert_eq!(crystal_slot_index(1, 1), 5);
    assert_eq!(crystal_flag_addr(2), ADDR_CRYSTALS_PLACED + 2);
    // DEC wraps (guarded unreachable in ROM, kept here).
    assert_eq!(crystals_left_after(6), 5);
    assert_eq!(crystals_left_after(0), 0xFF);
    assert_eq!((CRYSTAL_Y, CRYSTAL_X), (0x62, 0x7C));
}

#[test]
fn crystal_flight_seats_exactly_on_62() {
    assert_eq!(crystal_flight_step(0x63), (0x62, true));
    assert_eq!(crystal_flight_step(0xA0), (0x9F, false));
    assert_eq!(crystal_flight_step(0x62), (0x61, false));
    assert_eq!(CRYSTAL_SEAT_Y, 0x62);
    assert_eq!(CRYSTAL_PLACE_Y, 0xA0);
}

#[test]
fn crystal_refill_and_done_gates() {
    assert_eq!(crystal_refill_pending(0), Some(0xFF));
    assert_eq!(crystal_refill_pending(1), None);
    assert!(crystal_done(0, 0));
    assert!(!crystal_done(0xFF, 0xFF));
    assert!(!crystal_done(0x10, 0));
}

// ---------------------------------------------------------------------------
// Stone write-back.
// ---------------------------------------------------------------------------

#[test]
fn stone_stamp_maps_palace_to_rock_only() {
    assert_eq!(stone_tile(0x60), 0x56);
    assert_eq!(stone_tile(0x61), 0x57);
    assert_eq!(stone_tile(0x62), 0x58);
    assert_eq!(stone_tile(0x63), 0x59);
    assert_eq!(stone_tile(0x6D), 0x6D);
    assert_eq!(stone_tile(0xFE), 0xFE);
    assert!(is_palace_tile(0x60));
    assert!(!is_palace_tile(0x56));
    assert!(stone_patch_gate(true, true));
    assert!(!stone_patch_gate(true, false));
    assert!(!stone_patch_gate(false, true));
}

// ---------------------------------------------------------------------------
// Barrier + Thunderbird context.
// ---------------------------------------------------------------------------

#[test]
fn barrier_needs_all_six_crystals_and_position() {
    let open = BarrierIn {
        aux: 0,
        crystals_left: 0,
        decor: 0,
        page: 0,
        link_x: 0xC0,
    };
    assert_eq!(barrier_gate(open), BarrierGate::Open);
    assert_eq!(
        barrier_gate(BarrierIn { aux: 1, ..open }),
        BarrierGate::Dissolving
    );
    assert_eq!(
        barrier_gate(BarrierIn {
            crystals_left: 1,
            ..open
        }),
        BarrierGate::Hold,
        "one crystal left still holds"
    );
    assert_eq!(
        barrier_gate(BarrierIn { decor: 1, ..open }),
        BarrierGate::Hold
    );
    assert_eq!(
        barrier_gate(BarrierIn { page: 1, ..open }),
        BarrierGate::Hold
    );
    assert_eq!(
        barrier_gate(BarrierIn {
            link_x: 0xBF,
            ..open
        }),
        BarrierGate::Hold
    );
    assert_eq!(BARRIER_TRIGGER_X, 0xC0);
}

#[test]
fn thunder_door_slams_off_page_zero() {
    assert_eq!(
        thunder_door_gate(ThunderDoorIn { frozen: 1, page: 3 }),
        ThunderDoor::Combat
    );
    assert_eq!(
        thunder_door_gate(ThunderDoorIn { frozen: 0, page: 0 }),
        ThunderDoor::Hold
    );
    assert_eq!(
        thunder_door_gate(ThunderDoorIn { frozen: 0, page: 2 }),
        ThunderDoor::Slam
    );
    assert_eq!(THUNDER_DOOR_TIMER, 0x90);
    assert_eq!((THUNDER_HP_SPLIT, THUNDER_ENRAGE_HP), (0xC0, 0x60));
}

#[test]
fn thunder_kernel_wiring_init_wake_tick() {
    // Init: screen-check 0 despawns (kernel-owned).
    assert!(!thunder_init_gate(0x00));
    assert!(thunder_init_gate(0x01));
    // Wake: dormant until Thunder lands.
    assert!(!thunder_wake(false, false));
    assert!(thunder_wake(true, false));
    assert!(thunder_wake(false, true));
    // Tick delegates: dormant bird never fires/flaps.
    let out = thunder_tick(z2_core::enemy_boss::ThunderbirdIn {
        awake: false,
        aux: 0,
        frame: 0x20,
        rng: 0,
        thunder_struck: false,
    });
    assert!(!out.fire && !out.flap);
    let woken = thunder_tick(z2_core::enemy_boss::ThunderbirdIn {
        awake: false,
        aux: 0,
        frame: 0x20,
        rng: 0,
        thunder_struck: true,
    });
    assert!(woken.awake);
}

// ---------------------------------------------------------------------------
// Dark Link trigger + Triforce + ending.
// ---------------------------------------------------------------------------

#[test]
fn dark_phases_dispatch_and_gates() {
    assert_eq!(dark_phase(0), DarkPhase::Setup);
    assert_eq!(dark_phase(1), DarkPhase::Triforce);
    assert_eq!(dark_phase(2), DarkPhase::Flash);
    assert_eq!(dark_phase(3), DarkPhase::Spawn);
    for s in 4..=0xFFu8 {
        assert_eq!(dark_phase(s), DarkPhase::Duel);
    }
    assert_eq!(
        darklink_setup_gate(DarkSetupIn {
            page: 0,
            grounded: true
        }),
        DarkSetup::Hold
    );
    assert_eq!(
        darklink_setup_gate(DarkSetupIn {
            page: 2,
            grounded: true
        }),
        DarkSetup::Plant
    );
    assert_eq!(
        darklink_setup_gate(DarkSetupIn {
            page: 2,
            grounded: false
        }),
        DarkSetup::Freeze
    );
    assert!(darklink_spawn_gate(FLASH_SPAWN_COUNTER));
    assert!(!darklink_spawn_gate(0x80));
    assert_eq!((FLASH_SPAWN_COUNTER, FLASH_SPAWN_MACRO), (0x81, 0x0D));
    let init = darklink_init();
    assert_eq!(
        (init.mon_id, init.mon_hp, init.latch_753, init.sprite),
        (0x23, 0x08, 0x02, 0x01)
    );
}

#[test]
fn darklink_duel_delegates_to_kernel() {
    // Corner-crouch dice + whiff punish live in the kernel; the wrapper
    // must agree with it exactly.
    let inp = z2_core::enemy_boss::DarkLinkIn {
        link_speed: 0x10,
        link_anim: 0x05,
        link_y: 0x90,
        self_y: 0x90,
        frame: 0x08,
        rng: 0xFF,
        atk: 4,
    };
    assert_eq!(darklink_tick(inp), z2_core::enemy_boss::dark_link(inp));
}

#[test]
fn triforce_and_ending_chain() {
    assert_eq!(triforce_emit(), (0xD2, 0x01));
    assert_eq!((TRIFORCE_TILE, TRIFORCE_ATTR), (0xD2, 0x01));
    assert_eq!(wake_zelda(), STATE_WAKE_ZELDA);
    assert_eq!(STATE_WAKE_ZELDA, 3);
    assert_eq!(roll_credits(3), 4);
    assert_eq!(roll_credits(0xFF), 0x00);
    assert!(ending_chain(3));
    assert!(ending_chain(4));
    assert!(!ending_chain(1));
    assert_eq!((STATE_INGAME, STATE_CREDITS), (1, 4));
}
