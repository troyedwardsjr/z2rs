//! Cheats (group `C`).
//!
//! Options ([`CheatOpts`]) and their ZALiA reference behaviour (ZALiA keeps
//! these behind DEV TOOLS: the `I` invulnerability cycle and the
//! stab-to-cheat item grants):
//!
//! * `invincible`: Link takes no lasting damage and does not die from it.
//!   After each frame the life meter `$0774` is refilled (`LCB18`: heart
//!   containers x 32 - 1) and the kill flag `$0494` is cleared before the
//!   next frame's death gate (`bank7_check_if_link_died_0494__linkdeath`,
//!   `$D3CC`, runs first in the side-view main loop and also waits for the
//!   injured timer `$050C`). That covers enemy and projectile damage
//!   (`$E349`: meter underflow -> `INC $0494`). Contact knockback still
//!   happens (the hit is not suppressed, only its cost), so Moa XP steal
//!   still applies. **Pits and lava still kill**: they run a fall routine
//!   (bank 0 `$918E`) that sinks Link below the pit line (`$29 >= $D0`)
//!   before setting the kill flag, and clearing it there would strand him
//!   off-screen, so kills with Link below that line are left alone.
//! * `infinite_magic`: MP `$0773` refilled to the magic-container maximum
//!   after every frame.
//! * `infinite_lives`: lives `$0700` held at no less than 3 (the starting
//!   count), so a death never reaches game over.
//! * `max_stats`: attack/magic/life `$0777-$0779` = 8, containers
//!   `$0783/$0784` = 8 (plus the seven-magic-containers bit `$079D & $08`),
//!   "next level" `$0770/$0771` recomputed from the level table.
//!
//! All four are end-of-frame RAM writes: no trap, no PRG patch. They run on
//! every frame while on (title screens included; a file load from SRAM
//! simply overwrites them and the next frame writes them again).
//!
//! Cheats change gameplay, so they are part of the netplay identity like
//! every other group.

use serde::{Deserialize, Serialize};

use crate::game::Game;

/// Cheat options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CheatOpts {
    /// No damage.
    pub invincible: bool,
    /// MP never runs out.
    pub infinite_magic: bool,
    /// Lives never run out.
    pub infinite_lives: bool,
    /// Max levels and containers.
    pub max_stats: bool,
}

impl CheatOpts {
    /// Whether anything in this group is on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        for b in [
            self.invincible,
            self.infinite_magic,
            self.infinite_lives,
            self.max_stats,
        ] {
            out.push(u8::from(b));
        }
    }
}

/// Lives (`$0700`).
const ADDR_LIVES: usize = 0x0700;
/// Link Y (`$29`).
const ADDR_LINK_Y: usize = 0x0029;
/// Kill flag (`$0494`).
const ADDR_KILL: usize = 0x0494;
/// Current MP / HP.
const ADDR_MP: usize = 0x0773;
const ADDR_HP: usize = 0x0774;
/// Levels (`$0777-$0779`).
const ADDR_LEVELS: usize = 0x0777;
/// Magic / heart containers.
const ADDR_MAG_CTR: usize = 0x0783;
const ADDR_HEART_CTR: usize = 0x0784;
/// Seven-magic-containers flag (`$079D & $08`).
const ADDR_SEVEN: usize = 0x079D;

/// Pit line (bank 0 `$918E` fall routine: `LDA $29 : CMP #$D0 : BCC` ->
/// `INC $0494`).
pub const PIT_Y: u8 = 0xD0;
/// Lives floor for `infinite_lives`.
pub const LIVES_FLOOR: u8 = 3;
/// Level and container count for `max_stats`.
pub const MAX_STAT: u8 = 8;

/// Install this group's hooks ([`super::apply`], only when active). The
/// cheats are pure end-of-frame RAM writes, so nothing is hooked.
pub(crate) fn register(game: &mut Game, opts: &CheatOpts) {
    let _ = (game, opts);
}

