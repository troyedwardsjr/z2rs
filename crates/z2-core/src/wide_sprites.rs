//! Widescreen margin sprites: side-view objects the game keeps alive outside
//! the 256-px window, drawn into the margins. Display only.
//!
//! Zelda II keeps side-view enemies alive to about 352 px left and 607 px
//! right of the camera (items further still), but OAM X is 8-bit, so its draw
//! routine `bank7_Display` (`$EF11`, dispatched per enemy code through the
//! WRAM table `$6E65`) hides every 8-px column of an object that falls off
//! the window: `$C9` carries one bit per column (8/4/2/1, `+$F0` when the Y
//! is off screen) and `LF1D7` parks a hidden column at Y `$F8`. The screen X
//! of the object is `$CD = (x_lo - $072C) & $FF`.
//!
//! ## Method
//!
//! A display-only trap at `$EF11` ([`OBSERVER_TRAP_NAME`]) runs before the
//! real routine each time it is called in mode `$0B`:
//!
//! 1. RAM, WRAM, MMC1 and CPU registers are copied into a private *shadow*
//!    [`Game`] (plain interpretation, no traps).
//! 2. The shadow's column bits are cleared (`$C9 &= $F0`, so vertically
//!    hidden parts stay hidden), page 2 is filled with Y `$F8`, and the whole
//!    routine runs there ([`Game::try_call_asm`]).
//! 3. Every OAM entry it wrote is placed in window coordinates:
//!    `x = (object - camera) + (int8)(OAM X - $CD)` with the full
//!    page:pixel object position (`$3C,x`:`$4E,x`) and camera
//!    (`$072A`:`$072C`). Entries that land (partly) outside the window are
//!    kept.
//! 4. The trap emulates the real routine's first instruction
//!    (`LDA $040E,x`: A, N, Z; 4 cycles) and tail-jumps to `$EF14`, so the
//!    game itself runs exactly as without the trap.
//!
//! PPU OAM in frame N is page 2 as it stood at frame N's `$4014` DMA, i.e.
//! what the main loop drew during frame N-1. The entries collected since the
//! last DMA are therefore latched into the shown list at the next DMA
//! ([`on_oam_dma`]); a frame without a DMA keeps the previous list.
//!
//! ## What it must never do
//!
//! Change the game: RAM, WRAM, CPU state, cycle counts and
//! [`Game::frame_indexed`] are byte-identical with the feature on or off
//! (`tests/wide_sprites_rom.rs`). The state lives outside
//! [`crate::state::GameState`] and the trap is excluded from the netplay
//! trap-set identity ([`is_display_only_trap`]), so peers with different
//! widescreen settings still connect; after a rollback the next DMA refills
//! the list.
//!
//! ## Other providers
//!
//! [`Game::margin_sprites_mut`] is a second list, owned by other display
//! providers (e.g. an overworld object layer): whatever it holds is drawn
//! after (below) the side-view objects. Its owner refills it each frame.

use z2_ppu::MarginSprite;

use crate::cpu::{FLAG_N, FLAG_Z};
use crate::game::Game;
use crate::wide_margins::{ADDR_GAME_MODE, ADDR_SCROLL_HI, ADDR_SCROLL_LO, MODE_SIDEVIEW};

/// `bank7_Display` entry.
pub const DISPLAY_ADDR: u16 = 0xEF11;
/// `bank7_Display` after its first instruction (`LDA $040E,x`).
pub const DISPLAY_AFTER_LDA: u16 = 0xEF14;
/// Trap name of the observer; see [`is_display_only_trap`].
pub const OBSERVER_TRAP_NAME: &str = "wide_display_observer";
/// Cycles of the emulated `LDA $040E,x` (absolute,X; `$040E + x` never
/// crosses a page for the enemy slots).
pub const OBSERVER_CYCLES: u64 = 4;
/// Instruction budget of one shadow run (a real call is a few hundred).
pub const SHADOW_BUDGET: u64 = 100_000;
/// Enemy hit-state table read by the first instruction.
const ADDR_HIT_STATE: u16 = 0x040E;
/// Per-column off-window mask of the object being drawn.
const ADDR_COLUMN_MASK: usize = 0xC9;
/// Screen X of the object being drawn.
const ADDR_SCREEN_X: usize = 0xCD;
/// Object X page / pixel tables.
const ADDR_X_PAGE: usize = 0x3C;
const ADDR_X_PIXEL: usize = 0x4E;
/// Objects further than this outside the window cannot reach any margin
/// (`8 * MAX_MARGIN_TILES`).
const MAX_REACH: i32 = 8 * z2_ppu::MAX_MARGIN_TILES as i32;

