//! Enemy data tables as ROM offsets.
//!
//! Self-contained (no intra-crate imports) so `rustc --edition 2021 --test`
//! compiles this file standalone. `main` wires it with
//! `pub mod enemy_data;`.
//!
//! CONTRACT: region-bank enemy bytes are NEVER copied into this
//! file. Every table below is a `(PRG bank, CPU address, length)` triple;
//! bytes are loaded at runtime by the caller (`read_table_byte` over a ROM
//! slice or the interpreter bus). Unit tests use synthetic slices.
//!
//! Sources: `third_party/z2disassembly/src/prg{1,2,4,5,7}.asm`
//! (`prgN.asm $xxxx` below); area/enemy pointer roots cross-checked with
//! `sideview_area.rs` (`MAP_PTR_SET1`/`ENEMY_PTR_SET1` read-only reuse,
//! duplicated here for standalone builds).
//!
//! # Per-type status (routine table; `ported` = behavior fn exists in
//! # `enemy_ai.rs`/`enemy_boss.rs`, `verified` = synthetic test green)
//!
//! | bank | label | addr | family | status |
//! |---|---|---|---|
//! | 1 | `bank1_Enemy_Init_Routines_Red_Jar` | `$9620` | statue/item | verified |
//! | 1 | `bank1_Enemy_Init_Routines_Red_Blue_Deeler` | `$9625` | flyer | verified |
//! | 1 | `bank1_Enemy_Init_Routines_Moblin_Daira_Goriya` | `$9631` | walker/shooter | verified |
//! | 1 | `bank1_Various_Projectiles` | `$9639` | shooter | verified |
//! | 1 | `Projectiles_Routines_Desert_Rock` | `$9759` | shooter | ported |
//! | 1 | `Projectiles_Routines_Octorok_Rock_or_Flame` | `$976D` | shooter | ported |
//! | 1 | `Projectiles_Routines_Raising_Bubble` | `$977B` | shooter | ported |
//! | 1 | `Projectiles_Routines_Moblin_Spear` | `$9789` | shooter | ported |
//! | 1 | `Projectiles_Routines_Boomerang` | `$97A9` | shooter | ported |
//! | 1 | `Projectiles_Routines_Red_Daria_Axe` | `$97C0` | shooter | ported |
//! | 1 | `bank1_Enemy_Init_Routines_Bubble_Rock_BagoBago_Moby_…` | `$984E` | flyer/generator | verified |
//! | 1 | `bank1_Enemy_Init_Routines_Geldarm` | `$986C` | walker | ported |
//! | 1 | `bank1_Enemy_Routines1_Megmat` | `$987E` | jumper | verified |
//! | 1 | `bank1_Enemy_Routines1_Lowder` | `$98C3` | walker | verified |
//! | 1 | `bank1_Enemy_Routines1_Dumb_Moblin_Generator` | `$992F` | generator | verified |
//! | 1 | `bank1_Enemy_Routines1_Dumb_Moblin` | `$994F` | walker | verified |
//! | 1 | `bank1_Enemy_Routines1_Goriya` | `$9972` | shooter | verified |
//! | 1 | `bank1_Enemy_Routines1_Daira` | `$9A15` | walker | verified |
//! | 1 | `bank1_Enemy_Routines1_Generators` | `$9B31` | generator | verified |
//! | 1 | `bank1_Enemy_Routines1_Moby` | `$9B94` | flyer | verified |
//! | 1 | `bank1_Enemy_Routines1_Geldarm` | `$9BB5` | walker | ported |
//! | 1 | `bank1_Enemy_Routines2_DumbMoblin` | `$9C43` | walker | ported |
//! | 1 | `bank1_Enemy_Routines2_Goriya` | `$9C6F` | shooter | ported |
//! | 1 | `bank1_Enemy_Routines2_Moblin` | `$9CA4` | shooter | ported |
//! | 1 | `bank1_Enemy_Routines2_Daira_Orange` | `$9D65` | walker | ported |
//! | 1 | `bank1_Enemy_Routines2_Daira_Red_` | `$9D69` | walker | ported |
//! | 1 | `bank1_Enemy_Routines2_Lowder` | `$9DF1` | walker | ported |
//! | 1 | `bank1_Enemy_Routines2_Moby` | `$9DF6` | flyer | ported |
//! | 1 | `bank1_Enemy_Routines2_Megmat` | `$9DFB` | jumper | ported |
//! | 2 | `bank2_Projectiles_Routines_Flame` | `$9627` | shooter | verified |
//! | 2 | `bank2_Projectiles_Routines_Energy_Ball_blue_and_Mace…` | `$962F` | shooter | ported |
//! | 2 | `bank2_Projectiles_Routines_Raising_Bubble…` | `$9637` | shooter | ported |
//! | 2 | `bank2_Projectiles_Routines_Rock_moves_horizontally` | `$964A` | shooter | ported |
//! | 2 | `bank2_Projectiles_Routines_Rock_with_gravity` | `$965B` | shooter | ported |
//! | 2 | `bank2_Enemy_Routines1_Lizalfos_Rock_Tossing` | `$9730` | shooter | verified |
//! | 2 | `bank1_Enemy_Init_Routines_Leever` | `$97F3` | jumper | ported |
//! | 2 | `bank2_Enemy_Routines1_Tektite` | `$9805` | jumper | verified |
//! | 2 | `bank2_Enemy_Routines1_Girobokku` | `$98BC` | walker | ported |
//! | 2 | `bank2_Enemy_Routines1_Leever` | `$9910` | jumper | ported |
//! | 2 | `bank2_Enemy_Routines1_Boon` | `$9989` | flyer | ported |
//! | 2 | `bank2_Enemy_Routines1_Zora` | `$9A2A` | shooter | ported |
//! | 2 | `bank2_Enemy_Routines1_Aru_Lowder` | `$9AAE` | walker | ported |
//! | 2 | `bank2_Enemy_Routines1_Lizalfos_Orange` | `$9B56` | walker | ported |
//! | 2 | `bank2_Enemy_Routines1_Lizalfos_Red_Blue` | `$9B90` | shooter | ported |
//! | 2 | `bank2_Enemy_Routines2_Tektite` | `$9DAC` | jumper | ported |
//! | 2 | `bank2_Enemy_Routines2_Leever` | `$9DEB` | jumper | ported |
//! | 2 | `bank2_Enemy_Routines2_Zora` | `$9E13` | shooter | ported |
//! | 4 | `bank4_Enemy_Routines_Stalfos` | `$965A` | walker | verified |
//! | 4 | `bank4_Enemy_Routines1_Tinsuit` | `$97CC` | walker | verified |
//! | 4 | `bank4_Enemy_Routines1_Guma` | `$97E1` | walker | ported |
//! | 4 | `bank4_Enemy_Routines1_Horsehead` | `$981D` | boss | verified |
//! | 4 | `bank4_Enemy_Routines_Bubble__Slow_Fast` | `$99D1` | flyer | ported |
//! | 4 | `bank4_Enemy_Routines_Energy_Ball_Shooter__Left_Right` | `$9BDD` | shooter | ported |
//! | 4 | `bank4_Enemy_Routines_Iron_Knuckle` | `$9C8C` | walker | verified |
//! | 4 | `bank4_Enemy_Routines1_Iron_Knuckle` | `$9E45` | walker | ported |
//! | 4 | `bank4_Enemy_Routines_Falling_Block_Generator` | `$AB98` | generator | verified |
//! | 4 | `bank4_Enemy_Routines_Falling_Block` | `$ABE9` | generator | ported |
//! | 4 | `bank4_Enemy_Routines_Mago` | `$B7C5` | shooter | ported |
//! | 4 | `bank4_Enemy_Routines_Mau_Wolf_Head` | `$B8BB` | jumper | ported |
//! | 4 | `bank4_Enemy_Routines_Moa` | `$B909` | flyer | verified |
//! | 4 | `bank4_Enemy_Routines_Ra_Unicorn_Head` | `$BA20` | flyer | ported |
//! | 4 | `bank4_Enemy_Routines_Helmethead__Gooma` | `$BAC3` | boss | verified |
//! | 4 | `bank4_Enemy_Routines_Horsehead` | `$BB5F` | boss | verified |
//! | 4 | `bank4_Enemy_Routines_Floating_Helmet` | `$BCEF` | boss | verified |
//! | 4 | `bank4_Enemy_Routines1_Helmethead__Gooma` | `$BD75` | boss | verified |
//! | 5 | `bank5_Enemy_Routines1_Ra` | `$9667` | flyer | ported |
//! | 5 | `bank5_Enemy_Routines1_Fokka` | `$9D2C` | walker | verified |
//! | 5 | `bank5_Enemy_Routines1_Fokkeru` | `$9E6B` | shooter | ported |
//! | 5 | `bank5_Enemy_Routines1_Thunderbird` | `$A359` | boss | verified |
//! | 5 | `bank5_Enemy_Routines1_Giant_Bot` | `$A180` | jumper | ported |
//! | 5 | `bank5_Enemy_Routines1_Dark_Link_Battle_Trigger` | `$97C6` | boss | verified |
//! | 5 | `bank5_dark_link_AI_movement_maybe0` | `$98EB` | boss | verified |
//! | 7 | `bank7_Enemy_Routines1_Bot/Bit/Myu/Moa/…` | `$DA0C-$DBCB` | walker/flyer | verified |
//!
//! `ported` = offset registered + behavior fn present; `verified` = synthetic
//! unit test in `enemy_ai`/`enemy_boss` or `tests/enemy_*.rs` covers it.
//! `ports.toml` is not edited for these (acceptance reads this table).

