//! Boot-progression tests.
//!
//! ROM-gated end-to-end checks (skip gracefully without `Z2_ROM`): the
//! untrapped interpreter boots past the frame-8 `$C010` wait into the
//! bank-5 title-intro loop, the `$A740` sprite-0 wait resolves from the
//! real renderer with no test-hook override, and a Start press reaches
//! the file-select state (`$076C = 1`, bank-5 `$B22D` region).
//!
//! Wedge diagnosis (see the module docs for the full write-up): the
//! `$C010` wait predicate is `(mode == $08 || mode == $14) && stage == 1`
//! ([`z2_core::bank7_mode::boot_ready`]); the `$A737`/`$AB73` wait needs
//! `$2002` bit 6 (sprite-0 hit). Both resolve in the course of the title
//! intro — no wedge, only an NMI-beat phase offset versus the oracle.

#![cfg(feature = "interp")]

mod common;

use z2_core::boot_traps::register_boot_traps;
use z2_core::game::Game;

/// Load the pinned ROM into a reset `Game`, or `None` (skip) without it.
fn rom_game() -> Option<Game> {
    let raw = common::rom_bytes("boot_progression_tests")?;
    let mut game = Game::from_ines(&raw).ok()?;
    game.reset();
    Some(game)
}

/// Read `(bank, cpu_addr, len)` PRG bytes from `Z2_ROM` (16-byte header +
/// 16 KiB banks), or `None` (skip) without it. Banked windows map
/// `$8000-$BFFF`; the fixed bank 7 maps `$C000-$FFFF`.
fn prg_slice(bank: u8, addr: u16, len: u16) -> Option<Vec<u8>> {
    let img = common::rom_bytes("boot_progression_tests prg_slice")?;
    if img.len() < 16 || img[0..4] != *b"NES\x1A" {
        return None;
    }
    let base = if bank == 7 { 0xC000 } else { 0x8000 };
    let off = 16 + bank as usize * 0x4000 + (addr as usize - base);
    img.get(off..off + len as usize).map(|s| s.to_vec())
}

#[test]
fn rom_boot_bytes_match_cited_spans() {
    // `bank5_PowerON__Reset_Memory` $A6A0: JSR $B960.
    let Some(head) = prg_slice(5, 0xA6A0, 3) else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    assert_eq!(head, vec![0x20, 0x60, 0xB9]);
    // `LA737` spin $A73D: BIT $2002 : BVC.
    let spin = prg_slice(5, 0xA73D, 5).unwrap();
    assert_eq!(spin, vec![0x2C, 0x02, 0x20, 0x50, 0xFB]);
    // `LAB6D` spin $AB73: same shape.
    let spin2 = prg_slice(5, 0xAB73, 5).unwrap();
    assert_eq!(spin2, vec![0x2C, 0x02, 0x20, 0x50, 0xFB]);
    // `bank5_code27` $B960: LDX #$02.
    let slot = prg_slice(5, 0xB960, 2).unwrap();
    assert_eq!(slot, vec![0xA2, 0x02]);
    // `startup_init_begin_game` $AA08: STA $0738.
    let init = prg_slice(0, 0xAA08, 3).unwrap();
    assert_eq!(init, vec![0x8D, 0x38, 0x07]);
    // `LC2CA` $C2CA: LDY #$00 : STY $0727.
    let lc2ca = prg_slice(7, 0xC2CA, 5).unwrap();
    assert_eq!(lc2ca, vec![0xA0, 0x00, 0x8C, 0x27, 0x07]);
}

