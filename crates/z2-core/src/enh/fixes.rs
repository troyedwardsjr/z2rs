//! Engine and physics fixes (group `F`).
//!
//! Options ([`FixesOpts`]) and their ZALiA reference behaviour, with the
//! original routine each one changes (addresses are CPU addresses; "bank 0"
//! patches touch only that bank's bytes in the in-memory PRG copy):
//!
//! * `levelup_softlocks` (ZALiA `mod_FIX_SOFTLOCK_LVLUP_1`/`_2`).
//!   `Hub_Update_Routine` (bank 0 `$968D`, run every sideview frame) sets the
//!   dialog type `$074C = 1` (level-up window) whenever XP `$0775/6` reaches
//!   the next-level cost `$0770/1`, with no other check:
//!   1. while a dialogue is open (`$074C = 2`) it overwrites the dialogue
//!      type, and a level-up raised late in a conversation never closes;
//!   2. on the frame Link leaves the room (game mode `$0736` goes from the
//!      sideview main mode `$0B` to an exit mode such as `$10`) the window
//!      is raised before the exit runs and the exit mode never finishes.
//!
//!   Fix: the `LDA #$01 : STA $074C` at bank 0 `$969C` becomes a `JSR` to a
//!   trap in unused fixed-bank bytes ([`LEVELUP_GATE`]) that only raises the
//!   window when no dialog is open; and at the end of a frame that both
//!   raised the window and left the room, the window is withdrawn. Both just
//!   defer the level-up: XP is unchanged, so the next safe frame raises it.
//!   The previous frame's `$074C`/`$0736` live in `EnhState::scratch[5]`
//!   and `[6]`.
//! * `iframe_update_skip` (ZALiA `mod_PCUpdate1=1`). At the end of the
//!   sideview main body (`$D51F`), while Link's immunity timer `$0518` is 1
//!   or 2, odd frames branch past both Link's sprite draw
//!   (`bank7_Links_Display_Routine`, the blink) and the sword-vs-breakable
//!   block test (`bank7_related_to_breakable_block`, `$E1DD`). Fix: the
//!   branch at `$D52B` skips only the draw (one operand byte), so the block
//!   test runs every frame; the blink is kept.
//! * `jump_direction_balance` (ZALiA `mod_PCJumpDirBalancing=1`).
//!   `Link_Jumping_routine` (bank 0 `$950D`) picks the high jump with
//!   `LDA $70 : ADC #$13 : CMP #$26` without a `CLC`: right needs speed
//!   `$13 - C`, left `$14 + C`, where the stale carry `C` is whatever the
//!   frame's earlier code left. Leftward speed runs one unit ahead of
//!   rightward speed for the same run-up (`-(n + 2)` vs `n + 1` after `n`
//!   frames, measured), so without the carry both directions need the same
//!   run-up; the carry hands rightward runs a frame's head start (and costs
//!   leftward ones one) on about half the frames. Fix: those four bytes
//!   become a `JSR` to a trap ([`JUMP_RULE`]) computing `$70 + $13` with
//!   the carry clear.
//! * `shield_hitbox_symmetry` (ZALiA `PC_init`). `bank7_code45` (`$E9D8`)
//!   places the shield box (projectile/sword blocking) at `$CC + 8 +
//!   table27[$9F - 1]`, `table27 = [$0E, $FF]`: facing right it starts 6 px
//!   past Link's centre, facing left it ends 4 px before it. Fix: a hook
//!   moves the right-facing box 2 px in, mirroring the left one.
//! * `crumble_both_feet` (ZALiA `mod_CRUMBLE_TILES=1`). The per-frame
//!   Link-vs-level tick (`$E079`) shatters a step-on breakable tile
//!   (`$851F` code) only when it sits under the foot-centre probe (`$1D`,
//!   x + 15). Fix: a hook first checks the two foot probes (`4`/`5`, x + 20
//!   and x + 12) and shatters a breakable tile under either one the same way
//!   (tile `$8F`, debris slot, sound), then runs the original tick.
//! * `xp_drain_fix` (ZALiA `update_xp`). The XP tick (`$D433`) moves pending
//!   XP `$0755/6` into XP 10 at a time unless fewer than 10 are pending, so
//!   exactly 10 lands at once (`CMP #$0A` at `$D445`); and a drain from a
//!   point-stealing hit (`$05E8`, one XP per frame) keeps counting after XP
//!   hits 0, so XP gained before the counter runs out is stolen too. Fix:
//!   the compare becomes `CMP #$0B` (10 or fewer pending count one at a
//!   time) and the end of every frame drops the drain counter once XP is 0.
//!
//! Trap addresses [`LEVELUP_GATE`] and [`JUMP_RULE`] are in the 48 unused
//! `$FF` bytes at `$D39A-$D3C9` of the fixed bank (never executed by the
//! ROM); only patched `JSR`s reach them. Patches are applied only when the
//! original bytes match (a different ROM, or the blank `Game::new` image,
//! leaves that fix off).
//!
//! Status: **implemented**, all six. Each original bug is reproduced and
//! each fix checked in a ROM run by `crates/z2-core/tests/enh_fixes_rom.rs`
//! (save states from the any% corpus movie).