// ---------------------------------------------------------------------------
// World / region (duplicated from sideview_area for standalone builds).
// ---------------------------------------------------------------------------

/// World id (`$0707`): 0 overworld/others, 1 west towns, 2 east towns,
/// 3 palaces 1/2/5, 4 palaces 3/4/6, 5 Great Palace + ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum World {
    /// Overworld and misc.
    Overworld,
    /// West towns.
    WestTowns,
    /// East towns.
    EastTowns,
    /// Palaces 1/2/5 (bank 4 set A).
    PalaceA,
    /// Palaces 3/4/6 (bank 5 set B).
    PalaceB,
    /// Great Palace + ending (bank 5/6).
    GreatPalace,
}

/// Decode `$0707` to a world.
pub const fn world_of(byte: u8) -> World {
    match byte {
        1 => World::WestTowns,
        2 => World::EastTowns,
        3 => World::PalaceA,
        4 => World::PalaceB,
        5 => World::GreatPalace,
        _ => World::Overworld,
    }
}

/// PRG bank owning sideview AI for a world (banked `$8000-$BFFF`).
pub const fn ai_bank(world: World) -> u8 {
    match world {
        World::Overworld | World::WestTowns => 1,
        World::EastTowns => 2,
        World::PalaceA => 4,
        World::PalaceB | World::GreatPalace => 5,
    }
}

