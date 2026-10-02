//! New abilities (group `A`).
//!
//! Options ([`AbilityOpts`]), their ZALiA reference behaviour and how z2rs
//! does it (clean-room, on top of the bank 0 / bank 7 routines named in
//! README.md; addresses are CPU addresses):
//!
//! * `double_jump`: one extra jump while airborne (ZALiA FEATHER). Hook on
//!   `Link_Jumping_routine` (bank 0 `$94C5`): with Link airborne (`$0479`),
//!   not hurt (`$050C`), not on an elevator (`$0754`) and the token unused,
//!   an A press (`$F5 & $80`) re-runs the ROM's own jump kick (vspeed and
//!   gravity counter from the bank 0 tables at `$9470`/`$9474`, same
//!   speed/JUMP-spell index the ground jump uses). Token: `EnhState::flags`
//!   bit 0, cleared once `$0479` is back to 0 (landed).
//! * `stab_frenzy`: holding B stabs repeatedly. Hook on the side-view input
//!   latch (`L959C`, bank 0 `$959C`): with B held, a synthetic B press is
//!   fed to the ROM when the blade is idle (`$0400 == 0`) or leaning back
//!   (`$0400 >= 3`, re-entered at the ROM's own quick re-stab state 4),
//!   at most every [`FRENZY_GAP`] frames (`EnhState::timers[2]`). `$F5` is
//!   restored right after the latch, so nothing else sees the press.
//! * `sword_reach_px` (`0..=3`): PRG patch of the sword box (`bank7_code44`,
//!   `$E9A2`): width operand `$E9B5` `$0E` → `$0E + n` and the left-side X
//!   nudge `$E99D` `$02` → `$02 - n`, so the box grows `n` px in the facing
//!   direction (ZALiA SWORD item, +3 px). The Rust port reads the width
//!   operand from PRG, so trapped and interpreted runs agree.
//! * `damage_reduction_pct` (`0..=100`): PRG patch of
//!   `bank7_Table_for_Enemy_Damage` (`$E2AF`, damage codes 0-6 × life
//!   levels): every entry becomes `round(d × (100 - pct) / 100)`, minimum 1
//!   (ZALiA RING halves, RING + PROTECT ×0.33). Every hit on Link reads
//!   that table (`bank7_Link_Hit_Routine`, `$E2EF`), so contact, sword and
//!   projectile damage all shrink; the instant-kill code 7 row (`$FF`) is
//!   left alone. The PROTECT halving (`LSR`) still applies on top.
//! * `mp_regen`: +2 MP every `$80` frames of running gameplay (side view
//!   `$0736 == $0B` or overworld `$05`, no pause pane `$0524`, no dialog
//!   `$074C`), capped at the container cap `(containers << 5) - 1` (ZALiA
//!   PENDANT). Frame counter: `EnhState::timers[1]`.
//! * `reflect_more` (partial): hook on the projectile-vs-Link check
//!   (`LE3B9`, bank 7 `$E3B9`): while REFLECT is on (`$0710`), the shot's
//!   class bits in the per-area attribute table (`$6D17,type & $C0`) read as
//!   `$C0` ("reflectable") for that one check, so shots the ROM only absorbs
//!   or bounces are sent back at four times their speed. The second half of
//!   ZALiA's option (reflected shots damaging enemies) is a documented
//!   no-op: the ROM has no projectile-vs-enemy path for enemy shots, and
//!   faking one would need the beam slot / monster-death machinery
//!   (`LE694`, `bank7_monster_death`) that is not ported.
//! * `dash_speed`: holding **B** with the blade idle (`$0400 == 0`) raises
//!   the walk cap from `$18` (24) to `$27` (39, +62%; ZALiA uses 40, but 39
//!   keeps `|v| >> 3` inside the ROM's 5-entry walk-animation delay table at
//!   `$93B7`). Hook on the walk driver (`bank0_Side_View4__walking`, bank 0
//!   `$93BC`) swaps the max-velocity table bytes `$93B3/$93B4` for that one
//!   call. Pressing B still starts a stab (and stops Link on the ground,
//!   exactly as in the original); keeping B held after the stab dashes, so
//!   stabbing is untouched. Without the dash the speed decays 1 per frame
//!   back to 24 (the ROM's cap test is an equality, so it must never see a
//!   speed above the cap). With `stab_frenzy` also on, held B keeps stabbing
//!   and the dash never engages.
//! * `rescue_fairy`: touching lava/water (the only fatal "pit" in the NES
//!   game: `bank7_Link_touched_Lava_Water` sets `$B5 = 2`, Link sinks and
//!   `$0494` kills him at `Y >= $D0`) puts Link back on the last safe ground
//!   instead, once per room. Falling off the bottom of the screen is a room
//!   exit in Zelda II (mode `$0C` fall-in-hole), not a death, so it is left
//!   alone. Safe ground is sampled at the end of every side-view frame
//!   while Link stands on solid ground (`$0479 == 0`, `$A7 & 4`, `$B5 == 1`,
//!   not on an elevator).
//! * `flute_warp`: on the overworld, hold Select and press B to enter the
//!   next visited town of the current region (ZALiA rando flute warping).
//!   Hook on the overworld main (`overworld3`, bank 0 `$8558`): Link is put
//!   on the town's key-area tile (`$6A00`/`$6A3F`, slot in `$0748`) and the
//!   ROM's own area-entry sequence (`L85EB`) runs, so the town loads like a
//!   normal entry and leaving it lands next to it. The overworld's own B
//!   action (flute) is skipped on a warp frame.
//!
//! Every hook on a bank 0 address declines (runs the ROM untouched) unless
//! bank 0 is mapped at `$8000`, because the trap table is keyed by CPU
//! address only.
//!
//! # `EnhState` slots used
//!
//! * `flags` bit 0: double-jump token used.
//! * `timers[1]`: MP regen frame counter. `timers[2]`: stab frenzy gap.
//! * `scratch[8]`/`scratch[9]`: visited towns of the West / East region
//!   (bit = town ordinal among the region's town key areas).
//! * `scratch[10]`: flute warp cursor (ordinal + 1 of the last warp);
//!   `scratch[11]`: last frame's game mode (town visits are recorded on the
//!   overworld → area edge, while the key-area table is still loaded).
//! * `scratch[12..15]`: rescue fairy safe ground (`$4D`, `$3B`, `$29`);
//!   `scratch[15]`: bit 0 safe ground valid, bit 1 rescue used this room.

