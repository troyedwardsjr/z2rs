//! Enemy facts: ID groups per sideview bank, and where the per-enemy tables
//! and boss values live.
//!
//! The IDs and addresses are facts about the game. Each group's tables sit
//! in its own bank; the game copies the active group's tables into WRAM
//! (`$6D21` hit points, `$6DD5`/`$6DF9` attributes) when an area loads, so
//! editing the ROM tables in place is all that is needed.

use crate::rom::Rom;
use crate::RandoError;

/// One of the five sideview enemy groups (each has its own ID space and its
/// own hit point and attribute tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    /// West Hyrule and Death Mountain (bank 1).
    West,
    /// East Hyrule and Maze Island (bank 2).
    East,
    /// Palaces 1, 2 and 5 (bank 4).
    Palace125,
    /// Palaces 3, 4 and 6 (bank 4).
    Palace346,
    /// The Great Palace (bank 5).
    GreatPalace,
}

impl Group {
    /// All groups in table order.
    pub const ALL: [Group; 5] = [
        Group::West,
        Group::East,
        Group::Palace125,
        Group::Palace346,
        Group::GreatPalace,
    ];

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Group::West => "West",
            Group::East => "East",
            Group::Palace125 => "Palaces 1/2/5",
            Group::Palace346 => "Palaces 3/4/6",
            Group::GreatPalace => "Great Palace",
        }
    }

    /// PRG bank holding the group's tables.
    #[must_use]
    pub fn bank(self) -> u8 {
        match self {
            Group::West => 1,
            Group::East => 2,
            Group::Palace125 | Group::Palace346 => 4,
            Group::GreatPalace => 5,
        }
    }

    /// CPU address of the 36-entry hit point table.
    #[must_use]
    pub fn hp_table(self) -> u16 {
        match self {
            Group::Palace346 => 0xA921,
            _ => 0x9421,
        }
    }

    /// CPU address of the first attribute row (36 entries: bit 5 sword
    /// immune, bit 4 steals experience, bits 0-3 experience index). The
    /// second row (bit 5 fire/projectile immune, bits 0-3 damage class)
    /// follows at `+ TABLE_LEN`.
    #[must_use]
    pub fn attr_table(self) -> u16 {
        self.hp_table() + 0xB4
    }

    /// The group's shuffle categories.
    #[must_use]
    pub fn sets(self) -> &'static EnemySets {
        match self {
            Group::West => &WEST,
            Group::East => &EAST,
            Group::Palace125 => &PALACE125,
            Group::Palace346 => &PALACE346,
            Group::GreatPalace => &GREAT_PALACE,
        }
    }

    /// IDs whose regular hit points are scaled.
    #[must_use]
    pub fn hp_ids(self) -> Vec<u8> {
        match self {
            Group::West => (0x03..0x23).collect(),
            Group::East => (0x03..0x1E).collect(),
            Group::Palace125 | Group::Palace346 => {
                let mut v = vec![0x03, 0x04];
                v.extend(0x06..0x20);
                v.push(0x23);
                v
            }
            Group::GreatPalace => {
                let mut v = vec![0x03, 0x04];
                v.extend(0x06..0x0B);
                v.extend(0x0D..0x13);
                v.extend(0x14..0x1B);
                v.push(0x1D);
                v
            }
        }
    }
}

/// Entries per enemy table (IDs `0x00..0x24`).
pub const TABLE_LEN: u16 = 0x24;

/// Shuffle categories of one group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemySets {
    /// Small ground enemies.
    pub small: &'static [u8],
    /// Large ground enemies.
    pub large: &'static [u8],
    /// Flying enemies (the Great Palace's King Bot is listed here on
    /// purpose: it is placed like a flyer).
    pub flying: &'static [u8],
    /// Enemy generators.
    pub generators: &'static [u8],
}

impl EnemySets {
    /// Every listed ID: ground (small then large), flying, generators.
    #[must_use]
    pub fn all(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(self.small);
        v.extend_from_slice(self.large);
        v.extend_from_slice(self.flying);
        v.extend_from_slice(self.generators);
        v
    }

    /// Small then large.
    #[must_use]
    pub fn ground(&self) -> Vec<u8> {
        let mut v = self.small.to_vec();
        v.extend_from_slice(self.large);
        v
    }
}

/// West / Death Mountain.
pub const WEST: EnemySets = EnemySets {
    // Myu, Bot, Bit, Octorok (both), Lowder, Megmet.
    small: &[0x03, 0x04, 0x05, 0x11, 0x12, 0x1C, 0x1F],
    // Moblins, Dairas, Goriyas, Geldarm.
    large: &[0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x20],
    // Moa, Ache, Acheman, Deelers.
    flying: &[0x06, 0x07, 0x0A, 0x0D, 0x0E],
    // Bubble, rock, Bago Bago, Moby.
    generators: &[0x0B, 0x0C, 0x0F, 0x1D],
};

