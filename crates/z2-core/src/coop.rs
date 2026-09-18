//! Local two-player co-op: a second Link inside one [`Game`] (post-parity,
//! flag-gated, never part of lockstep verification).
//!
//! # Model: time-sliced Link bytes
//!
//! The ROM's Link driver, display and contact code all work on one fixed set
//! of RAM bytes. Player 2 is a *shadow block* of those bytes
//! ([`SWAP_ADDRS`]); around every P2 pass the block is exchanged with the live
//! bytes, the ROM (or its Rust port) runs once more on P2's state, and the
//! bytes are exchanged back. Nothing else in RAM is per-player: level, exp,
//! magic, items, lives, camera, enemies and RNG are shared.
//!
//! Hooks (all registered by [`register_coop_traps`], a separate trap group
//! that no verification path registers):
//!
//! * `$D5A7` (enemy loop, called once per sideview frame after P1's driver and
//!   the camera): the P2 *update slice* runs P2's driver on pad 2 first, then
//!   the original enemy loop.
//! * `$EBF0` (Link display): P1 draws, then P2 draws; P2's eight OAM entries
//!   are moved to spare slots ([`P2_OAM_SLOTS`]) with a different sprite
//!   palette.
//! * `$E4D9` (enemy body vs Link box), `$D6C1` (enemy contact hit), `$E677`
//!   (sword hit), `$E558` (enemy weapon vs Link): one pass per player; a
//!   per-enemy-slot touch mask tells the `$D6C1` pass which Link was touched.
//!
//! During the P2 update slice the screen-lock byte `$0728` is held at 1 so
//! screen edges act as walls for P2 (P2 never scrolls the camera or takes a
//! side exit), and grounded Up is masked (doors, elevators and NPC talk stay
//! P1-only). P2 is clamped into the visible window after each slice.
//!
//! # Default off / parity
//!
//! [`CoopState::enabled`] defaults to `false` and nothing registers the trap
//! group by default. With the group registered but the flag off, every
//! wrapper runs exactly what the default trap set runs (the original shim,
//! or the ROM bytes inside the caller's own frame with identical cycle
//! charges), so the game is byte-identical (pinned by a ROM-gated test).
//!
//! All P2 work is charged **zero** CPU cycles (its cycles are rewound): a
//! real charge (~10 % of a frame) makes the main loop lag a frame earlier
//! than a one-player run. The slices only touch RAM/OAM-RAM and the MMC1 PRG
//! register, never the PPU/APU, so rewinding the clock cannot desynchronise
//! the beam.
//!
//! While enabled, the PPU runs with [`SpriteLimit::Unlimited`] (two Links
//! side by side exceed 8 sprites per scanline); the previous limit is
//! restored on disable. Zelda II never reads the sprite-overflow status bit,
//! so this is not game-observable.
//!
//! # Limitations (v1)
//!
//! P2 exists only in sideview (mode `$0B`) and is hidden on the overworld;
//! it cannot cast spells, fire sword beams, use doors/elevators or talk;
//! enemy AI (aiming, facing, bosses) tracks P1 only; clamping moves P2
//! without a collision check; same-frame sound requests from both players
//! collide (last writer wins); pause/dialogs/level-up are P1-driven; when P1
//! is blinking (invulnerable) on odd frames the ROM skips the display call,
//! so P2 blinks too; P2 sprites can hide an enemy projectile that lands in
//! the same spare slot on the same frame; P2 deaths cost no lives and P2
//! respawns next to P1.

use crate::bank7_common::{inner_jsr_frame, push_word};
use crate::game::{Game, BTN_UP, CALL_ASM_BUDGET};
use crate::traps::TrapFn;
use z2_ppu::SpriteLimit;

// ------------------------------------------------------------ constants

/// Link bytes exchanged with the P2 block around every P2 pass, in block
/// order. Meaning: `$13` fairy, `$14` dx, `$15` sprite X, `$17` shield, `$19`
/// fall/visible, `$29` Y, `$3B` page, `$4D` X, `$5F` facing, `$70` hspeed,
/// `$80` anim, `$9F` direction, `$A7` collision bits, `$AE` walk-anim
/// counter, `$B5` driver state, `$C8` offscreen bits, `$CC` screen X, `$03D6`
/// x-sub, `$03E6` gravity counter, `$0400` slash frame, `$044A` B-press
/// direction, `$0479` midair, `$047E`/`$0480` sword anchors, `$0494` kill,
/// `$0495` duck, `$0497` crouch timer, `$049C`/`$049D` held item,
/// `$0501`/`$0502`/`$0503`/`$050A`/`$050C` timers (`$050C` injured), `$0518`
/// invulnerability, `$057D` vspeed, `$0741-$0743` button latches,
/// `$0752`/`$0753` water, `$0754` in-elevator, `$05E7`, `$070E` chimney,
/// `$0774` HP, `$075B` door counter.
///
/// Never swapped (shared): frame counter `$12`, RNG `$051A-$0522`, camera
/// `$072A-$072D`, screen lock `$0728`, mode `$0736`, spells, exp, items,
/// lives, containers, every enemy row, sound bytes, scratch `$00-$11`.
pub const SWAP_ADDRS: [u16; SWAP_LEN] = [
    0x0013, 0x0014, 0x0015, 0x0017, 0x0019, 0x0029, 0x003B, 0x004D, 0x005F, 0x0070, 0x0080, 0x009F,
    0x00A7, 0x00AE, 0x00B5, 0x00C8, 0x00CC, 0x03D6, 0x03E6, 0x0400, 0x044A, 0x0479, 0x047E, 0x0480,
    0x0494, 0x0495, 0x0497, 0x049C, 0x049D, 0x0501, 0x0502, 0x0503, 0x050A, 0x050C, 0x0518, 0x057D,
    0x0741, 0x0742, 0x0743, 0x0752, 0x0753, 0x0754, 0x05E7, 0x070E, 0x0774, 0x075B,
];
/// Length of [`SWAP_ADDRS`] / the P2 block.
pub const SWAP_LEN: usize = 46;