use serde::{Deserialize, Serialize};

use super::{call_original, hook, patch_prg};
use crate::game::Game;

/// Largest [`AbilityOpts::sword_reach_px`].
pub const MAX_SWORD_REACH_PX: u8 = 3;

/// Frames between two frenzy stabs (`EnhState::timers[2]` reload).
pub const FRENZY_GAP: u16 = 10;

/// MP regen period in frames (`$80`).
pub const MP_REGEN_PERIOD: u16 = 0x80;
/// MP added per regen tick.
pub const MP_REGEN_AMOUNT: u8 = 2;

/// Walk speed cap while dashing (`$27` = 39).
pub const DASH_CAP: u8 = 0x27;
/// Original walk speed cap (`$93B3`: `$18`).
pub const WALK_CAP: u8 = 0x18;

// ------------------------------------------------------------ addresses

/// `Link_Jumping_routine` (bank 0).
pub const ADDR_JUMP: u16 = 0x94C5;
/// Side-view input latch `L959C` (bank 0).
pub const ADDR_INPUT: u16 = 0x959C;
/// `bank0_Side_View4__walking` (bank 0).
pub const ADDR_WALK: u16 = 0x93BC;
/// `overworld3`, overworld main (bank 0).
pub const ADDR_OW_MAIN: u16 = 0x8558;
/// `LE3B9`, projectile vs Link / shield (bank 7).
pub const ADDR_PROJ_CHECK: u16 = 0xE3B9;

/// Jump vspeed table (bank 0 `$9470`, 4 entries).
const JUMP_VSPEED: u16 = 0x9470;
/// Jump gravity-counter table (bank 0 `$9474`, 4 entries).
const JUMP_GRAV: u16 = 0x9474;
/// Max walk velocity table (bank 0 `$93B3`: right, left).
const WALK_MAX: u16 = 0x93B3;

/// Enemy damage table (bank 7 `$E2AF`; `LE2AE` is the `RTS` before it).
pub const DAMAGE_TABLE: u16 = 0xE2AF;
/// Damage codes 0-6 (code 7 is the `$FF` instant-kill row, untouched).
pub const DAMAGE_ROWS: usize = 7;
/// Sword box left-side X nudge (`bank7_table26 + 1`).
pub const SWORD_DX_LEFT: u16 = 0xE99D;
/// Operand of `LDA #$0E` (sword box width) at `$E9B4`.
pub const SWORD_WIDTH_OPERAND: u16 = 0xE9B5;

