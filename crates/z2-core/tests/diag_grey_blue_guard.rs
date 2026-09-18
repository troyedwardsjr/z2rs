//! Grey/blue guard (windowed grey-slot + blue-after-unpause hunt).
//!
//! ROM-gated (skips without `Z2_ROM`): pins the emulation-side verdict that
//! no menu stage or transition produces a uniform-grey or solid-blue
//! fullscreen frame through the owned render path (`z2-ppu` +
//! [`z2_core::ppu_bind`]).
//!
//! Findings locked here (2026-09-07, VALID dump `BA322865`, scripted flow
//! title → file-select → register → named → END → occupied → lives →
//! gameplay North Castle):
//! * Settled stages render content with backdrop `$0F` (black), `PPUMASK`
//!   `$1E` (bg+sprites on, no greyscale/emphasis), non-empty nametables:
//!   title 13 colors, file-select/register 8, lives 6, gameplay 14.
//! * Transitions blank to solid-BLACK `$0F` (`PPUMASK` `$00`, rendering off)
//!   for 2-4 frames (38 frames on the lives → gameplay area load) — never
//!   grey `$00`, never blue-family, emphasis never set.
//! * Pause/resume is identity: rebuilding to a gap point and stepping is
//!   hash-identical to continuous stepping (the windowed pause touches only
//!   the frame timer + audio + keyboard, never `Game`).
//! * The title used to flicker (13↔10 colour alternation from a `$30`/`$B2`
//!   PPUCTRL oscillation: the sprite-0 spin inside the title NMI overran
//!   the next vblank under the frame-granular renderer). The scanline-timed
//!   PPU fixed the cadence; consecutive title frames are now identical.
//!
//! If the windowed app shows grey slots or solid blue after unpause while
//! this guard stays green, the source is presentation-side (surface/present/
//! resize), not `Game::frame_indexed`.

#![cfg(feature = "interp")]

mod common;

use std::collections::BTreeMap;
use z2_core::game::Game;

fn rom_game() -> Option<Game> {
    let raw = common::rom_bytes("diag_grey_blue_guard")?;
    let mut game = Game::from_ines(&raw).ok()?;
    game.reset();
    Some(game)
}

fn step_n(g: &mut Game, n: usize, input: u8) {
    for _ in 0..n {
        g.step(input);
    }
}

const A: u8 = 0x01;
const SELECT: u8 = 0x04;
const START: u8 = 0x08;

/// Grey-family backdrop indices (`$00` power-on grey, `$10/$20/$30` column,
/// `$2D` neutral grey).
fn is_grey(i: u8) -> bool {
    matches!(i, 0x00 | 0x10 | 0x20 | 0x30 | 0x0D | 0x1D | 0x2D | 0x3D)
}

/// Blue-family backdrop indices (bright `$01`, medium `$11`/`$12`,
/// sky `$21`/`$22`, pale `$31`/`$32`).
fn is_blue(i: u8) -> bool {
    matches!(i, 0x01 | 0x02 | 0x11 | 0x12 | 0x21 | 0x22 | 0x31 | 0x32)
}

fn color_counts(frame: &[u8; 61440]) -> BTreeMap<u8, usize> {
    let mut m = BTreeMap::new();
    for &b in frame.iter() {
        *m.entry(b).or_insert(0) += 1;
    }
    m
}

/// Fast uniform check without building a map.
fn uniform_index(frame: &[u8; 61440]) -> Option<u8> {
    let first = frame[0];
    if frame.iter().all(|&b| b == first) {
        Some(first)
    } else {
        None
    }
}

fn nt_nonzero_total(g: &Game) -> usize {
    let m = g.ppu.model();
    let mut n = 0;
    for slot in 0..4u16 {
        let base = 0x2000 + slot * 0x400;
        for off in 0..0x3C0u16 {
            if m.nt_read(base + off) != 0 {
                n += 1;
            }
        }
    }
    n
}

/// Drive the scripted flow to register-idle (title → file-select → register).
fn to_register(g: &mut Game) {
    step_n(g, 30, 0x00);
    step_n(g, 5, START);
    step_n(g, 20, 0x00);
    step_n(g, 5, START);
    step_n(g, 20, 0x00);
}