use serde::{Deserialize, Serialize};

use crate::cpu::bus_read;
use crate::enh::{call_original, hook, patch_prg_at};
use crate::game::Game;

/// Engine-fix options. `Default` is the original game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FixesOpts {
    /// Level-up window softlock fixes.
    pub levelup_softlocks: bool,
    /// No skipped updates while the i-frame timer is 1-2.
    pub iframe_update_skip: bool,
    /// Same jump speed rule in both directions.
    pub jump_direction_balance: bool,
    /// Same shield hitbox in both facings.
    pub shield_hitbox_symmetry: bool,
    /// Crumble tiles check both feet.
    pub crumble_both_feet: bool,
    /// XP drain fix.
    pub xp_drain_fix: bool,
}

impl FixesOpts {
    /// Whether anything in this group is on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Append this group's stable identity encoding (fixed field order).
    pub fn write_identity(&self, out: &mut Vec<u8>) {
        for b in [
            self.levelup_softlocks,
            self.iframe_update_skip,
            self.jump_direction_balance,
            self.shield_hitbox_symmetry,
            self.crumble_both_feet,
            self.xp_drain_fix,
        ] {
            out.push(u8::from(b));
        }
    }
}

/// Trap reached by the patched `JSR` in `Hub_Update_Routine`.
pub const LEVELUP_GATE: u16 = 0xD3BB;
/// Trap reached by the patched `JSR` in `Link_Jumping_routine`.
pub const JUMP_RULE: u16 = 0xD3BE;

/// Dialog type (0 none, 1 level-up, 2 talking, ...).
const DIALOG: usize = 0x074C;
/// Game mode; `$0B` is the sideview main mode.
const MODE: usize = 0x0736;
const MODE_SIDEVIEW: u8 = 0x0B;
/// `EnhState::scratch` bytes holding the previous frame's dialog / mode.
const SCRATCH_DIALOG: usize = 5;
const SCRATCH_MODE: usize = 6;

/// Fixed-bank (bank 7) offset of `addr` in the PRG image (the last bank;
/// see [`super::bank_offset`]).
fn bank7_offset(game: &Game, addr: u16) -> usize {
    super::bank_offset(game.prg.len(), 7, addr)
}

/// Bytes at bank `bank`, CPU `addr` equal `want`.
fn bytes_are(game: &Game, bank: u8, addr: u16, want: &[u8]) -> bool {
    let off = super::bank_offset(game.prg.len(), bank, addr);
    game.prg.get(off..off + want.len()) == Some(want)
}

/// The unused fixed-bank byte at `addr` is still `$FF` (safe trap target).
fn free_trap_slot(game: &Game, addr: u16) -> bool {
    game.prg.get(bank7_offset(game, addr)) == Some(&0xFF)
}

