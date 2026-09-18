//! PPU-binding integration tests.
//!
//! Covers the [`z2_core::ppu_bind::PpuBind`] wiring owned by this task:
//! register routing, `$2002` sequencing, OAM DMA forwarding, CHR sync from
//! the ROM image, the per-frame [`z2_core::game::Game::step`] hook
//! (render feed + mirrors + conditional NMI), and the sprite-0 test hook.
//! ROM-free throughout (synthetic programs / hand-built iNES images).

use z2_core::cpu::PpuBus;
use z2_core::game::Game;
use z2_core::ppu_bind::PpuBind;

// ------------------------------------------------- register routing

#[test]
fn ctrl_mask_scroll_routing() {
    let mut b = PpuBind::new();
    b.ppu_write(0x2000, 0x90);
    b.ppu_write(0x2001, 0x1E);
    assert_eq!(b.model().ctrl(), 0x90);
    assert_eq!(b.model().mask(), 0x1E);
    b.ppu_write(0x2005, 12);
    b.ppu_write(0x2005, 34);
    assert_eq!(b.model().scroll(), (12, 34));
    assert_eq!(b.writes, 4);
    assert_eq!(b.last_write, Some((0x2005, 34)));
}

#[test]
fn addr_data_roundtrip_through_nametable() {
    let mut b = PpuBind::new();
    // Point at $2108, store, point back, read past the buffer quirk.
    b.ppu_write(0x2006, 0x21);
    b.ppu_write(0x2006, 0x08);
    b.ppu_write(0x2007, 0x5A);
    b.ppu_write(0x2006, 0x21);
    b.ppu_write(0x2006, 0x08);
    let _junk = b.ppu_read(0x2007); // buffered read primes the latch
    assert_eq!(b.ppu_read(0x2007), 0x5A);
    assert_eq!(b.model().nt_read(0x2108), 0x5A);
}

#[test]
fn write_only_registers_read_back_open_bus() {
    let mut b = PpuBind::new();
    b.ppu_write(0x2000, 0xFF);
    b.ppu_write(0x2001, 0x1F);
    // Hardware returns open bus on write-only registers; the model uses 0.
    assert_eq!(b.ppu_read(0x2000), 0);
    assert_eq!(b.ppu_read(0x2001), 0);
    // ...but the latched values took effect.
    assert_eq!(b.model().ctrl(), 0xFF);
    assert_eq!(b.model().mask(), 0x1F);
}

#[test]
fn oam_addr_data_shadow() {
    let mut b = PpuBind::new();
    b.ppu_write(0x2003, 5);
    b.ppu_write(0x2004, 0xAB);
    // Address auto-incremented to 6; rewind and read back.
    b.ppu_write(0x2003, 5);
    assert_eq!(b.ppu_read(0x2004), 0xAB);
    assert_eq!(b.model().oam()[5], 0xAB);
}

// ------------------------------------------------- $2002 sequencing

#[test]
fn vblank_sets_at_frame_end_clears_on_read() {
    let mut b = PpuBind::new();
    assert_eq!(b.ppu_read(0x2002) & 0x80, 0, "power-on: vblank clear");
    let _frame = b.render_frame();
    assert!(b.vblank(), "render sets vblank");
    assert_ne!(b.ppu_read(0x2002) & 0x80, 0);
    assert!(!b.vblank(), "read clears vblank");
}

// ------------------------------------------------- OAM DMA via the CPU bus

/// Tiny program at `$8000`: `LDA #$02 : STA $4014 : RTS`.
fn dma_game() -> Game {
    let mut g = Game::with_test_program(0x8000, &[0xA9, 0x02, 0x8D, 0x14, 0x40, 0x60]);
    for i in 0..256usize {
        g.ram[0x200 + i] = (i ^ 0xA5) as u8;
    }
    g
}

#[test]
fn oam_dma_feeds_game_oam_and_ppu_oam() {
    let mut g = dma_game();
    g.call_asm(0x8000);
    for i in 0..256usize {
        assert_eq!(g.oam[i], (i ^ 0xA5) as u8, "game oam[{i}]");
        assert_eq!(g.ppu.model().oam()[i], (i ^ 0xA5) as u8, "ppu oam[{i}]");
    }
}

// ------------------------------------------------- CHR sync from the ROM image

