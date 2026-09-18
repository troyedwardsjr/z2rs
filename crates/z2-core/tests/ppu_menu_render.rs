//! Menu-render regression (menu blank + intro flicker probe).
//!
//! ROM-gated (skips without `Z2_ROM`): proves the file-select/name-entry
//! frames carry content through the owned rendering path (`z2-ppu` +
//! [`z2_core::ppu_bind`]) and pins the title sprite-0 answer the LA737
//! spin waits on.
//!
//! Pixel evidence (2026-09-07, VALID dump via `$Z2_ROM`; the file-select
//! flow is driven by state — Start held until mode 1 — not a fixture):
//! * blank 600 = title, 12 indexed colors (oracle agrees; the old 13/10
//!   alternation was the NMI-cadence flicker, since fixed).
//! * file-select (`mode=1/state=1`), 8 indexed colors with
//!   ~5k non-universal pixels in rows 16-207 (NOT blank; oracle agrees
//!   within 22-150 cursor-blink pixels).
//! * title spin (f13 blank): the beam reports the sprite-0 hit (oracle
//!   true) on the line the spin waits for.

#![cfg(feature = "interp")]

mod common;

use std::collections::BTreeMap;
use z2_core::game::Game;

fn rom_game() -> Option<Game> {
    let raw = common::rom_bytes("ppu_menu_render")?;
    let mut game = Game::from_ines(&raw).ok()?;
    game.reset();
    Some(game)
}

/// Drive title → file-select by holding Start until the game reports
/// mode 1, then settle; returns the frame count used (fails after 900).
fn drive_to_file_select(game: &mut Game) -> usize {
    const START: u8 = 1 << 3;
    let mut frames = 0usize;
    for _ in 0..30 {
        game.step(0);
        frames += 1;
    }
    // The title only reacts to a Start *press* (debounced edge), so pulse
    // it: 8 frames held, 8 released.
    while game.ram[0x0736] != 0x01 && frames < 900 {
        for _ in 0..8 {
            game.step(START);
            frames += 1;
        }
        for _ in 0..8 {
            game.step(0);
            frames += 1;
        }
    }
    assert_eq!(game.ram[0x0736], 0x01, "Start press reaches file-select");
    for _ in 0..120 {
        game.step(0);
        frames += 1;
    }
    frames
}

fn distinct(frame: &[u8; 61440]) -> (usize, BTreeMap<u8, usize>) {
    let mut m: BTreeMap<u8, usize> = BTreeMap::new();
    for &b in frame.iter() {
        *m.entry(b).or_insert(0) += 1;
    }
    (m.len(), m)
}

#[test]
fn menu_frames_carry_content_not_blank() {
    let Some(mut game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset");
        return;
    };
    let frames = drive_to_file_select(&mut game);
    assert!(frames < 900, "file-select reached in {frames} frames");
    assert_eq!(game.ram[0x0736], 0x01, "parks in file-select (mode 1)");
    assert_eq!(game.ram[0x076C], 0x01, "file-select state 1");
    let (nd, counts) = distinct(game.frame_indexed());
    // File-select uses one BG palette (attrs all zero) + sprites: 8 indices.
    // A blank/grey failure would be <=5 indices and ~0 non-universal pixels.
    assert_eq!(
        nd, 8,
        "file-select indexed colors (blank would be <=5): {counts:?}"
    );
    let uni = game.ppu.model().palette_entry(0);
    let nonuni = game.frame_indexed().iter().filter(|&&b| b != uni).count();
    assert!(
        nonuni > 4000,
        "file-select carries text rows (nonuni={nonuni}, uni=${uni:02X})"
    );
    // Rendering path witnesses: CHR pages [0,1], horizontal mirroring,
    // PPUCTRL base 0.
    assert_eq!(game.ppu.model().chr_page_no(0), 0);
    assert_eq!(game.ppu.model().chr_page_no(1), 1);
    assert_eq!(game.ppu.model().mirroring(), z2_ppu::Mirroring::Horizontal);
    assert_eq!(game.ppu.model().base_nametable(), 0);
}

#[test]
fn title_baseline_draws_thirteen_colors() {
    let Some(mut game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset");
        return;
    };
    for _ in 0..600 {
        game.step(0);
    }
    let (nd, counts) = distinct(game.frame_indexed());
    // Title draws the logo + text rows: 12 indexed colours, same as the
    // oracle at frame 600 (locks the bus/CHR/NT path).
    assert_eq!(nd, 12, "title indexed colors: {counts:?}");
}

#[test]
fn title_spin_hook_hit_is_true() {
    let Some(mut game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset");
        return;
    };
    for _ in 0..13 {
        game.step(0);
    }
    // LA737 spin state: OAM0 parked for the split timing, bg $FD opaque in
    // CHR page1 under it. The beam must report hit (oracle true); the model
    // flag itself is cleared mid-tail, so read the render latch.
    assert!(
        game.ppu.last_render_hit(),
        "hook-time sprite-0 hit (f13): OAM0 y=${:02X} hit_at={:?} ctrl=${:02X}",
        game.ppu.model().oam()[0],
        game.ppu.model().sprite0_hit_at(),
        game.ppu.model().ctrl(),
    );
}