/// Whether a trap only observes the game for display and must stay out of
/// the netplay trap-set identity (`trapset_id` in `z2-native` / `z2-web`).
#[must_use]
pub fn is_display_only_trap(name: &str) -> bool {
    name == OBSERVER_TRAP_NAME
}

/// Margin-sprite state on [`Game`]. Not part of any save state.
#[derive(Debug, Default)]
pub struct MarginSpriteState {
    /// Side-view observer registered.
    pub(crate) enabled: bool,
    /// Private interpreter the observer re-runs `bank7_Display` in.
    shadow: Option<Box<Game>>,
    /// Entries drawn since the last OAM DMA, with their OAM offset.
    pending: Vec<(u8, MarginSprite)>,
    /// Entries latched at the last OAM DMA: what the current frame shows.
    shown: Vec<MarginSprite>,
    /// Sprites appended by other providers ([`Game::margin_sprites_mut`]).
    pub(crate) extra: Vec<MarginSprite>,
    /// Shadow runs that failed (illegal opcode / budget); zero when healthy.
    pub(crate) shadow_errors: u64,
}

/// Turn the side-view observer on or off (see [`Game::set_margin_sprites`]).
pub fn set_enabled(game: &mut Game, on: bool) {
    let st = &mut game.margin_sprites;
    st.pending.clear();
    st.shown.clear();
    if on == st.enabled {
        return;
    }
    st.enabled = on;
    if on {
        let mut shadow = Game::new();
        shadow.prg.clone_from(&game.prg);
        game.margin_sprites.shadow = Some(Box::new(shadow));
        game.trap_register_cycles(
            OBSERVER_TRAP_NAME,
            Some(7),
            DISPLAY_ADDR,
            observer,
            OBSERVER_CYCLES,
        );
    } else {
        game.margin_sprites.shadow = None;
        game.traps.unregister(DISPLAY_ADDR);
    }
}

/// `$4014` hook: latch what was drawn since the last DMA (OAM order: a lower
/// offset wins, as in hardware).
pub(crate) fn on_oam_dma(game: &mut Game) {
    let st = &mut game.margin_sprites;
    if !st.enabled {
        return;
    }
    st.pending.sort_by_key(|&(off, _)| off);
    st.shown.clear();
    st.shown.extend(st.pending.iter().map(|&(_, s)| s));
    st.pending.clear();
}

/// The side-view entries latched for the current frame.
#[must_use]
pub fn shown(game: &Game) -> &[MarginSprite] {
    &game.margin_sprites.shown
}

/// The `$EF11` trap: observe, then run the real routine untouched.
fn observer(game: &mut Game) {
    let x = game.cpu.x;
    if game.ram[usize::from(ADDR_GAME_MODE)] == MODE_SIDEVIEW {
        observe(game, usize::from(x));
    }
    // LDA $040E,x — then continue with the ROM's own body at $EF14.
    let a = game.ram[usize::from(ADDR_HIT_STATE.wrapping_add(u16::from(x))) & 0x7FF];
    game.cpu.a = a;
    game.cpu.p &= !(FLAG_N | FLAG_Z);
    if a == 0 {
        game.cpu.p |= FLAG_Z;
    }
    game.cpu.p |= a & FLAG_N;
    game.trap_jump(DISPLAY_AFTER_LDA);
}