#[test]
fn settled_menu_stages_never_grey_or_blue() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // (label, setup, min_colors, min_nonuni)
    step_n(&mut g, 600, 0x00);
    // Title (even flicker phase).
    {
        let m = g.ppu.model();
        let bd = m.palette_entry(0);
        let counts = color_counts(g.frame_indexed());
        let nonuni = g.frame_indexed().iter().filter(|&&b| b != bd).count();
        assert_eq!(bd, 0x0F, "title backdrop black, got ${bd:02X}");
        assert!(!is_grey(bd) && !is_blue(bd), "title backdrop not grey/blue");
        assert_eq!(m.mask(), 0x1E, "title mask renders, no emphasis/grey");
        assert_eq!(m.mask() >> 5, 0, "title emphasis clear");
        assert!(counts.len() >= 10, "title contentful: {counts:?}");
        assert!(nonuni > 4000, "title nonuni={nonuni}");
        assert!(nt_nonzero_total(&g) > 1000, "title NT loaded");
    }
    // File-select.
    step_n(&mut g, 5, START);
    step_n(&mut g, 20, 0x00);
    assert_eq!((g.ram[0x736], g.ram[0x76C]), (0x00, 0x01));
    // Register idle.
    step_n(&mut g, 5, START);
    step_n(&mut g, 20, 0x00);
    assert_eq!((g.ram[0x736], g.ram[0x76C]), (0x01, 0x01));
    // Check both slot stages via a fresh replay to keep positions exact.
    let Some(mut h) = rom_game() else {
        return;
    };
    step_n(&mut h, 30, 0x00);
    step_n(&mut h, 5, START);
    step_n(&mut h, 20, 0x00);
    check_settled(&h, "file-select", 8, 4000);
    step_n(&mut h, 5, START);
    step_n(&mut h, 20, 0x00);
    check_settled(&h, "register-idle", 8, 4000);
    // Named + END.
    for _ in 0..8 {
        step_n(&mut h, 2, A);
        step_n(&mut h, 8, 0x00);
    }
    check_settled(&h, "register-named", 8, 4000);
    for _ in 0..3 {
        step_n(&mut h, 2, SELECT);
        step_n(&mut h, 8, 0x00);
    }
    check_settled(&h, "register-END", 8, 4000);
    // Occupied file-select.
    step_n(&mut h, 5, START);
    step_n(&mut h, 30, 0x00);
    assert_eq!(h.ram[0x1A], 0x06, "slot 0 occupied");
    check_settled(&h, "file-select-occupied", 8, 4000);
    // Lives screen (mode $11, mostly black but contentful, backdrop $0F).
    step_n(&mut h, 5, START);
    step_n(&mut h, 60, 0x00);
    assert_eq!(h.ram[0x700], 0x03, "lives reset to 3");
    {
        let m = h.ppu.model();
        let bd = m.palette_entry(0);
        let counts = color_counts(h.frame_indexed());
        let nonuni = h.frame_indexed().iter().filter(|&&b| b != bd).count();
        assert_eq!(bd, 0x0F, "lives backdrop black, got ${bd:02X}");
        assert!(!is_grey(bd) && !is_blue(bd));
        assert_eq!(m.mask() >> 5, 0, "lives emphasis clear");
        assert!(counts.len() >= 5, "lives contentful: {counts:?}");
        assert!(nonuni > 500, "lives nonuni={nonuni}");
    }
    // Gameplay North Castle.
    step_n(&mut h, 1100, 0x00);
    assert_eq!(h.ram[0x736], 0x0B, "sideview gameplay");
    check_settled(&h, "gameplay", 10, 30000);
}

fn check_settled(g: &Game, label: &str, min_colors: usize, min_nonuni: usize) {
    let m = g.ppu.model();
    let bd = m.palette_entry(0);
    let counts = color_counts(g.frame_indexed());
    let nonuni = g.frame_indexed().iter().filter(|&&b| b != bd).count();
    assert_eq!(bd, 0x0F, "{label}: backdrop black, got ${bd:02X}");
    assert!(
        !is_grey(bd) && !is_blue(bd),
        "{label}: backdrop ${bd:02X} must not be grey/blue"
    );
    assert!(
        m.show_bg() && m.show_sprites(),
        "{label}: rendering on (mask ${:02X})",
        m.mask()
    );
    assert_eq!(m.mask() >> 5, 0, "{label}: emphasis clear");
    assert_eq!(m.mask() & 0x01, 0, "{label}: greyscale clear");
    assert!(
        counts.len() >= min_colors,
        "{label}: {} colors, want >={min_colors}: {counts:?}",
        counts.len()
    );
    assert!(
        nonuni >= min_nonuni,
        "{label}: nonuni={nonuni}, want >={min_nonuni}"
    );
    assert!(nt_nonzero_total(g) > 1000, "{label}: nametables loaded");
}