/// East / Maze Island.
pub const EAST: EnemySets = EnemySets {
    // Myu, Bot, Bit, blue Octoroks, Leever.
    small: &[0x03, 0x04, 0x05, 0x11, 0x12, 0x16],
    // Tektite, Basilisk, Scorpion, Lizalfos x3.
    large: &[0x14, 0x18, 0x19, 0x1A, 0x1B, 0x1C],
    // Moa, Ache, Acheman, Deelers, Girubokku.
    flying: &[0x06, 0x07, 0x0A, 0x0D, 0x0E, 0x15],
    // Bubble, Bago Bago, Boon.
    generators: &[0x0B, 0x0F, 0x17],
};

/// Palaces 1, 2 and 5.
pub const PALACE125: EnemySets = EnemySets {
    // Myu, Bot, Ropes.
    small: &[0x03, 0x04, 0x11, 0x12],
    // Tinsuit, Iron Knuckles, Mago, Guma, Stalfos red/blue.
    large: &[0x0C, 0x18, 0x19, 0x1A, 0x1D, 0x1E, 0x1F, 0x23],
    // Slow bubble, orange Moa, fast bubble.
    flying: &[0x06, 0x07, 0x0E],
    // Tinsuit, Bago Bago, wolf head, blue dragon head.
    generators: &[0x0B, 0x0F, 0x1B, 0x0A],
};

/// Palaces 3, 4 and 6.
pub const PALACE346: EnemySets = EnemySets {
    // Myu, Bot, stationary Rope.
    small: &[0x03, 0x04, 0x11],
    // Tinsuit, Iron Knuckles, Wizard, Doomknocker, Stalfos red/blue.
    large: &[0x0C, 0x18, 0x19, 0x1A, 0x1D, 0x1E, 0x1F, 0x23],
    // Slow bubble, orange Moa, fast bubble.
    flying: &[0x06, 0x07, 0x0E],
    // Tinsuit, blue dragon head, wolf head.
    generators: &[0x0B, 0x0F, 0x1B],
};

/// The Great Palace.
pub const GREAT_PALACE: EnemySets = EnemySets {
    // Myu, Bot, Ropes.
    small: &[0x03, 0x04, 0x11, 0x12],
    // Fokkas, Fokkeru.
    large: &[0x18, 0x19, 0x1A, 0x1D],
    // Orange Moa, slow/fast/big bubble, King Bot.
    flying: &[0x06, 0x14, 0x15, 0x17, 0x1E],
    // Bubble, rock, fire Bago Bago, orange dragon head.
    generators: &[0x0B, 0x0C, 0x0F, 0x16],
};

/// Rebonack's ID in the palace 3/4/6 group.
pub const REBONACK: u8 = 0x20;

/// Bosses with randomizable hit points (the Great Palace bosses are left
/// alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Boss {
    /// Horsehead (palace 1).
    Horsehead,
    /// Helmethead (palace 2).
    Helmethead,
    /// Rebonack on his horse (palace 3).
    Rebonack,
    /// Rebonack after losing the horse.
    UnhorsedRebonack,
    /// Carock (palace 4).
    Carock,
    /// Gooma (palace 5).
    Gooma,
    /// Barba (palace 6).
    Barba,
}

impl Boss {
    /// Every boss.
    pub const ALL: [Boss; 7] = [
        Boss::Horsehead,
        Boss::Helmethead,
        Boss::Rebonack,
        Boss::UnhorsedRebonack,
        Boss::Carock,
        Boss::Gooma,
        Boss::Barba,
    ];

    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Boss::Horsehead => "Horsehead",
            Boss::Helmethead => "Helmethead",
            Boss::Rebonack => "Rebonack",
            Boss::UnhorsedRebonack => "Rebonack (unhorsed)",
            Boss::Carock => "Carock",
            Boss::Gooma => "Gooma",
            Boss::Barba => "Barba",
        }
    }

    /// Bank 4 address of the hit point byte (a table entry or, for the
    /// unhorsed Rebonack, an `LDA #` operand).
    #[must_use]
    pub fn hp_addr(self) -> u16 {
        match self {
            Boss::Horsehead => 0x9441,
            Boss::Helmethead => 0xBC76,
            Boss::Rebonack => 0xA941,
            Boss::UnhorsedRebonack => 0xB031,
            Boss::Carock => 0xA943,
            Boss::Gooma => 0xBC77,
            Boss::Barba => 0xA942,
        }
    }

    /// Bank 4 addresses of the experience byte (attribute row 1 layout).
    #[must_use]
    pub fn exp_addr(self) -> u16 {
        match self {
            Boss::Horsehead => 0x94F5,
            Boss::Helmethead => 0xBC78,
            Boss::Rebonack => 0xA9F5,
            Boss::UnhorsedRebonack => 0xA9DF,
            Boss::Carock => 0xA9F7,
            Boss::Gooma => 0xBC79,
            Boss::Barba => 0xA9F6,
        }
    }

    /// Bank 4 `LDA #divisor` operands of the boss hit point bar (the bar
    /// shows `hp / divisor` segments, 8 when full in vanilla).
    #[must_use]
    pub fn bar_divisor_addrs(self) -> &'static [u16] {
        match self {
            Boss::Horsehead => &[0xBB70],
            Boss::Helmethead => &[0xBAD2],
            Boss::Rebonack => &[0xAFC2],
            Boss::UnhorsedRebonack => &[0xB24C],
            Boss::Carock => &[0xAE82],
            // The shared Helmethead/Gooma bar code picks Gooma's divisor
            // outside the West; Gooma's own fight code has another copy.
            Boss::Gooma => &[0xBAD9, 0xB4BF],
            Boss::Barba => &[0xB126],
        }
    }
}

