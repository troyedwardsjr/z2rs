//! Boss verification: one synthetic test per boss + gated 3600-frame
//! snapshots.
//!
//! Boss snapshots are gated on corpus (`Z2_CORPUS`) — harness-ready + skip
//! without it. ROM-gated oracle diffs skip gracefully without `Z2_ROM`.

#[allow(dead_code)]
#[path = "../src/enemy_boss.rs"]
mod enemy_boss;

mod common;

use enemy_boss::*;

// ---------------------------------------------------------------------------
// 1. Horsehead ($BB5F / $981D / $BC4E).
// ---------------------------------------------------------------------------

#[test]
fn boss_horsehead_charges_and_swings() {
    let base = HorseheadIn {
        aux: 0,
        dist: 0x70,
        frame: 0x10,
        rng: 0x02,
        facing: 1,
        code: 0x20,
        anim: 0,
    };
    // Far → telegraph charge ($AF |= $10) and swing (L9C45 A=4 quirk).
    let o = horsehead(base);
    assert!(o.aux & HORSEHEAD_CHARGING != 0);
    assert!(o.swing);
    // Near + RNG gate → may hold or charge; swing always.
    let near = horsehead(HorseheadIn { dist: 0x10, ..base });
    assert!(near.swing);
    // Mid range clears a stale charge flag.
    let mid = horsehead(HorseheadIn {
        dist: 0x40,
        aux: 0,
        ..base
    });
    assert!(mid.aux & HORSEHEAD_CHARGING == 0);
    // Init gate ($BCA1): screen-check 0 despawns, else armor 2 + rank 7.
    assert!(!boss_init(0x00).0);
    assert_eq!(boss_init(0x01), (true, BOSS_BODY_ARMOR, 0x07));
}

// ---------------------------------------------------------------------------
// 2. Helmethead + head ($BAC3 / $BD75 / $AD29 / $AD90).
// ---------------------------------------------------------------------------

#[test]
fn boss_helmethead_fires_balls() {
    // $12 & $3F == 0 fires a musket ball.
    let f = helmethead(HelmetheadIn {
        aux: 0x02,
        frame: 0x40,
        rng: 0x00,
        strafing: false,
    });
    assert!(f.fire);
    assert_eq!(f.aux, 0x03);
    // Off-tick holds; strafing stacks the $AD29 steering nudge (quirk).
    let w = helmethead(HelmetheadIn {
        aux: 0x02,
        frame: 0x41,
        rng: 0x00,
        strafing: true,
    });
    assert!(!w.fire);
    assert_eq!(w.nudge, 0x02);
    // HP rows ($BC76 `30 90`): y=0 Helmethead, y=1 Gooma.
    assert_eq!(helmet_gooma_hp(0, 0x30, 0x90), 0x30);
    assert_eq!(helmet_gooma_hp(1, 0x30, 0x90), 0x90);
    // Body armor ($0444 == 2) forces head-only hits.
    assert!(body_armor_applies(0x02));
    assert!(!body_armor_applies(0x00));
}

// ---------------------------------------------------------------------------
// 3. Rebonack ($BCA1 shared init + charge overlay).
// ---------------------------------------------------------------------------

#[test]
fn boss_rebonack_mounted_then_duels() {
    let horse = HorseheadIn {
        aux: 0,
        dist: 0x70,
        frame: 0,
        rng: 0,
        facing: 1,
        code: 0x20,
        anim: 0,
    };
    // Mounted: horse driver runs, no waves.
    let m = rebonack(RebonackIn {
        mounted: true,
        horse,
        dismounted: false,
        cooldown: 0,
    });
    assert!(!m.fire);
    assert!(m.horse_out.swing);
    // Dismounted, cooldown 0 → slash wave + reload $30.
    let d = rebonack(RebonackIn {
        mounted: false,
        horse,
        dismounted: true,
        cooldown: 0,
    });
    assert!(d.fire);
    assert_eq!(d.cooldown, 0x30);
    // Cooldown decays.
    let w = rebonack(RebonackIn {
        mounted: false,
        horse,
        dismounted: true,
        cooldown: 0x10,
    });
    assert!(!w.fire);
    assert_eq!(w.cooldown, 0x0F);
}

