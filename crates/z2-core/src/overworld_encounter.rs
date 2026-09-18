//! Encounter system: demon spawn, wave timer, AI, collision, entry.
//!
//! Self-contained (no intra-crate imports; shared addresses repeated here on
//! purpose) so `rustc --edition 2021 --test` compiles this file standalone.
//! `main` wires it with `pub mod overworld_encounter;`.
//!
//! # Model
//!
//! Eight demon slots (`X = 7..0`) drift over the overworld map. Each slot:
//! screen-ish pos `$2A,x`/`$4E,x`, velocity `$56D,x`/`$575,x`, life timer
//! `$050E,x` (21-frame NMI ticks), type `$82,x` (0 none, 1 weak, 2 strong,
//! 3 fairy). All state transitions are pure functions over [`Demons`] — no
//! `Game` dependency, ROM-free by construction.
//!
//! # Spawn (`overworld1`, bank 0 `$8284-$8331`)
//!
//! 1. Region gate: West (`$0706 == 0`) with `$73 < $3C` (north half) skips
//!    the step-tally fast path and consults the wave timer only; elsewhere
//!    `$26 == 0` forces an immediate terrain check, otherwise the timer
//!    gates (`$8284-$8296`).
//! 2. Terrain group: `$73`/`$74` terrain (via bank-7 boundaries) is matched
//!    against `TERRAIN_BY_GROUP` (`L8231`, bank 0 `$8231`); group 0 never
//!    spawns.
//! 3. If stepping (`$26 != 0`), reload the wave timer from
//!    `WAVE_RELOAD[group]` (`L823F`, bank 0 `$823F`): 0/32/24/24/32/9/3 —
//!    the documented "3-32 by terrain".
//! 4. Up to 4 placement attempts (`$0A = 3..0`), skipping the attempt whose
//!    counter equals `$051C & 3`, into free slots (`$82,x == 0`); positions
//!    from `SPAWN_OFFSETS` (`tables_overworld_demons`, `$8229`) biased by
//!    scroll `$7F`/`$FD`; life timers from `DEMON_LIFE[group]` (`L8246`,
//!    `$8246`); types from `DEMON_TYPES` (`L8265`, `$8265`) indexed by the
//!    RNG threshold walk over `THRESHOLDS` (`L824D`, `$824D`).
//!
//! # Ticks
//!
//! * 21-frame NMI wide sweep (`$0500` reloads `$14` = 20, so expiry every
//!   21st NMI; `bank7_timers.rs` owns the sweep): decrements `$0516` and
//!   live `$050E,x`. [`dec_timer`] models one such tick.
//! * 16-frame demon AI (`L841B`, bank 0 `$841B`, `frame & $0F == 0`):
//!   velocities zeroed, then chase (`frame >= $40`, non-fairy) or random
//!   walk; positions integrate every frame (`L8336`: `$2A += $56D`).
//! * Collision (`L83AB` block, bank 0 `$83AB-$83FA`): OAM box
//!   Y `$64-$76` / X `$7A-$86`, `Blocked` probe, terrain `$04-$0C`
//!   except `$0D` → sideview entry params.
//!
//! # Entry (`L83EE`/`L85D5`, bank 0 `$83EE`/`$85D5`)
//!
//! `$0748 = $FF` (random), `$075A = 0` then demon type unless fairy
//! (fairy keeps `$075A = 0`, `$0759 = 1`; small/strong set
//! `$075A = 1/2`, `$0759 = 0`). Fixed encounters (towns/palaces/caves)
//! enter with `$075A = 0, $0759 = 0` via the key-area path. Sideview load
//! then keys off `$075A` (bank 7 `$C53A`): `>= 2` advances the enemy-data
//! pointer (big), `== 1` clears killed bits (small), `== 0` keeps area
//! data (fairy/fixed); `$0759 != 0` forces a fairy spawn (bank 7 `$D603`).
//!
//! # `$86-$89` note (honest gap)
//!
//! Community RAM notes list `$86-$89` "monster types", but no `$0086-$0089`
//! access exists anywhere in `prg0`-`prg7` (verified by grep); the live
//! encounter data are `$82,x` demon types + `$075A` + sideview params
//! `$0732-$0735` (bank 7 `$D625`, sideview scope). Reserved address
//! constants are still provided ([`ADDR_MONSTER_TYPE_0`]…) marked
//! unverified so a future listing can confirm or correct them.

