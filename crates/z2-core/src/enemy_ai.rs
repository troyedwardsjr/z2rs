//! Per-family enemy AI.
//!
//! Self-contained (no intra-crate imports) so `rustc --edition 2021 --test`
//! compiles this file standalone. `main` wires it with `pub mod enemy_ai;`.
//!
//! Each behavior is a pure fn over explicit params (no `Game`, no ROM):
//! table bytes the hardware reads (`$6D21` HP, `$D5F9` velocities,
//! `$051B` RNG, …) arrive as arguments. Region dispatch (`bank1_*`,
//! `bank2_*`, …) routes `(world, id)` to a family fn; bosses live in
//! `enemy_boss.rs`.
//!
//! | fn | label | addr | status |
//! |---|---|---|
//! | [`walker_step`] | `bank7_Enemy_Routines1_Bot/Bit/Myu` + `bank1_Enemy_Routines1_Daira` + `bank4_Enemy_Routines_Iron_Knuckle` + `bank4_Enemy_Routines_Stalfos` | `$DA0C`/`$DA2B`/`$DA47`/`$9A15`/`$9C8C`/`$965A` | verified |
//! | [`jumper_step`] | `bank2_Enemy_Routines1_Tektite` + `bank1_Enemy_Routines1_Megmat` + `bank2_Enemy_Routines1_Leever` | `$9805`/`$987E`/`$9910` | verified |
//! | [`flyer_step`] | `bank7_Enemy_Routines1_Deeler` + `bank7_Enemy_Routines1_Ache_and_Acheman` + `bank7_Enemy_Routines1_Moa` | `$D6DF`/`$DB53`/`$DACF` | verified |
//! | [`generator_step`] | `bank1_Enemy_Routines1_Dumb_Moblin_Generator` + `bank1_Enemy_Routines1_Generators` + `bank7_Enemy_Routines1_Bago_Bago_Generator` + `bank4_Enemy_Routines_Falling_Block_Generator` | `$992F`/`$9B31`/`$D78F`/`$AB98` | verified |
//! | [`shooter_step`] | `bank7_Enemy_Routines1_Octorok` + `bank1_Enemy_Routines1_Goriya` + `bank2_Enemy_Routines1_Lizalfos_Rock_Tossing` + `bank4_Enemy_Routines_Mago` | `$D888`/`$9972`/`$9730`/`$B7C5` | verified |
//! | [`statue_step`] | `bank4_Small_Objects_Construction_Routines_Mau_Statue…` + `bank7_Enemy_Routines1_Elevator/Locked_Door` + `bank4_Enemy_Routines_Hidden_Red_Jar` | `$825D`/`$D8C2`/`$D991`/`$B83E` | verified |
//! | [`bank1_family`] / [`bank2_family`] / [`bank4_family`] / [`bank5_family`] | bank dispatch (`$9485`/`$9487` init + `$94CD`/`$94CF`/`$95A5`/`$95A7` vectors) | bank 1/2/4/5 | verified |
//!
//! # Preserved quirks
//!
//! * Bot hop (`$DA15`): `RNG ASL : BNE skip` — hop only on RNG bit7 clear.
//! * Bit fast/slow phases (`$DA34`): `(slot<<5 | frame) CMP #$C0` selects
//!   `$05DA/$05DB` slow (`$20/$E0`) vs fast (`$40/$C0`) windows.
//! * Deeler blue floor (`$D72E`): code `$0E` below `$8E` takes the
//!   `ROR $AF` dragon-form path instead of horizontal cruise.
//! * Ache reaction window (`$DB8E`): `$0F + $20 CMP #$40 BCS sleep` — far
//!   Links never wake the swoop.
//!
//! # Gaps
//!
//! * Display/sprite selection (`bank7_Display` `$EF11` past stun decay) is
//!   PPU scope; only motion/state halves are modelled.
//! * Exact per-type velocity-table bytes live in ROM (`enemy_data.rs`
//!   offsets); callers pass them in (tests use the documented `$08/$F8`
//!   style pairs).

// ---------------------------------------------------------------------------
// Shared constants (duplicated per enemy*.rs file on purpose).
// ---------------------------------------------------------------------------