// RAM.
const LINK_Y: usize = 0x29;
const LINK_PAGE: usize = 0x3B;
const LINK_X: usize = 0x4D;
const LINK_HSPEED: usize = 0x70;
const OW_TILE_Y: usize = 0x73;
const OW_TILE_X: usize = 0x74;
const OW_AUTOMOVE: usize = 0x7D;
const PROJ_TYPE: usize = 0x87;
const COLLISION: usize = 0xA7;
const SUBSTATE: usize = 0xB5;
const JUMP_SPELL: usize = 0xD0;
const FAIRY: usize = 0x13;
const PRESSED: usize = 0xF5;
const HELD: usize = 0xF7;
const GRAV_CTR: usize = 0x03E6;
const SLASH: usize = 0x0400;
const MIDAIR: usize = 0x0479;
const LAND_CROUCH: usize = 0x0497;
const HURT: usize = 0x050C;
const MENU: usize = 0x0524;
const VSPEED: usize = 0x057D;
const AREA_SLOT: usize = 0x0748;
const REGION: usize = 0x0706;
const OW_STEP: usize = 0x0729;
const TRANSITION: usize = 0x0726;
const MODE: usize = 0x0736;
const DIALOG: usize = 0x074C;
const ELEVATOR: usize = 0x0754;
const PPU_FX: usize = 0x0768;
const REFLECT: usize = 0x0710;
const MP: usize = 0x0773;
const MAGIC_CONTAINERS: usize = 0x0783;

// Pad bits as the ROM reads `$F5`/`$F7` (A in bit 7).
const PAD_A: u8 = 0x80;
const PAD_B: u8 = 0x40;
const PAD_SELECT: u8 = 0x20;

/// Side-view gameplay mode.
const MODE_SIDEVIEW: u8 = 0x0B;
/// Overworld main mode.
const MODE_OVERWORLD: u8 = 0x05;

/// `EnhState::flags` bit: double-jump token used.
const FLAG_DOUBLE_JUMP: u32 = 1 << 0;

/// Key-area slots per region (`X = $3D..0`).
const KEY_AREAS: usize = 0x3E;
/// WRAM offsets of the region's key-area strides (`$6A00`, `$6A3F`, `$6ABD`).
const AREA_Y: usize = 0x0A00;
const AREA_X: usize = 0x0A3F;
const AREA_WORLD: usize = 0x0ABD;
/// WRAM offset of the per-area projectile attribute table (`$6D17`).
const PROJ_ATTR: usize = 0x0D17;

// Scratch bytes (see the module docs).
const S_TOWNS_WEST: usize = 8;
const S_TOWNS_EAST: usize = 9;
const S_WARP_CURSOR: usize = 10;
const S_PREV_MODE: usize = 11;
const S_SAFE: usize = 12;
const S_RESCUE: usize = 15;
const RESCUE_VALID: u8 = 0x01;
const RESCUE_USED: u8 = 0x02;

/// Ability options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AbilityOpts {
    /// One extra mid-air jump.
    pub double_jump: bool,
    /// Hold B to stab repeatedly.
    pub stab_frenzy: bool,
    /// Extra sword reach, `0..=3` px.
    pub sword_reach_px: u8,
    /// Percent of incoming damage removed (`0` = original).
    pub damage_reduction_pct: u8,
    /// Slow MP regeneration.
    pub mp_regen: bool,
    /// REFLECT bounces more and reflected shots hurt.
    pub reflect_more: bool,
    /// Faster ground speed while dashing.
    pub dash_speed: bool,
    /// Pits return Link to safe ground.
    pub rescue_fairy: bool,
    /// Overworld town warp.
    pub flute_warp: bool,
}

impl AbilityOpts {
    /// Whether anything in this group is on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.double_jump));
        out.push(u8::from(self.stab_frenzy));
        out.push(self.sword_reach_px);
        out.push(self.damage_reduction_pct);
        for b in [
            self.mp_regen,
            self.reflect_more,
            self.dash_speed,
            self.rescue_fairy,
            self.flute_warp,
        ] {
            out.push(u8::from(b));
        }
    }
}

/// Damage after `pct`% reduction: `round(d × (100 - pct) / 100)`, at least
/// 1 for a nonzero `d` (`pct` above 100 counts as 100).
#[must_use]
pub fn reduced_damage(d: u8, pct: u8) -> u8 {
    if d == 0 || pct == 0 {
        return d;
    }
    let keep = 100 - u32::from(pct.min(100));
    let v = (u32::from(d) * keep + 50) / 100;
    v.clamp(1, 255) as u8
}

// ------------------------------------------------------------ helpers

/// PRG image offset of a fixed-bank (`$C000-$FFFF`) address.
fn fixed_offset(game: &Game, addr: u16) -> Option<usize> {
    let len = game.prg.len();
    if len < 0x4000 {
        return None;
    }
    Some(len - 0x4000 + usize::from(addr & 0x3FFF))
}