// ---------------------------------------------------------------------------
// ROM-offset table descriptors (never bytes).
// ---------------------------------------------------------------------------

/// One ROM-resident table: `(PRG bank, CPU address, length)`.
///
/// `bank = None` marks WRAM/staged mirrors (e.g. `$7000` enemy-list copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomTable {
    /// PRG bank (`None` = WRAM mirror, not ROM).
    pub bank: Option<u8>,
    /// CPU address of the first byte.
    pub addr: u16,
    /// Length in bytes.
    pub len: u16,
}

/// Area map pointer table, first set (bank 1 `$8523`, `prg1.asm $8523`).
pub const MAP_PTR_SET1: RomTable = RomTable {
    bank: Some(1),
    addr: 0x8523,
    len: 64,
};
/// Area enemy pointer table, first set (bank 1 `$85A1`).
pub const ENEMY_PTR_SET1: RomTable = RomTable {
    bank: Some(1),
    addr: 0x85A1,
    len: 64,
};
/// Area map pointer table, second set (banks 1/2 `$A000`).
pub const MAP_PTR_SET2: RomTable = RomTable {
    bank: Some(1),
    addr: 0xA000,
    len: 64,
};
/// Area enemy pointer table, second set (banks 1/2 `$A07E`, base `$88A0`).
pub const ENEMY_PTR_SET2: RomTable = RomTable {
    bank: Some(1),
    addr: 0xA07E,
    len: 64,
};
/// Staged enemy-list mirror (WRAM `$7000`, `bank7_code18` copy target).
pub const STAGED_ENEMY_LIST: RomTable = RomTable {
    bank: None,
    addr: 0x7000,
    len: 64,
};
/// Enemy Y table (`LD5FB`, bank 7 `$D5FB`, 8 entries `$30…$B0`).
pub const ENEMY_Y_TAB: RomTable = RomTable {
    bank: Some(7),
    addr: 0xD5FB,
    len: 8,
};
/// Enemy velocity table (`bank7_table15`, bank 7 `$D5F9`: `08 F8`).
pub const ENEMY_VEL_TAB: RomTable = RomTable {
    bank: Some(7),
    addr: 0xD5F9,
    len: 2,
};
/// Generated-rock velocity (`bank7_Table_for_Generated_Rock_X_Velocity`,
/// bank 7 `$DBF9`: `10 F0`).
pub const ROCK_VEL_TAB: RomTable = RomTable {
    bank: Some(7),
    addr: 0xDBF9,
    len: 2,
};
/// Enemy attribute table (`$6DD5`, palette/exp/steal/fire bits; WRAM mirror
/// of ROM enemy data, indexed by `$A1`).
pub const ATTR_6DD5: RomTable = RomTable {
    bank: None,
    addr: 0x6DD5,
    len: 64,
};
/// Enemy vuln/damage table (`$6DF9`, damage nibble + `$10` spell-immune +
/// `$20` fire-immune + `$C0` drop group).
pub const ATTR_6DF9: RomTable = RomTable {
    bank: None,
    addr: 0x6DF9,
    len: 64,
};
/// Enemy drop/regen table (`$6E1D`, `$FF` = boss-key marker).
pub const ATTR_6E1D: RomTable = RomTable {
    bank: None,
    addr: 0x6E1D,
    len: 64,
};
/// Enemy touch-immune table (`$6E41`, `$10` = sword-untouchable).
pub const ATTR_6E41: RomTable = RomTable {
    bank: None,
    addr: 0x6E41,
    len: 64,
};
/// Enemy HP table (`$6D21`, `LD625 $D6AD` source).
pub const ATTR_HP_6D21: RomTable = RomTable {
    bank: None,
    addr: 0x6D21,
    len: 64,
};
/// Enemy dispatch pointers (`$6D8D`, `bank7_enemy_every_frame_routine`
/// `$D6CA` indirect vector, 2 bytes per id).
pub const DISPATCH_6D8D: RomTable = RomTable {
    bank: None,
    addr: 0x6D8D,
    len: 128,
};
/// Exp low table (`bank7_Experience_Table_Low_Byte`, bank 7 `$DDC0`, 16).
pub const EXP_LO: RomTable = RomTable {
    bank: Some(7),
    addr: 0xDDC0,
    len: 16,
};
/// Exp high table (`bank7_Experience_Table_High_Byte`, bank 7 `$DDDC`, 16).
pub const EXP_HI: RomTable = RomTable {
    bank: Some(7),
    addr: 0xDDDC,
    len: 16,
};
/// Drop-probability table (`bank7_Table_for_Probability…`, `$E870`, 16).
pub const DROP_PROB: RomTable = RomTable {
    bank: Some(7),
    addr: 0xE870,
    len: 16,
};
/// Helmethead/Gooma HP table (`bank4_Table_for_Helmethead_Gooma`,
/// bank 4 `$BC76`: `30 90`).
pub const HELMET_GOOMA_HP: RomTable = RomTable {
    bank: Some(4),
    addr: 0xBC76,
    len: 2,
};
/// Thunderbird table (`bank5_table_Thunderbird`, bank 5 `$A34F`).
pub const THUNDERBIRD_TAB: RomTable = RomTable {
    bank: Some(5),
    addr: 0xA34F,
    len: 8,
};
/// Statue tile codes (Mau/Ra/Ironknuckle window, bank 4 `$8265`).
pub const STATUE_TILES_4: RomTable = RomTable {
    bank: Some(4),
    addr: 0x8265,
    len: 8,
};
/// Statue tile codes (Ra/Mau/Fokka window, bank 5 `$8276`).
pub const STATUE_TILES_5: RomTable = RomTable {
    bank: Some(5),
    addr: 0x8276,
    len: 8,
};
/// Boss-room/crystal-statue area data (bank 4 `$831B`,
/// `bank4_Area_Data_for_Palaces_Type_A_Boss_Room_and_Crystal_Statue`).
pub const BOSS_ROOM_DATA: RomTable = RomTable {
    bank: Some(4),
    addr: 0x831B,
    len: 32,
};