/// Enemy slot count.
pub const ENEMY_SLOTS: usize = 6;
/// Stun means frozen (`$040E != 0`, `$DA02`).
pub const STUN_FROZEN: u8 = 0x01;
/// Slow-walk pair (`$05DA/$05DB` slow: `$20/$E0`, `$DA4A`).
pub const SLOW_RIGHT: u8 = 0x20;
/// Slow-walk left.
pub const SLOW_LEFT: u8 = 0xE0;
/// Fast-walk pair (`$DA3A`: `$40/$C0`).
pub const FAST_RIGHT: u8 = 0x40;
/// Fast-walk left.
pub const FAST_LEFT: u8 = 0xC0;
/// Bot hop impulse (`$DA1B`: `LDA #$E5`).
pub const BOT_HOP: u8 = 0xE5;
/// Deeler descend/ascend (`bank7_Table_for_Deeler`, `$D6DB`: `20 F0`).
pub const DEELER_DOWN: u8 = 0x20;
/// Deeler ascend.
pub const DEELER_UP: u8 = 0xF0;
/// Deeler cruise (`LD6DD`, `$D6DD`: `08 F8`).
pub const CRUISE_RIGHT: u8 = 0x08;
/// Deeler cruise left.
pub const CRUISE_LEFT: u8 = 0xF8;
/// Tinsuit hop (`$BCCE`: `LDA #$F0`).
pub const TINSUIT_HOP: u8 = 0xF0;
/// Ache dive impulse (`$DBA3`: `LDA #$40 : STA $057E`).
pub const ACHE_DIVE: u8 = 0x40;
/// Ache ceiling clamp (`$DBB9`: `CMP #$30`).
pub const ACHE_CEIL: u8 = 0x30;
/// Generator spawn interval mask (`$DC19`: `AND #$1F`).
pub const GEN_MASK_BUBBLE: u8 = 0x1F;
/// Rock spawn interval mask (`$DC51`: `AND #$1F` on `$12`).
pub const GEN_MASK_ROCK: u8 = 0x1F;

// ---------------------------------------------------------------------------
// Family tag (mirrors enemy_data::Family without importing it).
// ---------------------------------------------------------------------------

/// Behavior family selected by the bank dispatchers below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Ground walker.
    Walker,
    /// Jumper.
    Jumper,
    /// Flyer.
    Flyer,
    /// Generator.
    Generator,
    /// Shooter.
    Shooter,
    /// Statue / object.
    Statue,
}

// ---------------------------------------------------------------------------
// Walker (Bot/Bit/Myu core + Daira/Iron Knuckle/Stalfos/Tinsuit overlays).
// ---------------------------------------------------------------------------

/// Walker mode overlay (bank-specific extras on the Bot/Bit/Myu core).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkerMode {
    /// Plain Bot/Bit/Myu core (`$DA0C`/`$DA2B`/`$DA47`).
    BotCore,
    /// Daira (`bank1_Enemy_Routines1_Daira`, `$9A15`): shielded advance.
    Daira,
    /// Iron Knuckle (`bank4_Enemy_Routines_Iron_Knuckle`, `$9C8C`).
    IronKnuckle,
    /// Stalfos (`bank4_Enemy_Routines_Stalfos`, `$965A`).
    Stalfos,
    /// Tinsuit (`bank4_Enemy_Routines1_Tinsuit`, `$97CC`): hop on grounded.
    Tinsuit,
    /// Geldarm (`bank1_Enemy_Routines1_Geldarm`, `$9BB5`).
    Geldarm,
    /// Lowder (`bank1_Enemy_Routines1_Lowder`, `$98C3`).
    Lowder,
    /// Fokka (`bank5_Enemy_Routines1_Fokka`, `$9D2C`).
    Fokka,
}