/// Minimal MMC1 iNES image: 1×16 KiB PRG + 1×8 KiB CHR; CHR page 0 filled
/// with `0x11`, page 1 with `0x22`.
fn min_ines() -> Vec<u8> {
    let mut img = vec![0u8; 16];
    img[0..4].copy_from_slice(b"NES\x1A");
    img[4] = 1; // 1 PRG unit (16 KiB)
    img[5] = 1; // 1 CHR unit (8 KiB)
    img[6] = 0x10; // mapper 1 (high nibble of flags6... low nibble: bit4)
    img.extend(vec![0u8; 0x4000]);
    img.extend(vec![0x11u8; 0x1000]);
    img.extend(vec![0x22u8; 0x1000]);
    img
}

#[test]
fn from_ines_seeds_chr_pages_and_mirroring() {
    let mut g = Game::from_ines(&min_ines()).expect("synthetic MMC1 image loads");
    // Power-on MMC1: 8 KiB CHR mode, banks 0/1.
    assert_eq!(g.ppu.model().chr_page_no(0), 0);
    assert_eq!(g.ppu.model().chr_page_no(1), 1);
    // CHR contents observable through the buffered $2007 port: slot 0 at
    // $0000 is page 0 (0x11), slot 1 at $1000 is page 1 (0x22).
    g.ppu.ppu_write(0x2006, 0x00);
    g.ppu.ppu_write(0x2006, 0x00);
    let _junk = g.ppu.ppu_read(0x2007);
    assert_eq!(g.ppu.ppu_read(0x2007), 0x11);
    g.ppu.ppu_write(0x2006, 0x10);
    g.ppu.ppu_write(0x2006, 0x00);
    let _junk = g.ppu.ppu_read(0x2007);
    assert_eq!(g.ppu.ppu_read(0x2007), 0x22);
    // flags6 bit 0 clear -> horizontal mirroring.
    assert_eq!(g.ppu.model().mirroring(), z2_ppu::Mirroring::Horizontal);
}

#[test]
fn mapper_resync_follows_chr_bank_switch() {
    let mut g = Game::from_ines(&min_ines()).expect("synthetic MMC1 image loads");
    // Commit ctrl = 0x1C (4 KiB CHR mode, PRG mode 3) via five `STA $8000`
    // with bit-0 pattern LSB-first `0,0,1,1,1`, then chr0 = 1 via five
    // `STA $A000` with `1,0,0,0,0`. (In power-on 8 KiB mode chr0's low bit
    // is ignored by hardware, so the mode switch comes first.)
    let mut blob = Vec::new();
    for bit in [0u8, 0, 1, 1, 1] {
        blob.extend([0xA9, bit, 0x8D, 0x00, 0x80]);
    }
    for bit in [1u8, 0, 0, 0, 0] {
        blob.extend([0xA9, bit, 0x8D, 0x00, 0xA0]);
    }
    blob.push(0x60); // RTS
                     // NOTE: `with_test_program` would clobber the CHR setup; instead poke
                     // the blob over the loaded PRG's bank-0 window ($8000) directly. The
                     // image PRG is one 16 KiB unit visible at $8000 in mode 3.
    g.prg[0..blob.len()].copy_from_slice(&blob);
    g.set_reset_vector(0x8000);
    g.reset();
    g.call_asm(0x8000);
    assert_eq!(g.mmc1.ctrl, 0x1C, "serial commit landed in ctrl");
    assert_eq!(g.mmc1.chr0, 1, "serial commit landed in chr0");
    // The lazy per-frame sync (no bus hook involved) picks both up.
    g.step(0);
    assert_eq!(g.ppu.model().chr_page_no(0), 1, "slot 0 follows chr0");
    assert_eq!(g.ppu.model().mirroring(), z2_ppu::Mirroring::SingleLower);
}

// ------------------------------------------------- per-frame step hook

/// Game parked on a `JMP $8000` self-loop (never touches the PPU).
fn spinning_game() -> Game {
    Game::with_test_program(0x8000, &[0x4C, 0x00, 0x80])
}

#[test]
fn step_renders_frame_and_feeds_mirrors_without_nmi_when_disabled() {
    let mut g = spinning_game();
    // Palette entry 0 (universal background) -> $0F through the PPU port,
    // then park `v` at $0000 like the game's NMI latch quad does (with
    // rendering off, a `v` left inside $3F00-$3FFF would show that palette
    // entry instead — the hardware backdrop hijack).
    g.ppu.ppu_write(0x2006, 0x3F);
    g.ppu.ppu_write(0x2006, 0x00);
    g.ppu.ppu_write(0x2007, 0x0F);
    g.ppu.ppu_write(0x2006, 0x00);
    g.ppu.ppu_write(0x2006, 0x00);
    g.step(0);
    assert_eq!(g.frame_count(), 1);
    assert!(
        g.frame_indexed().iter().all(|&px| px == 0x0F),
        "framebuffer fed from the render, not zeros-by-default"
    );
    assert_eq!(g.palette()[0], 0x0F, "palette mirror follows PPU RAM");
    assert!(
        !g.cpu.nmi_pending,
        "PPUCTRL bit 7 clear: no NMI edge (unconditional pend is gone)"
    );
}