/// Block index of a swapped address (compile-time; panics for an address
/// outside [`SWAP_ADDRS`]).
pub const fn idx(addr: u16) -> usize {
    let mut i = 0;
    while i < SWAP_LEN {
        if SWAP_ADDRS[i] == addr {
            return i;
        }
        i += 1;
    }
    panic!("address not in SWAP_ADDRS");
}

pub const IX_FAIRY: usize = idx(0x0013);
pub const IX_DX: usize = idx(0x0014);
pub const IX_Y: usize = idx(0x0029);
pub const IX_PAGE: usize = idx(0x003B);
pub const IX_X: usize = idx(0x004D);
pub const IX_HSPEED: usize = idx(0x0070);
pub const IX_ANIM: usize = idx(0x0080);
pub const IX_STATE: usize = idx(0x00B5);
pub const IX_SCREEN_X: usize = idx(0x00CC);
pub const IX_MIDAIR: usize = idx(0x0479);
pub const IX_KILL: usize = idx(0x0494);
pub const IX_CROUCH: usize = idx(0x0497);
pub const IX_T0501: usize = idx(0x0501);
pub const IX_T0502: usize = idx(0x0502);
pub const IX_T0503: usize = idx(0x0503);
pub const IX_T050A: usize = idx(0x050A);
pub const IX_INJURED: usize = idx(0x050C);
pub const IX_INVULN: usize = idx(0x0518);
pub const IX_HP: usize = idx(0x0774);

/// P2 OAM destination slots for Link's eight entries (source slots 2..=9).
/// Body (2-7) -> 56-61, sword/held item (8-9) -> 18-19: slots no corpus
/// sideview frame ever used.
pub const P2_OAM_SLOTS: [u8; 8] = [56, 57, 58, 59, 60, 61, 18, 19];
/// Link's OAM entries as the display routine writes them (slots 2..=9).
pub const LINK_OAM_FIRST: usize = 0x0208;
/// One past the last Link OAM byte.
pub const LINK_OAM_END: usize = 0x0228;
/// Default P2 spawn offset right of P1 on (re)anchor, pixels.
pub const P2_X_OFFSET: u8 = 0x20;
/// Frames P2 stays down after dying before it respawns next to P1.
pub const P2_RESPAWN_FRAMES: u16 = 60;
/// Sprite attribute XOR for P2 (sprite palette 0 -> 1).
pub const P2_ATTR_XOR: u8 = 0x01;
/// Held-Up bit in the MSB-first pad bytes `$F5-$F8`.
pub const HELD_UP: u8 = 0x08;
/// Hidden-sprite Y value.
const OAM_HIDDEN_Y: u8 = 0xF8;
/// Enemy slots with a touch mask (`X` = 0..5 in the contact code).
pub const ENEMY_SLOTS: usize = 6;
/// Addresses the co-op group traps.
pub const COOP_TRAP_ADDRS: [u16; 6] = [0xD5A7, 0xEBF0, 0xE4D9, 0xD6C1, 0xE677, 0xE558];

/// Mode byte and the sideview mode value.
const MODE: usize = 0x0736;
const MODE_SIDEVIEW: u8 = 0x0B;
/// Timer reload counter (`$0500`): the slow timers tick when it reloads.
const TIMER_RELOAD: usize = 0x0500;
const TIMER_RELOAD_VALUE: u8 = 0x14;
/// Screen lock / frame counter / pad bytes.
const SCREEN_LOCK: usize = 0x0728;
const FRAME_CTR: usize = 0x0012;
const PAD1_EDGE: usize = 0x00F5;
const PAD2_EDGE: usize = 0x00F6;
const PAD1_HELD: usize = 0x00F7;
const PAD2_HELD: usize = 0x00F8;
/// Camera left edge (page, pixel).
const CAM_PAGE: usize = 0x072A;
const CAM_X: usize = 0x072C;
/// Heart containers (feeds the refill value).
const CONTAINERS: usize = 0x0783;
/// Enemy contact flags row and the touched bit.
const CONTACT_ROW: usize = 0x00A8;
const TOUCH_BIT: u8 = 0x10;
/// Enemy slot state row (red-jar pickup changes it).
const SLOT_STATE_ROW: usize = 0x00B6;
/// Held item byte (a red-jar pickup changes it).
const HELD_ITEM: usize = 0x049C;
/// JSR sites used for the synthetic frames around P2's driver calls.
const SITE_D3E9: u16 = 0xD3E9;
const SITE_D4F3: u16 = 0xD4F3;
const SITE_D501: u16 = 0xD501;
const SITE_DE40: u16 = 0xDE40;
const SITE_D6C7: u16 = 0xD6C7;
/// Swap PRG bank 0 in / swap the saved bank back / Link driver.
const SWAP_TO_PRG0: u16 = 0xFFC5;
const SWAP_TO_SAVED: u16 = 0xFFC9;
const LINK_DRIVER: u16 = 0x903A;

// ------------------------------------------------------------ state

/// Co-op tunables (part of [`Game::coop_hash`]; netplay peers must agree, so
/// frontends should keep the defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoopOptions {
    /// P2 spawn offset right of P1, pixels.
    pub p2_x_offset: u8,
    /// Sprite attribute XOR applied to P2's sprites.
    pub p2_attr_xor: u8,
    /// Run the PPU with [`SpriteLimit::Unlimited`] while enabled.
    pub unlimited_sprites: bool,
    /// Frames P2 stays down after dying.
    pub respawn_frames: u16,
    /// Mask grounded Up for P2 (doors/elevators/talk are P1-only).
    pub mask_grounded_up: bool,
    /// Run the per-player contact passes for P2 (enemy body/weapons, sword).
    /// Off makes P2 a non-interacting ghost (used by the slice-parity test).
    pub p2_contact: bool,
}