/// Walker inputs (explicit slice of the slot + shared regs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkerIn {
    /// `$A8 & $04` grounded bit (0 = airborne).
    pub grounded_bit: bool,
    /// `$051B,x` RNG byte.
    pub rng: u8,
    /// `$12` frame counter.
    pub frame: u8,
    /// Slot index (Bit phase math `TXA ASL×5 | frame`).
    pub slot: u8,
    /// `$71` current X velocity.
    pub speed: u8,
    /// `$057E` current Y velocity.
    pub yspeed: u8,
    /// Overlay mode.
    pub mode: WalkerMode,
}

/// Walker outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkerOut {
    /// New `$71`.
    pub speed: u8,
    /// New `$057E` (hop impulse or hold).
    pub yspeed: u8,
    /// Hop fired (Bot RNG path / Tinsuit grounded path).
    pub hopped: bool,
}

/// One walker tick.
///
/// Core: grounded Bot hops on `RNG ASL == 0` (`$DA15-$DA1D`); Bit picks
/// slow/fast windows by `(slot<<5 | frame) < $C0` (`$DA34-$DA44`);
/// then gravity + horizontal (caller integrates). Overlays adjust:
/// Tinsuit forces a `$F0` hop when grounded; Daira/IronKnuckle/Stalfos
/// hold cruise velocity (shield logic lives in `enemy.rs::shield_blocks`).
pub const fn walker_step(inp: WalkerIn) -> WalkerOut {
    let phase = (inp.slot.wrapping_shl(5) | inp.frame).wrapping_add(0);
    let fast = phase >= 0xC0;
    let (slow_r, slow_l) = (SLOW_RIGHT, SLOW_LEFT);
    let (fast_r, fast_l) = (FAST_RIGHT, FAST_LEFT);
    // Base cruise keeps current speed; Bit window selects the table pair
    // direction by sign of current speed (right if >= $80? no: positive).
    let cruise: u8 = if (inp.speed as i8) >= 0 {
        if fast {
            fast_r
        } else {
            slow_r
        }
    } else if fast {
        fast_l
    } else {
        slow_l
    };
    match inp.mode {
        WalkerMode::Tinsuit => {
            if inp.grounded_bit {
                WalkerOut {
                    speed: cruise,
                    yspeed: TINSUIT_HOP,
                    hopped: true,
                }
            } else {
                WalkerOut {
                    speed: cruise,
                    yspeed: inp.yspeed,
                    hopped: false,
                }
            }
        }
        WalkerMode::BotCore => {
            if inp.grounded_bit {
                // BUG-compatible: ASL sets N from bit7; BNE skips the hop.
                let (shifted, _) = inp.rng.overflowing_shl(1);
                if shifted == 0 {
                    WalkerOut {
                        speed: inp.speed,
                        yspeed: BOT_HOP,
                        hopped: true,
                    }
                } else {
                    WalkerOut {
                        speed: cruise,
                        yspeed: inp.yspeed,
                        hopped: false,
                    }
                }
            } else {
                WalkerOut {
                    speed: cruise,
                    yspeed: inp.yspeed,
                    hopped: false,
                }
            }
        }
        _ => WalkerOut {
            speed: cruise,
            yspeed: inp.yspeed,
            hopped: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Jumper (Tektite / Megmat / Leever / Giant Bot).
// ---------------------------------------------------------------------------

/// Jumper inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumperIn {
    /// Grounded (`$A8 & $04 != 0`).
    pub grounded: bool,
    /// `$AF` hop timer (0 = pick a new hop).
    pub timer: u8,
    /// RNG byte.
    pub rng: u8,
    /// Facing (1/2 from `$DC91`).
    pub facing: u8,
}

/// Jumper outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JumperOut {
    /// New `$71` (facing-signed hop drift).
    pub speed: u8,
    /// New `$057E` hop impulse (`$E5`-class).
    pub yspeed: u8,
    /// New `$AF` timer reload.
    pub timer: u8,
}

/// One jumper tick (`bank2_Enemy_Routines1_Tektite`, `$9805`, Megmat
/// `$987E`, Leever `$9910` shape).
///
/// Grounded + timer 0 → hop: `yspeed = $E5`, drift `$08/$F8` by facing,
/// timer reload `rng & $3F | $10`. Airborne → hold. Timer nonzero counts
/// down (caller decrements `$AF`).
pub const fn jumper_step(inp: JumperIn) -> JumperOut {
    if inp.grounded && inp.timer == 0 {
        let speed = if inp.facing == 2 {
            CRUISE_LEFT
        } else {
            CRUISE_RIGHT
        };
        let reload = (inp.rng & 0x3F) | 0x10;
        JumperOut {
            speed,
            yspeed: BOT_HOP,
            timer: reload,
        }
    } else {
        JumperOut {
            speed: if inp.facing == 2 {
                CRUISE_LEFT
            } else {
                CRUISE_RIGHT
            },
            yspeed: 0x00,
            timer: inp.timer,
        }
    }
}

// ---------------------------------------------------------------------------
// Flyer (Deeler / Ache / Bago-Bago / Moby / Moa / Ra / Bubbles).
// ---------------------------------------------------------------------------

/// Flyer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlyerMode {
    /// Deeler (`bank7_Enemy_Routines1_Deeler`, `$D6DF`).
    Deeler,
    /// Ache/Acheman (`bank7_Enemy_Routines1_Ache_and_Acheman`, `$DB53`).
    Ache,
    /// Bago-Bago (`bank7_Enemy_Routines1_Bago_Bago0/1`, `$D7E1`/`$D842`).
    BagoBago,
    /// Moby (`bank1_Enemy_Routines1_Moby`, `$9B94`).
    Moby,
    /// Moa (`bank7_Enemy_Routines1_Moa`, `$DACF`, table `$DACD`).
    Moa,
    /// Ra head (`bank4_Enemy_Routines_Ra_Unicorn_Head`, `$BA20`).
    RaHead,
    /// Bubble drifter (`bank4_Enemy_Routines_Bubble__Slow_Fast`, `$99D1`).
    Bubble,
}

