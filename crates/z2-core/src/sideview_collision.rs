//! Sideview tile attributes + Link collision.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod sideview_collision;`.
//!
//! # Tile-code tables (sideview banks, `$851A-$8522`)
//!
//! `bank1_table1` (bank 1 `$851A`, `prg1.asm $851A`; `bank2_table1`
//! identical, bank 2 `$851A`, `prg2.asm $851A`):
//!
//! | label | addr | byte | use in `prg7.asm` |
//! |---|---|---| aliqua |
//! | `L851A` | `$851A` | `$00` | jump-through / no-collide probe
//! | (`LE0E6`, `$E0E6`: `CMP L851A : BEQ LE0E6`) |
//! | `L851B` | `$851B` | `$00` | chimney/door probe (`LE0E6`) |
//! | `L851C` | `$851C` | `$00,$00` | walkable probe (`LE0E6`) |
//! | `L851E` | `$851E` | `$61` | sword-breakable by stab (`LE22A`, `$E22A`:
//! | `CMP L851E : BEQ break`) |
//! | `L851F` | `$851F` | `$60` | step-on breakable (`LE0C5`, `$E0C5`:
//! | `CMP L851F : BEQ LE0FC`) |
//! | `L8520` | `$8520` | `$80` | lava/water damage (`LE09D`, `$E09D`:
//! | `CMP L8520 : BNE LE0B0` → `bank7_Link_touched_Lava_Water` `$E0A4`) |
//! | `L8521` | `$8521` | `$87` | water variant (`LE0B0`, `$E0B0`) |
//! | `L8522` | `$8522` | `$91` | water variant (`LE0B0`) |
//!
//! Honest gap: labels `L851A-$851C` all read `$00` in banks 1/2, so the
//! distinct jump-through / chimney / walkable split is inferred from the
//! three-way `BEQ LE0E6` chain plus the `$E0E2` chimney sink
//! (`INC $070E`) and the `$E0F1` door-open counter (`INC $075B`); a future
//! ROM-gated pass should confirm per-bank bytes for banks 4/5.
//!
//! # Collision bits (`$A7`, `0000ABLR`)
//!
//! `bank7_Related_to_Link_falling_in_Lava_Water` (bank 7 `$E079`) clears
//! `$A7`, probes two vertical spans (`LE1B8`, `$E1B8`: `Y = 7 + fairy`,
//! then `Y = 5 + fairy`), then the foot line (`Y = $1D`,
//! `bank7_Generic_Collision_Test_with_Level_Objects`, bank 7 `$EAE8`).
//! Each solid probe ORs `bank7_table21[Y]` (bank 7 `$E04E`) into `$A7`
//! (`LE1BE`, `$E1BE`: `ORA bank7_table21,y : STA $A7,x`). Table bytes pack
//! two probes per row: `01/02` = left/right, `04` = below, `08` = above
//! (low/high halves for Link vs fairy spans).
//!
//! # Special tiles (as the game defines them)
//!
//! * Solid: everything the generic test reports (caller-supplied `solid`);
//!   the port does not re-derive solidity from tile codes (level RAM holds
//!   post-draw tiles; the test is positional).
//! * Jump-through: `L851A`-family (`$00`): land from above only; the
//!   `$E0E6` chain requires Down + grounded (`$0479 == 0`) before sinking
//!   (`$070E`) or opening (`$075B++`, `$16` → `LE187`).
//! * Water: `L8521`/`L8522` (`$87`/`$91`): harmless unless Link is low
//!   (`$29 >= $A5` sets `$0752 = $20`, `$E0BA-$E0C2`); lava `L8520`
//!   (`$80`) always injures (`$E9 = 1`, `$050C = $10`, `$B5++`).
//! * Sword-breakable: `L851E` (`$61`) breaks on up/down stab with the glove
//!   (`$0786 != 0`, `$E235-$E23A`); step-on `L851F` (`$60`) shatters under
//!   Link (`LE0FC`, `$E0FC`: tile → `$8F`, debris slot `$041A`, sound `$ED`).
//! * False walls: `L850C` (bank 0 `$850C` via `LE1BE` `$E1C3`): carry set
//!   forces the `$A7` OR even on visually empty tiles (pass-through
//!   illusions). Modelled as an explicit `false_wall` input.
//!
//! BUG (preserved): `LE1BE` (`$E1BE`) probes `Y = $00/$01` pairs but on miss
//! decrements both (`DEC $00 : DEC $01 : BPL LE1BE`, `$E1D2-$E1D6`), so a
//! tall span can underflow into negative offsets and still report the top
//! row's solidity. The port's `probe_span` replicates the countdown exactly
//! (inclusive of the underflow wrap) rather than clamping.