// ---------------------------------------------------------------------------
// Addresses (duplicated per overworld*.rs file; see overworld.rs).
// ---------------------------------------------------------------------------

/// Demon-wave timer (`$0516`).
pub const ADDR_WAVE_TIMER: u16 = 0x0516;
/// Alternate randomizer (`$051C`).
pub const ADDR_ALT_RNG: u16 = 0x051C;
/// NMI LFSR byte (`$051B`).
pub const ADDR_RNG: u16 = 0x051B;
/// Fairy-force flag (`$0759`).
pub const ADDR_FAIRY_FLAG: u16 = 0x0759;
/// Encounter type (`$075A`): 0 fairy/fixed, 1 small, 2 big.
pub const ADDR_ENCOUNTER_TYPE: u16 = 0x075A;
/// Area index (`$0748`); `$FF` = random demon encounter.
pub const ADDR_AREA_INDEX: u16 = 0x0748;
/// Step tally (`$0026`).
pub const ADDR_STEP_TALLY: u16 = 0x0026;
/// Region index (`$0706`).
pub const ADDR_OVERWORLD_INDEX: u16 = 0x0706;
/// Tile Y (`$0073`); north-Hyrule split at `$3C` (bank 0 `$828B`).
pub const ADDR_TILE_Y: u16 = 0x0073;
/// Reserved per the RAM notes (`$0086-$0089` "monster types"): UNVERIFIED — no
/// disassembly access found; see module docs.
pub const ADDR_MONSTER_TYPE_0: u16 = 0x0086;
/// Reserved per the RAM notes (`$0087`): UNVERIFIED — see [`ADDR_MONSTER_TYPE_0`].
pub const ADDR_MONSTER_TYPE_1: u16 = 0x0087;
/// Reserved per the RAM notes (`$0088`): UNVERIFIED — see [`ADDR_MONSTER_TYPE_0`].
pub const ADDR_MONSTER_TYPE_2: u16 = 0x0088;
/// Reserved per the RAM notes (`$0089`): UNVERIFIED — see [`ADDR_MONSTER_TYPE_0`].
pub const ADDR_MONSTER_TYPE_3: u16 = 0x0089;

// ---------------------------------------------------------------------------
// Timing + table constants (all bytes quoted from prg0.asm).
// ---------------------------------------------------------------------------

/// NMI wide-sweep period: `$0500` reloads `$14` (20), expiry on the 21st
/// `DEC` — so `$0516`/`$050E,x` tick about every 21 frames.
pub const WAVE_TICK_FRAMES: u8 = 21;
/// Wave timer reset on sideview exit (`LDA #$08 : STA $0516`, `$8879`).
pub const WAVE_TIMER_ON_EXIT: u8 = 8;
/// Demon AI velocity tick mask (`L841B`: `frame & $0F`, bank 0 `$841D`).
pub const AI_TICK_MASK: u8 = 0x0F;
/// Chase behaviour starts at this frame value (`CMP #$40`, `$843D`).
pub const CHASE_FROM_FRAME: u8 = 0x40;
/// North/south split for the West-Hyrule timer-only zone (`CMP #$3C`).
pub const NORTH_SPLIT_Y: u8 = 0x3C;
/// Demon slot count (`LDX #$07`, `$82F7`/`$8332` loops).
pub const DEMON_SLOTS: usize = 8;
/// Random-encounter area index (`LDY #$FF : STY $0748`, `$83D9`).
pub const RANDOM_AREA_INDEX: u8 = 0xFF;