/// Per-frame work after [`Game::step`] (only while any enhancement is on).
pub(crate) fn end_of_frame(game: &mut Game, opts: &CheatOpts) {
    if opts.max_stats {
        let ram = &mut game.ram;
        let before = [ram[ADDR_LEVELS], ram[ADDR_LEVELS + 1], ram[ADDR_LEVELS + 2]];
        ram[ADDR_LEVELS..ADDR_LEVELS + 3].fill(MAX_STAT);
        ram[ADDR_MAG_CTR] = MAX_STAT;
        ram[ADDR_HEART_CTR] = MAX_STAT;
        ram[ADDR_SEVEN] |= 0x08;
        if before != [MAX_STAT; 3] {
            super::rando::recompute_exp_next(game);
        }
    }
    let ram = &mut game.ram;
    if opts.infinite_magic {
        ram[ADDR_MP] = crate::save::refill_meter(ram[ADDR_MAG_CTR]);
    }
    if opts.invincible {
        ram[ADDR_HP] = crate::save::refill_meter(ram[ADDR_HEART_CTR]);
        // A kill with Link below the pit line comes from the pit / lava
        // fall routine (bank 0 `$918E`), which keeps sinking him: let it
        // finish rather than strand him off-screen.
        if ram[ADDR_KILL] != 0 && ram[ADDR_LINK_Y] < PIT_Y {
            ram[ADDR_KILL] = 0;
        }
    }
    if opts.infinite_lives && ram[ADDR_LIVES] < LIVES_FLOOR {
        ram[ADDR_LIVES] = LIVES_FLOOR;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cheats_hold_meters_lives_and_stats() {
        let mut g = Game::new();
        g.ram[ADDR_MAG_CTR] = 5;
        g.ram[ADDR_HEART_CTR] = 6;
        g.ram[ADDR_MP] = 3;
        g.ram[ADDR_HP] = 1;
        g.ram[ADDR_LIVES] = 1;
        g.ram[ADDR_KILL] = 1;
        g.ram[ADDR_LINK_Y] = 0x80;
        let o = CheatOpts {
            invincible: true,
            infinite_magic: true,
            infinite_lives: true,
            max_stats: false,
        };
        end_of_frame(&mut g, &o);
        assert_eq!(g.ram[ADDR_MP], 5 * 32 - 1);
        assert_eq!(g.ram[ADDR_HP], 6 * 32 - 1);
        assert_eq!(g.ram[ADDR_LIVES], 3);
        assert_eq!(g.ram[ADDR_KILL], 0);
        // A pit fall (Link below the pit line) is left to kill.
        g.ram[ADDR_KILL] = 1;
        g.ram[ADDR_LINK_Y] = 0xE0;
        end_of_frame(&mut g, &o);
        assert_eq!(g.ram[ADDR_KILL], 1);
        // More lives than the floor are kept.
        g.ram[ADDR_LIVES] = 7;
        end_of_frame(&mut g, &o);
        assert_eq!(g.ram[ADDR_LIVES], 7);
        let m = CheatOpts {
            max_stats: true,
            ..CheatOpts::default()
        };
        end_of_frame(&mut g, &m);
        assert_eq!(&g.ram[ADDR_LEVELS..ADDR_LEVELS + 3], &[8, 8, 8]);
        assert_eq!((g.ram[ADDR_MAG_CTR], g.ram[ADDR_HEART_CTR]), (8, 8));
        assert_eq!(g.ram[ADDR_SEVEN] & 0x08, 0x08);
    }

    #[test]
    fn off_writes_nothing() {
        let mut g = Game::new();
        g.ram[ADDR_HP] = 1;
        g.ram[ADDR_KILL] = 1;
        let before = g.ram;
        end_of_frame(&mut g, &CheatOpts::default());
        assert_eq!(g.ram, before);
    }
}