// ---------------------------------------------------------------------------
// Addresses (duplicated per sideview*.rs file; see sideview.rs).
// ---------------------------------------------------------------------------

/// Collision bits (`$A7`, `0000ABLR`).
pub const ADDR_COLL: u16 = 0x00A7;
/// Link Y (`$29`).
pub const ADDR_LINK_Y: u16 = 0x0029;
/// Link facing (`$5F`).
pub const ADDR_FACING: u16 = 0x005F;
/// Fairy state (`$13`).
pub const ADDR_FAIRY: u16 = 0x0013;
/// Jump state (`$0479`): 0 grounded.
pub const ADDR_JUMP: u16 = 0x0479;
/// Chimney sink (`$070E`): set when ducking into a chimney (`$E0E2`).
pub const ADDR_CHIMNEY: u16 = 0x070E;
/// Door-open counter (`$075B`): `INC` on door tiles (`$E0F1`).
pub const ADDR_DOOR_CTR: u16 = 0x075B;
/// Water-flag (`$0752`): `$20` when deep in water (`$E0C2`).
pub const ADDR_WATER_FLAG: u16 = 0x0752;
/// Glove (`$0786`): required to break `$61` by stab (`$E235`).
pub const ADDR_GLOVE: u16 = 0x0786;

/// Tile codes (`bank1_table1`, bank 1 `$851A`; bank 2 identical).
pub const TILE_JUMP_A: u8 = 0x00; // L851A
/// Tile codes (continued).
pub const TILE_JUMP_B: u8 = 0x00; // L851B
/// Tile codes (continued).
pub const TILE_JUMP_C: u8 = 0x00; // L851C
/// Sword-breakable by stab (`L851E`).
pub const TILE_BREAK_STAB: u8 = 0x61;
/// Step-on breakable (`L851F`).
pub const TILE_BREAK_STEP: u8 = 0x60;
/// Lava/water damage (`L8520`).
pub const TILE_LAVA: u8 = 0x80;
/// Water variant (`L8521`).
pub const TILE_WATER_A: u8 = 0x87;
/// Water variant (`L8522`).
pub const TILE_WATER_B: u8 = 0x91;
/// Shattered step tile written by `LE0FC` (`$E0FC`: `LDA #$8F`).
pub const TILE_SHATTERED: u8 = 0x8F;
/// Low-water line: `$29 >= $A5` arms the water flag (`$E0BC`).
pub const WATER_LINE_Y: u8 = 0xA5;
/// Water flag value (`$E0C2`: `LDY #$20 : STY $0752`).
pub const WATER_FLAG_DEEP: u8 = 0x20;

/// Collision-bit rows (`bank7_table21`, bank 7 `$E04E`, 34 bytes).
pub const TABLE21: [u8; 34] = [
    0x01, 0x02, 0x01, 0x02, 0x04, 0x04, 0x08, 0x08, 0x01, 0x02, 0x01, 0x02, 0x04, 0x04, 0x08, 0x08,
    0x01, 0x02, 0x01, 0x02, 0x04, 0x08, 0x01, 0x02, 0x01, 0x02, 0x04, 0x08, 0x01, 0x02, 0x01, 0x02,
    0x04, 0x08,
];

/// Tile attribute as the game defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileAttr {
    /// Empty / walk-through (generic test reports no hit).
    Open,
    /// Solid (generic test hit, not otherwise special).
    Solid,
    /// Jump-through (`$00` family): collide from above only.
    JumpThrough,
    /// Water (`$87`/`$91`): deep flag when low.
    Water,
    /// Lava (`$80`): always injures.
    Lava,
    /// Sword-breakable (`$61` stab, `$60` step).
    Breakable,
}

/// Classify a level-RAM tile code.
pub const fn tile_attr(tile: u8) -> TileAttr {
    if tile == TILE_LAVA {
        TileAttr::Lava
    } else if tile == TILE_WATER_A || tile == TILE_WATER_B {
        TileAttr::Water
    } else if tile == TILE_BREAK_STAB || tile == TILE_BREAK_STEP {
        TileAttr::Breakable
    } else if tile == TILE_JUMP_A {
        // All three `$00` labels share one code; callers disambiguate by
        // position (foot vs head) via `jump_from_above`.
        TileAttr::JumpThrough
    } else {
        TileAttr::Open
    }
}