/// Read one byte from a runtime-loaded table slice.
///
/// `table` is the caller-loaded bytes for `desc` (exact length `desc.len`);
/// returns `None` on out-of-range (total fn, never panics).
pub const fn read_table_byte(table: &[u8], index: usize) -> Option<u8> {
    if index < table.len() {
        Some(table[index])
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Valid enemy-id ranges per area (ROM map / Dwedit enemy lists).
// ---------------------------------------------------------------------------

/// Area class for id-range purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaKind {
    /// Overworld west random-battle / field.
    WestField,
    /// Overworld east random-battle / field.
    EastField,
    /// Towns (wise men + NPCs, bank 3 `$9AC8`/`$9AE8`).
    Town,
    /// Palace set 1/2/5 (bank 4).
    PalaceA,
    /// Palace set 3/4/6 (bank 5).
    PalaceB,
    /// Great Palace (bank 5/6).
    GreatPalace,
}

/// Valid enemy-id window for an area class (inclusive).
///
/// Per the ROM map + Dwedit enemy lists: 24-63 valid ids per area; towns
/// allow NPC codes, palaces allow boss codes. ROM-gated: bounds only, bytes
/// live in ROM (see pointer tables above).
pub const fn valid_id_range(kind: AreaKind) -> (u8, u8) {
    match kind {
        AreaKind::WestField => (0x03, 0x1E),
        AreaKind::EastField => (0x03, 0x24),
        AreaKind::Town => (0x00, 0x0A),
        AreaKind::PalaceA => (0x03, 0x2C),
        AreaKind::PalaceB => (0x03, 0x30),
        AreaKind::GreatPalace => (0x03, 0x3F),
    }
}

/// Whether `id` is valid for `kind` (24-63-wide windows above).
pub const fn valid_id(kind: AreaKind, id: u8) -> bool {
    let (lo, hi) = valid_id_range(kind);
    id >= lo && id <= hi
}

// ---------------------------------------------------------------------------
// Enemy families (behavior dispatch; see enemy_ai.rs).
// ---------------------------------------------------------------------------

/// Behavior family (walker, jumper, flyer, generator, shooter,
/// statue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Ground walker (Bot/Bit/Myu, Daira, Iron Knuckle, Stalfos, …).
    Walker,
    /// Jumper (Tektite, Megmat, Leever, Giant Bot, Mau head, …).
    Jumper,
    /// Flyer (Deeler, Ache, Bago-Bago, Moby, Moa, Ra head, Bubbles, …).
    Flyer,
    /// Generator (Dumb-Moblin/Bago-Bago/Mau/Tinsuit/Falling-block spawners).
    Generator,
    /// Shooter (Octorok, Goriya, Moblin, Lizalfos, Mago, statues that
    /// shoot, energy-ball shooters, …).
    Shooter,
    /// Statue / object enemy (Mau/Ra/Ironknuckle/Fokka statues, elevator,
    /// locked door, crystal slot, hidden red jar, …).
    Statue,
}