impl Default for CoopOptions {
    fn default() -> Self {
        Self {
            p2_x_offset: P2_X_OFFSET,
            p2_attr_xor: P2_ATTR_XOR,
            unlimited_sprites: true,
            respawn_frames: P2_RESPAWN_FRAMES,
            mask_grounded_up: true,
            p2_contact: true,
        }
    }
}

/// Per-[`Game`] co-op state.
#[derive(Debug, Clone)]
pub struct CoopState {
    /// Runtime flag ([`Game::set_coop`]); default `false`.
    pub enabled: bool,
    /// Tunables.
    pub opts: CoopOptions,
    /// P2 is live in the current sideview area.
    pub active: bool,
    /// P2's Link bytes (valid while `active`), [`SWAP_ADDRS`] order.
    pub block: [u8; SWAP_LEN],
    /// True while the P2 update slice runs (nested display guard).
    pub in_update: bool,
    /// True while the co-op display wrapper runs.
    pub in_display: bool,
    /// Per enemy slot: bit 0 = P1 touched it in the last `$E4D9` pass, bit 1
    /// = P2 touched it.
    pub touch_mask: [u8; ENEMY_SLOTS],
    /// P2 HP carried across areas (0 = take P1's / full on next anchor).
    pub p2_hp: u8,
    /// Frames left while P2 is down (0 = alive or inactive).
    pub respawn_timer: u16,
    /// P2 deaths so far.
    pub p2_deaths: u32,
    /// Diagnostics (not hashed): cycles rewound this frame / in total.
    pub cycles_saved_frame: u64,
    /// Lifetime rewound cycles.
    pub cycles_saved_total: u64,
    /// Update slices / display passes / nested display calls run.
    pub n_update: u64,
    pub n_display: u64,
    pub n_nested_display: u64,
    /// P2 contact events (P2 touched an enemy body, or a P2 sword/weapon pass
    /// reported a hit).
    pub n_p2_contacts: u64,
    pub(crate) saved_sprite_limit: Option<SpriteLimit>,
}

impl Default for CoopState {
    fn default() -> Self {
        Self {
            enabled: false,
            opts: CoopOptions::default(),
            active: false,
            block: [0; SWAP_LEN],
            in_update: false,
            in_display: false,
            touch_mask: [0; ENEMY_SLOTS],
            p2_hp: 0,
            respawn_timer: 0,
            p2_deaths: 0,
            cycles_saved_frame: 0,
            cycles_saved_total: 0,
            n_update: 0,
            n_display: 0,
            n_nested_display: 0,
            n_p2_contacts: 0,
            saved_sprite_limit: None,
        }
    }
}

/// Read-only co-op view for frontends (HUD / status line).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CoopStatus {
    /// P2 is live in the current area (false on the overworld / loaders).
    pub active: bool,
    /// `active` and not down.
    pub p2_alive: bool,
    /// P2's HP (the live block while active, else the carried value).
    pub p2_hp: u8,
    /// Full meter for the current containers.
    pub hp_max: u8,
    /// Frames until P2 respawns (0 when alive).
    pub respawn_frames: u16,
    /// P2 deaths so far.
    pub p2_deaths: u32,
    /// P2 world X (page:pixel) and Y.
    pub p2_page: u8,
    pub p2_x: u8,
    pub p2_y: u8,
    /// P2 screen X as the display code last computed it.
    pub p2_screen_x: u8,
}

// ------------------------------------------------------------ pure helpers

/// Pad-2 filter applied by [`Game::step2`]: while co-op is enabled the exact
/// manual-save chord (Up+A held on pad 2, see
/// [`crate::save::manual_save_gate`]) loses Up, so P2 input can never
/// trigger save-and-quit. Everything else passes through.
pub fn mask_pad2(enabled: bool, pad2: u8) -> u8 {
    // Pad bytes are LSB-first (A = bit 0); the ROM's held byte is MSB-first.
    if enabled && crate::save::manual_save_gate(pad2.reverse_bits()) {
        pad2 & !BTN_UP
    } else {
        pad2
    }
}

/// Exchange the live Link bytes with the P2 block.
pub fn swap_block(ram: &mut [u8; 0x800], block: &mut [u8; SWAP_LEN]) {
    for (b, &a) in block.iter_mut().zip(SWAP_ADDRS.iter()) {
        core::mem::swap(&mut ram[usize::from(a)], b);
    }
}

/// Per-frame P2 timer ticks the NMI performs for P1's bytes: `$0501`,
/// `$0502`, `$0503`, `$050A`, `$050C` every frame, `$0518` only on reload
/// frames, and the crouch timer `$0497` (forcing anim 6 while it runs).
pub fn tick_p2_timers(block: &mut [u8; SWAP_LEN], reload_frame: bool) {
    for i in [IX_T0501, IX_T0502, IX_T0503, IX_T050A, IX_INJURED] {
        block[i] = block[i].saturating_sub(1);
    }
    if reload_frame {
        block[IX_INVULN] = block[IX_INVULN].saturating_sub(1);
    }
    if block[IX_CROUCH] != 0 {
        block[IX_CROUCH] -= 1;
        block[IX_ANIM] = 6;
    }
}

/// Clamp P2 into the visible window `[left, left + 240]` (camera left edge
/// `$072A:$072C`); a clamped P2 loses its horizontal speed.
pub fn clamp_p2_into_window(block: &mut [u8; SWAP_LEN], ram: &[u8; 0x800]) {
    let left = (u16::from(ram[CAM_PAGE]) << 8) | u16::from(ram[CAM_X]);
    let x = (u16::from(block[IX_PAGE]) << 8) | u16::from(block[IX_X]);
    let right = left.saturating_add(240);
    let nx = x.clamp(left, right);
    if nx != x {
        block[IX_PAGE] = (nx >> 8) as u8;
        block[IX_X] = nx as u8;
        block[IX_HSPEED] = 0;
    }
}