// ---------------------------------------------------------------------------
// 4. Carock ($9489 vector + spell duel).
// ---------------------------------------------------------------------------

#[test]
fn boss_carock_only_reflect_hurts() {
    let base = CarockIn {
        aux: 0x01,
        frame: 0x80,
        rng: 0x55,
        link_reflect: true,
        spell_hit: true,
    };
    let o = carock(base);
    // Teleport tick ($12 & $7F == 0) advances phase.
    assert!(o.teleport);
    assert_eq!(o.aux, 0x02);
    // Reflected-spell overlap is the ONLY damage path.
    assert!(o.damaged);
    assert!(
        !carock(CarockIn {
            link_reflect: false,
            ..base
        })
        .damaged
    );
    assert!(
        !carock(CarockIn {
            spell_hit: false,
            ..base
        })
        .damaged
    );
    // Off-tick: no teleport, phase holds.
    let w = carock(CarockIn {
        frame: 0x81,
        ..base
    });
    assert!(!w.teleport);
    assert_eq!(w.aux, 0x01);
}

// ---------------------------------------------------------------------------
// 5. Gooma ($BAC3 Gooma half + $BC7E init + $BC76[1] HP).
// ---------------------------------------------------------------------------

#[test]
fn boss_gooma_hops_and_swings() {
    // Grounded on a $1F tick → chase hop $E0.
    let h = gooma(GoomaIn {
        aux: 0x00,
        frame: 0x20,
        rng: 0x00,
        grounded: true,
    });
    assert_eq!(h.yspeed, 0xE0);
    assert!(h.swing);
    // Airborne: no hop impulse; swing only on $3F ticks.
    let a = gooma(GoomaIn {
        aux: 0x00,
        frame: 0x21,
        rng: 0x00,
        grounded: false,
    });
    assert_eq!(a.yspeed, 0x00);
    assert!(!a.swing);
}

// ---------------------------------------------------------------------------
// 6. Barba ($BC7E shared init + pit overlay).
// ---------------------------------------------------------------------------

#[test]
fn boss_barba_emerges_and_spits() {
    // Rising half (aux even) moves up; crest + rng&7==0 spits.
    let r = barba(BarbaIn {
        aux: 0x00,
        frame: 0x01,
        rng: 0x00,
        y: 0xA0,
    });
    assert_eq!(r.rise, -4);
    assert!(r.fire);
    // Diving half (aux odd) moves down, never spits.
    let d = barba(BarbaIn {
        aux: 0x01,
        frame: 0x01,
        rng: 0x00,
        y: 0xA0,
    });
    assert_eq!(d.rise, 4);
    assert!(!d.fire);
    // Phase advances on $12 & $7F == 0.
    let adv = barba(BarbaIn {
        aux: 0x00,
        frame: 0x80,
        rng: 0xFF,
        y: 0xA0,
    });
    assert_eq!(adv.aux, 0x01);
}

// ---------------------------------------------------------------------------
// 7. Thunderbird ($A359 / $9EBF / $A33B / $A34F).
// ---------------------------------------------------------------------------

#[test]
fn boss_thunderbird_sleeps_until_thunder() {
    // Dormant: no fire, no flap.
    let s = thunderbird(ThunderbirdIn {
        awake: false,
        aux: 0,
        frame: 0x20,
        rng: 0x02,
        thunder_struck: false,
    });
    assert!(!s.awake && !s.fire && !s.flap);
    // Thunder strike wakes (sticky).
    let w = thunderbird(ThunderbirdIn {
        awake: s.awake,
        aux: s.aux,
        frame: 0x20,
        rng: 0x02,
        thunder_struck: true,
    });
    assert!(w.awake);
    // Awake volley: $12 & $1F == 0 + rng-even fires, aux advances.
    assert!(w.fire);
    assert_eq!(w.aux, 0x01);
    // Wake gate is sticky without a new strike.
    assert!(thunderbird_awake(true, false));
    assert!(thunderbird_awake(false, true));
    assert!(!thunderbird_awake(false, false));
}