/// Terrain code per spawn group (`L8231`, bank 0 `$8231`).
/// Index 0 (`$00`) never matches-spawns (loop exits via `BNE` at Y=0).
pub const TERRAIN_BY_GROUP: [u8; 7] = [0x00, 0x05, 0x04, 0x06, 0x07, 0x08, 0x0A];
/// Wave-timer reload per group (`L823F`, bank 0 `$823F`): 0/32/24/24/32/9/3.
pub const WAVE_RELOAD: [u8; 7] = [0x00, 0x20, 0x18, 0x18, 0x20, 0x09, 0x03];
/// Demon-count seed base per group (`L8238`, bank 0 `$8238`).
pub const COUNT_SEED: [u8; 7] = [0x00, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00];
/// Demon life timers per group (`L8246`, bank 0 `$8246`), 21-frame ticks.
pub const DEMON_LIFE: [u8; 7] = [0x00, 0x0A, 0x0A, 0x18, 0x18, 0x30, 0x30];
/// RNG threshold rows per group (`L824D`, bank 0 `$824D`, 6×4).
pub const THRESHOLDS: [[u8; 4]; 6] = [
    [0x00, 0x60, 0xB0, 0xD0],
    [0x00, 0x60, 0xD0, 0xF0],
    [0x00, 0x60, 0xC0, 0xE0],
    [0x00, 0x50, 0xBB, 0xF0],
    [0x00, 0x57, 0xD7, 0xF8],
    [0x00, 0x57, 0xD7, 0xFF],
];
/// Flat RNG thresholds (`L824D`, bank 0 `$824D`, 24 bytes = [`THRESHOLDS`]
/// row-major). The walker indexes this flat (`CMP L824D,y`), so it can
/// cross into lower groups' rows; keep it flat here for fidelity.
pub const THRESHOLDS_FLAT: [u8; 24] = [
    0x00, 0x60, 0xB0, 0xD0, 0x00, 0x60, 0xD0, 0xF0, 0x00, 0x60, 0xC0, 0xE0, 0x00, 0x50, 0xBB, 0xF0,
    0x00, 0x57, 0xD7, 0xF8, 0x00, 0x57, 0xD7, 0xFF,
];
/// Demon-type probabilities (`L8265`, bank 0 `$8265`): 1 weak, 2 strong,
/// 3 fairy.
pub const DEMON_TYPES: [u8; 16] = [
    0x01, 0x02, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x02, 0x01, 0x02, 0x01, 0x01, 0x03, 0x01, 0x03,
];
/// Spawn screen offsets (`tables_overworld_demons`, bank 0 `$8229`):
/// high nibble added to `$7F` (Y), byte `<< 4` added to `$FD` (X).
pub const SPAWN_OFFSETS: [u8; 8] = [0x58, 0x76, 0x98, 0x7A, 0x38, 0x74, 0xB8, 0x7C];

/// Demon type codes (`$82,x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DemonType {
    /// Empty slot.
    None = 0,
    /// Weak demon (`$075A = 1` small encounter on hit).
    Weak = 1,
    /// Strong demon (`$075A = 2` big encounter on hit).
    Strong = 2,
    /// Fairy (`$075A = 0`, `$0759 = 1` on hit).
    Fairy = 3,
}

/// Encounter type (`$075A`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EncounterType {
    /// Fairy or fixed (town/palace/cave) encounter.
    FairyOrFixed = 0,
    /// Small enemy set.
    Small = 1,
    /// Big enemy set.
    Big = 2,
}

// ---------------------------------------------------------------------------
// Pure spawn helpers.
// ---------------------------------------------------------------------------

/// Match terrain to spawn group 1..6 (`L8298-$82AF` scan of `L8231`).
/// Returns `None` for non-spawning terrain (incl. group-0 `$00`).
pub const fn spawn_group_for_terrain(terrain: u8) -> Option<usize> {
    let mut y = 6usize;
    loop {
        if TERRAIN_BY_GROUP[y] == terrain {
            return Some(y);
        }
        if y == 1 {
            return None;
        }
        y -= 1;
    }
}

/// Whether `overworld1` attempts a spawn this frame (`$8284-$8296`).
///
/// * West-north (`region == 0 && tile_y < $3C`): timer only.
/// * Elsewhere: `$26 == 0` forces the terrain check; otherwise the wave
///   timer must read 0.
pub const fn spawn_attempt(region: u8, tile_y: u8, step_tally: u8, wave_timer: u8) -> bool {
    if region == 0 && tile_y < NORTH_SPLIT_Y {
        return wave_timer == 0;
    }
    if step_tally == 0 {
        return true;
    }
    wave_timer == 0
}

/// Threshold walk for the type-table index (`$82D9-$82EB`):
/// `y = group*4 - 1` over the flat [`THRESHOLDS_FLAT`]; while
/// `rng < flat[y]` and `y != 0`, `y -= 1`; index `= ((y & 3) << 2) | 3`.
///
/// `group` is 1..=6 (from [`spawn_group_for_terrain`]).
pub fn type_index_for_rng(group: usize, rng: u8) -> usize {
    let mut y: i8 = (group.clamp(1, 6) as i8) * 4 - 1;
    // Walk down while the sample sits below the threshold (BCC path).
    loop {
        let t = THRESHOLDS_FLAT[(y as usize) & 23];
        if rng >= t {
            break;
        }
        y -= 1;
        if y == 0 {
            break;
        }
    }
    (((y as usize) & 3) << 2) | 3
}