/// Whether bank 0 is mapped at `$8000` (bank 0 hooks decline otherwise).
fn bank0_mapped(game: &Game) -> bool {
    let len = game.prg.len();
    len >= 0x4000 && len.is_multiple_of(0x4000) && game.mmc1.map_prg(0x8000, len) < 0x4000
}

/// Byte of bank 0 at CPU address `addr` (`$8000-$BFFF`).
fn bank0_byte(game: &Game, addr: u16) -> u8 {
    game.prg
        .get(usize::from(addr & 0x3FFF))
        .copied()
        .unwrap_or(0)
}

/// The patched sword-box bytes `(left nudge, width)` for `reach` px.
#[must_use]
pub fn sword_patch(reach: u8) -> (u8, u8) {
    let n = reach.min(MAX_SWORD_REACH_PX);
    (0x02u8.wrapping_sub(n), 0x0E + n)
}

// ------------------------------------------------------------ register

/// Install this group's hooks ([`super::apply`], only when active).
pub(crate) fn register(game: &mut Game, opts: &AbilityOpts) {
    if opts.double_jump {
        hook(
            game,
            "enh_abilities_double_jump",
            Some(0),
            ADDR_JUMP,
            double_jump_hook,
            None,
        );
    }
    if opts.stab_frenzy {
        hook(
            game,
            "enh_abilities_stab_frenzy",
            Some(0),
            ADDR_INPUT,
            frenzy_hook,
            None,
        );
    }
    if opts.dash_speed {
        hook(
            game,
            "enh_abilities_dash",
            Some(0),
            ADDR_WALK,
            dash_hook,
            None,
        );
    }
    if opts.flute_warp {
        hook(
            game,
            "enh_abilities_flute_warp",
            Some(0),
            ADDR_OW_MAIN,
            flute_hook,
            None,
        );
    }
    if opts.reflect_more {
        hook(
            game,
            "enh_abilities_reflect_more",
            Some(7),
            ADDR_PROJ_CHECK,
            reflect_hook,
            None,
        );
    }
    if opts.sword_reach_px > 0 {
        patch_sword(game, opts.sword_reach_px);
    }
    if opts.damage_reduction_pct > 0 {
        patch_damage(game, opts.damage_reduction_pct);
    }
}

/// Patch the sword box (only over the expected original bytes).
fn patch_sword(game: &mut Game, reach: u8) {
    let (Some(dx), Some(wd)) = (
        fixed_offset(game, SWORD_DX_LEFT),
        fixed_offset(game, SWORD_WIDTH_OPERAND),
    ) else {
        return;
    };
    // `bank7_table26` = `F8 02`; `$E9B4` = `LDA #$0E`.
    if game.prg[dx - 1..=dx] != [0xF8, 0x02] || game.prg[wd - 1..=wd] != [0xA9, 0x0E] {
        return;
    }
    let (nudge, width) = sword_patch(reach);
    patch_prg(game, dx, &[nudge]);
    patch_prg(game, wd, &[width]);
}

/// Patch the enemy damage table (only when it sits where expected: the
/// `RTS` at `$E2AE` before it and `LDY $A1,x` at `$E2EF` after it).
fn patch_damage(game: &mut Game, pct: u8) {
    let Some(start) = fixed_offset(game, DAMAGE_TABLE) else {
        return;
    };
    let end = start + DAMAGE_ROWS * 8;
    if game.prg[start - 1] != 0x60 || game.prg[start + 64..start + 66] != [0xB4, 0xA1] {
        return;
    }
    let table: Vec<u8> = game.prg[start..end]
        .iter()
        .map(|&d| reduced_damage(d, pct))
        .collect();
    patch_prg(game, start, &table);
}

// ------------------------------------------------------------ double jump

/// `Link_Jumping_routine` wrapper.
fn double_jump_hook(game: &mut Game) {
    let mine = bank0_mapped(game) && game.enh.abilities.double_jump;
    if mine {
        try_double_jump(game);
    }
    call_original(game, ADDR_JUMP);
    if mine && game.ram[MIDAIR] == 0 {
        game.enh_state.flags &= !FLAG_DOUBLE_JUMP;
    }
}

/// The mid-air jump kick (same tables and index as the ground jump at
/// `$950D-$952A`).
fn try_double_jump(game: &mut Game) {
    let r = &game.ram;
    if r[MIDAIR] == 0
        || r[PRESSED] & PAD_A == 0
        || r[HURT] != 0
        || r[ELEVATOR] != 0
        || r[FAIRY] != 0
        || game.enh_state.flags & FLAG_DOUBLE_JUMP != 0
    {
        return;
    }
    // LDA $70 : ADC #$13 : CMP #$26 : BCC +1 ; LDA $D0 : BNE : INY INY.
    let mut y = u16::from(r[LINK_HSPEED].wrapping_add(0x13) >= 0x26);
    if r[JUMP_SPELL] == 0 {
        y += 2;
    }
    let vs = bank0_byte(game, JUMP_VSPEED + y);
    let gc = bank0_byte(game, JUMP_GRAV + y);
    let r = &mut game.ram;
    r[SLASH] = 0;
    r[LAND_CROUCH] = 0;
    r[VSPEED] = vs;
    r[GRAV_CTR] = gc;
    r[MIDAIR] = 2;
    game.enh_state.flags |= FLAG_DOUBLE_JUMP;
}