/// Install this group's hooks ([`super::apply`], only when active).
pub(crate) fn register(game: &mut Game, opts: &FixesOpts) {
    if opts.levelup_softlocks
        && free_trap_slot(game, LEVELUP_GATE)
        && bytes_are(game, 0, 0x969C, &[0xA9, 0x01, 0x8D, 0x4C, 0x07])
    {
        hook(
            game,
            "enh_fixes_levelup_gate",
            Some(7),
            LEVELUP_GATE,
            levelup_gate,
            Some(18),
        );
        let [lo, hi] = LEVELUP_GATE.to_le_bytes();
        patch_prg_at(game, 0, 0x969C, &[0x20, lo, hi, 0xEA, 0xEA]);
    }
    if opts.iframe_update_skip && bytes_are(game, 7, 0xD51F, &[0xAD, 0x18, 0x05]) {
        // `$D52B BCS LD538` -> `BCS LD530`: skip the draw only.
        if bytes_are(game, 7, 0xD52B, &[0xB0, 0x0B]) {
            patch_prg_at(game, 7, 0xD52C, &[0x03]);
        }
    }
    if opts.jump_direction_balance
        && free_trap_slot(game, JUMP_RULE)
        && bytes_are(game, 0, 0x950D, &[0xA5, 0x70, 0x69, 0x13, 0xC9, 0x26])
    {
        hook(
            game,
            "enh_fixes_jump_rule",
            Some(7),
            JUMP_RULE,
            jump_rule,
            Some(20),
        );
        let [lo, hi] = JUMP_RULE.to_le_bytes();
        patch_prg_at(game, 0, 0x950D, &[0x20, lo, hi, 0xEA]);
    }
    if opts.shield_hitbox_symmetry {
        hook(
            game,
            "enh_fixes_shield_box",
            Some(7),
            0xE9D8,
            shield_box,
            None,
        );
    }
    if opts.crumble_both_feet {
        hook(
            game,
            "enh_fixes_crumble_feet",
            Some(7),
            0xE079,
            crumble_feet,
            None,
        );
    }
    if opts.xp_drain_fix && bytes_are(game, 7, 0xD445, &[0xC9, 0x0A]) {
        patch_prg_at(game, 7, 0xD446, &[0x0B]);
    }
}

/// Per-frame work after [`Game::step`] (only while any enhancement is on).
pub(crate) fn end_of_frame(game: &mut Game, opts: &FixesOpts) {
    if opts.levelup_softlocks {
        let prev_dialog = game.enh_state.scratch[SCRATCH_DIALOG];
        let prev_mode = game.enh_state.scratch[SCRATCH_MODE];
        if levelup_on_exit(prev_dialog, prev_mode, game.ram[DIALOG], game.ram[MODE]) {
            // Raised on the frame Link left the room: withdraw it. XP still
            // covers the cost, so the next room raises it again.
            game.ram[DIALOG] = 0;
        }
        game.enh_state.scratch[SCRATCH_DIALOG] = game.ram[DIALOG];
        game.enh_state.scratch[SCRATCH_MODE] = game.ram[MODE];
    }
    if opts.xp_drain_fix && game.ram[0x0775] == 0 && game.ram[0x0776] == 0 {
        // No XP left to drain: drop the rest of the drain.
        game.ram[0x05E8] = 0;
    }
}

/// The level-up window was raised this frame (dialog 0 -> 1) on the same
/// frame the sideview main mode was left.
#[must_use]
pub fn levelup_on_exit(prev_dialog: u8, prev_mode: u8, dialog: u8, mode: u8) -> bool {
    prev_dialog == 0 && prev_mode == MODE_SIDEVIEW && dialog == 1 && mode != MODE_SIDEVIEW
}

/// [`LEVELUP_GATE`]: the original `LDA #$01 : STA $074C`, skipped while a
/// dialog is open (`$074C != 0`). `A = 1` either way (callers reload it).
fn levelup_gate(game: &mut Game) {
    if game.ram[DIALOG] == 0 {
        game.ram[DIALOG] = 1;
    }
    game.cpu.a = 1;
}

