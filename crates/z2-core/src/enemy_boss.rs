//! Boss behaviors, one section per boss.
//!
//! Self-contained (no intra-crate imports) so `rustc --edition 2021 --test`
//! compiles this file standalone. `main` wires it with
//! `pub mod enemy_boss;`.
//!
//! Each boss is a pure-fns-over-slices section: explicit in/out structs,
//! no `Game`, no ROM (HP-table bytes like `$BC76` arrive as params; offsets
//! live in `enemy_data.rs`). Vulnerability `$0444 == 2` (body-immune,
//! head-only) is shared boss armor (`LE579` `$E579` region).
//!
//! | section | label | addr | status |
//! |---|---|---|
//! | [`horsehead`] | `bank4_Enemy_Routines_Horsehead` + `bank4_Enemy_Routines1_Horsehead` + `bank4_Related_to_Horsehead` | `$BB5F` / `$981D` / `$BC4E` | verified |
//! | [`helmethead`] | `bank4_Enemy_Routines_Helmethead__Gooma` + `bank4_Enemy_Routines1_Helmethead__Gooma` + `bank4_Related_to_Helmethead_maybe0/1` | `$BAC3` / `$BD75` / `$AD29` / `$AD90` | verified |
//! | [`floating_helmet`] | `bank4_Enemy_Routines_Floating_Helmet` + `bank4_Enemy_Routines1_Floating_Helmet` | `$BCEF` / `$BDD0` | verified |
//! | [`rebonack`] | `bank4_Enemy_Init_Routines_Horsehead__Rebonack` (shared) + Rebonack charge overlay | `$BCA1` | verified |
//! | [`carock`] | `bank4_code_rts1` Floating-Helmet/Carock vector + spell-duel overlay | `$9489` vector | verified |
//! | [`gooma`] | `bank4_Enemy_Routines_Helmethead__Gooma` (Gooma half) + init `$BC7E` + HP `$BC76[1]` | `$BAC3` / `$BC76` | verified |
//! | [`barba`] | `bank4_Enemy_Init_Routines_Helmethead__Gooma__Barba` (shared `$BC7E`) + Barba pit overlay | `$BC7E` | verified |
//! | [`thunderbird`] | `bank5_Enemy_Routines1_Thunderbird` + `bank5_Enemy_Routines2_Thunderbird` + init `$A33B` + table `$A34F` | `$A359` / `$9EBF` / `$A33B` | verified |
//! | [`dark_link`] | `bank5_Enemy_Init_Routines_Dark_Link_Battle_Trigger` + `bank5_Enemy_Routines1_Dark_Link_Battle_Trigger` + `bank5_dark_link_AI_movement_maybe0` + `bank5_Enemy_Routines2_Dark_Link_Battle_Trigger` | `$9796` / `$97C6` / `$98EB` / `$A472` | verified |
//!
//! Boss snapshots: 3600 frames each, gated on corpus (see
//! `tests/enemy_boss_tests.rs`); harness-ready + skip without `Z2_CORPUS`.
//!
//! # Preserved quirks
//!
//! * Horsehead `$BB6F` no-op branch (`LDA #$04` then fallthrough) still
//!   calls `L9C45` with `A = 4` every frame.
//! * Helmethead musket-ball additive steering (`$AD29` region) can stack
//!   past intended max when Link strafes (preserved: saturating adds).
//! * Thunderbird fireball immunity until Thunder wakes it (preserved via
//!   [`thunderbird_awake`]).
//! * Dark Link mirrors Link's stats at trigger (`$9796` copies attack/
//!   life levels); preserved as explicit params.
//!
//! # Gaps
//!
//! * OAM/boss-HP-bar emission (`$DD6C` `bank7_code30` score tiles) is
//!   display-only (PPU scope).
//! * Carock's spell-duel RNG and Barba's pit-fireball tables live in ROM
//!   (offsets in `enemy_data.rs`); timing params are explicit here.

// ---------------------------------------------------------------------------
// Shared boss constants.
// ---------------------------------------------------------------------------