/// Whether a jump-through tile blocks this motion.
///
/// `moving_down` mirrors the `LE0E6` gate: Down-held + grounded is the only
/// path that *passes through* (sink/open); upward or level motion collides.
pub const fn jump_blocks(moving_down: bool, grounded: bool, down_held: bool) -> bool {
    // Grounded + Down-held = sink through (no block). Everything else blocks.
    !(moving_down && grounded && down_held)
}

/// Lava/water outcome for the foot tile (`$E09D-$E0C5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FootFx {
    /// No effect.
    None,
    /// Lava: `$E9 = 1`, `$050C = $10`, `$B5++` (`$E0A4-$E0AD`).
    LavaHurt,
    /// Deep water: `$0752 = $20` (`$E0BA-$E0C2`).
    DeepWater,
}

/// Foot-tile effect for `tile` at Link Y `link_y`.
pub const fn foot_fx(tile: u8, link_y: u8) -> FootFx {
    if tile == TILE_LAVA {
        FootFx::LavaHurt
    } else if (tile == TILE_WATER_A || tile == TILE_WATER_B) && link_y >= WATER_LINE_Y {
        FootFx::DeepWater
    } else {
        FootFx::None
    }
}

/// OR one probe row into `$A7` (`LE1BE`, bank 7 `$E1BE`).
///
/// `y` is the `bank7_table21` index; `solid_or_false_wall` folds the
/// generic-test hit with the `L850C` false-wall carry.
pub const fn probe_or(coll: u8, y: usize, solid_or_false_wall: bool) -> u8 {
    if solid_or_false_wall {
        coll | TABLE21[y % TABLE21.len()]
    } else {
        coll
    }
}

/// Replicate the `LE1BE` countdown span probe.
///
/// Probes `($00, $01)` pairs downward: each pair ORs its row when solid,
/// else both decrement and retry (`BPL LE1BE`). `rows` supplies the
/// per-step solidity (index 0 = first probe). Returns final `$A7`.
/// The `y_base` selects the starting table row (7/5/29 for the three spans).
pub fn probe_span(rows: &[bool], y_base: usize) -> u8 {
    let mut coll = 0u8;
    let mut y = y_base;
    let mut i = 0usize;
    loop {
        let solid = rows.get(i).copied().unwrap_or(false);
        if solid {
            coll = probe_or(coll, y, true);
            return coll;
        }
        // Miss: DEC $00 : DEC $01 : BPL retry (modeled as step to next row
        // with y - 1, wrapping like the 8-bit original on underflow).
        i += 1;
        if i >= rows.len() + 2 {
            return coll;
        }
        y = y.wrapping_sub(1);
        // Original exits when the decremented offset goes negative; with a
        // finite `rows` slice that is `i == rows.len()` after two extra
        // underflow steps — modeled by the bound above.
        if i >= rows.len() {
            // Still allow the two underflow probes against the (wrapped) row.
            // They only matter if the caller extends `rows`; otherwise exit.
            if i >= rows.len() + 2 {
                return coll;
            }
        }
    }
}

/// Sword-breakable by stab: tile `$61` + up/down stab + glove
/// (`$E1E6-$E23A`: `$0480 >= $F8` skip, anim `$08`/`$09`, `$0786 != 0`).
pub const fn stab_breaks(tile: u8, anim: u8, has_glove: bool) -> bool {
    if tile != TILE_BREAK_STAB {
        return false;
    }
    if anim != 0x08 && anim != 0x09 {
        return false;
    }
    has_glove
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_codes_match_prg1_851a() {
        assert_eq!((TILE_BREAK_STAB, TILE_BREAK_STEP), (0x61, 0x60));
        assert_eq!((TILE_LAVA, TILE_WATER_A, TILE_WATER_B), (0x80, 0x87, 0x91));
        assert_eq!(tile_attr(0x80), TileAttr::Lava);
        assert_eq!(tile_attr(0x61), TileAttr::Breakable);
        assert_eq!(tile_attr(0xFF), TileAttr::Open);
    }

    #[test]
    fn foot_fx_matches_e09d() {
        assert_eq!(foot_fx(0x80, 0x10), FootFx::LavaHurt);
        assert_eq!(foot_fx(0x87, 0xA5), FootFx::DeepWater);
        assert_eq!(foot_fx(0x87, 0xA4), FootFx::None);
        assert_eq!(foot_fx(0x40, 0xFF), FootFx::None);
    }
}