// ---------------------------------------------------------------------------
// Demon slot state machine.
// ---------------------------------------------------------------------------

/// Eight demon slots (index = original `X`). Positions are map-anchored
/// (`$2A,x`/`$4E,x`), velocities `$56D,x`/`$575,x` as signed bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demons {
    /// Y positions (`$2A,x`).
    pub y: [u8; DEMON_SLOTS],
    /// X positions (`$4E,x`).
    pub x: [u8; DEMON_SLOTS],
    /// Y velocities, signed (`$56D,x`).
    pub vy: [i8; DEMON_SLOTS],
    /// X velocities, signed (`$575,x`).
    pub vx: [i8; DEMON_SLOTS],
    /// Life timers, 21-frame ticks (`$050E,x`).
    pub timer: [u8; DEMON_SLOTS],
    /// Types (`$82,x`).
    pub kind: [u8; DEMON_SLOTS],
}

impl Demons {
    /// All slots empty (sideview-exit state; `L8871` zeroes `$82,y`).
    pub const fn empty() -> Self {
        Self {
            y: [0; DEMON_SLOTS],
            x: [0; DEMON_SLOTS],
            vy: [0; DEMON_SLOTS],
            vx: [0; DEMON_SLOTS],
            timer: [0; DEMON_SLOTS],
            kind: [0; DEMON_SLOTS],
        }
    }

    /// Slot occupied: timer nonzero AND type nonzero (`L8336` skip rule).
    pub const fn occupied(&self, slot: usize) -> bool {
        self.timer[slot] != 0 && self.kind[slot] != 0
    }
}

/// Placement-attempt outcome: which slots were filled, in fill order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnOut {
    /// New wave-timer value (`L823F[group]` when stepping, else unchanged).
    pub wave_timer: u8,
    /// Slots filled (ascending attempt order, each ≤ 7).
    pub filled: [Option<usize>; 4],
    /// How many slots were filled.
    pub filled_count: usize,
}

/// Run one spawn event (`L82B0-$8331`) over explicit state.
///
/// * `demons` — mutated in place (free slots get pos/type/timer).
/// * `group` — 1..=6 from [`spawn_group_for_terrain`].
/// * `alt_rng` — `$051C` (`& 3` = skipped attempt counter).
/// * `rng` — `$051B` sample used by every threshold comparison this event.
/// * `scroll_y`/`scroll_x` — `$7F`/`$FD` screen anchors.
/// * `step_tally` — `$26`; reloads the wave timer when nonzero.
/// * `wave_timer` — current `$0516` (returned unchanged when not stepping).
///
/// Eight args mirror the ASM register/stack handoff; bundling them would
/// obscure the 1:1 mapping to the disassembly (see `L8xxx` cites below).
#[allow(clippy::too_many_arguments)]
pub fn spawn_event(
    demons: &mut Demons,
    group: usize,
    alt_rng: u8,
    rng: u8,
    scroll_y: u8,
    scroll_x: u8,
    step_tally: u8,
    wave_timer: u8,
) -> SpawnOut {
    let g = group.clamp(1, 6);
    let out_timer = if step_tally != 0 {
        WAVE_RELOAD[g]
    } else {
        wave_timer
    };
    let skip_counter = alt_rng & 3;
    let mut type_idx = type_index_for_rng(g, rng) as i16;
    let mut off_idx = (((COUNT_SEED[g] as usize) << 2) | 3) as i16;
    let mut counter = 3i16;
    let mut x = 7i16;
    let mut filled: [Option<usize>; 4] = [None; 4];
    let mut filled_count = 0usize;
    // Attempt loop (`L82ED`): occupied slots are skipped without consuming
    // counters (`DEC $80 : DEX : BPL`); each attempt consumes one counter
    // step; the attempt matching `skip_counter` places nothing.
    while counter >= 0 && x >= 0 {
        let s = x as usize;
        if demons.kind[s] != 0 {
            x -= 1;
            continue;
        }
        if counter as u8 != skip_counter {
            let o = SPAWN_OFFSETS[(off_idx as usize) & 7];
            demons.y[s] = scroll_y.wrapping_add(o & 0xF0);
            demons.x[s] = scroll_x.wrapping_add(o << 4);
            demons.timer[s] = DEMON_LIFE[g];
            demons.kind[s] = DEMON_TYPES[(type_idx as usize) & 15];
            if filled_count < 4 {
                filled[filled_count] = Some(s);
                filled_count += 1;
            }
        }
        type_idx -= 1;
        off_idx -= 1;
        counter -= 1;
        x -= 1;
    }
    SpawnOut {
        wave_timer: out_timer,
        filled,
        filled_count,
    }
}