// ------------------------------------------------------------ stab frenzy

/// `L959C` wrapper: feed a B press while B is held.
fn frenzy_hook(game: &mut Game) {
    let pressed = game.ram[PRESSED];
    let mut injected = false;
    if bank0_mapped(game)
        && game.enh.abilities.stab_frenzy
        && game.ram[HELD] & PAD_B != 0
        && pressed & PAD_B == 0
        && game.ram[HURT] == 0
        && game.enh_state.timers[2] == 0
    {
        let s = game.ram[SLASH];
        if s == 0 || s >= 3 {
            if s >= 3 {
                // The ROM's quick re-stab state (B at `$0400 == 4`).
                game.ram[SLASH] = 4;
            }
            game.ram[PRESSED] = pressed | PAD_B;
            game.enh_state.timers[2] = FRENZY_GAP;
            injected = true;
        }
    }
    call_original(game, ADDR_INPUT);
    if injected {
        game.ram[PRESSED] = (game.ram[PRESSED] & !PAD_B) | (pressed & PAD_B);
    }
}

// ------------------------------------------------------------ dash

/// `bank0_Side_View4__walking` wrapper.
fn dash_hook(game: &mut Game) {
    if !bank0_mapped(game) || !game.enh.abilities.dash_speed {
        call_original(game, ADDR_WALK);
        return;
    }
    let r = &game.ram;
    let dashing = r[HELD] & PAD_B != 0 && r[SLASH] == 0 && r[HURT] == 0;
    let v0 = r[LINK_HSPEED] as i8;
    let off = usize::from(WALK_MAX & 0x3FFF);
    let table_ok = game.prg.get(off..off + 2) == Some(&[WALK_CAP, WALK_CAP.wrapping_neg()][..]);
    if dashing && table_ok {
        game.prg[off] = DASH_CAP;
        game.prg[off + 1] = DASH_CAP.wrapping_neg();
        call_original(game, ADDR_WALK);
        game.prg[off] = WALK_CAP;
        game.prg[off + 1] = WALK_CAP.wrapping_neg();
    } else {
        call_original(game, ADDR_WALK);
        // Above the cap the ROM's equality test would never stop: decay.
        let v1 = game.ram[LINK_HSPEED] as i8;
        let cap = i16::from(WALK_CAP);
        let target = cap.max(i16::from(v0).abs() - 1);
        if i16::from(v1).abs() > target {
            let t = target as i8;
            game.ram[LINK_HSPEED] = (if v1 < 0 { -t } else { t }) as u8;
        }
    }
}

// ------------------------------------------------------------ flute warp

/// Whether key-area slot `slot` of the loaded region is a town.
fn is_town(game: &Game, slot: usize) -> bool {
    matches!((game.wram[AREA_WORLD + slot] >> 2) & 7, 1 | 2)
}

/// Ordinal of town slot `slot` among the region's town key areas.
fn town_ordinal(game: &Game, slot: usize) -> usize {
    (0..slot).filter(|&s| is_town(game, s)).count()
}

/// Key-area slot of town ordinal `ord`.
fn town_slot(game: &Game, ord: usize) -> Option<usize> {
    (0..KEY_AREAS).filter(|&s| is_town(game, s)).nth(ord)
}

/// Scratch byte holding the visited towns of `region`.
fn towns_byte(region: u8) -> Option<usize> {
    match region {
        0 => Some(S_TOWNS_WEST),
        2 => Some(S_TOWNS_EAST),
        _ => None,
    }
}

/// Visited-town bits of `region` (bit = town ordinal).
#[must_use]
pub fn visited_towns(game: &Game, region: u8) -> u8 {
    towns_byte(region).map_or(0, |i| game.enh_state.scratch[i])
}