/// The original high-jump test on X speed `v` with the carry `c` the `ADC`
/// picks up: `v + $13 + C >= $26` (8-bit).
#[must_use]
pub fn high_jump_og(v: u8, carry: bool) -> bool {
    v.wrapping_add(0x13).wrapping_add(u8::from(carry)) >= 0x26
}

/// The fixed test: the same with the carry clear (right `v >= $13`, left
/// `v <= -$14`).
#[must_use]
pub fn high_jump(v: u8) -> bool {
    high_jump_og(v, false)
}

/// [`JUMP_RULE`]: stands in for `LDA $70 : ADC #$13` with the carry clear.
/// The following `CMP #$26 : BCC` only looks at `A`.
fn jump_rule(game: &mut Game) {
    game.cpu.a = game.ram[0x70].wrapping_add(0x13);
}

/// `bank7_code45` hook: the original box, then the right-facing one
/// (`$9F = 1`) moved 2 px towards Link.
fn shield_box(game: &mut Game) {
    call_original(game, 0xE9D8);
    if game.ram[0x9F] == 1 {
        game.ram[0x00] = game.ram[0x00].wrapping_sub(2);
    }
}

/// Foot probes (`bank7_table28` / `LEAC0` index) checked besides the
/// original foot centre (`$1D`).
const FOOT_PROBES: [u8; 2] = [0x04, 0x05];
const FOOT_CENTRE: u8 = 0x1D;

/// What a level-collision probe read: row pointer, row, tile and the
/// probe's world column (16 px).
struct Cell {
    base: u16,
    row: u8,
    tile: u8,
    column: u16,
}

/// Run the generic level-collision test for Link (`X = 0`) at probe `y`
/// and read back its outputs (`$0E/$0F` row pointer, `$02` row, `$03`
/// tile), plus the probe's world column.
fn probe(game: &mut Game, y: u8) -> Cell {
    game.cpu.x = 0;
    game.cpu.y = y;
    crate::sideview_traps::sv_collision_test(game);
    let off = bus_read(game, 0xEAA0u16.wrapping_add(u16::from(y)));
    let world =
        (u16::from(game.ram[0x3B]) << 8 | u16::from(game.ram[0x4D])).wrapping_add(u16::from(off));
    Cell {
        base: u16::from(game.ram[0x0E]) | (u16::from(game.ram[0x0F]) << 8),
        row: game.ram[0x02],
        tile: game.ram[0x03],
        column: world >> 4,
    }
}

/// `$E079` hook: shatter a step-on breakable tile under either foot probe
/// (the original only looks under the foot centre), then the original tick.
fn crumble_feet(game: &mut Game) {
    if game.ram[0x13] == 0 {
        crumble_side_feet(game);
    }
    call_original(game, 0xE079);
}

fn crumble_side_feet(game: &mut Game) {
    // Probing writes scratch RAM and registers and charges cycles: keep the
    // routine's entry state for the original tick.
    let saved_zp: [u8; 16] = game.ram[0x00..0x10].try_into().unwrap_or([0; 16]);
    let (a, x, y, p, cycles) = (
        game.cpu.a,
        game.cpu.x,
        game.cpu.y,
        game.cpu.p,
        game.cpu.cycles,
    );
    let breakable = bus_read(game, 0x851F);
    let centre = probe(game, FOOT_CENTRE);
    let mut done: Option<u16> = None;
    for idx in FOOT_PROBES {
        let foot = probe(game, idx);
        if foot.column == centre.column || Some(foot.column) == done {
            continue;
        }
        if foot.tile != breakable || foot.row >= 0xD0 {
            continue;
        }
        shatter(game, &foot);
        done = Some(foot.column);
    }
    game.ram[0x00..0x10].copy_from_slice(&saved_zp);
    game.cpu.a = a;
    game.cpu.x = x;
    game.cpu.y = y;
    game.cpu.p = p;
    game.cpu.cycles = cycles;
}