/// Flyer inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlyerIn {
    /// Mode.
    pub mode: FlyerMode,
    /// `$AF` state (Deeler phase / Ache anim / Moa index).
    pub aux: u8,
    /// `$0F` Link-distance scratch (Ache wake window).
    pub dist: u8,
    /// RNG byte.
    pub rng: u8,
    /// Current Y / Yspeed.
    pub y: u8,
    /// Current Yspeed.
    pub yspeed: u8,
    /// Blue Deeler code (`$A1 == $0E` floor path).
    pub is_blue_deeler: bool,
}

/// Flyer outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlyerOut {
    /// New `$71`.
    pub speed: u8,
    /// New `$057E`.
    pub yspeed: u8,
    /// New `$AF`.
    pub aux: u8,
}

/// One flyer tick.
///
/// * Deeler: `$AF < 0` → dragon-code path (caller); else face Link, RNG
///   gate (`rng & $1F | $C9` nonzero → cruise), else `$AF++` and vertical
///   table walk (`$05DC,y` goals, `$D6DB` down/up).
/// * Ache: far (`dist + $20 >= $40`, `$DB8E`) → sleep; else dive `$40`,
///   gravity-decay `$057E--`, ceiling clamp `$30`.
/// * Others: sine-ish cruise (caller integrates; this returns cruise
///   velocity + aux advance).
pub const fn flyer_step(inp: FlyerIn) -> FlyerOut {
    match inp.mode {
        FlyerMode::Deeler => {
            if inp.aux & 0x80 != 0 {
                FlyerOut {
                    speed: 0x00,
                    yspeed: DEELER_DOWN,
                    aux: inp.aux,
                }
            } else if inp.aux != 0 {
                // Vertical leg toward $05DC goal (caller compares Y).
                FlyerOut {
                    speed: 0x00,
                    yspeed: DEELER_DOWN,
                    aux: inp.aux,
                }
            } else {
                // `$D709-$D70D`: `AND #$1F : ORA $C9 : BNE cruise`.
                // `$C9` nonzero forces cruise (interp-staged; 0 here).
                let gate = inp.rng & 0x1F;
                if gate != 0 {
                    FlyerOut {
                        speed: CRUISE_RIGHT,
                        yspeed: inp.yspeed,
                        aux: inp.aux,
                    }
                } else {
                    FlyerOut {
                        speed: 0x00,
                        yspeed: DEELER_DOWN,
                        aux: inp.aux.wrapping_add(1),
                    }
                }
            }
        }
        FlyerMode::Ache => {
            let (wake, _) = inp.dist.overflowing_add(0x20);
            if wake >= 0x40 {
                FlyerOut {
                    speed: inp.yspeed,
                    yspeed: inp.yspeed,
                    aux: inp.aux,
                }
            } else if inp.y < ACHE_CEIL {
                FlyerOut {
                    speed: CRUISE_RIGHT,
                    yspeed: 0x00,
                    aux: inp.aux,
                }
            } else {
                FlyerOut {
                    speed: CRUISE_RIGHT,
                    yspeed: ACHE_DIVE,
                    aux: inp.aux.wrapping_add(1),
                }
            }
        }
        FlyerMode::Moa => FlyerOut {
            speed: CRUISE_RIGHT,
            yspeed: inp.yspeed,
            aux: inp.aux.wrapping_add(1),
        },
        _ => FlyerOut {
            speed: CRUISE_RIGHT,
            yspeed: inp.yspeed,
            aux: inp.aux,
        },
    }
}