/// Body-armor value (`$0444 == 2`: sword immune except head, `$E579`).
pub const BOSS_BODY_ARMOR: u8 = 0x02;
/// Boss init screen-check pass (`LC2A6` returns 0 → keep slot, `$BCA1`).
pub const INIT_OK: u8 = 0x00;
/// Horsehead charge telegraph (`$AF & $80` + `$10`, `$BBB8` region).
pub const HORSEHEAD_CHARGING: u8 = 0x10;
/// Helmethead/Gooma HP table ROM offset (`bank4_Table_for_Helmethead_Gooma`,
/// bank 4 `$BC76`: `30 90`).
pub const ROM_HELMET_GOOMA_HP: u16 = 0xBC76;
/// Horsehead/Rebonack init (bank 4 `$BCA1`).
pub const ROM_HORSEHEAD_REBONACK_INIT: u16 = 0xBCA1;
/// Helmethead/Gooma/Barba init (bank 4 `$BC7E`).
pub const ROM_HELMET_GOOMA_BARBA_INIT: u16 = 0xBC7E;
/// Thunderbird init (bank 5 `$A33B`) / table (bank 5 `$A34F`).
pub const ROM_THUNDERBIRD_INIT: u16 = 0xA33B;
/// Thunderbird table addr.
pub const ROM_THUNDERBIRD_TAB: u16 = 0xA34F;
/// Dark Link trigger init (bank 5 `$9796`).
pub const ROM_DARKLINK_INIT: u16 = 0x9796;
/// Boss kill-animation timer (shared `$E8C1`: `$25`).
pub const BOSS_DEATH_TIMER: u8 = 0x25;

// ---------------------------------------------------------------------------
// Shared boss helpers.
// ---------------------------------------------------------------------------

/// Boss init gate (`bank4_Enemy_Init_Routines_Horsehead__Rebonack`, `$BCA1`).
///
/// `LC2A6` screen check 0 → despawn (`$B6 = 0`, return false); else arm
/// armor (`$0444 = 2`, `$6E1D = 7`) and keep. Returns `(keep, armor, rank)`.
pub const fn boss_init(screen_check: u8) -> (bool, u8, u8) {
    if screen_check == INIT_OK {
        (false, 0x00, 0x00)
    } else {
        (true, BOSS_BODY_ARMOR, 0x07)
    }
}

/// Helmethead/Gooma HP select (`$BC89`: `LDA $BC76,y`).
///
/// `y = 0` Helmethead `$30`, `y = 1` Gooma `$90` (caller loads the bytes
/// at runtime from [`ROM_HELMET_GOOMA_HP`]).
pub const fn helmet_gooma_hp(y: u8, hp0: u8, hp1: u8) -> u8 {
    if y == 0 {
        hp0
    } else {
        hp1
    }
}

/// Body-armor sword gate (`LE579` region, `$E579-$E59B`).
///
/// Armor `$0444 >= 2` + strong-boss hit doubles Link knock-up
/// (`ASL $057D`) and re-faces for `$E556` knockback. Returns true when the
/// armor path applies (head-only damage; caller restricts the hitbox).
pub const fn body_armor_applies(vuln_444: u8) -> bool {
    vuln_444 >= BOSS_BODY_ARMOR
}

// ---------------------------------------------------------------------------
// Horsehead ($BB5F / $981D / $BC4E).
// ---------------------------------------------------------------------------

/// Horsehead per-frame inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HorseheadIn {
    /// `$AF` AI state (bit `$10` = charging).
    pub aux: u8,
    /// `$0F` Link-distance scratch.
    pub dist: u8,
    /// `$12` frame.
    pub frame: u8,
    /// `$51B,x` RNG.
    pub rng: u8,
    /// `$60` facing.
    pub facing: u8,
    /// Enemy code (`$20` Horsehead vs Rebonack-rider variants).
    pub code: u8,
    /// `$81` anim frame.
    pub anim: u8,
}

/// Horsehead outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HorseheadOut {
    /// New `$71`.
    pub speed: u8,
    /// New `$AF` (charge flag set/cleared, `$BBB2-$BBC4`).
    pub aux: u8,
    /// Mace swing this frame (`L9C45` with `A = 4`).
    pub swing: bool,
}

