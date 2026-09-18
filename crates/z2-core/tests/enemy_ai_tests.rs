//! ROM-free enemy AI + data tests: families, bank dispatch, id windows,
//! ROM-offset descriptors.

#[allow(dead_code)]
#[path = "../src/enemy_ai.rs"]
mod enemy_ai;

#[allow(dead_code)]
#[path = "../src/enemy_data.rs"]
mod enemy_data;

// ---------------------------------------------------------------------------
// Walkers ($DA0C core + Daira/IronKnuckle/Stalfos/Tinsuit overlays).
// ---------------------------------------------------------------------------

#[test]
fn walker_hop_and_cruise() {
    use enemy_ai::*;
    // Bot hops only on RNG-bit7-clear (ASL/BNE quirk, $DA15).
    let hop = WalkerIn {
        grounded_bit: true,
        rng: 0x00,
        frame: 0,
        slot: 0,
        speed: 0x08,
        yspeed: 0,
        mode: WalkerMode::BotCore,
    };
    assert!(walker_step(hop).hopped);
    assert!(!walker_step(WalkerIn { rng: 0x41, ..hop }).hopped);
    // Airborne never hops.
    assert!(
        !walker_step(WalkerIn {
            grounded_bit: false,
            ..hop
        })
        .hopped
    );
    // Bit windows: (slot<<5 | frame) >= $C0 is fast ($DA34). Use a
    // non-hopping RNG so the cruise select runs (rng 0x00 would hop).
    let fast = walker_step(WalkerIn {
        frame: 0xC0,
        speed: 0x08,
        rng: 0x41,
        ..hop
    });
    assert_eq!(fast.speed, FAST_RIGHT);
    let slow = walker_step(WalkerIn {
        frame: 0x00,
        speed: 0x08,
        rng: 0x41,
        ..hop
    });
    assert_eq!(slow.speed, SLOW_RIGHT);
    // Tinsuit hops whenever grounded ($BCCE $F0).
    let tin = walker_step(WalkerIn {
        mode: WalkerMode::Tinsuit,
        ..hop
    });
    assert_eq!(tin.yspeed, TINSUIT_HOP);
    // Overlays hold cruise without hopping.
    for mode in [
        WalkerMode::Daira,
        WalkerMode::IronKnuckle,
        WalkerMode::Stalfos,
        WalkerMode::Geldarm,
        WalkerMode::Lowder,
        WalkerMode::Fokka,
    ] {
        let o = walker_step(WalkerIn { mode, ..hop });
        assert!(!o.hopped, "{mode:?} must not hop");
    }
}

// ---------------------------------------------------------------------------
// Jumpers / flyers ($9805 / $987E / $D6DF / $DB53 / $DACF).
// ---------------------------------------------------------------------------

#[test]
fn jumper_and_flyer_ticks() {
    use enemy_ai::*;
    // Tektite-style hop on timer 0 + grounded.
    let j = jumper_step(JumperIn {
        grounded: true,
        timer: 0,
        rng: 0x12,
        facing: 1,
    });
    assert_eq!(j.yspeed, BOT_HOP);
    assert_eq!(j.speed, CRUISE_RIGHT);
    assert_ne!(j.timer, 0);
    // Facing 2 drifts left.
    let jl = jumper_step(JumperIn {
        grounded: true,
        timer: 0,
        rng: 0x12,
        facing: 2,
    });
    assert_eq!(jl.speed, CRUISE_LEFT);
    // Deeler dragon path ($AF bit7) holds dive.
    let d = flyer_step(FlyerIn {
        mode: FlyerMode::Deeler,
        aux: 0x80,
        dist: 0,
        rng: 0,
        y: 0x80,
        yspeed: 0,
        is_blue_deeler: false,
    });
    assert_eq!(d.yspeed, DEELER_DOWN);
    // Ache sleeps when far ($DB8E window), dives when near.
    let sleep = flyer_step(FlyerIn {
        mode: FlyerMode::Ache,
        aux: 0,
        dist: 0x40,
        rng: 0,
        y: 0x80,
        yspeed: 0,
        is_blue_deeler: false,
    });
    assert_eq!(sleep.aux, 0);
    let dive = flyer_step(FlyerIn {
        mode: FlyerMode::Ache,
        aux: 0,
        dist: 0x05,
        rng: 0,
        y: 0x80,
        yspeed: 0,
        is_blue_deeler: false,
    });
    assert_eq!(dive.yspeed, ACHE_DIVE);
    // Ache ceiling clamp ($DBB9 $30).
    let ceil = flyer_step(FlyerIn {
        mode: FlyerMode::Ache,
        aux: 1,
        dist: 0x05,
        rng: 0,
        y: 0x20,
        yspeed: 0xE4,
        is_blue_deeler: false,
    });
    assert_eq!(ceil.yspeed, 0x00);
    // Moa advances its table index.
    let moa = flyer_step(FlyerIn {
        mode: FlyerMode::Moa,
        aux: 0x03,
        dist: 0,
        rng: 0,
        y: 0x80,
        yspeed: 0,
        is_blue_deeler: false,
    });
    assert_eq!(moa.aux, 0x04);
}

// ---------------------------------------------------------------------------
// Generators / shooters / statues.
// ---------------------------------------------------------------------------