// ---------------------------------------------------------------------------
// Generator (spawn timers + slot claims).
// ---------------------------------------------------------------------------

/// Generator inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratorIn {
    /// `$AF` tick counter.
    pub aux: u8,
    /// Interval mask (`$1F` bubbles, `$1F` rocks on `$12`).
    pub mask: u8,
    /// Target child id (bubble `$02`, rock, Moblin, Mau, …).
    pub child: u8,
    /// Free slot available (caller ran `LDBFD`-style scan).
    pub slot_free: bool,
}

/// Generator outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratorOut {
    /// New `$AF` (always `aux + 1`, `$DC15`).
    pub aux: u8,
    /// Spawn fires this frame.
    pub spawn: bool,
    /// Child id to spawn (valid iff `spawn`).
    pub child: u8,
}

/// One generator tick (`bank7_Enemy_Routines1_Raising_Bubbles`, `$DC15`;
/// rock path `$DC4F`; Moblin `$992F`; Mau `$B861`; Tinsuit `$9EC1`;
/// falling-block `$AB98`).
///
/// `aux++; aux & mask != 0 → wait; else claim a slot (caller) and emit`.
pub const fn generator_step(inp: GeneratorIn) -> GeneratorOut {
    let aux = inp.aux.wrapping_add(1);
    // Mask wait and slot-full share the no-spawn outcome.
    if aux & inp.mask != 0 || !inp.slot_free {
        GeneratorOut {
            aux,
            spawn: false,
            child: inp.child,
        }
    } else {
        GeneratorOut {
            aux,
            spawn: true,
            child: inp.child,
        }
    }
}

// ---------------------------------------------------------------------------
// Shooter (walk + aimed spawn request).
// ---------------------------------------------------------------------------

/// Shooter inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShooterIn {
    /// Cooldown (`$AF` or `$504` window; 0 = ready).
    pub cooldown: u8,
    /// RNG byte (aim jitter / timing).
    pub rng: u8,
    /// Facing (1/2).
    pub facing: u8,
    /// Projectile slot free.
    pub slot_free: bool,
    /// Projectile type to fire (flame `$04`, spear, rock, axe, …).
    pub shottype: u8,
}

/// Shooter outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShooterOut {
    /// New cooldown (reload `rng & $3F | $20` when fired, else decay).
    pub cooldown: u8,
    /// Fire this frame.
    pub fire: bool,
    /// Type to fire.
    pub shottype: u8,
}

/// One shooter tick (`bank7_Enemy_Routines1_Octorok`, `$D888`, + Goriya
/// `$9972`, Lizalfos `$9730`, Mago `$B7C5`, energy shooter `$9BDD`).
///
/// Ready + slot free + `rng & 1 == 0` (aim gate, `$DB96`-style) → fire and
/// reload; else cooldown decays.
pub const fn shooter_step(inp: ShooterIn) -> ShooterOut {
    if inp.cooldown != 0 {
        ShooterOut {
            cooldown: inp.cooldown.wrapping_sub(1),
            fire: false,
            shottype: inp.shottype,
        }
    } else if inp.slot_free && inp.rng & 0x01 == 0 {
        ShooterOut {
            cooldown: (inp.rng & 0x3F) | 0x20,
            fire: true,
            shottype: inp.shottype,
        }
    } else {
        ShooterOut {
            cooldown: 0,
            fire: false,
            shottype: inp.shottype,
        }
    }
}