/// The original step-on shatter (`LE0FC`) for `cell`: stamp tile `$8F`,
/// claim a debris slot (highest free of 4..0, else 0) with its level-RAM
/// pointer and tile-aligned world position, and play sound `$ED = 2`.
fn shatter(game: &mut Game, cell: &Cell) {
    crate::cpu::bus_write(game, cell.base.wrapping_add(u16::from(cell.row)), 0x8F);
    let slot = (0..=4usize)
        .rev()
        .find(|&s| game.ram[0x041A + s] == 0)
        .unwrap_or(0);
    let [lo, hi] = cell.base.to_le_bytes();
    game.ram[0x042E + slot] = cell.row;
    game.ram[0x0433 + slot] = lo;
    game.ram[0x0438 + slot] = hi;
    game.ram[0x041A + slot] = 0x81;
    game.ram[0xED] = 0x02;
    game.ram[0x0429 + slot] = game.ram[0x29].wrapping_add(0x20) & 0xF0;
    let world = cell.column << 4;
    game.ram[0x0424 + slot] = (world & 0xF0) as u8;
    game.ram[0x041F + slot] = (world >> 8) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_jump_needs_the_same_run_up_both_ways() {
        // After `n` frames of run-up the speed is `n + 1` right and
        // `-(n + 2)` left (measured in the ROM).
        let right = |n: u8| n + 1;
        let left = |n: u8| (n + 2).wrapping_neg();
        for n in 0u8..=0x17 {
            assert_eq!(high_jump(right(n)), high_jump(left(n)), "n {n}");
            // The stale carry splits them in the original.
            assert!(high_jump_og(right(n), true) >= high_jump_og(left(n), true));
        }
        assert!(high_jump_og(right(17), true) && !high_jump_og(left(17), true));
        assert!(!high_jump(right(17)) && high_jump(right(18)));
    }

    #[test]
    fn levelup_withdrawn_only_when_raised_on_an_exit_frame() {
        assert!(levelup_on_exit(0, 0x0B, 1, 0x10));
        assert!(!levelup_on_exit(1, 0x0B, 1, 0x10), "already open");
        assert!(!levelup_on_exit(0, 0x0B, 1, 0x0B), "same room");
        assert!(!levelup_on_exit(0, 0x10, 1, 0x07), "not from sideview");
        assert!(!levelup_on_exit(0, 0x0B, 2, 0x10), "talking");
    }

    #[test]
    fn levelup_gate_respects_open_dialogs() {
        let mut g = Game::new();
        g.ram[DIALOG] = 0;
        levelup_gate(&mut g);
        assert_eq!(g.ram[DIALOG], 1);
        g.ram[DIALOG] = 2;
        levelup_gate(&mut g);
        assert_eq!(g.ram[DIALOG], 2);
    }

    #[test]
    fn blank_image_gets_no_patches() {
        let mut g = Game::new();
        let prg = g.prg.clone();
        g.set_enhancements(crate::enh::Enhancements {
            fixes: FixesOpts {
                levelup_softlocks: true,
                iframe_update_skip: true,
                jump_direction_balance: true,
                shield_hitbox_symmetry: true,
                crumble_both_feet: true,
                xp_drain_fix: true,
            },
            ..crate::enh::Enhancements::default()
        });
        assert_eq!(g.prg, prg);
        assert_eq!(g.enh_hooks.patch_count(), 0);
        // Hook-only fixes still install (and undo cleanly).
        assert_eq!(g.enh_hooks.trap_count(), 2);
        g.set_enhancements(crate::enh::Enhancements::default());
        assert_eq!(g.enh_hooks.trap_count(), 0);
    }

    #[test]
    fn xp_drain_dropped_at_zero_xp() {
        let mut g = Game::new();
        let o = FixesOpts {
            xp_drain_fix: true,
            ..FixesOpts::default()
        };
        g.ram[0x05E8] = 7;
        g.ram[0x0776] = 1;
        end_of_frame(&mut g, &o);
        assert_eq!(g.ram[0x05E8], 7);
        g.ram[0x0776] = 0;
        end_of_frame(&mut g, &o);
        assert_eq!(g.ram[0x05E8], 0);
    }
}