#[test]
fn untrapped_reaches_title_intro_past_frame_8() {
    let Some(mut game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // Blank input; the title sequence runs itself (modes 0 → 1 → 2 → 3).
    let mut reached = false;
    for _ in 0..15 {
        game.step(0);
        if game.ram[0x736] == 0x03
            && game.ram[0x76C] == 0x00
            && game.oam[0..4] == [0x7F, 0xF0, 0x01, 0xF8]
        {
            reached = true;
            break;
        }
    }
    assert!(
        reached,
        "title intro (mode 3, stage 0, intro sprites staged): m736={:02X} oam0={:02X?}",
        game.ram[0x736],
        &game.oam[0..4],
    );
    // Frame-end PC is either mid-spin ($A73D/$A740) or back in the $C010
    // main loop after a natural pass — both prove forward execution.
    let (_, _, _, _, pc, _) = game.cpu_state();
    let in_spin = pc == 0xA73D || pc == 0xA740;
    let in_main = (0xC010..=0xC020).contains(&pc);
    assert!(
        in_spin || in_main,
        "pc ${pc:04X} should be in the intro spin or the boot wait"
    );
}

#[test]
fn sprite0_wait_resolves_without_test_hook() {
    let Some(mut game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // The override must default off (policy: renderer-driven reads).
    assert_eq!(game.ppu.sprite0_override(), None);
    // Boot into the intro, then watch for the post-wait PPUCTRL merge
    // (`$FF = ($B0 & $FC) | $36 = $B2`): it is only reachable past the
    // `$A740`/`$AB76` spin, so observing it proves a natural sprite-0 hit.
    for _ in 0..12 {
        game.step(0);
    }
    let mut passed = game.ram[0x0FF] == 0xB2;
    for _ in 0..30 {
        game.step(0);
        if game.ram[0x0FF] == 0xB2 {
            passed = true;
            break;
        }
    }
    assert!(
        passed,
        "spin never passed in 42 frames ($FF={:02X}, override={:?})",
        game.ram[0x0FF],
        game.ppu.sprite0_override(),
    );
    assert_eq!(game.ppu.sprite0_override(), None);
}

#[test]
fn start_press_reaches_file_select() {
    let Some(mut game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    // Start = bit 3 ($08) on this contract.
    let mut file_select = false;
    let mut saw_filesel_pc = false;
    for f in 0..45 {
        let input = if (14..30).contains(&f) { 0x08 } else { 0x00 };
        game.step(input);
        if game.ram[0x76C] == 0x01 {
            file_select = true;
        }
        let (_, _, _, _, pc, _) = game.cpu_state();
        if (0xB22D..0xC000).contains(&pc) {
            saw_filesel_pc = true;
        }
    }
    assert!(
        file_select,
        "Start never advanced $076C (still {:02X})",
        game.ram[0x76C]
    );
    assert!(
        saw_filesel_pc,
        "no frame ended in the bank-5 file-select region"
    );
    // The LD174 stage-change clears the game mode on entry.
    assert_eq!(game.ram[0x736], 0x00);
}

#[test]
fn boot_traps_are_inert_through_the_wedge() {
    let Some(mut plain) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    let Some(mut trapped_game) = rom_game() else {
        eprintln!("SKIP: Z2_ROM unset (public CI)");
        return;
    };
    register_boot_traps(&mut trapped_game);
    // The boot/title window runs the bank-5 NMI path, never the bank-7
    // tail that LC2CA/code5/code6 replace: trajectories must agree on all
    // selector bytes and staged OAM.
    for _ in 0..20 {
        plain.step(0);
        trapped_game.step(0);
    }
    for a in [0x0736, 0x076C, 0x0726, 0x00FF, 0x00FE, 0x00FC, 0x0727] {
        assert_eq!(
            trapped_game.ram[a as usize], plain.ram[a as usize],
            "divergence at ${a:04X} with only boot traps registered"
        );
    }
    assert_eq!(trapped_game.oam[0..4], plain.oam[0..4]);
    // And none of the new traps fired at all in this window.
    assert!(
        trapped_game
            .trap_log()
            .iter()
            .all(|r| { r.addr != 0xC2CA && r.addr != 0xC2E6 && r.addr != 0xC31E }),
        "boot trap fired pre-area-load (unexpected)"
    );
}

#[test]
fn force_sprite0_hook_defaults_off() {
    // Policy guard: the sprite-0 override exists for tests only and must
    // never be armed by default (it would mask renderer gaps).
    let game = Game::new();
    assert_eq!(game.ppu.sprite0_override(), None);
    assert!(!game.ppu.sprite0_hit());
}