/// Re-run `bank7_Display` for slot `x` in the shadow with every column
/// visible and collect the entries that fall outside the window.
fn observe(game: &mut Game, x: usize) {
    let Some(mut sh) = game.margin_sprites.shadow.take() else {
        return;
    };
    sh.ram = game.ram;
    sh.wram = game.wram;
    sh.mmc1 = game.mmc1;
    sh.cpu = game.cpu;
    sh.cpu.nmi_pending = false;
    sh.cpu.irq_pending = false;
    sh.ram[ADDR_COLUMN_MASK] &= 0xF0;
    for i in 0..64 {
        sh.ram[0x200 + 4 * i] = 0xF8;
    }
    if sh.try_call_asm(DISPLAY_ADDR, SHADOW_BUDGET).is_err() {
        game.margin_sprites.shadow_errors += 1;
        game.margin_sprites.shadow = Some(sh);
        return;
    }
    let ram = &game.ram;
    let camera = (i32::from(ram[usize::from(ADDR_SCROLL_HI)]) << 8)
        | i32::from(ram[usize::from(ADDR_SCROLL_LO)]);
    let slot = x & 0xFF;
    let object = (i32::from(ram[(ADDR_X_PAGE + slot) & 0x7FF]) << 8)
        | i32::from(ram[(ADDR_X_PIXEL + slot) & 0x7FF]);
    let dx = object - camera;
    let cd = ram[ADDR_SCREEN_X];
    let pending = &mut game.margin_sprites.pending;
    for i in 0..64 {
        let e = &sh.ram[0x200 + 4 * i..0x204 + 4 * i];
        // Y $EF and above never reaches a visible line.
        if e[0] >= 0xEF {
            continue;
        }
        let wx = dx + i32::from(e[3].wrapping_sub(cd) as i8);
        let inside = (0..=z2_ppu::WIDTH as i32 - 8).contains(&wx);
        let reachable = (-MAX_REACH - 8..z2_ppu::WIDTH as i32 + MAX_REACH).contains(&wx);
        if inside || !reachable {
            continue;
        }
        pending.push((
            (4 * i) as u8,
            MarginSprite {
                x: wx as i16,
                y: e[0],
                tile: e[1],
                attr: e[2],
            },
        ));
    }
    game.margin_sprites.shadow = Some(sh);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_only_names() {
        assert!(is_display_only_trap(OBSERVER_TRAP_NAME));
        assert!(!is_display_only_trap("bank7_Display"));
        assert!(!is_display_only_trap("coop_code21"));
    }

    #[test]
    fn toggling_registers_and_unregisters_the_observer() {
        let mut g = Game::new();
        assert!(g.traps.get(DISPLAY_ADDR).is_none());
        g.set_margin_sprites(true);
        assert!(g.margin_sprites_enabled());
        assert_eq!(
            g.traps.get(DISPLAY_ADDR).map(|t| t.name),
            Some(OBSERVER_TRAP_NAME)
        );
        g.set_margin_sprites(true); // idempotent
        assert_eq!(g.traps.len(), 1);
        g.set_margin_sprites(false);
        assert!(!g.margin_sprites_enabled());
        assert!(g.traps.get(DISPLAY_ADDR).is_none());
    }

    #[test]
    fn dma_latches_pending_in_oam_order() {
        let mut g = Game::new();
        g.set_margin_sprites(true);
        let s = |x: i16| MarginSprite {
            x,
            y: 10,
            tile: 1,
            attr: 0,
        };
        g.margin_sprites.pending.push((0x40, s(-20)));
        g.margin_sprites.pending.push((0x10, s(300)));
        assert!(shown(&g).is_empty(), "nothing shown before the DMA");
        on_oam_dma(&mut g);
        assert_eq!(shown(&g), &[s(300), s(-20)]);
        on_oam_dma(&mut g);
        assert!(shown(&g).is_empty(), "the next DMA latches the next frame");
    }

    #[test]
    fn observer_emulates_the_first_instruction() {
        let mut g = Game::new();
        g.set_margin_sprites(true);
        g.cpu.x = 3;
        g.ram[0x40E + 3] = 0x85;
        g.ram[usize::from(ADDR_GAME_MODE)] = 0x05; // not side view: no shadow run
        observer(&mut g);
        assert_eq!(g.cpu.a, 0x85);
        assert_eq!(g.cpu.p & (FLAG_N | FLAG_Z), FLAG_N);
        assert_eq!(g.take_trap_jump(), Some(DISPLAY_AFTER_LDA));
        g.ram[0x40E + 3] = 0;
        observer(&mut g);
        assert_eq!(g.cpu.p & (FLAG_N | FLAG_Z), FLAG_Z);
        assert_eq!(g.take_trap_jump(), Some(DISPLAY_AFTER_LDA));
    }
}