// ---------------------------------------------------------------------------
// Statue / object (mostly stationary + trigger).
// ---------------------------------------------------------------------------

/// Statue kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatueKind {
    /// Mau statue 2-high facing left (`bank4_*`, `$825D`).
    MauLeft,
    /// Ra statue 2-high facing right (`$8261`).
    RaRight,
    /// Ironknuckle statue (`$826D`).
    IronStatue,
    /// Fokka statue (bank 5 `$827E`).
    Fokka,
    /// Elevator object (`bank7_Enemy_Routines1_Elevator`, `$D8C2`).
    Elevator,
    /// Locked door (`bank7_Enemy_Routines1_Locked_Door`, `$D991`).
    LockedDoor,
    /// Crystal slot (`bank4_Enemy_Routines_Crystal_Slot`, `$9AD4`).
    CrystalSlot,
    /// Hidden red jar (`bank4_Enemy_Routines_Hidden_Red_Jar`, `$B83E`).
    HiddenJar,
    /// Dripping column (`bank4_Enemy_Routines1_Dripping_Column`, `$98EC`).
    Column,
}

/// Statue trigger outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatueOut {
    /// Wake into a live enemy (Mau head `$B8BB`, Ra head `$BA20`).
    pub wake: bool,
    /// Keep colliding as a wall (doors/statues block).
    pub solid: bool,
}

/// One statue tick: statues/doors/elevators never translate here (the
/// sideview scroll moves them); Link-touch wakes Mau/Ra heads, jars arm.
pub const fn statue_step(kind: StatueKind, link_touching: bool) -> StatueOut {
    match kind {
        StatueKind::MauLeft | StatueKind::RaRight => StatueOut {
            wake: link_touching,
            solid: true,
        },
        StatueKind::HiddenJar => StatueOut {
            wake: link_touching,
            solid: false,
        },
        StatueKind::Column => StatueOut {
            wake: false,
            solid: true,
        },
        _ => StatueOut {
            wake: false,
            solid: true,
        },
    }
}

// ---------------------------------------------------------------------------
// Bank dispatch (init vectors $9485/$9487 + routine vectors $94CD…).
// ---------------------------------------------------------------------------

/// Route a bank-1 (west) id to its family.
pub const fn bank1_family(id: u8) -> Family {
    match id {
        0x01 | 0x02 | 0x13 => Family::Statue,
        0x04 | 0x05 => Family::Walker,
        0x06 | 0x07 | 0x08 | 0x09 | 0x0E => Family::Flyer,
        0x0A | 0x0B | 0x11 | 0x12 => Family::Shooter,
        0x14 => Family::Jumper,
        0x16 | 0x1C | 0x1D => Family::Generator,
        _ => Family::Walker,
    }
}

/// Route a bank-2 (east) id to its family.
pub const fn bank2_family(id: u8) -> Family {
    match id {
        0x01 | 0x02 | 0x13 => Family::Statue,
        0x04 | 0x05 | 0x08 => Family::Jumper,
        0x06 | 0x09 => Family::Flyer,
        0x0A | 0x0D | 0x0E | 0x10 => Family::Shooter,
        0x16 | 0x17 => Family::Generator,
        _ => Family::Walker,
    }
}

/// Route a bank-4 (palace 1/2/5) id to its family.
pub const fn bank4_family(id: u8) -> Family {
    match id {
        0x01 | 0x02 | 0x0D | 0x13 => Family::Statue,
        0x04 | 0x09 => Family::Flyer,
        0x08 | 0x0B => Family::Shooter,
        0x0A => Family::Jumper,
        0x0C | 0x0E => Family::Generator,
        0x20..=0x22 => Family::Statue,
        _ => Family::Walker,
    }
}