/// P2 is dead: its kill counter is set with the injured timer run out (the
/// sideview death gate's rule), or it fell below the play field while
/// airborne (the screen lock skips the ROM's pit check).
pub fn p2_is_dead(block: &[u8; SWAP_LEN]) -> bool {
    (block[IX_KILL] != 0 && block[IX_INJURED] == 0)
        || (block[IX_Y] >= 0xE4 && block[IX_MIDAIR] != 0)
}

/// Blink gate the NMI applies before Link's display call, for P2's bytes:
/// hidden while `0 < invulnerability < 3` on odd frames.
pub fn p2_blink_hidden(block: &[u8; SWAP_LEN], frame_ctr: u8) -> bool {
    let t = block[IX_INVULN];
    t != 0 && t < 3 && frame_ctr & 1 != 0
}

/// Copy P2's eight Link OAM entries (`p2`, slots 2..=9 layout) into
/// [`P2_OAM_SLOTS`] with the attribute XOR applied (hidden entries too, so a
/// hidden P2 hides its slots).
pub fn relocate_p2_oam(ram: &mut [u8; 0x800], p2: &[u8; 32], attr_xor: u8) {
    for (i, &slot) in P2_OAM_SLOTS.iter().enumerate() {
        let src = &p2[i * 4..i * 4 + 4];
        let dst = 0x0200 + usize::from(slot) * 4;
        ram[dst] = src[0];
        ram[dst + 1] = src[1];
        ram[dst + 2] = if src[0] == OAM_HIDDEN_Y {
            src[2]
        } else {
            src[2] ^ attr_xor
        };
        ram[dst + 3] = src[3];
    }
}

/// Build P2's block from P1's live bytes: offset right by `x_offset` (page
/// carry), HP = `hp` (or P1's when 0), no kill/injury/invulnerability/speed,
/// driver state 1 (normal play), no fairy; then clamp into the window.
pub fn anchor_block(ram: &[u8; 0x800], x_offset: u8, hp: u8) -> [u8; SWAP_LEN] {
    let mut block = [0u8; SWAP_LEN];
    for (b, &a) in block.iter_mut().zip(SWAP_ADDRS.iter()) {
        *b = ram[usize::from(a)];
    }
    let (x, carry) = block[IX_X].overflowing_add(x_offset);
    block[IX_X] = x;
    if carry {
        block[IX_PAGE] = block[IX_PAGE].wrapping_add(1);
    }
    block[IX_HP] = if hp == 0 { ram[0x0774] } else { hp };
    for i in [IX_KILL, IX_INJURED, IX_INVULN, IX_HSPEED, IX_DX, IX_FAIRY] {
        block[i] = 0;
    }
    block[IX_STATE] = 1;
    clamp_p2_into_window(&mut block, ram);
    block
}

/// FNV-1a over the co-op state that influences future frames (diagnostic
/// counters excluded).
pub fn hash(state: &CoopState) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(u8::from(state.enabled));
    mix(u8::from(state.active));
    mix(u8::from(state.in_update));
    mix(u8::from(state.in_display));
    state.block.iter().for_each(|&b| mix(b));
    state.touch_mask.iter().for_each(|&b| mix(b));
    mix(state.p2_hp);
    state
        .respawn_timer
        .to_le_bytes()
        .iter()
        .for_each(|&b| mix(b));
    state.p2_deaths.to_le_bytes().iter().for_each(|&b| mix(b));
    let o = &state.opts;
    mix(o.p2_x_offset);
    mix(o.p2_attr_xor);
    mix(u8::from(o.unlimited_sprites));
    o.respawn_frames.to_le_bytes().iter().for_each(|&b| mix(b));
    mix(u8::from(o.mask_grounded_up));
    mix(u8::from(o.p2_contact));
    h
}

// ------------------------------------------------------------ Game glue

/// [`Game::set_coop`]: flip the flag; enabling also registers the trap group
/// (idempotent) and switches the sprite limit; disabling restores the limit
/// and drops P2 from the area. Register the default trap groups first.
pub fn set_enabled(game: &mut Game, on: bool) {
    if on {
        register_coop_traps(game);
        if !game.coop.enabled && game.coop.opts.unlimited_sprites {
            game.coop.saved_sprite_limit = Some(game.ppu.model().sprite_limit());
            game.ppu
                .model_mut()
                .set_sprite_limit(SpriteLimit::Unlimited);
        }
    } else if game.coop.enabled {
        if let Some(limit) = game.coop.saved_sprite_limit.take() {
            game.ppu.model_mut().set_sprite_limit(limit);
        }
        leave_area(&mut game.coop);
    }
    game.coop.enabled = on;
}

/// [`Game::coop_status`].
pub fn status(game: &Game) -> Option<CoopStatus> {
    let st = &game.coop;
    if !st.enabled {
        return None;
    }
    let b = &st.block;
    Some(CoopStatus {
        active: st.active,
        p2_alive: st.active && st.respawn_timer == 0,
        p2_hp: if st.active { b[IX_HP] } else { st.p2_hp },
        hp_max: crate::save::refill_meter(game.ram[CONTAINERS]),
        respawn_frames: st.respawn_timer,
        p2_deaths: st.p2_deaths,
        p2_page: b[IX_PAGE],
        p2_x: b[IX_X],
        p2_y: b[IX_Y],
        p2_screen_x: b[IX_SCREEN_X],
    })
}

