//! Wide gameplay: enemies live out in the widescreen margins (optional,
//! off by default, never part of verification).
//!
//! Widescreen ([`crate::wide_margins`]) paints scenery beside the 256-pixel
//! picture, but the game still spawns and removes its enemies at the edges
//! of that picture, so they pop in and out in plain view. This module moves
//! the spawn and despawn lines out past the margins, `M` pixels per side:
//!
//! * **Overworld encounter blobs** (mode `$05`). Two bank-0 traps, both
//!   registered only while the mode is on:
//!   * `$8284` (`overworld1`, the spawner): the ROM runs unchanged, then each
//!     blob it just placed to the left or right of Link (spawn table `$8229`,
//!     screen X `$40`/`$60` or `$A0`/`$C0`) is pushed a further `M` pixels
//!     outwards and its life timer (`$050E,x`, one tick per 21 frames) gets
//!     `ceil(M / 21)` extra ticks for the walk back. Blobs placed above or
//!     below Link stay where the ROM put them.
//!   * `$841B` (`L841B` + `L8332`, blob AI, movement, drawing, contact and
//!     despawn): a Rust port that keeps each blob's **extended screen X**
//!     (`i16`, window coordinates) next to the ROM's 8-bit `$4E,x`
//!     (`$4E = (ex + $FD) & $FF` always holds). A blob inside `0..248` is
//!     drawn into OAM exactly like the ROM; a blob further out gets OAM Y
//!     `$F8` (hidden) and is described as two [`MarginSprite`]s instead.
//!     Blobs despawn when `ex < -M` or `ex >= 256 + M - 8` (the ROM's own
//!     `ex >= 248` / wrap-below-zero rule at `M = 0`).
//!
//!   At `M = 0` the port reproduces the ROM byte for byte, cycle for cycle
//!   and register for register (pinned by a ROM-gated lockstep test over
//!   corpus movies), which is the correctness proof for the port.
//! * **Side-view enemies** (mode `$0B`): the list spawner `LD625`
//!   ([`crate::sideview_traps::sv_enemy_spawn`]) compares each list entry
//!   against a streaming column pushed [`sideview_spawn_shift`] 16-pixel
//!   columns further out; the ROM's own column bytes `$0732-$0735` are
//!   never written (bank 0 streams level columns from them).
//! * **Townsfolk** (bank 3 `$96E0`): their spawn offsets are a 4-byte table
//!   (`$96DC-$96DF`) the port cannot trap (banked), so the in-memory PRG
//!   copy is patched while the mode is on and restored when it goes off.
//!
//! Everything else (rocks, bubbles, the AI that removes objects at the window
//! edge, the despawn window) is untouched.
//!
//! # Display contract
//!
//! [`Game::overworld_margin_sprites`] is the list of margin sprites that
//! belong to the picture the last [`Game::step`] rendered, in window
//! coordinates and OAM priority order (blob slot 0 first). The hardware
//! shows OAM written by the previous frame's game logic, so the list the
//! port builds in frame `N` is latched and shown for frame `N + 1`, exactly
//! like the in-window blobs. The X values are the ones the port wrote at
//! draw time (before Link's step moves the scroll), so margin blobs move
//! against the background exactly like the ROM's own sprites.

use crate::bank7_common::{adc_val, cmp_val, inc_val, jsr_sub, sbc_val, set_nz};
use crate::cpu::FLAG_C;
use crate::game::{Game, CALL_ASM_BUDGET};
use z2_ppu::MarginSprite;

// ------------------------------------------------------------ constants

/// Overworld blob slots (`LDX #$07` loops).
pub const DEMON_SLOTS: usize = 8;
/// Most margin sprites one frame can hold (two 8x16 halves per blob).
pub const MAX_MARGIN_SPRITES: usize = 2 * DEMON_SLOTS;
/// Widest supported margin in tiles per side (the widescreen maximum).
pub const MAX_MARGIN_TILES: u8 = 16;
/// Frames per blob life-timer tick (`$050E,x`, NMI sweep every 21 frames).
pub const LIFE_TICK_FRAMES: u16 = 21;

/// `overworld1` (bank 0): blob spawner.
pub const ADDR_SPAWN: u16 = 0x8284;
/// `L841B` (bank 0): blob AI, then the `L8332` move/draw/contact loop.
pub const ADDR_BLOBS: u16 = 0x841B;
/// Trap names (the netplay trap-set identity includes them).
pub const NAME_SPAWN: &str = "wide_overworld1";
/// See [`NAME_SPAWN`].
pub const NAME_BLOBS: &str = "wide_L841B";

/// Game mode byte (`$0736`).
const MODE: usize = 0x0736;
/// Overworld main mode.
const MODE_OVERWORLD: u8 = 0x05;
/// Frame counter.
const FRAME: usize = 0x12;
/// Blob Y (`$2A,x`), X (`$4E,x`), type (`$82,x`: 1 weak, 2 strong, 3 fairy).
const BLOB_Y: usize = 0x2A;
const BLOB_X: usize = 0x4E;
const BLOB_TYPE: usize = 0x82;
/// Blob Y / X velocity (`$056D,x` / `$0575,x`).
const BLOB_VY: usize = 0x056D;
const BLOB_VX: usize = 0x0575;
/// Blob life timer (`$050E,x`).
const BLOB_LIFE: usize = 0x050E;
/// Loop slot mirror (`$80`).
const SLOT: usize = 0x80;
/// Scroll X / Y (`$FD` / `$7F`).
const SCROLL_X: usize = 0xFD;
const SCROLL_Y: usize = 0x7F;
/// Link's OAM X (`$0203`, written by `L8726` after the blobs run).
const LINK_OAM_X: usize = 0x0203;
/// OAM page base of blob slot 0 (`$0280 + 16 * slot`).
const BLOB_OAM: usize = 0x0280;
/// First byte of the NMI RNG (`$051B,x` feeds the random walk).
const RNG: usize = 0x051B;
/// Blob palette by type (`L8281`) and tiles (`L8275`/`L8276`, indexed by
/// `(type - 1) * 4 + ((frame & 8) >> 2)`), read from the live PRG.
const PALETTE_TABLE: u16 = 0x8281;
const TILE_LEFT_TABLE: u16 = 0x8275;
const TILE_RIGHT_TABLE: u16 = 0x8276;