// ---------------------------------------------------------------------------
// 8. Dark Link ($9796 / $97C6 / $98EB / $A472) + floating helmet ($BCEF).
// ---------------------------------------------------------------------------

#[test]
fn boss_dark_link_mirrors_and_punishes() {
    // Mirror-drift follows Link's advance direction.
    let r = dark_link(DarkLinkIn {
        link_speed: 0x10,
        link_anim: 0x00,
        link_y: 0x90,
        self_y: 0x90,
        frame: 0x01,
        rng: 0xFF,
        atk: 4,
    });
    assert_eq!(r.speed, 0x08);
    let l = dark_link(DarkLinkIn {
        link_speed: 0xF0,
        link_anim: 0x00,
        link_y: 0x90,
        self_y: 0x90,
        frame: 0x01,
        rng: 0xFF,
        atk: 4,
    });
    assert_eq!(l.speed, 0xF8);
    // Whiff punish: Link stab + frame&7==0 → stab back.
    let p = dark_link(DarkLinkIn {
        link_speed: 0x00,
        link_anim: 0x05,
        link_y: 0x90,
        self_y: 0x90,
        frame: 0x08,
        rng: 0xFF,
        atk: 4,
    });
    assert!(p.stab);
    // Crouch dice: rng&$0F==0 ducks (speed 0).
    let c = dark_link(DarkLinkIn {
        link_speed: 0x10,
        link_anim: 0x00,
        link_y: 0x90,
        self_y: 0x90,
        frame: 0x01,
        rng: 0x00,
        atk: 4,
    });
    assert!(c.crouch);
    assert_eq!(c.speed, 0x00);
}

#[test]
fn boss_floating_helmet_climbs_then_slams() {
    // Phase < 3 at/above $C4 climbs ($BD07 setup: impulse $E0 + timer $30).
    let c = floating_helmet(FloatingHelmetIn {
        aux: 0,
        y: 0xD0,
        yspeed: 0,
        timer: 0,
        armor: 2,
    });
    assert!(!c.slam);
    assert_eq!((c.aux, c.yspeed, c.timer), (1, 0xE0, 0x30));
    // Phase 3+, armor 0, at/above $70 → hover-slam (Y-- twice).
    let s = floating_helmet(FloatingHelmetIn {
        aux: 3,
        y: 0x80,
        yspeed: 0,
        timer: 0,
        armor: 0,
    });
    assert!(s.slam);
}

// ---------------------------------------------------------------------------
// Gated: boss snapshots, 3600 frames each (harness-ready + skip).
// ---------------------------------------------------------------------------

#[test]
fn boss_snapshots_3600_frames_when_present() {
    let Some(rd) = common::corpus_snapshots("enemy boss corpus snapshots") else {
        return;
    };
    let snaps: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("boss-"))
        })
        .collect();
    if snaps.is_empty() {
        eprintln!("SKIP: no boss-* snapshots (need 8 x 3600-frame boss snapshots)");
        return;
    }
    // Harness-ready: each snapshot must carry the 8-byte magic + a full
    // 3600-frame enemy/boss state log once the corpus lands.
    for p in snaps {
        let b = std::fs::read(&p).expect("read snapshot");
        assert!(b.starts_with(b"Z2SNAP01"), "{}: bad magic", p.display());
        assert!(
            b.len() >= 8 + 3600,
            "{}: too small for 3600 frames",
            p.display()
        );
    }
}