/// Mark a town as visited on the frame Link leaves the overworld into it.
/// The key-area table at `$6A00` is only valid on the overworld (side-view
/// loads reuse that WRAM), so the check runs on the mode `$05` → other edge.
fn record_town(game: &mut Game) {
    let prev = game.enh_state.scratch[S_PREV_MODE];
    let mode = game.ram[MODE];
    game.enh_state.scratch[S_PREV_MODE] = mode;
    if prev != MODE_OVERWORLD || mode == MODE_OVERWORLD {
        return;
    }
    let r = &game.ram;
    let slot = usize::from(r[AREA_SLOT]);
    let Some(i) = towns_byte(r[REGION]) else {
        return;
    };
    if slot >= KEY_AREAS || !is_town(game, slot) {
        return;
    }
    let ord = town_ordinal(game, slot);
    if ord < 8 {
        game.enh_state.scratch[i] |= 1 << ord;
    }
}

/// `overworld3` wrapper: Select held + B pressed warps.
fn flute_hook(game: &mut Game) {
    let r = &game.ram;
    if bank0_mapped(game)
        && game.enh.abilities.flute_warp
        && r[MODE] == MODE_OVERWORLD
        && r[HELD] & PAD_SELECT != 0
        && r[PRESSED] & PAD_B != 0
        && r[OW_AUTOMOVE] == 0
        && try_warp(game)
    {
        return;
    }
    call_original(game, ADDR_OW_MAIN);
}

/// Enter the next visited town after the cursor. `false` = nothing to do.
fn try_warp(game: &mut Game) -> bool {
    let mask = visited_towns(game, game.ram[REGION]);
    if mask == 0 {
        return false;
    }
    let cursor = usize::from(game.enh_state.scratch[S_WARP_CURSOR]);
    let Some(ord) = (0..8)
        .map(|k| (cursor + k) % 8)
        .find(|&o| mask & (1 << o) != 0)
    else {
        return false;
    };
    let Some(slot) = town_slot(game, ord) else {
        return false;
    };
    game.enh_state.scratch[S_WARP_CURSOR] = (ord + 1) as u8;
    let y = game.wram[AREA_Y + slot] & 0x7F;
    let x = game.wram[AREA_X + slot] & 0x3F;
    let r = &mut game.ram;
    r[OW_TILE_Y] = y;
    r[OW_TILE_X] = x;
    r[AREA_SLOT] = slot as u8;
    // `L85EB`: the ROM's own key-area entry.
    r[OW_STEP] = 0;
    r[TRANSITION] = r[TRANSITION].wrapping_add(1);
    r[MODE] = r[MODE].wrapping_add(1);
    r[PPU_FX] = 6;
    r[DIALOG] = 0;
    true
}

// ------------------------------------------------------------ reflect

/// `LE3B9` wrapper: under REFLECT every shot class reads as reflectable.
fn reflect_hook(game: &mut Game) {
    if !game.enh.abilities.reflect_more || game.ram[REFLECT] == 0 {
        call_original(game, ADDR_PROJ_CHECK);
        return;
    }
    let slot = usize::from(game.cpu.x);
    let ty = usize::from(game.ram[(PROJ_TYPE + slot) & 0x7FF]);
    let a = PROJ_ATTR + ty;
    let orig = game.wram[a];
    game.wram[a] = orig | 0xC0;
    call_original(game, ADDR_PROJ_CHECK);
    // Restore unless the routine itself rewrote the byte (it never does).
    if game.wram[a] == orig | 0xC0 {
        game.wram[a] = orig;
    }
}

// ------------------------------------------------------------ per frame

/// Per-frame work after [`Game::step`] (only while any enhancement is on).
pub(crate) fn end_of_frame(game: &mut Game, opts: &AbilityOpts) {
    if opts.stab_frenzy {
        let t = &mut game.enh_state.timers[2];
        *t = t.saturating_sub(1);
    }
    if opts.mp_regen {
        mp_regen(game);
    }
    if opts.rescue_fairy {
        rescue_fairy(game);
    }
    if opts.flute_warp {
        record_town(game);
    }
}

/// +2 MP every `$80` frames of running gameplay, capped.
fn mp_regen(game: &mut Game) {
    let r = &game.ram;
    if !matches!(r[MODE], MODE_SIDEVIEW | MODE_OVERWORLD) || r[MENU] != 0 || r[DIALOG] != 0 {
        return;
    }
    let t = &mut game.enh_state.timers[1];
    *t += 1;
    if *t < MP_REGEN_PERIOD {
        return;
    }
    *t = 0;
    let cap = crate::player_magic::meter_cap(game.ram[MAGIC_CONTAINERS]);
    let mp = game.ram[MP];
    if mp < cap {
        game.ram[MP] = mp.saturating_add(MP_REGEN_AMOUNT).min(cap);
    }
}