/// Townsfolk spawn offsets (bank 3 `$96DC-$96DF`: low left, low right,
/// high left, high right) — the vanilla bytes (`-32`, `+264`).
pub const TOWN_OFFSETS_VANILLA: [u8; 4] = [0xE0, 0x08, 0xFF, 0x01];
/// PRG image offset of bank 3 `$96DC`.
const TOWN_OFFSETS_PRG: usize = 3 * 0x4000 + (0x96DC - 0x8000);

// ------------------------------------------------------------ state

/// Wide-gameplay state (part of [`crate::state::GameState`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WideGameplayState {
    /// Mode on (traps registered, PRG patched).
    pub enabled: bool,
    /// Margin width in pixels per side (`8 * tiles`).
    pub margin_px: u8,
    /// Extended screen X per blob slot (window coordinates).
    pub ex: [i16; DEMON_SLOTS],
    /// Slots whose [`WideGameplayState::ex`] is known (bit per slot).
    pub tracked: u8,
    /// Slots whose OAM entries the port hid this frame (bit per slot).
    pub hidden: u8,
    /// Unhidden OAM Y of each slot as last computed (read back by the ROM's
    /// clobbered-`Y` despawn quirk when it lands on a hidden slot).
    pub shadow_oy: [u8; DEMON_SLOTS],
    /// Margin sprites the current frame's logic produced (shown next frame).
    pub pending: [MarginSprite; MAX_MARGIN_SPRITES],
    /// Length of [`WideGameplayState::pending`].
    pub pending_len: u8,
    /// Margin sprites belonging to the last rendered picture.
    pub shown: [MarginSprite; MAX_MARGIN_SPRITES],
    /// Length of [`WideGameplayState::shown`].
    pub shown_len: u8,
    /// The blob loop ran during the current frame.
    pub ran_this_frame: bool,
    /// Blobs pushed out at spawn (lifetime counter, diagnostics/tests).
    pub n_pushed: u64,
    /// Blob-frames spent alive in a margin (lifetime counter).
    pub n_margin_frames: u64,
    /// Blob passes run (lifetime counter).
    pub n_passes: u64,
}

impl Default for WideGameplayState {
    fn default() -> Self {
        Self {
            enabled: false,
            margin_px: 0,
            ex: [0; DEMON_SLOTS],
            tracked: 0,
            hidden: 0,
            shadow_oy: [0; DEMON_SLOTS],
            pending: [MarginSprite::default(); MAX_MARGIN_SPRITES],
            pending_len: 0,
            shown: [MarginSprite::default(); MAX_MARGIN_SPRITES],
            shown_len: 0,
            ran_this_frame: false,
            n_pushed: 0,
            n_margin_frames: 0,
            n_passes: 0,
        }
    }
}

impl WideGameplayState {
    /// Margin sprites of the last rendered picture.
    #[must_use]
    pub fn shown(&self) -> &[MarginSprite] {
        &self.shown[..usize::from(self.shown_len)]
    }

    /// Margin sprites the current frame's logic produced.
    #[must_use]
    pub fn pending(&self) -> &[MarginSprite] {
        &self.pending[..usize::from(self.pending_len)]
    }

    /// Drop all per-blob tracking and both sprite lists.
    pub fn clear_blobs(&mut self) {
        self.tracked = 0;
        self.hidden = 0;
        self.pending_len = 0;
        self.shown_len = 0;
        self.ran_this_frame = false;
    }

    fn latch(&mut self) {
        self.shown = self.pending;
        self.shown_len = self.pending_len;
        self.pending_len = 0;
    }

    fn push_sprite(&mut self, s: MarginSprite) {
        let n = usize::from(self.pending_len);
        if n < MAX_MARGIN_SPRITES {
            self.pending[n] = s;
            self.pending_len += 1;
        }
    }

    /// Right despawn bound: `ex >= 256 + M - 8` removes a blob.
    #[must_use]
    pub fn right_bound(&self) -> i16 {
        256 + i16::from(self.margin_px) - 8
    }

    /// Left despawn bound: `ex < -M` removes a blob.
    #[must_use]
    pub fn left_bound(&self) -> i16 {
        -i16::from(self.margin_px)
    }
}

// ------------------------------------------------------------ pure helpers

/// Extra life-timer ticks for a blob pushed `margin_px` further out: the
/// blobs walk one pixel per frame, so the walk back takes `margin_px`
/// frames, `ceil(margin_px / 21)` ticks.
#[must_use]
pub fn life_bonus_ticks(margin_px: u8) -> u8 {
    u16::from(margin_px).div_ceil(LIFE_TICK_FRAMES) as u8
}