/// Drop P2 from the current area (it re-anchors at P1 on the next sideview
/// frame). Carries P2's HP when it leaves alive.
pub fn leave_area(st: &mut CoopState) {
    if st.active && st.respawn_timer == 0 {
        st.p2_hp = st.block[IX_HP];
    }
    st.active = false;
    st.respawn_timer = 0;
    st.touch_mask = [0; ENEMY_SLOTS];
    st.in_update = false;
    st.in_display = false;
}

/// End-of-frame observer ([`Game::step`], only while enabled): P2 leaves the
/// area whenever the frame ends outside sideview main (overworld, area
/// loaders, death/continue screens), so every area entry re-anchors it.
pub(crate) fn end_of_frame(game: &mut Game) {
    if game.ram[MODE] != MODE_SIDEVIEW && game.coop.active {
        leave_area(&mut game.coop);
    }
}

/// Register the co-op trap group. NOT part of any verification trap set.
/// Call after the default groups: it overrides `$D6C1` (enemy group) and
/// `$E677`/`$E558` (player group), inheriting their cycle costs so the
/// disabled wrappers charge exactly what the originals did. Idempotent.
/// When an original is absent the override is skipped (P2 then has no pass
/// through that routine).
pub fn register_coop_traps(game: &mut Game) {
    if game.traps.get(0xD5A7).map(|t| t.name) == Some("coop_code21") {
        return;
    }
    game.trap_register_cycles("coop_code21", Some(7), 0xD5A7, coop_code21, 0);
    game.trap_register_cycles("coop_links_display", Some(7), 0xEBF0, coop_links_display, 0);
    game.trap_register_cycles("coop_le4d9", Some(7), 0xE4D9, coop_le4d9, 0);
    let overrides: [(&'static str, u16, TrapFn); 3] = [
        ("coop_link_collision", 0xD6C1, coop_link_collision),
        ("coop_sword_hit", 0xE677, coop_sword_hit),
        ("coop_code39", 0xE558, coop_code39),
    ];
    for (name, addr, f) in overrides {
        if let Some(cycles) = game.traps.get(addr).map(|t| t.cycles) {
            game.trap_register_cycles(name, Some(7), addr, f, cycles);
        }
    }
}

/// Run the ROM bytes at `target` inside the frame the dispatcher is about to
/// return through (the `JSR`'s return or, for a `JMP`-reached routine, the
/// caller's): the ROM's own `RTS` pops it, then `SP` is put back so the
/// dispatcher's emulated return pops the same bytes. Dead stack bytes and
/// cycles are exactly the ROM's (the wrapper is registered at cost 0).
fn run_rom_in_frame(game: &mut Game, target: u16) {
    let sp = game.cpu.sp;
    if let Err(e) = game.run_until_sp(target, sp.wrapping_add(2), CALL_ASM_BUDGET) {
        panic!("coop: ROM body ${target:04X}: {e}");
    }
    game.cpu.sp = sp;
}

/// `JSR target` from `site` with a synthetic frame (P2 slices only).
fn run_rom_jsr(game: &mut Game, site: u16, target: u16) {
    let sp = game.cpu.sp;
    push_word(game, site.wrapping_add(2));
    if let Err(e) = game.run_until_sp(target, sp, CALL_ASM_BUDGET) {
        panic!("coop: JSR ${target:04X}: {e}");
    }
}

/// Run `f` and rewind the cycles it consumed; returns them.
fn zero_cost(game: &mut Game, f: impl FnOnce(&mut Game)) -> u64 {
    let c0 = game.cpu.cycles;
    f(game);
    let d = game.cpu.cycles.wrapping_sub(c0);
    game.cpu.cycles = c0;
    game.coop.cycles_saved_frame = game.coop.cycles_saved_frame.wrapping_add(d);
    game.coop.cycles_saved_total = game.coop.cycles_saved_total.wrapping_add(d);
    d
}

fn swap(game: &mut Game) {
    swap_block(&mut game.ram, &mut game.coop.block);
}

/// P2 is live and not down, in sideview.
fn p2_live(game: &Game) -> bool {
    let st = &game.coop;
    st.enabled && st.active && st.respawn_timer == 0 && game.ram[MODE] == MODE_SIDEVIEW
}

fn p2_contact_live(game: &Game) -> bool {
    p2_live(game) && game.coop.opts.p2_contact && !game.coop.in_update
}

/// Saved `A/X/Y/P`.
#[derive(Clone, Copy)]
struct Regs(u8, u8, u8, u8);

fn regs(game: &Game) -> Regs {
    Regs(game.cpu.a, game.cpu.x, game.cpu.y, game.cpu.p)
}

fn set_regs(game: &mut Game, r: Regs) {
    game.cpu.a = r.0;
    game.cpu.x = r.1;
    game.cpu.y = r.2;
    game.cpu.p = r.3;
}

/// One P2 pass of a Rust shim inside a synthetic `JSR` frame, on P2's bytes,
/// charged zero cycles. Tail jumps run inside the frame; a non-local exit
/// the shim hands back is dropped (the P1 pass owns the control flow).
fn p2_shim_pass(game: &mut Game, x: u8, entry: Regs, site: u16, body: fn(&mut Game)) {
    swap(game);
    set_regs(game, entry);
    game.cpu.x = x;
    zero_cost(game, |g| inner_jsr_frame(g, site, body));
    let _ = game.take_trap_jump();
    swap(game);
}

/// Activation/respawn observer, run at the top of every sideview `$D5A7`.
fn observe(game: &mut Game) {
    let r = &game.ram;
    let in_sv = r[MODE] == MODE_SIDEVIEW && r[0x00B5] == 1 && r[0x0503] == 0;
    let st = &game.coop;
    if !st.active {
        if in_sv {
            let block = anchor_block(&game.ram, st.opts.p2_x_offset, st.p2_hp);
            game.coop.block = block;
            game.coop.active = true;
            game.coop.touch_mask = [0; ENEMY_SLOTS];
        }
    } else if st.respawn_timer > 0 {
        game.coop.respawn_timer -= 1;
        if game.coop.respawn_timer == 0 {
            let hp = crate::save::refill_meter(game.ram[CONTAINERS]);
            game.coop.block = anchor_block(&game.ram, game.coop.opts.p2_x_offset, hp);
        }
    }
}

/// `$D5A7` wrapper: P2 update slice, then the ROM enemy loop.
fn coop_code21(game: &mut Game) {
    if game.coop.enabled && game.ram[MODE] == MODE_SIDEVIEW {
        game.coop.cycles_saved_frame = 0;
        observe(game);
        if p2_live(game) {
            p2_update_slice(game);
        }
    }
    run_rom_in_frame(game, 0xD5A7);
}

fn p2_update_slice(game: &mut Game) {
    let entry = regs(game);
    game.coop.in_update = true;
    let reload = game.ram[TIMER_RELOAD] == TIMER_RELOAD_VALUE;
    tick_p2_timers(&mut game.coop.block, reload);
    swap(game);
    let saved = (
        game.ram[PAD1_EDGE],
        game.ram[PAD1_HELD],
        game.ram[SCREEN_LOCK],
    );
    game.ram[PAD1_EDGE] = game.ram[PAD2_EDGE];
    game.ram[PAD1_HELD] = game.ram[PAD2_HELD];
    game.ram[SCREEN_LOCK] = 1;
    if game.coop.opts.mask_grounded_up && game.ram[0x0479] == 0 {
        game.ram[PAD1_HELD] &= !HELD_UP;
    }
    zero_cost(game, |g| {
        run_rom_jsr(g, SITE_D3E9, SWAP_TO_PRG0);
        run_rom_jsr(g, SITE_D4F3, LINK_DRIVER);
        run_rom_jsr(g, SITE_D501, SWAP_TO_SAVED);
    });
    game.coop.n_update += 1;
    game.ram[PAD1_EDGE] = saved.0;
    game.ram[PAD1_HELD] = saved.1;
    game.ram[SCREEN_LOCK] = saved.2;
    swap(game);
    let st = &mut game.coop;
    clamp_p2_into_window(&mut st.block, &game.ram);
    if p2_is_dead(&st.block) {
        st.p2_deaths += 1;
        st.p2_hp = 0;
        st.respawn_timer = st.opts.respawn_frames.max(1);
        st.block[IX_STATE] = 0;
        st.touch_mask = [0; ENEMY_SLOTS];
    } else {
        st.p2_hp = st.block[IX_HP];
    }
    st.in_update = false;
    set_regs(game, entry);
}

/// `$EBF0` wrapper: P1 display, then P2 display relocated to spare slots.
fn coop_links_display(game: &mut Game) {
    if game.coop.in_update {
        // The driver's own tail jump to the display during the P2 slice.
        game.coop.n_nested_display += 1;
        run_rom_in_frame(game, 0xEBF0);
        return;
    }
    if game.coop.in_display || !p2_live(game) {
        run_rom_in_frame(game, 0xEBF0);
        return;
    }
    game.coop.in_display = true;
    run_rom_in_frame(game, 0xEBF0);
    let exit = regs(game);
    let mut p1oam = [0u8; 32];
    p1oam.copy_from_slice(&game.ram[LINK_OAM_FIRST..LINK_OAM_END]);
    swap(game);
    for s in (LINK_OAM_FIRST..LINK_OAM_END).step_by(4) {
        game.ram[s] = OAM_HIDDEN_Y;
    }
    if !p2_blink_hidden(&game.coop.block, game.ram[FRAME_CTR]) {
        zero_cost(game, |g| run_rom_in_frame(g, 0xEBF0));
    }
    let mut p2oam = [0u8; 32];
    p2oam.copy_from_slice(&game.ram[LINK_OAM_FIRST..LINK_OAM_END]);
    swap(game);
    game.ram[LINK_OAM_FIRST..LINK_OAM_END].copy_from_slice(&p1oam);
    let xor = game.coop.opts.p2_attr_xor;
    relocate_p2_oam(&mut game.ram, &p2oam, xor);
    game.coop.n_display += 1;
    game.coop.in_display = false;
    set_regs(game, exit);
}

/// `$E4D9` wrapper: enemy box vs each Link box; the enemy's touched bit is
/// the OR of both passes and the touch mask records who touched it.
fn coop_le4d9(game: &mut Game) {
    let x = game.cpu.x;
    let xs = usize::from(x);
    if !game.coop.enabled || xs >= ENEMY_SLOTS {
        run_rom_in_frame(game, 0xE4D9);
        return;
    }
    let entry = regs(game);
    let state_before = game.ram[SLOT_STATE_ROW + xs];
    let held_before = game.ram[HELD_ITEM];
    run_rom_in_frame(game, 0xE4D9);
    let exit = regs(game);
    let t1 = game.ram[CONTACT_ROW + xs] & TOUCH_BIT != 0;
    let consumed =
        game.ram[SLOT_STATE_ROW + xs] != state_before || game.ram[HELD_ITEM] != held_before;
    let mut t2 = false;
    if !consumed && p2_contact_live(game) {
        swap(game);
        set_regs(game, entry);
        zero_cost(game, |g| run_rom_in_frame(g, 0xE4D9));
        t2 = game.ram[CONTACT_ROW + xs] & TOUCH_BIT != 0;
        game.coop.n_p2_contacts += u64::from(t2);
        swap(game);
        let v = game.ram[CONTACT_ROW + xs] & !TOUCH_BIT;
        game.ram[CONTACT_ROW + xs] = v | if t1 || t2 { TOUCH_BIT } else { 0 };
        set_regs(game, exit);
    }
    game.coop.touch_mask[xs] = u8::from(t1) | (u8::from(t2) << 1);
}

/// `$D6C1` wrapper: route the enemy's contact hit to the Link(s) touched.
fn coop_link_collision(game: &mut Game) {
    let x = game.cpu.x;
    let xs = usize::from(x);
    let touched = game.ram[CONTACT_ROW + xs] & TOUCH_BIT != 0;
    if !game.coop.enabled || !touched || xs >= ENEMY_SLOTS {
        crate::enemy_traps::en_link_collision(game);
        return;
    }
    let mut m = core::mem::take(&mut game.coop.touch_mask[xs]);
    if m == 0 {
        m = 1;
    }
    let entry = regs(game);
    if m & 2 != 0 && p2_contact_live(game) {
        p2_shim_pass(game, x, entry, SITE_D6C7, crate::player_traps::pl_link_hit);
        game.coop.n_p2_contacts += 1;
        set_regs(game, entry);
    }
    if m & 1 != 0 {
        crate::enemy_traps::en_link_collision(game);
    } else {
        // What the ROM leaves on its not-touched return (`A = $A8,x & $10`
        // is nonzero here, but P1 was not touched): same flags as the
        // shim's `LDA`, 15 cycles, no hit.
        let v = TOUCH_BIT;
        game.cpu.a = v;
        crate::bank7_common::set_nz(&mut game.cpu.p, v);
        game.cpu.cycles += 15;
    }
}

/// `$E677` wrapper: P1's sword, then P2's unless P1 killed the enemy.
fn coop_sword_hit(game: &mut Game) {
    let x = game.cpu.x;
    let entry = regs(game);
    crate::player_traps::pl_sword_hit(game);
    if !game.coop.enabled || game.cpu.trap_jump.is_some() || !p2_contact_live(game) {
        return;
    }
    let exit = regs(game);
    p2_shim_pass(game, x, entry, SITE_DE40, crate::player_traps::pl_sword_hit);
    if game.cpu.p & crate::cpu::FLAG_C == 0 {
        set_regs(game, exit);
    } else {
        game.coop.n_p2_contacts += 1;
    }
}

/// `$E558` wrapper: enemy weapon vs P1, then vs P2 unless P1 consumed it.
fn coop_code39(game: &mut Game) {
    let x = game.cpu.x;
    let xs = usize::from(x);
    let entry = regs(game);
    let slot_before = game.ram.get(SLOT_STATE_ROW + xs).copied();
    crate::player_traps::pl_code39(game);
    if !game.coop.enabled || game.cpu.trap_jump.is_some() || !p2_contact_live(game) {
        return;
    }
    if game.ram.get(SLOT_STATE_ROW + xs).copied() != slot_before || game.ram[0x00EC] == 2 {
        return;
    }
    let exit = regs(game);
    p2_shim_pass(game, x, entry, SITE_DE40, crate::player_traps::pl_code39);
    if game.cpu.p & crate::cpu::FLAG_C == 0 {
        set_regs(game, exit);
    } else {
        game.coop.n_p2_contacts += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::BTN_A;

    #[test]
    fn swap_addrs_unique_and_indices() {
        for (i, a) in SWAP_ADDRS.iter().enumerate() {
            assert!(!SWAP_ADDRS[i + 1..].contains(a), "${a:04X} listed twice");
        }
        assert_eq!(IX_X, 7);
        assert_eq!(SWAP_ADDRS[IX_HP], 0x0774);
        assert_eq!(SWAP_ADDRS[IX_INVULN], 0x0518);
        assert_eq!(SWAP_ADDRS[IX_INJURED], 0x050C);
    }

    #[test]
    fn swap_roundtrip_restores_bytes() {
        let mut ram = [0u8; 0x800];
        for (i, b) in ram.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        let mut block = [0u8; SWAP_LEN];
        for (i, b) in block.iter_mut().enumerate() {
            *b = 0x80 | i as u8;
        }
        let (ram0, block0) = (ram, block);
        swap_block(&mut ram, &mut block);
        assert_eq!(ram[0x004D], 0x80 | IX_X as u8);
        assert_eq!(block[IX_X], ram0[0x004D]);
        swap_block(&mut ram, &mut block);
        assert_eq!(ram, ram0);
        assert_eq!(block, block0);
    }

    #[test]
    fn timer_tick_rules() {
        let mut b = [0u8; SWAP_LEN];
        b[IX_T0501] = 2;
        b[IX_T0503] = 1;
        b[IX_INJURED] = 0;
        b[IX_INVULN] = 5;
        b[IX_CROUCH] = 2;
        b[IX_ANIM] = 1;
        tick_p2_timers(&mut b, false);
        assert_eq!((b[IX_T0501], b[IX_T0503], b[IX_INJURED]), (1, 0, 0));
        assert_eq!(b[IX_INVULN], 5, "invulnerability only ticks on reload");
        assert_eq!((b[IX_CROUCH], b[IX_ANIM]), (1, 6));
        b[IX_ANIM] = 2;
        tick_p2_timers(&mut b, true);
        assert_eq!((b[IX_T0501], b[IX_T0503]), (0, 0), "saturates at 0");
        assert_eq!(b[IX_INVULN], 4);
        assert_eq!((b[IX_CROUCH], b[IX_ANIM]), (0, 6));
        b[IX_ANIM] = 3;
        tick_p2_timers(&mut b, false);
        assert_eq!(b[IX_ANIM], 3, "no crouch: anim untouched");
    }

    #[test]
    fn clamp_math() {
        let mut ram = [0u8; 0x800];
        ram[CAM_PAGE] = 1;
        ram[CAM_X] = 0x80;
        let mut b = [0u8; SWAP_LEN];
        b[IX_PAGE] = 1;
        b[IX_X] = 0x10;
        b[IX_HSPEED] = 0x18;
        clamp_p2_into_window(&mut b, &ram);
        assert_eq!((b[IX_PAGE], b[IX_X], b[IX_HSPEED]), (1, 0x80, 0));
        b[IX_PAGE] = 2;
        b[IX_X] = 0x71; // left + 241, crosses the page
        b[IX_HSPEED] = 0x18;
        clamp_p2_into_window(&mut b, &ram);
        assert_eq!((b[IX_PAGE], b[IX_X], b[IX_HSPEED]), (2, 0x70, 0));
        b[IX_X] = 0x00;
        b[IX_HSPEED] = 0x18;
        clamp_p2_into_window(&mut b, &ram);
        assert_eq!(
            (b[IX_PAGE], b[IX_X], b[IX_HSPEED]),
            (2, 0x00, 0x18),
            "inside: untouched"
        );
    }

    #[test]
    fn anchor_offsets_and_resets() {
        let mut ram = [0u8; 0x800];
        ram[CAM_PAGE] = 0;
        ram[CAM_X] = 0xF0;
        ram[0x003B] = 0;
        ram[0x004D] = 0xF0;
        ram[0x0774] = 0x40;
        ram[0x0494] = 1;
        ram[0x0518] = 3;
        ram[0x00B5] = 2;
        let b = anchor_block(&ram, 0x20, 0);
        assert_eq!((b[IX_PAGE], b[IX_X]), (1, 0x10), "page carry");
        assert_eq!(b[IX_HP], 0x40, "hp 0 clones P1");
        assert_eq!((b[IX_KILL], b[IX_INVULN], b[IX_STATE]), (0, 0, 1));
        let b = anchor_block(&ram, 0x20, 0x77);
        assert_eq!(b[IX_HP], 0x77);
        ram[CAM_PAGE] = 2; // P1 far left of the window -> clamped
        let b = anchor_block(&ram, 0x20, 0);
        assert_eq!((b[IX_PAGE], b[IX_X]), (2, 0xF0));
    }

    #[test]
    fn dead_and_blink_rules() {
        let mut b = [0u8; SWAP_LEN];
        assert!(!p2_is_dead(&b));
        b[IX_KILL] = 1;
        b[IX_INJURED] = 5;
        assert!(!p2_is_dead(&b));
        b[IX_INJURED] = 0;
        assert!(p2_is_dead(&b));
        let mut b = [0u8; SWAP_LEN];
        b[IX_Y] = 0xE4;
        assert!(!p2_is_dead(&b));
        b[IX_MIDAIR] = 1;
        assert!(p2_is_dead(&b));
        let mut b = [0u8; SWAP_LEN];
        b[IX_INVULN] = 2;
        assert!(p2_blink_hidden(&b, 1));
        assert!(!p2_blink_hidden(&b, 2));
        b[IX_INVULN] = 3;
        assert!(!p2_blink_hidden(&b, 1));
    }

    #[test]
    fn oam_relocation() {
        let mut ram = [0xAAu8; 0x800];
        let mut p2 = [0u8; 32];
        for i in 0..8 {
            p2[i * 4] = if i == 7 { OAM_HIDDEN_Y } else { 0x40 + i as u8 };
            p2[i * 4 + 1] = i as u8;
            p2[i * 4 + 2] = 0x40;
            p2[i * 4 + 3] = 0x10 * i as u8;
        }
        relocate_p2_oam(&mut ram, &p2, 0x01);
        for (i, &slot) in P2_OAM_SLOTS.iter().enumerate() {
            let d = 0x200 + usize::from(slot) * 4;
            assert_eq!(ram[d], p2[i * 4]);
            assert_eq!(ram[d + 1], i as u8);
            assert_eq!(ram[d + 2], if i == 7 { 0x40 } else { 0x41 });
            assert_eq!(ram[d + 3], 0x10 * i as u8);
        }
        let touched: Vec<usize> = P2_OAM_SLOTS
            .iter()
            .flat_map(|&s| (0..4).map(move |k| 0x200 + usize::from(s) * 4 + k))
            .collect();
        for (a, &v) in ram.iter().enumerate() {
            if !touched.contains(&a) {
                assert_eq!(v, 0xAA, "${a:04X} untouched");
            }
        }
    }

    #[test]
    fn pad2_mask() {
        let chord = BTN_UP | BTN_A;
        assert_eq!(mask_pad2(true, chord), BTN_A);
        assert_eq!(mask_pad2(false, chord), chord);
        let b = crate::game::BTN_B;
        assert_eq!(mask_pad2(true, chord | b), chord | b);
        assert_eq!(mask_pad2(true, BTN_UP), BTN_UP);
        assert!(crate::save::manual_save_gate(chord.reverse_bits()));
    }

    #[test]
    fn hash_stable_and_sensitive() {
        let a = CoopState::default();
        let mut b = CoopState::default();
        assert_eq!(hash(&a), hash(&b));
        b.cycles_saved_total = 99;
        b.n_update = 5;
        assert_eq!(hash(&a), hash(&b), "diagnostics excluded");
        b.block[IX_X] = 1;
        assert_ne!(hash(&a), hash(&b));
        let mut c = CoopState::default();
        c.touch_mask[3] = 2;
        assert_ne!(hash(&a), hash(&c));
    }

    #[test]
    fn set_enabled_toggles_sprite_limit() {
        let mut g = Game::new();
        assert!(!g.coop.enabled);
        assert!(status(&g).is_none());
        let before = g.ppu.model().sprite_limit();
        set_enabled(&mut g, true);
        assert!(g.coop.enabled);
        assert_eq!(g.ppu.model().sprite_limit(), SpriteLimit::Unlimited);
        assert!(g.traps.is_trapped(0xD5A7) && g.traps.is_trapped(0xEBF0));
        assert!(!g.traps.is_trapped(0xE677), "no original: no override");
        set_enabled(&mut g, true); // idempotent
        g.coop.active = true;
        set_enabled(&mut g, false);
        assert_eq!(g.ppu.model().sprite_limit(), before);
        assert!(!g.coop.active);
    }
}