/// Sample safe ground; undo a lava/water sink once per room.
fn rescue_fairy(game: &mut Game) {
    if game.ram[MODE] != MODE_SIDEVIEW {
        game.enh_state.scratch[S_RESCUE] = 0;
        return;
    }
    let r = &game.ram;
    let st = game.enh_state.scratch[S_RESCUE];
    if r[SUBSTATE] == 1
        && r[MIDAIR] == 0
        && r[COLLISION] & 0x04 != 0
        && r[ELEVATOR] == 0
        && r[FAIRY] == 0
    {
        let s = &mut game.enh_state.scratch;
        s[S_SAFE] = r[LINK_X];
        s[S_SAFE + 1] = r[LINK_PAGE];
        s[S_SAFE + 2] = r[LINK_Y];
        s[S_RESCUE] = st | RESCUE_VALID;
        return;
    }
    if r[SUBSTATE] != 2 || st & RESCUE_VALID == 0 || st & RESCUE_USED != 0 {
        return;
    }
    let s = game.enh_state.scratch;
    let r = &mut game.ram;
    r[LINK_X] = s[S_SAFE];
    r[LINK_PAGE] = s[S_SAFE + 1];
    r[LINK_Y] = s[S_SAFE + 2];
    r[SUBSTATE] = 1;
    r[VSPEED] = 0;
    r[GRAV_CTR] = 0;
    r[MIDAIR] = 0;
    r[LINK_HSPEED] = 0;
    r[HURT] = 0;
    r[SLASH] = 0;
    game.enh_state.scratch[S_RESCUE] = st | RESCUE_USED;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enh::Enhancements;

    #[test]
    fn reduced_damage_rounds_and_keeps_one() {
        assert_eq!(reduced_damage(0x10, 0), 0x10);
        assert_eq!(reduced_damage(0x10, 50), 0x08);
        assert_eq!(reduced_damage(0x0C, 67), 4); // RING + PROTECT ~ x0.33
        assert_eq!(reduced_damage(0x01, 50), 1);
        assert_eq!(reduced_damage(0x04, 100), 1);
        assert_eq!(reduced_damage(0xE0, 200), 1);
        assert_eq!(reduced_damage(0, 50), 0);
        for d in 1..=255u8 {
            for pct in 0..=100u8 {
                let r = reduced_damage(d, pct);
                assert!(r >= 1 && r <= d, "d {d} pct {pct} -> {r}");
            }
        }
    }

    #[test]
    fn sword_patch_bytes() {
        assert_eq!(sword_patch(0), (0x02, 0x0E));
        assert_eq!(sword_patch(3), (0xFF, 0x11));
        assert_eq!(sword_patch(9), sword_patch(MAX_SWORD_REACH_PX));
    }

    /// A 128 KiB PRG with the sword-box and damage-table bytes where the
    /// patches expect them.
    fn fake_game() -> Game {
        let mut g = Game::new();
        g.prg = vec![0; 0x20000];
        let b7 = 0x1C000;
        let o = |a: u16| b7 + usize::from(a & 0x3FFF);
        g.prg[o(0xE99C)] = 0xF8;
        g.prg[o(0xE99D)] = 0x02;
        g.prg[o(0xE9B4)] = 0xA9;
        g.prg[o(0xE9B5)] = 0x0E;
        g.prg[o(0xE2AE)] = 0x60;
        for (i, b) in g.prg[o(0xE2AF)..o(0xE2AF) + 64].iter_mut().enumerate() {
            *b = if i >= 56 { 0xFF } else { 0x10 + i as u8 };
        }
        g.prg[o(0xE2EF)] = 0xB4;
        g.prg[o(0xE2F0)] = 0xA1;
        g
    }

    #[test]
    fn patches_apply_and_undo() {
        let mut g = fake_game();
        let prg = g.prg.clone();
        let traps = g.traps.len();
        let mut e = Enhancements::default();
        e.abilities.sword_reach_px = 3;
        e.abilities.damage_reduction_pct = 50;
        e.abilities.double_jump = true;
        e.abilities.flute_warp = true;
        g.set_enhancements(e);
        let b7 = 0x1C000;
        assert_eq!(g.prg[b7 + 0x299D], 0xFF);
        assert_eq!(g.prg[b7 + 0x29B5], 0x11);
        assert_eq!(g.prg[b7 + 0x22AF], reduced_damage(0x10, 50));
        assert_eq!(g.prg[b7 + 0x22AF + 56], 0xFF, "code 7 untouched");
        assert_eq!(g.traps.len(), traps + 2);
        g.set_enhancements(Enhancements::default());
        assert_eq!(g.prg, prg);
        assert_eq!(g.traps.len(), traps);
    }

    #[test]
    fn patches_skip_unexpected_bytes() {
        let mut g = Game::new();
        g.prg = vec![0x55; 0x20000];
        let prg = g.prg.clone();
        let mut e = Enhancements::default();
        e.abilities.sword_reach_px = 2;
        e.abilities.damage_reduction_pct = 30;
        g.set_enhancements(e);
        assert_eq!(g.prg, prg);
    }

    #[test]
    fn mp_regen_ticks_every_period_and_caps() {
        let mut g = Game::new();
        let mut e = Enhancements::default();
        e.abilities.mp_regen = true;
        g.set_enhancements(e);
        g.ram[MODE] = MODE_SIDEVIEW;
        g.ram[MAGIC_CONTAINERS] = 4; // cap $7F
        g.ram[MP] = 0x10;
        for _ in 0..MP_REGEN_PERIOD - 1 {
            end_of_frame(&mut g, &e.abilities);
        }
        assert_eq!(g.ram[MP], 0x10);
        end_of_frame(&mut g, &e.abilities);
        assert_eq!(g.ram[MP], 0x12);
        g.ram[MP] = 0x7E;
        for _ in 0..MP_REGEN_PERIOD {
            end_of_frame(&mut g, &e.abilities);
        }
        assert_eq!(g.ram[MP], 0x7F, "capped at (containers << 5) - 1");
        // Paused: no ticks.
        g.ram[MP] = 0;
        g.ram[MENU] = 1;
        for _ in 0..MP_REGEN_PERIOD * 2 {
            end_of_frame(&mut g, &e.abilities);
        }
        assert_eq!(g.ram[MP], 0);
    }

    #[test]
    fn rescue_returns_to_safe_ground_once_per_room() {
        let mut g = Game::new();
        let mut e = Enhancements::default();
        e.abilities.rescue_fairy = true;
        g.set_enhancements(e);
        let a = e.abilities;
        g.ram[MODE] = MODE_SIDEVIEW;
        g.ram[SUBSTATE] = 1;
        g.ram[COLLISION] = 0x04;
        g.ram[LINK_X] = 0x40;
        g.ram[LINK_PAGE] = 1;
        g.ram[LINK_Y] = 0x90;
        end_of_frame(&mut g, &a);
        // Jump off and land in lava.
        g.ram[COLLISION] = 0;
        g.ram[MIDAIR] = 1;
        g.ram[LINK_X] = 0x70;
        g.ram[LINK_Y] = 0xA0;
        g.ram[SUBSTATE] = 2;
        end_of_frame(&mut g, &a);
        assert_eq!(g.ram[SUBSTATE], 1);
        assert_eq!(
            (g.ram[LINK_X], g.ram[LINK_PAGE], g.ram[LINK_Y]),
            (0x40, 1, 0x90)
        );
        assert_eq!(g.ram[MIDAIR], 0);
        // Second time in the same room: the ROM's death path is untouched.
        g.ram[MIDAIR] = 1;
        g.ram[COLLISION] = 0;
        g.ram[SUBSTATE] = 2;
        end_of_frame(&mut g, &a);
        assert_eq!(g.ram[SUBSTATE], 2);
        // A new room re-arms it.
        g.ram[MODE] = 0x10;
        end_of_frame(&mut g, &a);
        assert_eq!(g.enh_state.scratch[S_RESCUE], 0);
    }

    #[test]
    fn town_bookkeeping() {
        let mut g = Game::new();
        // Towns at slots 3 and 7 (world 1), a palace at 5 (world 3).
        g.wram[AREA_WORLD + 3] = 1 << 2;
        g.wram[AREA_WORLD + 5] = 3 << 2;
        g.wram[AREA_WORLD + 7] = 1 << 2;
        assert_eq!(town_ordinal(&g, 3), 0);
        assert_eq!(town_ordinal(&g, 7), 1);
        assert_eq!(town_slot(&g, 1), Some(7));
        assert_eq!(town_slot(&g, 2), None);
        g.ram[REGION] = 0;
        g.ram[AREA_SLOT] = 7;
        // Only the overworld -> area edge records.
        g.ram[MODE] = MODE_OVERWORLD;
        record_town(&mut g);
        assert_eq!(visited_towns(&g, 0), 0);
        g.ram[MODE] = 0x06;
        record_town(&mut g);
        assert_eq!(visited_towns(&g, 0), 0b10);
        // A palace entry records nothing.
        g.enh_state.scratch[S_TOWNS_WEST] = 0;
        g.ram[AREA_SLOT] = 5;
        g.ram[MODE] = MODE_OVERWORLD;
        record_town(&mut g);
        g.ram[MODE] = 0x06;
        record_town(&mut g);
        assert_eq!(visited_towns(&g, 0), 0);
        assert_eq!(visited_towns(&g, 2), 0);
    }
}