/// Thunderbird's experience byte (Great Palace attribute row 1, ID `0x22`).
pub const THUNDERBIRD_EXP: (u8, u16) = (5, 0x94F7);

/// Boss hit points.
pub fn boss_hp(rom: &Rom, b: Boss) -> Result<u8, RandoError> {
    rom.read_cpu(4, b.hp_addr())
}

/// Set boss hit points.
pub fn set_boss_hp(rom: &mut Rom, b: Boss, hp: u8) -> Result<(), RandoError> {
    rom.write_cpu(4, b.hp_addr(), &[hp])
}

/// Hit points of `id` in `g`.
pub fn hp(rom: &Rom, g: Group, id: u8) -> Result<u8, RandoError> {
    rom.read_cpu(g.bank(), g.hp_table() + u16::from(id))
}

/// Set hit points of `id` in `g`.
pub fn write_hp(rom: &mut Rom, g: Group, id: u8, v: u8) -> Result<(), RandoError> {
    rom.write_cpu(g.bank(), g.hp_table() + u16::from(id), &[v])
}

/// Attribute row 1 of `id` in `g`.
pub fn attr1(rom: &Rom, g: Group, id: u8) -> Result<u8, RandoError> {
    rom.read_cpu(g.bank(), g.attr_table() + u16::from(id))
}

/// Set attribute row 1 of `id` in `g`.
pub fn set_attr1(rom: &mut Rom, g: Group, id: u8, v: u8) -> Result<(), RandoError> {
    rom.write_cpu(g.bank(), g.attr_table() + u16::from(id), &[v])
}

/// Attribute row 2 of `id` in `g`.
pub fn attr2(rom: &Rom, g: Group, id: u8) -> Result<u8, RandoError> {
    rom.read_cpu(g.bank(), g.attr_table() + TABLE_LEN + u16::from(id))
}

/// Set attribute row 2 of `id` in `g`.
pub fn set_attr2(rom: &mut Rom, g: Group, id: u8, v: u8) -> Result<(), RandoError> {
    rom.write_cpu(g.bank(), g.attr_table() + TABLE_LEN + u16::from(id), &[v])
}

/// For every boss whose hit points differ from vanilla, set its bar divisor
/// to `ceil(hp / 8)` so a full bar is still 8 segments.
pub fn update_boss_bar_divisors(rom: &mut Rom, vanilla: &Rom) -> Result<(), RandoError> {
    for b in Boss::ALL {
        let hp = boss_hp(rom, b)?;
        if hp == boss_hp(vanilla, b)? {
            continue;
        }
        let div = hp.div_ceil(8).max(1);
        for &a in b.bar_divisor_addrs() {
            if rom.read_cpu(4, a - 1)? == 0xA9 {
                rom.write_cpu(4, a, &[div])?;
            }
        }
    }
    Ok(())
}

/// Big Bubble ID in the Great Palace.
pub const BIG_BUBBLE: u8 = 0x17;
/// Bank 5 `CMP #` operand: the Big Bubble splits once its hit points drop
/// below this.
pub const BIG_BUBBLE_SPLIT: (u8, u16) = (5, 0xA0A8);

/// When the Big Bubble's hit points changed, move its split threshold to
/// three quarters of them, so it does not split as soon as it appears.
pub fn update_big_bubble_threshold(rom: &mut Rom, vanilla: &Rom) -> Result<(), RandoError> {
    let g = Group::GreatPalace;
    let now = hp(rom, g, BIG_BUBBLE)?;
    if now == hp(vanilla, g, BIG_BUBBLE)? {
        return Ok(());
    }
    if rom.read_cpu(BIG_BUBBLE_SPLIT.0, BIG_BUBBLE_SPLIT.1 - 1)? != 0xC9 {
        return Ok(());
    }
    let t = (u16::from(now) * 3 / 4) as u8;
    rom.write_cpu(BIG_BUBBLE_SPLIT.0, BIG_BUBBLE_SPLIT.1, &[t])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_lists_are_disjoint_and_in_range() {
        for g in Group::ALL {
            let all = g.sets().all();
            let mut s = all.clone();
            s.sort_unstable();
            s.dedup();
            assert_eq!(s.len(), all.len(), "{g:?}");
            assert!(all.iter().all(|&id| u16::from(id) < TABLE_LEN));
            assert!(g.hp_ids().iter().all(|&id| u16::from(id) < TABLE_LEN));
        }
    }

    #[test]
    fn attribute_tables_follow_the_hp_tables() {
        assert_eq!(Group::West.attr_table(), 0x94D5);
        assert_eq!(Group::Palace346.attr_table(), 0xA9D5);
        assert_eq!(
            Group::Palace125.attr_table() + REBONACK as u16,
            Boss::Horsehead.exp_addr()
        );
    }
}