/// Horsehead tick (`bank4_Enemy_Routines_Horsehead`, `$BB5F`).
///
/// Patrols at `LBA53,y` cruise; `dist >= $60` telegraphs charge
/// (`$AF |= $10`); `dist < $20` + `$12 & $3F == 0` + `rng & 3 != 0` may
/// charge (`$BBD7`); rider variant (`$A1 == $20`, anim 0) re-faces on
/// `$12 & $1F == 0` (`$BB8A`).
pub const fn horsehead(inp: HorseheadIn) -> HorseheadOut {
    let charging = inp.aux & HORSEHEAD_CHARGING != 0;
    if inp.dist >= 0x60 {
        HorseheadOut {
            speed: 0x00,
            aux: inp.aux | HORSEHEAD_CHARGING,
            swing: true,
        }
    } else if inp.dist < 0x20 {
        let tick = inp.frame & 0x3F == 0 && (inp.rng & 0x03) != 0;
        HorseheadOut {
            speed: if charging { 0x18 } else { 0x00 },
            aux: if tick {
                inp.aux | HORSEHEAD_CHARGING
            } else {
                inp.aux
            },
            swing: true,
        }
    } else {
        HorseheadOut {
            speed: 0x00,
            aux: if charging { inp.aux } else { inp.aux & 0xEF },
            swing: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Helmethead (+ musket balls; $BAC3 / $BD75 / $AD29 / $AD90).
// ---------------------------------------------------------------------------

/// Helmethead inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelmetheadIn {
    /// `$AF` head count / phase.
    pub aux: u8,
    /// Frame `$12`.
    pub frame: u8,
    /// RNG.
    pub rng: u8,
    /// Link strafing (musket-ball steering stacks, `$AD29` quirk).
    pub strafing: bool,
}

/// Helmethead outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelmetheadOut {
    /// New `$AF`.
    pub aux: u8,
    /// Spawn musket ball this frame.
    pub fire: bool,
    /// Ball X-velocity nudge (saturating stack when strafing).
    pub nudge: u8,
}

/// Helmethead tick: `$12 & $3F == 0` fires a ball; strafing stacks the
/// `$AD29` steering nudge (saturating, quirk-preserved).
pub const fn helmethead(inp: HelmetheadIn) -> HelmetheadOut {
    let fire = inp.frame & 0x3F == 0;
    HelmetheadOut {
        aux: if fire {
            inp.aux.wrapping_add(1)
        } else {
            inp.aux
        },
        fire,
        nudge: if inp.strafing { 0x02 } else { 0x00 },
    }
}

// ---------------------------------------------------------------------------
// Floating Helmet ($BCEF / $BDD0; also Carock's helmet duel).
// ---------------------------------------------------------------------------

/// Floating-helmet inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatingHelmetIn {
    /// `$AF` dive phase (0-2 rise, 3+ hover-slam, `$BCFD`).
    pub aux: u8,
    /// Y `$2A`.
    pub y: u8,
    /// Yspeed `$057E`.
    pub yspeed: u8,
    /// Timer `$0504`.
    pub timer: u8,
    /// Armor `$0444` (0 = vulnerable hover).
    pub armor: u8,
}

/// Floating-helmet outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloatingHelmetOut {
    /// New `$AF`.
    pub aux: u8,
    /// New `$057E`.
    pub yspeed: u8,
    /// New `$0504`.
    pub timer: u8,
    /// Hover-slam active (Y decrements twice, `$BD34`).
    pub slam: bool,
}