/// Extended X of a blob the spawner just placed at 8-bit screen X `s8`:
/// left spawns (`s8 < $80`) move `margin_px` left, right spawns
/// (`s8 > $80`) move right, above/below spawns (`s8 == $80`) stay.
/// Returns `(ex, pushed)`.
#[must_use]
pub fn spawn_push(s8: u8, margin_px: u8) -> (i16, bool) {
    let m = i16::from(margin_px);
    match s8.cmp(&0x80) {
        core::cmp::Ordering::Less => (i16::from(s8) - m, m != 0),
        core::cmp::Ordering::Greater => (i16::from(s8) + m, m != 0),
        core::cmp::Ordering::Equal => (i16::from(s8), false),
    }
}

/// The representative of `s8` (mod 256) nearest to `prev` — how a tracked
/// blob's extended X follows the ROM's 8-bit X from frame to frame (the
/// per-frame change is a few pixels at most).
#[must_use]
pub fn follow(prev: i16, s8: u8) -> i16 {
    let d = s8.wrapping_sub(prev as u8) as i8;
    prev.wrapping_add(i16::from(d))
}

/// Whether a blob at extended X `ex` is removed (`M` = `margin_px`).
#[must_use]
pub fn out_of_bounds(ex: i16, margin_px: u8) -> bool {
    let m = i16::from(margin_px);
    ex < -m || ex >= 256 + m - 8
}

/// Townsfolk spawn offsets for a margin: `[lo_left, lo_right, hi_left,
/// hi_right]` of `-(32 + M)` and `264 + M` (the vanilla `-32` / `+264`
/// already leave the 16-pixel walker just outside the window).
#[must_use]
pub fn town_offsets(margin_px: u8) -> [u8; 4] {
    let left = (-32 - i16::from(margin_px)) as u16;
    let right = (264 + i16::from(margin_px)) as u16;
    [
        left as u8,
        right as u8,
        (left >> 8) as u8,
        (right >> 8) as u8,
    ]
}

/// Measured side-view spawn distances at `M = 0` (window coordinates of the
/// spawned enemy's left edge relative to the camera): the left streaming
/// column puts enemies at `-99..=-88`, the right one at `+329..=+340`,
/// depending on the camera's position inside its 16-pixel column.
pub const SV_LEFT_SPAWN_MAX: i16 = -88;
/// See [`SV_LEFT_SPAWN_MAX`].
pub const SV_RIGHT_SPAWN_MIN: i16 = 329;
/// Width of a side-view enemy for the "fully outside" rule.
pub const SV_ENEMY_W: i16 = 16;

/// Columns to shift the side-view spawn comparison outwards so a freshly
/// spawned 16-pixel enemy is entirely outside an `M`-pixel margin:
/// `(left, right)`, each `>= 0`.
#[must_use]
pub fn sideview_spawn_shift(margin_px: u8) -> (u8, u8) {
    let m = i16::from(margin_px);
    // Left: x + 16 <= -M for every x <= SV_LEFT_SPAWN_MAX - 16k.
    let need_l = (SV_LEFT_SPAWN_MAX + SV_ENEMY_W + m).max(0);
    // Right: x >= 256 + M for every x >= SV_RIGHT_SPAWN_MIN + 16k.
    let need_r = (256 + m - SV_RIGHT_SPAWN_MIN).max(0);
    (
        (need_l as u16).div_ceil(16) as u8,
        (need_r as u16).div_ceil(16) as u8,
    )
}

/// Shift a side-view streaming column `(page, col)` by `delta` 16-pixel
/// columns along the area: linear, no wrap (a column before page 0 comes
/// back with page `$FF`, which no list entry can match).
#[must_use]
pub fn shift_column(page: u8, col: u8, delta: i16) -> (u8, u8) {
    let lin = i16::from(page as i8) * 16 + i16::from(col & 0x0F) + delta;
    ((lin.div_euclid(16)) as u8, (lin.rem_euclid(16)) as u8)
}

// ------------------------------------------------------------ Game glue

/// [`Game::set_wide_gameplay`].
pub fn set(game: &mut Game, margin_tiles: Option<u8>) {
    match margin_tiles {
        Some(t) => {
            let px = t.min(MAX_MARGIN_TILES) * 8;
            if !game.wide_game.enabled || game.wide_game.margin_px != px {
                game.wide_game.clear_blobs();
            }
            game.wide_game.enabled = true;
            game.wide_game.margin_px = px;
            register_traps(game);
            patch_town_offsets(game, Some(px));
        }
        None => {
            unregister_traps(game);
            patch_town_offsets(game, None);
            let st = &mut game.wide_game;
            st.enabled = false;
            st.margin_px = 0;
            st.clear_blobs();
        }
    }
}

/// Register the two overworld traps (idempotent). NOT part of any
/// verification trap set; [`set`] calls it.
pub fn register_traps(game: &mut Game) {
    game.trap_register_cycles(NAME_SPAWN, Some(0), ADDR_SPAWN, wide_spawn, 0);
    game.trap_register_cycles(NAME_BLOBS, Some(0), ADDR_BLOBS, wide_blobs, 0);
}

fn unregister_traps(game: &mut Game) {
    for (addr, name) in [(ADDR_SPAWN, NAME_SPAWN), (ADDR_BLOBS, NAME_BLOBS)] {
        if game.traps.get(addr).map(|t| t.name) == Some(name) {
            game.traps.unregister(addr);
        }
    }
}