#[test]
fn step_services_nmi_only_when_armed() {
    let mut g = spinning_game();
    // NMI handler at $8100: `INC $00 : RTI`. Its runs are observable via $00.
    g.prg[0x100..0x100 + 3].copy_from_slice(&[0xE6, 0x00, 0x40]);
    g.set_nmi_vector(0x8100);
    // Arming blob at $8200: `LDA #$80 : STA $2000 : RTS`.
    g.prg[0x200..0x200 + 6].copy_from_slice(&[0xA9, 0x80, 0x8D, 0x00, 0x20, 0x60]);
    // Disarmed: three steps (short + k=0 + k=1 hooks) run no NMI.
    g.step(0);
    g.step(0);
    g.step(0);
    assert_eq!(g.ram[0x00], 0, "disarmed: no NMI serviced");
    // Arm and step: exactly one NMI per armed hook (the short step has no
    // vblank point and k=0 is warmup-suppressed, but those already passed,
    // so the next hook fires).
    g.set_cpu(0, 0, 0, 0xFD, 0x8200, 0x24);
    g.call_asm(0x8200);
    assert_eq!(g.ppu.model().ctrl() & 0x80, 0x80, "NMI armed via $2000");
    // Arming may pend immediately (stale set vblank + 0->1 retrigger rule);
    // drop it so the step below measures exactly one hook NMI.
    g.cpu.nmi_pending = false;
    g.ram[0x00] = 0;
    g.set_cpu(0, 0, 0, 0xFD, 0x8000, 0x24);
    g.step(0);
    assert_eq!(g.ram[0x00], 1, "armed: exactly one NMI serviced");
}

#[test]
fn first_step_is_short_and_vblank_clear() {
    use z2_core::game::FIRST_FRAME_END;
    let mut g = spinning_game();
    g.step(0);
    assert!(
        (FIRST_FRAME_END..FIRST_FRAME_END + 10).contains(&g.cpu.cycles),
        "short power-on frame (got {})",
        g.cpu.cycles
    );
    assert!(!g.ppu.vblank(), "no vblank occurred during it");
    // FIRST_VBLANK_AT (27395) > FIRST_FRAME_END (27281) holds by
    // construction (see the const docs); the k=0 hook depends on it.
}

#[test]
fn nmi_retrigger_fires_when_arming_during_vblank() {
    let mut g = spinning_game();
    // Arming blob at $8100: `LDA #$80 : STA $2000`.
    g.prg[0x100..0x100 + 5].copy_from_slice(&[0xA9, 0x80, 0x8D, 0x00, 0x20]);
    // Force a set vblank outside any hook, then single-step the arm: the
    // 0->1 transition must pend at once (hardware retrigger rule).
    // (`call_asm` would service the pend at its loop top and hide it.)
    g.ppu.model_mut().end_frame();
    assert!(g.ppu.vblank());
    g.set_cpu(0, 0, 0, 0xFD, 0x8100, 0x24);
    g.step_instruction().unwrap(); // LDA #$80
    g.step_instruction().unwrap(); // STA $2000
    assert!(g.cpu.nmi_pending, "arm-during-vblank pends at once");
    // Re-writing armed (1->1, as every NMI prologue does) must NOT re-pend.
    g.cpu.nmi_pending = false;
    g.set_cpu(0, 0, 0, 0xFD, 0x8100, 0x24);
    g.step_instruction().unwrap();
    g.step_instruction().unwrap();
    assert!(!g.cpu.nmi_pending, "1->1 rewrite does not retrigger");
}

// ------------------------------------------------- sprite-0 hook

#[test]
fn force_sprite0_hook_defaults_off_and_overrides() {
    let mut b = PpuBind::new();
    assert_eq!(b.sprite0_override(), None);
    let _frame = b.render_frame(); // blank CHR: no natural hit
    assert_eq!(b.ppu_read(0x2002) & 0x40, 0);
    b.force_sprite0_hit(true);
    assert_eq!(b.sprite0_override(), Some(true));
    assert_ne!(b.ppu_read(0x2002) & 0x40, 0, "forced hit reads set");
    b.clear_sprite0_override();
    assert_eq!(
        b.ppu_read(0x2002) & 0x40,
        0,
        "cleared hook reads renderer state"
    );
}