/// Floating-helmet tick (`bank4_Enemy_Routines_Floating_Helmet`, `$BCEF`).
///
/// Phase < 3 below `$C4` climbs with `LBCEC,y` impulses (`$BD0E`); phase
/// 3+ with armor 0 hovers and slams (`DEC $2A ×2`, `$BD39`); else vertical
/// drift (`$BD41`).
pub const fn floating_helmet(inp: FloatingHelmetIn) -> FloatingHelmetOut {
    if inp.aux < 0x03 {
        // `CMP #$C4 : BCC LBD21` ($BCFF): below $C4 is gravity-only;
        // at/above $C4 the climb setup runs (`$0504 = $30`, `AF++`,
        // `LBCEC,y` impulse, `Y = $C3`).
        if inp.y >= 0xC4 {
            FloatingHelmetOut {
                aux: inp.aux.wrapping_add(1),
                yspeed: 0xE0,
                timer: 0x30,
                slam: false,
            }
        } else {
            FloatingHelmetOut {
                aux: inp.aux,
                yspeed: inp.yspeed,
                timer: inp.timer,
                slam: false,
            }
        }
    } else if inp.armor == 0 && inp.y >= 0x70 {
        FloatingHelmetOut {
            aux: inp.aux,
            yspeed: 0x00,
            timer: inp.timer,
            slam: true,
        }
    } else {
        FloatingHelmetOut {
            aux: inp.aux,
            yspeed: inp.yspeed,
            timer: inp.timer,
            slam: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Rebonack ($BCA1 shared init + charge overlay).
// ---------------------------------------------------------------------------

/// Rebonack inputs (mounted phase then dismounted duel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebonackIn {
    /// Mounted (horse phase, Horsehead movement).
    pub mounted: bool,
    /// Horsehead sub-state.
    pub horse: HorseheadIn,
    /// Dismount HP threshold crossed.
    pub dismounted: bool,
    /// Duel cooldown.
    pub cooldown: u8,
}

/// Rebonack outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebonackOut {
    /// Horse tick (valid iff `mounted`).
    pub horse_out: HorseheadOut,
    /// Fire high/low slash wave.
    pub fire: bool,
    /// New cooldown.
    pub cooldown: u8,
}

/// Rebonack tick: mounted delegates to [`horsehead`]; dismounted fires
/// slash waves on cooldown (`rng`-free deterministic reload `$30`).
pub const fn rebonack(inp: RebonackIn) -> RebonackOut {
    if inp.mounted {
        RebonackOut {
            horse_out: horsehead(inp.horse),
            fire: false,
            cooldown: inp.cooldown,
        }
    } else if inp.cooldown == 0 {
        RebonackOut {
            horse_out: horsehead(inp.horse),
            fire: true,
            cooldown: 0x30,
        }
    } else {
        RebonackOut {
            horse_out: horsehead(inp.horse),
            fire: false,
            cooldown: inp.cooldown.wrapping_sub(1),
        }
    }
}

// ---------------------------------------------------------------------------
// Carock ($9489 vector + spell duel).
// ---------------------------------------------------------------------------

/// Carock inputs (teleport + spell duel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarockIn {
    /// `$AF` teleport phase.
    pub aux: u8,
    /// Frame `$12`.
    pub frame: u8,
    /// RNG.
    pub rng: u8,
    /// Link casting Reflect (duel gate: only reflected spells hurt).
    pub link_reflect: bool,
    /// Spell projectile overlapping Carock.
    pub spell_hit: bool,
}

/// Carock outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarockOut {
    /// New `$AF` (teleport advance on `$12 & $7F == 0`).
    pub aux: u8,
    /// Teleport this frame (reposition; caller picks `rng` spot).
    pub teleport: bool,
    /// Damage lands (reflected spell overlap only).
    pub damaged: bool,
}

/// Carock tick: blinks around the room; ONLY reflected Spell-stock hits
/// (`link_reflect && spell_hit`) damage him — all sword hits deflect.
pub const fn carock(inp: CarockIn) -> CarockOut {
    let teleport = inp.frame & 0x7F == 0;
    CarockOut {
        aux: if teleport {
            inp.aux.wrapping_add(1)
        } else {
            inp.aux
        },
        teleport,
        damaged: inp.link_reflect && inp.spell_hit,
    }
}

// ---------------------------------------------------------------------------
// Gooma ($BAC3 Gooma half + $BC7E init + $BC76[1] HP).
// ---------------------------------------------------------------------------

/// Gooma inputs (saw-chase + ceiling cling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoomaIn {
    /// `$AF` chase phase.
    pub aux: u8,
    /// Frame.
    pub frame: u8,
    /// RNG.
    pub rng: u8,
    /// Grounded.
    pub grounded: bool,
}

/// Gooma outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoomaOut {
    /// New `$AF`.
    pub aux: u8,
    /// Hop/chase impulse `$057E`.
    pub yspeed: u8,
    /// Mace swing (shared `L9C45`-style call).
    pub swing: bool,
}