/// Patch (`Some(margin_px)`) or restore (`None`) the townsfolk spawn table
/// in the in-memory PRG copy. A PRG image whose bytes there are not the
/// vanilla ones (another ROM revision, a synthetic test image) is left
/// alone.
fn patch_town_offsets(game: &mut Game, margin_px: Option<u8>) {
    let at = TOWN_OFFSETS_PRG;
    if game.prg.len() < at + 4 {
        return;
    }
    let cur = [
        game.prg[at],
        game.prg[at + 1],
        game.prg[at + 2],
        game.prg[at + 3],
    ];
    let known =
        cur == TOWN_OFFSETS_VANILLA || (0..=MAX_MARGIN_TILES).any(|t| cur == town_offsets(t * 8));
    if !known {
        return;
    }
    let want = match margin_px {
        Some(px) => town_offsets(px),
        None => TOWN_OFFSETS_VANILLA,
    };
    game.prg[at..at + 4].copy_from_slice(&want);
}

/// Re-apply registration and the PRG patch after a snapshot restore
/// ([`Game::load_state`]).
pub(crate) fn after_load(game: &mut Game) {
    if game.wide_game.enabled {
        register_traps(game);
        patch_town_offsets(game, Some(game.wide_game.margin_px));
    } else {
        unregister_traps(game);
        patch_town_offsets(game, None);
    }
}

/// End-of-frame observer ([`Game::step`], only while enabled): when the
/// blob loop did not run this frame, the OAM page was not rewritten, so the
/// picture just rendered showed the previous frame's blobs and the next one
/// will show them again: the pending list is shown and kept. Outside the
/// overworld it is dropped after that one frame.
pub(crate) fn end_of_frame(game: &mut Game) {
    let st = &mut game.wide_game;
    if !st.ran_this_frame {
        st.shown = st.pending;
        st.shown_len = st.pending_len;
        if game.ram[MODE] != MODE_OVERWORLD {
            st.pending_len = 0;
            st.tracked = 0;
            st.hidden = 0;
        }
    }
    st.ran_this_frame = false;
}

/// FNV-1a over the state that influences future frames (mix into a netplay
/// desync hash alongside RAM/WRAM).
#[must_use]
pub fn hash(st: &WideGameplayState) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut f = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    f(u8::from(st.enabled));
    f(st.margin_px);
    for e in st.ex {
        for b in e.to_le_bytes() {
            f(b);
        }
    }
    f(st.tracked);
    f(st.hidden);
    for b in st.shadow_oy {
        f(b);
    }
    h
}

/// Whether bank 0 is mapped at `$8000` (the trap table is keyed by address
/// only, so a bank-0 trap must decline in every other bank).
fn bank0_mapped(game: &Game) -> bool {
    game.mmc1.map_prg(ADDR_SPAWN, game.prg.len()) < 0x4000
}

/// Run the ROM bytes at `target` inside the frame the dispatcher is about to
/// return through; dead stack bytes and cycles are exactly the ROM's.
fn run_rom_in_frame(game: &mut Game, target: u16) {
    let sp = game.cpu.sp;
    if let Err(e) = game.run_until_sp(target, sp.wrapping_add(2), CALL_ASM_BUDGET) {
        panic!("wide gameplay: ROM body ${target:04X}: {e}");
    }
    game.cpu.sp = sp;
}

// ------------------------------------------------------------ $8284 spawn

/// `$8284` wrapper: the ROM spawner, then push new left/right blobs out.
fn wide_spawn(game: &mut Game) {
    if !game.wide_game.enabled || !bank0_mapped(game) {
        run_rom_in_frame(game, ADDR_SPAWN);
        return;
    }
    let mut before = [0u8; DEMON_SLOTS];
    before.copy_from_slice(&game.ram[BLOB_TYPE..BLOB_TYPE + DEMON_SLOTS]);
    run_rom_in_frame(game, ADDR_SPAWN);
    let m = game.wide_game.margin_px;
    let fd = game.ram[SCROLL_X];
    for (s, &was) in before.iter().enumerate() {
        if was != 0 || game.ram[BLOB_TYPE + s] == 0 {
            continue;
        }
        let s8 = game.ram[BLOB_X + s].wrapping_sub(fd);
        let (ex, pushed) = spawn_push(s8, m);
        let st = &mut game.wide_game;
        st.ex[s] = ex;
        st.tracked |= 1 << s;
        st.hidden &= !(1 << s);
        if pushed {
            st.n_pushed += 1;
            game.ram[BLOB_X + s] = (ex as u8).wrapping_add(fd);
            let life = &mut game.ram[BLOB_LIFE + s];
            *life = life.saturating_add(life_bonus_ticks(m));
        }
    }
}

// ------------------------------------------------------------ $841B blobs

fn cyc(game: &mut Game, n: u64) {
    game.cpu.cycles += n;
}

/// Page-cross extra cycle of an indexed read (`base + idx`).
fn cross(base: u16, idx: u8) -> u64 {
    u64::from((base & 0xFF00) != (base.wrapping_add(u16::from(idx)) & 0xFF00))
}

fn lda(game: &mut Game, v: u8) {
    game.cpu.a = v;
    set_nz(&mut game.cpu.p, v);
}

fn ldx(game: &mut Game, v: u8) {
    game.cpu.x = v;
    set_nz(&mut game.cpu.p, v);
}

fn ldy(game: &mut Game, v: u8) {
    game.cpu.y = v;
    set_nz(&mut game.cpu.p, v);
}