/// Decrement a wave/life timer by one 21-frame tick if nonzero
/// (NMI wide-sweep semantics; saturates at 0).
pub const fn dec_timer(t: u8) -> u8 {
    if t == 0 {
        0
    } else {
        t - 1
    }
}

/// Per-frame integrate (`L8336` path): `pos += vel` for occupied slots.
/// Expired timers (`$050E == 0`) clear the slot type (`L840B`).
pub fn integrate(demons: &mut Demons) {
    for s in 0..DEMON_SLOTS {
        if demons.timer[s] == 0 {
            demons.kind[s] = 0;
            continue;
        }
        if demons.kind[s] == 0 {
            continue;
        }
        demons.y[s] = demons.y[s].wrapping_add(demons.vy[s] as u8);
        demons.x[s] = demons.x[s].wrapping_add(demons.vx[s] as u8);
    }
}

/// 16-frame velocity tick (`L841B-$84AA`).
///
/// Velocities are zeroed first; occupied slots then get:
/// * chase (`frame >= $40`, type weak/strong), all in screen space
///   (`$8441-$8485`):
///   `vtest = $70 − y + scroll_y + $10`; if `< $20`, step X toward Link
///   (`screen_x >= link_scr_x → −1`, else `+1`);
///   else `htest = link_scr_x − x + scroll_x + $10`; if `< $20`,
///   `screen_y < $70 → vy = +1` else `−1`;
/// * else random walk (`L8488`): sign from `rng_indexed` bit 7
///   (`LDA $051B,x : BPL`), axis from `rng` bit 2; the chosen axis moves
///   **now** by ±1 as well as holding the velocity (`STA $056D/$0575` +
///   immediate `ADC`).
///
/// `link_scr_x` is Link's sprite screen X (`$0203 = $84`, set by `L8726`);
/// pass `0x84` unless the caller models a different anchor.
pub fn ai_tick(
    demons: &mut Demons,
    frame: u8,
    link_scr_x: u8,
    scroll_y: u8,
    scroll_x: u8,
    rng: u8,
    rng_indexed: u8,
) {
    if frame & AI_TICK_MASK != 0 {
        return;
    }
    for s in 0..DEMON_SLOTS {
        demons.vy[s] = 0;
        demons.vx[s] = 0;
        if !demons.occupied(s) {
            continue;
        }
        let fairy = demons.kind[s] >= DemonType::Fairy as u8;
        if !fairy && frame >= CHASE_FROM_FRAME {
            // Vertical closeness → horizontal step (`$8441-$8461`).
            let vtest = 0x70u8
                .wrapping_sub(demons.y[s])
                .wrapping_add(scroll_y)
                .wrapping_add(0x10);
            if vtest < 0x20 {
                let scr_x = demons.x[s].wrapping_sub(scroll_x);
                demons.vx[s] = if scr_x >= link_scr_x { -1 } else { 1 };
                continue;
            }
            // Horizontal closeness → vertical step (`$8464-$8485`).
            let htest = link_scr_x
                .wrapping_sub(demons.x[s])
                .wrapping_add(scroll_x)
                .wrapping_add(0x10);
            if htest < 0x20 {
                let scr_y = demons.y[s].wrapping_sub(scroll_y);
                demons.vy[s] = if scr_y < 0x70 { 1 } else { -1 };
                continue;
            }
        }
        // Random walk (`L8488-$84AA`): sign from bit 7, axis from bit 2.
        let sign: i8 = if rng_indexed & 0x80 == 0 { 1 } else { -1 };
        if rng & 0x04 == 0 {
            demons.vy[s] = sign;
            demons.y[s] = demons.y[s].wrapping_add(sign as u8);
        } else {
            demons.vx[s] = sign;
            demons.x[s] = demons.x[s].wrapping_add(sign as u8);
        }
    }
}