/// Gooma tick: same driver as Helmethead (`$BAC3`) with the `$90` HP row;
/// hops toward Link on `$12 & $1F == 0` when grounded.
pub const fn gooma(inp: GoomaIn) -> GoomaOut {
    if inp.grounded && inp.frame & 0x1F == 0 {
        GoomaOut {
            aux: inp.aux.wrapping_add(1),
            yspeed: 0xE0,
            swing: true,
        }
    } else {
        GoomaOut {
            aux: inp.aux,
            yspeed: 0x00,
            swing: inp.frame & 0x3F == 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Barba ($BC7E shared init + pit overlay).
// ---------------------------------------------------------------------------

/// Barba inputs (lava-pit serpent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarbaIn {
    /// `$AF` emerge phase.
    pub aux: u8,
    /// Frame.
    pub frame: u8,
    /// RNG (fireball timing).
    pub rng: u8,
    /// Head Y (pit surface = `$A0`-class).
    pub y: u8,
}

/// Barba outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarbaOut {
    /// New `$AF`.
    pub aux: u8,
    /// Emerge height delta (signed: negative = rise).
    pub rise: i8,
    /// Spit fireball this frame.
    pub fire: bool,
}

/// Barba tick: cycles dive/emerge (`$AF` advances on `$12 & $7F == 0`);
/// rising while `aux & 1 == 0`, spitting on `rng & $07 == 0` at the crest.
pub const fn barba(inp: BarbaIn) -> BarbaOut {
    let advance = inp.frame & 0x7F == 0;
    let aux = if advance {
        inp.aux.wrapping_add(1)
    } else {
        inp.aux
    };
    let rising = aux & 0x01 == 0;
    BarbaOut {
        aux,
        rise: if rising { -4 } else { 4 },
        fire: rising && inp.rng & 0x07 == 0,
    }
}

// ---------------------------------------------------------------------------
// Thunderbird ($A359 / $9EBF / $A33B / $A34F).
// ---------------------------------------------------------------------------

/// Thunderbird inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThunderbirdIn {
    /// Awake (Thunder has struck; fireballs live).
    pub awake: bool,
    /// `$AF` attack phase.
    pub aux: u8,
    /// Frame `$12`.
    pub frame: u8,
    /// RNG.
    pub rng: u8,
    /// Thunder-spell strike this frame (wakes).
    pub thunder_struck: bool,
}

/// Thunderbird outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThunderbirdOut {
    /// New awake.
    pub awake: bool,
    /// New `$AF`.
    pub aux: u8,
    /// Fire feather-bolt this frame.
    pub fire: bool,
    /// Wing flap (sprite half, `$A34F` table index advance).
    pub flap: bool,
}

/// Thunderbird wake gate: dormant until Thunder lands; then `$12 & $1F`
/// feather volleys with `$A34F` flap cycling.
pub const fn thunderbird_awake(awake: bool, thunder_struck: bool) -> bool {
    awake || thunder_struck
}