fn set_c(game: &mut Game, c: bool) {
    if c {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
}

fn carry(game: &Game) -> bool {
    game.cpu.p & FLAG_C != 0
}

fn cmp_a(game: &mut Game, v: u8) {
    let a = game.cpu.a;
    cmp_val(&mut game.cpu.p, a, v);
}

/// `ADC v` into `A`.
fn adc(game: &mut Game, v: u8) {
    let a = game.cpu.a;
    game.cpu.a = adc_val(&mut game.cpu.p, a, v);
}

/// `SBC v` into `A`.
fn sbc(game: &mut Game, v: u8) {
    let a = game.cpu.a;
    game.cpu.a = sbc_val(&mut game.cpu.p, a, v);
}

/// `DEX` (+ `STX $80` where the loop mirrors it).
fn dex(game: &mut Game) {
    let x = game.cpu.x.wrapping_sub(1);
    ldx(game, x);
}

/// Extended X of slot `s` at the top of the blob pass: follow the ROM's
/// 8-bit X from last frame's value (scroll and spawns moved it since).
fn reconcile(game: &mut Game, s: usize) -> i16 {
    let s8 = game.ram[BLOB_X + s].wrapping_sub(game.ram[SCROLL_X]);
    let st = &mut game.wide_game;
    let ex = if st.tracked & (1 << s) != 0 {
        follow(st.ex[s], s8)
    } else {
        i16::from(s8)
    };
    st.ex[s] = ex;
    st.tracked |= 1 << s;
    ex
}

/// Store extended X and keep `$4E,x = (ex + $FD) & $FF`.
fn put_ex(game: &mut Game, s: usize, ex: i16) {
    game.wide_game.ex[s] = ex;
    game.ram[BLOB_X + s] = (ex as u8).wrapping_add(game.ram[SCROLL_X]);
}

/// `$841B` wrapper (mode on): the blob pass.
fn wide_blobs(game: &mut Game) {
    if !bank0_mapped(game) {
        // Another bank's `$841B`: nothing in the ROM calls it, but never run
        // the blob port on foreign bytes.
        run_rom_in_frame(game, ADDR_BLOBS);
        return;
    }
    let st = &mut game.wide_game;
    st.latch();
    st.ran_this_frame = true;
    st.n_passes += 1;
    // Every slot with a type gets steered (even one whose timer ran out this
    // frame; the move loop removes it afterwards), so all of them follow.
    for s in 0..DEMON_SLOTS {
        if game.ram[BLOB_TYPE + s] != 0 {
            reconcile(game, s);
        } else {
            game.wide_game.tracked &= !(1 << s);
        }
    }
    blob_ai(game);
    if move_draw_collide(game) {
        return;
    }
    // Pending list in OAM priority order: slot 0 (lowest OAM index) first.
    let st = &mut game.wide_game;
    let n = usize::from(st.pending_len);
    st.pending[..n].reverse();
}

/// `L841B`: every 16th frame, zero velocities and pick new ones (chase or
/// random walk).
fn blob_ai(game: &mut Game) {
    // LDA $12 : AND #$0F : BNE L8432
    let f = game.ram[FRAME];
    lda(game, f & 0x0F);
    cyc(game, 3 + 2 + 2);
    if f & 0x0F != 0 {
        // L8432: JMP L8332
        cyc(game, 1 + 3);
        return;
    }
    ldx(game, 7);
    cyc(game, 2);
    loop {
        let x = game.cpu.x;
        let s = usize::from(x);
        // LDA #0 : STA vy : STA vx : LDA type : BNE
        game.ram[BLOB_VY + s] = 0;
        game.ram[BLOB_VX + s] = 0;
        let ty = game.ram[BLOB_TYPE + s];
        lda(game, ty);
        cyc(game, 2 + 5 + 5 + 4 + 2);
        if ty != 0 {
            cyc(game, 1);
            blob_steer(game, s);
        }
        // L842F: DEX : BPL L8423
        dex(game);
        cyc(game, 2 + 2);
        if game.cpu.x & 0x80 != 0 {
            break;
        }
        cyc(game, 1);
    }
    // JMP L8332
    cyc(game, 3);
}

/// A blob's X for the chase compares: extended with a margin, the ROM's
/// 8-bit screen X at `M = 0` (a blob one pixel left of the window reads as
/// `$FF` there, and the port must agree).
fn chase_x(game: &Game, s: usize) -> i16 {
    let ex = game.wide_game.ex[s];
    if game.wide_game.margin_px == 0 {
        i16::from(ex as u8)
    } else {
        ex
    }
}

/// `L8435`: chase Link (weak/strong blobs once `$12 >= $40`) or random walk.
fn blob_steer(game: &mut Game, s: usize) {
    // LDA type : CMP #3 : BCS L8488
    let ty = game.ram[BLOB_TYPE + s];
    lda(game, ty);
    cmp_a(game, 3);
    cyc(game, 4 + 2 + 2);
    let mut chase = false;
    if ty < 3 {
        // LDA $12 : CMP #$40 : BCC L8488
        let f = game.ram[FRAME];
        lda(game, f);
        cmp_a(game, 0x40);
        cyc(game, 3 + 2 + 2);
        chase = f >= 0x40;
        if !chase {
            cyc(game, 1);
        }
    } else {
        cyc(game, 1);
    }
    if chase {
        // LDA #$70 : SBC y : CLC : ADC $7F : CLC : ADC #$10 : CMP #$20
        lda(game, 0x70);
        let y = game.ram[BLOB_Y + s];
        sbc(game, y);
        set_c(game, false);
        let sy = game.ram[SCROLL_Y];
        adc(game, sy);
        set_c(game, false);
        adc(game, 0x10);
        cmp_a(game, 0x20);
        cyc(game, 2 + 4 + 2 + 3 + 2 + 2 + 2 + 2);
        if !carry(game) {
            // Level with Link: step horizontally towards him.
            // LDY #1 : LDA x : SEC : SBC $FD : CMP $0203 : BCC : LDY #$FF
            ldy(game, 1);
            let ex = chase_x(game, s);
            let link = game.ram[LINK_OAM_X];
            lda(game, ex as u8);
            set_c(game, true);
            // Flags of `CMP $0203` against the 8-bit value, with the carry
            // taken from the extended compare (identical inside 0..256).
            cmp_a(game, link);
            set_c(game, ex >= i16::from(link));
            cyc(game, 2 + 4 + 2 + 3 + 4 + 2);
            if carry(game) {
                ldy(game, 0xFF);
                cyc(game, 2);
            } else {
                cyc(game, 1);
            }
            // L845D: TYA : STA vx : JMP L842F
            let v = game.cpu.y;
            lda(game, v);
            game.ram[BLOB_VX + s] = v;
            cyc(game, 2 + 5 + 3);
            return;
        }
        cyc(game, 1);
        // L8464: LDA $0203 : SEC : SBC x : CLC : ADC $FD : CLC : ADC #$10 : CMP #$20
        let ex = chase_x(game, s);
        let link = game.ram[LINK_OAM_X];
        let d = i16::from(link) - ex + 0x10;
        lda(game, link);
        set_c(game, true);
        let x8 = game.ram[BLOB_X + s];
        sbc(game, x8);
        set_c(game, false);
        let fd = game.ram[SCROLL_X];
        adc(game, fd);
        set_c(game, false);
        adc(game, 0x10);
        cmp_a(game, 0x20);
        // Extended: the band test must not wrap for blobs out in a margin.
        set_c(game, !(0..0x20).contains(&d));
        cyc(game, 4 + 2 + 4 + 2 + 3 + 2 + 2 + 2 + 2);
        if !carry(game) {
            // In Link's column: step vertically towards him.
            // LDY #1 : LDA y : SEC : SBC $7F : CMP #$70 : BCC : LDY #$FF
            ldy(game, 1);
            let y = game.ram[BLOB_Y + s];
            lda(game, y);
            set_c(game, true);
            let sy = game.ram[SCROLL_Y];
            sbc(game, sy);
            cmp_a(game, 0x70);
            cyc(game, 2 + 4 + 2 + 3 + 2 + 2);
            if carry(game) {
                ldy(game, 0xFF);
                cyc(game, 2);
            } else {
                cyc(game, 1);
            }
            // L8481: TYA : STA vy : JMP L842F
            let v = game.cpu.y;
            lda(game, v);
            game.ram[BLOB_VY + s] = v;
            cyc(game, 2 + 5 + 3);
            return;
        }
        cyc(game, 1);
    }
    // L8488: random walk one pixel along one axis, from the RNG byte.
    ldy(game, 1);
    let r = game.ram[RNG + s];
    lda(game, r);
    cyc(game, 2 + 4 + 2);
    if r & 0x80 != 0 {
        ldy(game, 0xFF);
        cyc(game, 2);
    } else {
        cyc(game, 1);
    }
    // L8491: AND #4 : BNE L84A1
    lda(game, r & 0x04);
    cyc(game, 2 + 2);
    let v = game.cpu.y;
    if r & 0x04 == 0 {
        // TYA : STA vy : CLC : ADC y : STA y : JMP L842F
        lda(game, v);
        game.ram[BLOB_VY + s] = v;
        set_c(game, false);
        let y = game.ram[BLOB_Y + s];
        adc(game, y);
        game.ram[BLOB_Y + s] = game.cpu.a;
        cyc(game, 2 + 5 + 2 + 4 + 4 + 3);
    } else {
        cyc(game, 1);
        // L84A1: TYA : STA vx : CLC : ADC x : STA x : JMP L842F
        lda(game, v);
        game.ram[BLOB_VX + s] = v;
        set_c(game, false);
        let x8 = game.ram[BLOB_X + s];
        adc(game, x8);
        let ex = game.wide_game.ex[s] + i16::from(v as i8);
        put_ex(game, s, ex);
        cyc(game, 2 + 5 + 2 + 4 + 4 + 3);
    }
}

/// `L8332`: move, draw, contact and despawn every blob. Returns `true` when
/// a blob touched Link (the pass then left through `L85D5`).
fn move_draw_collide(game: &mut Game) -> bool {
    // LDX #7 : STX $80
    ldx(game, 7);
    game.ram[SLOT] = 7;
    cyc(game, 2 + 3);
    let frame_sp = game.cpu.sp;
    loop {
        let s = usize::from(game.cpu.x);
        // L8336: LDA life : BEQ L833F : LDA type : BNE L8342
        let life = game.ram[BLOB_LIFE + s];
        lda(game, life);
        cyc(game, 4 + 2);
        let live = if life == 0 {
            cyc(game, 1);
            false
        } else {
            let ty = game.ram[BLOB_TYPE + s];
            lda(game, ty);
            cyc(game, 4 + 2);
            ty != 0
        };
        let remove = if live {
            cyc(game, 1);
            match blob_live(game, s, frame_sp) {
                Live::Kept => false,
                Live::Removed => true,
                Live::Hit => return true,
            }
        } else {
            // L833F: JMP L840B
            cyc(game, 3);
            true
        };
        if remove {
            // L840B: LDA #0 : STA type : STA life
            lda(game, 0);
            game.ram[BLOB_TYPE + s] = 0;
            game.ram[BLOB_LIFE + s] = 0;
            game.wide_game.tracked &= !(1 << s);
            cyc(game, 2 + 4 + 5);
        }
        // L8412: DEX : STX $80 : BMI L841A : JMP L8336
        dex(game);
        game.ram[SLOT] = game.cpu.x;
        cyc(game, 2 + 3 + 2);
        if game.cpu.x & 0x80 != 0 {
            // L841A: RTS (a cost-0 trap charges its own return).
            cyc(game, 1 + 6);
            break;
        }
        cyc(game, 3);
    }
    false
}

enum Live {
    Kept,
    Removed,
    Hit,
}

/// `L8342-$840A` for one live blob.
fn blob_live(game: &mut Game, s: usize, frame_sp: u8) -> Live {
    let x = s as u8;
    let oam = x << 4;
    // TXA : ASL x4 : TAY
    game.cpu.y = oam;
    // LDA y : CLC : ADC vy : STA y : SEC : SBC $7F : STA oam.y (both halves)
    let y = game.ram[BLOB_Y + s];
    lda(game, y);
    set_c(game, false);
    let vy = game.ram[BLOB_VY + s];
    adc(game, vy);
    game.ram[BLOB_Y + s] = game.cpu.a;
    set_c(game, true);
    let sy = game.ram[SCROLL_Y];
    sbc(game, sy);
    let oy = game.cpu.a;
    // LDA x : CLC : ADC vx : STA x : SEC : SBC $FD : STA oam.x
    let x8 = game.ram[BLOB_X + s];
    lda(game, x8);
    set_c(game, false);
    let vx = game.ram[BLOB_VX + s];
    adc(game, vx);
    let ex = game.wide_game.ex[s] + i16::from(vx as i8);
    put_ex(game, s, ex);
    set_c(game, true);
    let fd = game.ram[SCROLL_X];
    sbc(game, fd);
    let ox = game.cpu.a;
    // CLC : ADC #8 : STA oam2.x
    set_c(game, false);
    adc(game, 8);
    let ox2 = game.cpu.a;
    cyc(
        game,
        2 + 8 + 2 + 4 + 2 + 4 + 4 + 2 + 3 + 5 + 5 + 4 + 2 + 4 + 4 + 2 + 3 + 5 + 2 + 2 + 5,
    );
    // TXA : PHA — the slot index is left as a dead stack byte.
    game.ram[0x0100 + usize::from(frame_sp)] = x;
    // LDA $12 : AND #8 : LSR : LSR : STA $07
    let phase = (game.ram[FRAME] & 0x08) >> 2;
    game.ram[0x07] = phase;
    // Palette by type, tiles by (type - 1) * 4 + phase (ASL : ASL : CLC :
    // ADC $07 leaves C/V from that add).
    let ty = game.ram[BLOB_TYPE + s];
    let t = ty.wrapping_sub(1);
    let mut attr = rom_table(game, PALETTE_TABLE, t);
    set_c(game, false);
    let tix = adc_val(&mut game.cpu.p, t << 2, phase);
    let tile_l = rom_table(game, TILE_LEFT_TABLE, tix);
    let tile_r = rom_table(game, TILE_RIGHT_TABLE, tix);
    cyc(
        game,
        2 + 3 + 3 + 2 + 2 + 2 + 3 + 4 + 2 + 2 + 4 + 5 + 5 + 2 + 2 + 2 + 2 + 3 + 2 + 4 + 5 + 4 + 5,
    );
    // PLA : TAX : LDA $07 : BEQ L83AB
    ldx(game, x);
    lda(game, phase);
    cyc(game, 4 + 2 + 3 + 2);
    if phase == 0 {
        cyc(game, 1);
    } else {
        // LDA type : CMP #2 : BNE L83AB : LDA #$42 (strong blobs flash)
        lda(game, ty);
        cmp_a(game, 2);
        cyc(game, 4 + 2 + 2);
        if ty == 2 {
            lda(game, 0x42);
            attr = 0x42;
            cyc(game, 2 + 5 + 5);
        } else {
            cyc(game, 1);
        }
    }

    // Draw: in the window like the ROM; out in a margin hidden in OAM and
    // described as margin sprites instead. A blob about to be removed is
    // drawn like the ROM draws it (it is gone before the picture shows it).
    let m = game.wide_game.margin_px;
    let doomed = out_of_bounds(ex, m);
    let hide = !(0..248).contains(&ex) && !doomed;
    let shown_y = if hide { 0xF8 } else { oy };
    let base = BLOB_OAM + usize::from(oam);
    game.ram[base..base + 8]
        .copy_from_slice(&[shown_y, tile_l, attr, ox, shown_y, tile_r, attr, ox2]);
    let st = &mut game.wide_game;
    st.shadow_oy[s] = oy;
    if hide {
        st.hidden |= 1 << s;
        st.n_margin_frames += 1;
    } else {
        st.hidden &= !(1 << s);
    }
    if m != 0 && !doomed {
        // Right half first: the finished list is reversed into OAM order.
        for (hx, tile) in [(ex + 8, tile_r), (ex, tile_l)] {
            if hide || hx < 0 || hx + 8 > 256 {
                st.push_sprite(MarginSprite {
                    x: hx,
                    y: oy,
                    tile,
                    attr,
                });
            }
        }
    }

    // L83AB: contact box OAM Y $64-$75, X $7A-$85 (extended X with a
    // margin: a margin blob whose low byte lands in the box is not touching
    // Link; the ROM's 8-bit X at `M = 0`).
    lda(game, oy);
    cmp_a(game, 0x64);
    cyc(game, 4 + 2 + 2);
    let mut in_box = false;
    if oy < 0x64 {
        cyc(game, 1);
    } else {
        cmp_a(game, 0x76);
        cyc(game, 2 + 2);
        if oy >= 0x76 {
            cyc(game, 1);
        } else {
            lda(game, ox);
            cmp_a(game, 0x7A);
            let bx = if m == 0 { i16::from(ox) } else { ex };
            set_c(game, bx >= 0x7A);
            cyc(game, 4 + 2 + 2);
            if !carry(game) {
                cyc(game, 1);
            } else {
                cmp_a(game, 0x86);
                set_c(game, bx >= 0x86);
                cyc(game, 2 + 2);
                if carry(game) {
                    cyc(game, 1);
                } else {
                    in_box = true;
                }
            }
        }
    }
    if in_box {
        // LDA $70 : PHA : JSR Blocked_by_Tile : PLA : STA $70 : LDX $80
        let v70 = game.ram[0x70];
        lda(game, v70);
        game.ram[0x0100 + usize::from(game.cpu.sp)] = v70;
        game.cpu.sp = game.cpu.sp.wrapping_sub(1);
        cyc(game, 3 + 3 + 6);
        jsr_sub(game, 0x83C4, 0x870F);
        game.cpu.sp = game.cpu.sp.wrapping_add(1);
        let a = game.ram[0x0100 + usize::from(game.cpu.sp)];
        lda(game, a);
        game.ram[0x70] = a;
        let slot = game.ram[SLOT];
        ldx(game, slot);
        // BCS L83FD (blocked: no encounter)
        cyc(game, 4 + 3 + 3 + 2);
        if carry(game) {
            cyc(game, 1);
        } else {
            // LDA $0563 : CMP #4 : BCC L83FD : CMP #$0D : BEQ L83FD
            let terr = game.ram[0x0563];
            lda(game, terr);
            cmp_a(game, 4);
            cyc(game, 4 + 2 + 2);
            if terr < 4 {
                cyc(game, 1);
            } else {
                cmp_a(game, 0x0D);
                cyc(game, 2 + 2);
                if terr == 0x0D {
                    cyc(game, 1);
                } else {
                    blob_hit(game, s);
                    return Live::Hit;
                }
            }
        }
    }

    // L83FD: removed when OAM Y >= $F8 or OAM X >= $F8 (extended: outside
    // the margin bounds). `Blocked_by_Tile` can leave `Y` pointing at another
    // slot's OAM (Link without boots facing walkable water), in which case
    // the ROM tests that slot's bytes; so does the port, reading a hidden
    // slot through its shadow.
    let yreg = game.cpu.y;
    let rb = BLOB_OAM + usize::from(yreg);
    let (ry, rx) = (game.ram[rb], game.ram[rb + 3]);
    let k = usize::from(yreg >> 4);
    let (vy, x_out) = if yreg == oam {
        (oy, out_of_bounds(ex, m))
    } else if yreg & 0x0F == 0 && k < DEMON_SLOTS && game.wide_game.hidden & (1 << k) != 0 {
        let st = &game.wide_game;
        (st.shadow_oy[k], out_of_bounds(st.ex[k], m))
    } else {
        (ry, rx >= 0xF8)
    };
    // LDA oam.y : CMP #$F8 : BCS L840B
    lda(game, ry);
    cmp_a(game, 0xF8);
    set_c(game, vy >= 0xF8);
    cyc(game, 4 + cross(0x0280, yreg) + 2 + 2);
    if vy >= 0xF8 {
        cyc(game, 1);
        return Live::Removed;
    }
    // LDA oam.x : CMP #$F8 : BCC L8412
    lda(game, rx);
    cmp_a(game, 0xF8);
    set_c(game, x_out);
    cyc(game, 4 + cross(0x0283, yreg) + 2 + 2);
    if x_out {
        return Live::Removed;
    }
    cyc(game, 1);
    Live::Kept
}

/// `LDA table,x` from the mapped PRG (bank 0 is mapped, see
/// [`bank0_mapped`]).
fn rom_table(game: &Game, base: u16, idx: u8) -> u8 {
    let addr = base.wrapping_add(u16::from(idx));
    game.prg[game.mmc1.map_prg(addr, game.prg.len())]
}

/// `$83CF-$83FA`: a blob touched Link — set up the encounter and leave the
/// whole overworld step through `L85D5`.
fn blob_hit(game: &mut Game, s: usize) {
    // LDY #$FF : STY $0748 : INY : STY $075A : LDA #1 : LDY type : CPY #3
    game.ram[0x0748] = 0xFF;
    game.ram[0x075A] = 0;
    lda(game, 1);
    let ty = game.ram[BLOB_TYPE + s];
    ldy(game, ty);
    let yv = game.cpu.y;
    cmp_val(&mut game.cpu.p, yv, 3);
    cyc(game, 2 + 4 + 2 + 4 + 2 + 4 + 2 + 2);
    if ty == 3 {
        cyc(game, 1);
    } else {
        // STY $075A : LSR
        game.ram[0x075A] = ty;
        lda(game, 0);
        set_c(game, true);
        cyc(game, 4 + 2);
    }
    // L83EE: STA $0759 : PLA : PLA : LDA #2 : STA $EE : INC $07AC : JMP L85D5
    game.ram[0x0759] = game.cpu.a;
    game.cpu.sp = game.cpu.sp.wrapping_add(2);
    lda(game, 2);
    game.ram[0xEE] = 2;
    let v = inc_val(&mut game.cpu.p, game.ram[0x07AC]);
    game.ram[0x07AC] = v;
    cyc(game, 4 + 4 + 4 + 2 + 3 + 6 + 3);
    game.wide_game.pending_len = 0;
    game.trap_jump(0x85D5);
}