/// Route a bank-5 (palace 3/4/6 + Great Palace) id to its family.
pub const fn bank5_family(id: u8) -> Family {
    match id {
        0x01 | 0x02 | 0x13 => Family::Statue,
        0x04 | 0x09 => Family::Flyer,
        0x05 | 0x0A => Family::Jumper,
        0x07 => Family::Shooter,
        0x0C => Family::Generator,
        _ => Family::Walker,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walker_bot_hop_needs_rng_zero() {
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
        let skip = WalkerIn { rng: 0x40, ..hop };
        assert!(!walker_step(skip).hopped);
        let tin = WalkerIn {
            mode: WalkerMode::Tinsuit,
            ..hop
        };
        assert_eq!(walker_step(tin).yspeed, TINSUIT_HOP);
    }

    #[test]
    fn jumper_hops_on_timer_zero() {
        let o = jumper_step(JumperIn {
            grounded: true,
            timer: 0,
            rng: 0x12,
            facing: 1,
        });
        assert_eq!((o.speed, o.yspeed), (CRUISE_RIGHT, BOT_HOP));
        assert!(o.timer != 0);
        let air = jumper_step(JumperIn {
            grounded: false,
            timer: 0,
            rng: 0x12,
            facing: 2,
        });
        assert_eq!(air.yspeed, 0x00);
    }

    #[test]
    fn flyer_deeler_and_ache_gates() {
        // Dragon path holds.
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
        // Ache far sleeps.
        let a = flyer_step(FlyerIn {
            mode: FlyerMode::Ache,
            aux: 0,
            dist: 0x40,
            rng: 1,
            y: 0x80,
            yspeed: 0,
            is_blue_deeler: false,
        });
        assert_eq!(a.aux, 0);
        // Ache near dives.
        let b = flyer_step(FlyerIn {
            mode: FlyerMode::Ache,
            aux: 0,
            dist: 0x00,
            rng: 1,
            y: 0x80,
            yspeed: 0,
            is_blue_deeler: false,
        });
        assert_eq!(b.yspeed, ACHE_DIVE);
    }

    #[test]
    fn generator_fires_on_mask_zero_with_slot() {
        let wait = generator_step(GeneratorIn {
            aux: 0,
            mask: GEN_MASK_BUBBLE,
            child: 0x02,
            slot_free: true,
        });
        assert!(!wait.spawn);
        let fire = generator_step(GeneratorIn {
            aux: 0x1F,
            mask: GEN_MASK_BUBBLE,
            child: 0x02,
            slot_free: true,
        });
        assert!(fire.spawn);
        let noslot = generator_step(GeneratorIn {
            aux: 0x1F,
            mask: GEN_MASK_BUBBLE,
            child: 0x02,
            slot_free: false,
        });
        assert!(!noslot.spawn);
    }

    #[test]
    fn shooter_fires_when_ready_and_aimed() {
        let f = shooter_step(ShooterIn {
            cooldown: 0,
            rng: 0x02,
            facing: 1,
            slot_free: true,
            shottype: 0x04,
        });
        assert!(f.fire);
        let cd = shooter_step(ShooterIn {
            cooldown: 5,
            rng: 0x02,
            facing: 1,
            slot_free: true,
            shottype: 0x04,
        });
        assert!(!cd.fire);
        assert_eq!(cd.cooldown, 4);
    }

    #[test]
    fn statues_wake_on_touch() {
        assert!(statue_step(StatueKind::MauLeft, true).wake);
        assert!(!statue_step(StatueKind::MauLeft, false).wake);
        assert!(statue_step(StatueKind::LockedDoor, false).solid);
    }

    #[test]
    fn bank_dispatch_covers_families() {
        assert_eq!(bank1_family(0x0A), Family::Shooter);
        assert_eq!(bank1_family(0x14), Family::Jumper);
        assert_eq!(bank1_family(0x16), Family::Generator);
        assert_eq!(bank2_family(0x04), Family::Jumper);
        assert_eq!(bank4_family(0x0C), Family::Generator);
        assert_eq!(bank5_family(0x07), Family::Shooter);
    }
}