#[test]
fn transition_blanks_are_black_never_grey_or_blue() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // Full scripted flow; every uniform frame past boot must be $0F black
    // with rendering off (intentional upload blanking), never grey/blue.
    let mut seq: Vec<u8> = vec![0u8; 30];
    seq.extend([START; 5]);
    seq.extend([0u8; 20]);
    seq.extend([START; 5]);
    seq.extend([0u8; 20]);
    for _ in 0..8 {
        seq.extend([A; 2]);
        seq.extend([0u8; 8]);
    }
    for _ in 0..3 {
        seq.extend([SELECT; 2]);
        seq.extend([0u8; 8]);
    }
    seq.extend([START; 5]);
    seq.extend([0u8; 30]);
    seq.extend([START; 5]);
    seq.extend([0u8; 80]);
    seq.extend([0u8; 300]);
    let mut blanks = 0;
    for &b in &seq {
        g.step(b);
        if g.frame_count() <= 20 {
            continue; // boot warmup owns its grey frames
        }
        if let Some(idx) = uniform_index(g.frame_indexed()) {
            blanks += 1;
            assert_eq!(
                idx,
                0x0F,
                "f={}: uniform frame must be black $0F, got ${idx:02X} (grey/blue hunt)",
                g.frame_count()
            );
            assert!(
                !is_grey(idx) || idx == 0x0F,
                "f={}: uniform frame ${idx:02X} must not be grey",
                g.frame_count()
            );
            assert!(
                !is_blue(idx),
                "f={}: uniform frame ${idx:02X} must not be blue",
                g.frame_count()
            );
            // Legitimate black sources: $00 rendering-off (upload blanking
            // during transitions) or $1E rendering-on with empty NT/CHR
            // (boot/intro before the first uploads land).
            let bgsp = g.ppu.model().mask() & 0x18;
            assert!(
                bgsp == 0x00 || bgsp == 0x18,
                "f={}: uniform black frame mask ${:02X} (want $00 upload-blank or $1E empty-NT boot)",
                g.frame_count(),
                g.ppu.model().mask()
            );
        }
        assert_eq!(g.ppu.model().mask() >> 5, 0, "emphasis never set");
    }
    assert!(blanks >= 4, "flow must cross upload blanks (saw {blanks})");
}

#[test]
fn pause_resume_is_identity() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    to_register(&mut g);
    let gap_at = g.frame_count();
    fn snap(g: &Game) -> (u64, [u8; 32], u8, u8, u64) {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in g.frame_indexed().iter() {
            h = h.wrapping_mul(0x100000001b3).wrapping_add(b as u64);
        }
        let mut pal = [0u8; 32];
        pal.copy_from_slice(g.ppu.model().palette());
        (
            h,
            pal,
            g.ppu.model().ctrl(),
            g.ppu.model().mask(),
            g.frame_count(),
        )
    }
    let pre = snap(&g);
    g.step(0x00);
    let a1 = snap(&g);
    g.step(0x00);
    let a2 = snap(&g);
    // Rebuild to the gap point (deterministic), idle (pause = no steps),
    // then resume with identical inputs.
    let Some(mut h) = rom_game() else {
        return;
    };
    to_register(&mut h);
    assert_eq!(h.frame_count(), gap_at);
    assert_eq!(snap(&h), pre, "replay reaches gap point");
    h.step(0x00);
    assert_eq!(snap(&h), a1, "resume-first-frame == continuous");
    h.step(0x00);
    assert_eq!(snap(&h), a2, "resume-second-frame == continuous");
}

#[test]
fn title_flicker_pair_is_content_not_grey_or_blue() {
    let Some(mut g) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    step_n(&mut g, 600, 0x00);
    // The title runs its logic inside the NMI and spins on sprite-0; with
    // the scanline-timed PPU the spin exits on the right line, the NMI
    // fires every frame and consecutive frames are identical (the old
    // every-other-frame $30/$B2 flicker pair is gone). Both frames stay
    // contentful on black.
    let counts0 = color_counts(g.frame_indexed());
    let (ctrl0, mask0, bd0) = (
        g.ppu.model().ctrl(),
        g.ppu.model().mask(),
        g.ppu.model().palette_entry(0),
    );
    let nonuni0 = g.frame_indexed().iter().filter(|&&b| b != bd0).count();
    g.step(0x00);
    let counts1 = color_counts(g.frame_indexed());
    let (ctrl1, mask1, bd1) = (
        g.ppu.model().ctrl(),
        g.ppu.model().mask(),
        g.ppu.model().palette_entry(0),
    );
    let nonuni1 = g.frame_indexed().iter().filter(|&&b| b != bd1).count();
    assert_eq!(
        ctrl0, ctrl1,
        "title PPUCTRL steady (no NMI-cadence flicker)"
    );
    assert_eq!(counts0, counts1, "consecutive title frames identical");
    assert!(counts0.len() >= 10, "title contentful: {counts0:?}");
    for (bd, mask, nonuni, tag) in [(bd0, mask0, nonuni0, "even"), (bd1, mask1, nonuni1, "odd")] {
        assert_eq!(bd, 0x0F, "title {tag} backdrop black");
        assert!(!is_grey(bd) && !is_blue(bd), "title {tag} not grey/blue");
        assert_eq!(mask, 0x1E, "title {tag} rendering on");
        assert!(nonuni > 4000, "title {tag} contentful nonuni={nonuni}");
    }
}