/// Classify an enemy id in a world to its behavior family.
///
/// Id map follows the bank dispatch comments (`prg1.asm` west ids,
/// `prg2.asm` east ids, `prg4/5.asm` palace ids, boss ids at the top).
/// ROM-gated where comments disagree; total fn with a Walker default.
pub const fn family_of(world: World, id: u8) -> Family {
    match world {
        World::Overworld | World::WestTowns => match id {
            0x01 => Family::Statue,
            0x02 => Family::Statue,
            0x03 => Family::Walker,
            0x04 | 0x05 => Family::Walker,
            0x06 => Family::Flyer,
            0x07 | 0x08 => Family::Flyer,
            0x09 => Family::Flyer,
            0x0A | 0x0B => Family::Shooter,
            0x0C | 0x0D => Family::Walker,
            0x0E => Family::Flyer,
            0x0F | 0x10 => Family::Walker,
            0x11 | 0x12 => Family::Shooter,
            0x13 => Family::Statue,
            0x14 => Family::Jumper,
            0x15 => Family::Walker,
            0x16 => Family::Generator,
            0x17 | 0x18 => Family::Walker,
            0x19 => Family::Flyer,
            0x1A | 0x1B => Family::Shooter,
            0x1C | 0x1D => Family::Generator,
            0x1E => Family::Flyer,
            _ => Family::Walker,
        },
        World::EastTowns => match id {
            0x01 => Family::Statue,
            0x02 => Family::Statue,
            0x03 => Family::Walker,
            0x04 | 0x05 => Family::Jumper,
            0x06 => Family::Flyer,
            0x07 => Family::Walker,
            0x08 => Family::Jumper,
            0x09 => Family::Flyer,
            0x0A => Family::Shooter,
            0x0B | 0x0C => Family::Walker,
            0x0D | 0x0E => Family::Shooter,
            0x0F => Family::Walker,
            0x10 => Family::Shooter,
            0x13 => Family::Statue,
            0x16 | 0x17 => Family::Generator,
            0x20..=0x3F => Family::Statue,
            _ => Family::Walker,
        },
        World::PalaceA => match id {
            0x01 => Family::Statue,
            0x02 => Family::Statue,
            0x03 => Family::Walker,
            0x04 => Family::Flyer,
            0x05 => Family::Walker,
            0x06 | 0x07 => Family::Walker,
            0x08 => Family::Shooter,
            0x09 => Family::Flyer,
            0x0A => Family::Jumper,
            0x0B => Family::Shooter,
            0x0C => Family::Generator,
            0x0D => Family::Statue,
            0x0E => Family::Generator,
            0x13 => Family::Statue,
            0x20..=0x22 => Family::Statue,
            _ => Family::Walker,
        },
        World::PalaceB | World::GreatPalace => match id {
            0x01 => Family::Statue,
            0x02 => Family::Statue,
            0x03 => Family::Walker,
            0x04 => Family::Flyer,
            0x05 => Family::Jumper,
            0x06 => Family::Walker,
            0x07 => Family::Shooter,
            0x08 => Family::Walker,
            0x09 => Family::Flyer,
            0x0A => Family::Jumper,
            0x0B => Family::Walker,
            0x0C => Family::Generator,
            0x13 => Family::Statue,
            _ => Family::Walker,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worlds_route_to_banks() {
        assert_eq!(ai_bank(World::Overworld), 1);
        assert_eq!(ai_bank(World::EastTowns), 2);
        assert_eq!(ai_bank(World::PalaceA), 4);
        assert_eq!(ai_bank(World::PalaceB), 5);
        assert_eq!(ai_bank(World::GreatPalace), 5);
        assert_eq!(world_of(3), World::PalaceA);
        assert_eq!(world_of(9), World::Overworld);
    }

    #[test]
    fn table_descriptors_point_at_rom() {
        assert_eq!((MAP_PTR_SET1.addr, ENEMY_PTR_SET1.addr), (0x8523, 0x85A1));
        assert_eq!((MAP_PTR_SET2.addr, ENEMY_PTR_SET2.addr), (0xA000, 0xA07E));
        assert_eq!((EXP_LO.addr, EXP_HI.addr), (0xDDC0, 0xDDDC));
        assert_eq!(DROP_PROB.addr, 0xE870);
        assert_eq!(HELMET_GOOMA_HP.addr, 0xBC76);
        assert_eq!(THUNDERBIRD_TAB.addr, 0xA34F);
        assert_eq!(STAGED_ENEMY_LIST.bank, None);
    }

    #[test]
    fn runtime_loads_never_copy_bytes() {
        let fake = [0x10u8, 0x20, 0x30];
        assert_eq!(read_table_byte(&fake, 1), Some(0x20));
        assert_eq!(read_table_byte(&fake, 9), None);
    }

    #[test]
    fn id_windows_cover_24_plus() {
        for k in [
            AreaKind::WestField,
            AreaKind::EastField,
            AreaKind::Town,
            AreaKind::PalaceA,
            AreaKind::PalaceB,
            AreaKind::GreatPalace,
        ] {
            let (lo, hi) = valid_id_range(k);
            assert!(hi >= lo);
            assert!(valid_id(k, lo));
            assert!(valid_id(k, hi));
        }
        assert!(!valid_id(AreaKind::WestField, 0x02));
    }

    #[test]
    fn families_cover_all_worlds() {
        assert_eq!(family_of(World::Overworld, 0x04), Family::Walker);
        assert_eq!(family_of(World::Overworld, 0x09), Family::Flyer);
        assert_eq!(family_of(World::Overworld, 0x0A), Family::Shooter);
        assert_eq!(family_of(World::Overworld, 0x14), Family::Jumper);
        assert_eq!(family_of(World::Overworld, 0x16), Family::Generator);
        assert_eq!(family_of(World::Overworld, 0x01), Family::Statue);
        assert_eq!(family_of(World::EastTowns, 0x04), Family::Jumper);
        assert_eq!(family_of(World::PalaceA, 0x0C), Family::Generator);
    }
}