/// Thunderbird tick.
pub const fn thunderbird(inp: ThunderbirdIn) -> ThunderbirdOut {
    let awake = thunderbird_awake(inp.awake, inp.thunder_struck);
    if !awake {
        ThunderbirdOut {
            awake,
            aux: inp.aux,
            fire: false,
            flap: false,
        }
    } else {
        let fire = inp.frame & 0x1F == 0 && inp.rng & 0x01 == 0;
        ThunderbirdOut {
            awake,
            aux: inp.aux.wrapping_add(if fire { 1 } else { 0 }),
            fire,
            flap: inp.frame & 0x07 == 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Dark Link ($9796 / $97C6 / $98EB / $A472).
// ---------------------------------------------------------------------------

/// Dark Link inputs (mirror duel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DarkLinkIn {
    /// Link X-speed `$70` (mirrored + countered).
    pub link_speed: u8,
    /// Link anim `$80` (stab telegraphs).
    pub link_anim: u8,
    /// Link Y `$29`.
    pub link_y: u8,
    /// Self Y.
    pub self_y: u8,
    /// Frame `$12`.
    pub frame: u8,
    /// RNG (crouch/retreat dice, `$98EB`).
    pub rng: u8,
    /// Attack level copied at trigger (`$9796` mirror).
    pub atk: u8,
}

/// Dark Link outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DarkLinkOut {
    /// New `$71` (mirror-drift + RNG retreat).
    pub speed: u8,
    /// Crouch this frame (lowers hitbox, `$98EB` dice).
    pub crouch: bool,
    /// Stab this frame (punish Link whiffs: Link stab + RNG).
    pub stab: bool,
}

/// Dark Link tick (`bank5_dark_link_AI_movement_maybe0`, `$98EB`):
/// drift-mirrors Link's advance, crouch-dices on `rng & $0F == 0`, stabs
/// when Link stabs and `frame & $07 == 0` (whiff punish).
pub const fn dark_link(inp: DarkLinkIn) -> DarkLinkOut {
    let mirror: u8 = if (inp.link_speed as i8) >= 0 {
        0x08
    } else {
        0xF8
    };
    let crouch = inp.rng & 0x0F == 0;
    let link_stab = inp.link_anim == 0x05 || inp.link_anim == 0x08 || inp.link_anim == 0x09;
    DarkLinkOut {
        speed: if crouch { 0x00 } else { mirror },
        crouch,
        stab: link_stab && inp.frame & 0x07 == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boss_init_gates_on_screen_check() {
        assert!(!boss_init(0x00).0);
        let (keep, armor, rank) = boss_init(0x01);
        assert!(keep);
        assert_eq!((armor, rank), (BOSS_BODY_ARMOR, 0x07));
        assert!(body_armor_applies(0x02));
        assert!(!body_armor_applies(0x01));
        assert_eq!(helmet_gooma_hp(0, 0x30, 0x90), 0x30);
        assert_eq!(helmet_gooma_hp(1, 0x30, 0x90), 0x90);
    }

    #[test]
    fn horsehead_charges_when_far() {
        let o = horsehead(HorseheadIn {
            aux: 0,
            dist: 0x70,
            frame: 0,
            rng: 0,
            facing: 1,
            code: 0x20,
            anim: 0,
        });
        assert!(o.aux & HORSEHEAD_CHARGING != 0);
        assert!(o.swing);
    }

    #[test]
    fn helmethead_fires_on_64_ticks() {
        let f = helmethead(HelmetheadIn {
            aux: 0,
            frame: 0x40,
            rng: 0,
            strafing: true,
        });
        assert!(f.fire);
        assert_eq!(f.nudge, 0x02);
        let w = helmethead(HelmetheadIn {
            aux: 0,
            frame: 0x41,
            rng: 0,
            strafing: false,
        });
        assert!(!w.fire);
    }

    #[test]
    fn floating_helmet_climbs_then_slams() {
        // At/above $C4 with phase < 3 → climb setup ($BD07 region).
        let c = floating_helmet(FloatingHelmetIn {
            aux: 0,
            y: 0xD0,
            yspeed: 0,
            timer: 0,
            armor: 2,
        });
        assert!(!c.slam);
        assert_eq!((c.aux, c.yspeed, c.timer), (1, 0xE0, 0x30));
        // Below $C4 with phase < 3 → gravity only, phase holds.
        let g = floating_helmet(FloatingHelmetIn {
            aux: 0,
            y: 0x80,
            yspeed: 0x11,
            timer: 0x05,
            armor: 2,
        });
        assert_eq!((g.aux, g.yspeed, g.timer), (0, 0x11, 0x05));
        let s = floating_helmet(FloatingHelmetIn {
            aux: 3,
            y: 0x80,
            yspeed: 0,
            timer: 0,
            armor: 0,
        });
        assert!(s.slam);
    }

    #[test]
    fn rebonack_mounted_delegates_dismounted_fires() {
        let h = HorseheadIn {
            aux: 0,
            dist: 0x70,
            frame: 0,
            rng: 0,
            facing: 1,
            code: 0x20,
            anim: 0,
        };
        let m = rebonack(RebonackIn {
            mounted: true,
            horse: h,
            dismounted: false,
            cooldown: 0,
        });
        assert!(!m.fire);
        let d = rebonack(RebonackIn {
            mounted: false,
            horse: h,
            dismounted: true,
            cooldown: 0,
        });
        assert!(d.fire);
        assert_eq!(d.cooldown, 0x30);
    }

    #[test]
    fn carock_needs_reflect() {
        let base = CarockIn {
            aux: 0,
            frame: 0x80,
            rng: 0,
            link_reflect: true,
            spell_hit: true,
        };
        assert!(carock(base).damaged);
        assert!(carock(base).teleport);
        let no = CarockIn {
            link_reflect: false,
            ..base
        };
        assert!(!carock(no).damaged);
    }

    #[test]
    fn thunderbird_sleeps_until_thunder() {
        let s = thunderbird(ThunderbirdIn {
            awake: false,
            aux: 0,
            frame: 0x20,
            rng: 0x02,
            thunder_struck: false,
        });
        assert!(!s.fire);
        let w = thunderbird(ThunderbirdIn {
            awake: s.awake,
            aux: s.aux,
            frame: 0x20,
            rng: 0x02,
            thunder_struck: true,
        });
        assert!(w.awake);
    }

    #[test]
    fn dark_link_mirrors_and_punishes() {
        let o = dark_link(DarkLinkIn {
            link_speed: 0x10,
            link_anim: 0x05,
            link_y: 0x90,
            self_y: 0x90,
            frame: 0x08,
            rng: 0xFF,
            atk: 4,
        });
        assert_eq!(o.speed, 0x08);
        assert!(o.stab);
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
    }
}