#[test]
fn generator_shooter_statue_ticks() {
    use enemy_ai::*;
    // Generator: aux always ++; fires on (aux&mask)==0 with a free slot.
    let wait = generator_step(GeneratorIn {
        aux: 0x00,
        mask: GEN_MASK_BUBBLE,
        child: 0x02,
        slot_free: true,
    });
    assert!(!wait.spawn);
    assert_eq!(wait.aux, 0x01);
    let fire = generator_step(GeneratorIn {
        aux: 0x1F,
        mask: GEN_MASK_BUBBLE,
        child: 0x02,
        slot_free: true,
    });
    assert!(fire.spawn);
    assert!(
        !generator_step(GeneratorIn {
            aux: 0x1F,
            mask: GEN_MASK_BUBBLE,
            child: 0x02,
            slot_free: false,
        })
        .spawn
    );
    // Shooter: cooldown decay; ready + aimed + slot → fire + reload.
    let cd = shooter_step(ShooterIn {
        cooldown: 3,
        rng: 0,
        facing: 1,
        slot_free: true,
        shottype: 0x04,
    });
    assert_eq!((cd.fire, cd.cooldown), (false, 2));
    let f = shooter_step(ShooterIn {
        cooldown: 0,
        rng: 0x02,
        facing: 1,
        slot_free: true,
        shottype: 0x04,
    });
    assert!(f.fire);
    assert_ne!(f.cooldown, 0);
    // Odd RNG = no aim → hold.
    assert!(
        !shooter_step(ShooterIn {
            cooldown: 0,
            rng: 0x03,
            facing: 1,
            slot_free: true,
            shottype: 0x04,
        })
        .fire
    );
    // Statues: Mau/Ra wake on touch; doors stay solid walls.
    assert!(statue_step(StatueKind::MauLeft, true).wake);
    assert!(!statue_step(StatueKind::MauLeft, false).wake);
    assert!(statue_step(StatueKind::RaRight, true).wake);
    assert!(statue_step(StatueKind::LockedDoor, false).solid);
    assert!(!statue_step(StatueKind::HiddenJar, false).solid);
    assert!(statue_step(StatueKind::Column, true).solid);
}

// ---------------------------------------------------------------------------
// Bank dispatch + id windows + ROM-offset descriptors.
// ---------------------------------------------------------------------------

#[test]
fn bank_dispatch_routes_families() {
    use enemy_ai::*;
    // West (bank 1): shooters, jumpers, generators, statues.
    assert_eq!(bank1_family(0x0A), Family::Shooter);
    assert_eq!(bank1_family(0x14), Family::Jumper);
    assert_eq!(bank1_family(0x16), Family::Generator);
    assert_eq!(bank1_family(0x01), Family::Statue);
    assert_eq!(bank1_family(0x09), Family::Flyer);
    // East (bank 2): Leever/Tektite jump, Lizalfos shoot.
    assert_eq!(bank2_family(0x04), Family::Jumper);
    assert_eq!(bank2_family(0x0A), Family::Shooter);
    assert_eq!(bank2_family(0x16), Family::Generator);
    // Palace A (bank 4): Stalfos walk, bubbles fly, generators gen.
    assert_eq!(bank4_family(0x05), Family::Walker);
    assert_eq!(bank4_family(0x04), Family::Flyer);
    assert_eq!(bank4_family(0x0C), Family::Generator);
    assert_eq!(bank4_family(0x08), Family::Shooter);
    // Palace B / GP (bank 5): Fokka walk, Ra fly, Thunderbird boss-walk.
    assert_eq!(bank5_family(0x06), Family::Walker);
    assert_eq!(bank5_family(0x04), Family::Flyer);
    assert_eq!(bank5_family(0x07), Family::Shooter);
    assert_eq!(bank5_family(0x05), Family::Jumper);
}

#[test]
fn data_windows_and_offsets() {
    use enemy_data::*;
    // Worlds route to banks.
    assert_eq!(ai_bank(World::Overworld), 1);
    assert_eq!(ai_bank(World::EastTowns), 2);
    assert_eq!(ai_bank(World::PalaceA), 4);
    assert_eq!(ai_bank(World::PalaceB), 5);
    // Encounter areas carry 24+ valid ids (24-63 per area);
    // towns only allow NPC codes (narrow window, no RNG encounters).
    for k in [
        AreaKind::WestField,
        AreaKind::EastField,
        AreaKind::PalaceA,
        AreaKind::PalaceB,
        AreaKind::GreatPalace,
    ] {
        let (lo, hi) = valid_id_range(k);
        assert!(hi - lo >= 23, "{k:?} window too narrow");
        assert!(valid_id(k, lo) && valid_id(k, hi));
    }
    let (tlo, thi) = valid_id_range(AreaKind::Town);
    assert!(thi >= tlo && valid_id(AreaKind::Town, tlo));
    // ROM-offset descriptors (never bytes): spot-check addrs.
    assert_eq!(MAP_PTR_SET1.addr, 0x8523);
    assert_eq!(ENEMY_PTR_SET1.addr, 0x85A1);
    assert_eq!(EXP_LO.addr, 0xDDC0);
    assert_eq!(EXP_HI.addr, 0xDDDC);
    assert_eq!(DROP_PROB.addr, 0xE870);
    assert_eq!(HELMET_GOOMA_HP.addr, 0xBC76);
    assert_eq!(THUNDERBIRD_TAB.addr, 0xA34F);
    assert_eq!(STAGED_ENEMY_LIST.bank, None);
    // Runtime loads only.
    assert_eq!(read_table_byte(&[9u8, 8, 7], 2), Some(7));
    assert_eq!(read_table_byte(&[9u8, 8, 7], 3), None);
    // Family coverage across worlds.
    assert_eq!(family_of(World::Overworld, 0x01), Family::Statue);
    assert_eq!(family_of(World::Overworld, 0x14), Family::Jumper);
    assert_eq!(family_of(World::PalaceA, 0x0C), Family::Generator);
    assert_eq!(family_of(World::GreatPalace, 0x07), Family::Shooter);
}