/// Sideview-entry params computed on demon hit (`L83CF-$83FA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnterParams {
    /// `$0748` — always `$FF` for random encounters.
    pub area_index: u8,
    /// `$075A` — 0 fairy, 1 small, 2 big.
    pub encounter_type: u8,
    /// `$0759` — 1 forces fairy next time (fairy hit), else 0.
    pub fairy_flag: u8,
}

/// Collision → entry check for one slot's OAM sprite (`L83AB-$83FA`).
///
/// `sprite_y`/`sprite_x` are the OAM bytes (`$0280,y`/`$0283,y`); the Link
/// box is Y `$64-$76`, X `$7A-$86`. `blocked` is the `Blocked` probe result
/// (carry), `terrain` is `$0563`, `demon_type` is `$82,x`. Returns `None`
/// when no encounter triggers.
pub const fn collide(
    sprite_y: u8,
    sprite_x: u8,
    blocked: bool,
    terrain: u8,
    demon_type: u8,
) -> Option<EnterParams> {
    if sprite_y < 0x64 || sprite_y >= 0x76 {
        return None;
    }
    if sprite_x < 0x7A || sprite_x >= 0x86 {
        return None;
    }
    if blocked {
        return None;
    }
    if terrain < 0x04 || terrain == 0x0D {
        return None;
    }
    if demon_type == DemonType::Fairy as u8 {
        return Some(EnterParams {
            area_index: RANDOM_AREA_INDEX,
            encounter_type: EncounterType::FairyOrFixed as u8,
            fairy_flag: 1,
        });
    }
    Some(EnterParams {
        area_index: RANDOM_AREA_INDEX,
        encounter_type: demon_type,
        fairy_flag: 0,
    })
}

/// Sideview-exit reset (`LE179`, bank 7 `$E179-$E183`, + `overworld4`
/// `$8871-$887B`): clears `$0759`/`$075A`/`$70`, empties demons,
/// returns the fresh wave timer (8).
pub fn exit_reset(demons: &mut Demons) -> u8 {
    *demons = Demons::empty();
    WAVE_TIMER_ON_EXIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_match_covers_spawn_terrains_only() {
        assert_eq!(spawn_group_for_terrain(0x00), None);
        assert_eq!(spawn_group_for_terrain(0x05), Some(1));
        assert_eq!(spawn_group_for_terrain(0x04), Some(2));
        assert_eq!(spawn_group_for_terrain(0x0A), Some(6));
        assert_eq!(spawn_group_for_terrain(0x0B), None);
        assert_eq!(spawn_group_for_terrain(0x0D), None);
        assert_eq!(spawn_group_for_terrain(0x0E), None);
    }

    #[test]
    fn wave_reload_spans_3_to_32() {
        let min = WAVE_RELOAD[1..].iter().min().copied().unwrap();
        let max = WAVE_RELOAD[1..].iter().max().copied().unwrap();
        assert_eq!((min, max), (3, 32));
    }

    #[test]
    fn gates_match_overworld1() {
        // West-north: timer only.
        assert!(spawn_attempt(0, 0x10, 5, 0));
        assert!(!spawn_attempt(0, 0x10, 5, 7));
        assert!(!spawn_attempt(0, 0x10, 0, 7));
        // Elsewhere: tally-zero forces, else timer.
        assert!(spawn_attempt(0, 0x40, 0, 9));
        assert!(spawn_attempt(1, 0x10, 3, 0));
        assert!(!spawn_attempt(1, 0x10, 3, 9));
    }

    #[test]
    fn fairy_hit_reports_fairy_entry() {
        let e = collide(0x6A, 0x80, false, 0x05, 3).unwrap();
        assert_eq!((e.area_index, e.encounter_type, e.fairy_flag), (0xFF, 0, 1));
        let e = collide(0x6A, 0x80, false, 0x05, 2).unwrap();
        assert_eq!((e.encounter_type, e.fairy_flag), (2, 0));
        assert!(collide(0x6A, 0x80, false, 0x03, 1).is_none());
        assert!(collide(0x6A, 0x80, false, 0x0D, 1).is_none());
        assert!(collide(0x10, 0x80, false, 0x05, 1).is_none());
    }
}
