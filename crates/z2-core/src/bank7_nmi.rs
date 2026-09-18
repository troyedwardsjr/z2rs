//! NMI handler ports.
//!
//! `bank7_NMI_Entry_Point` (`prg7.asm $C07B-$C1B5`) is too PPU-bound to
//! trap wholesale on the stubbed bus (its bank-5/6/0 callees wait on
//! vblank/sprite-0/sound state owned by the PPU and APU models), so
//! this module ports its *terminating bank-7 spans* as library functions
//! and documents the rest:
//!
//! | Rust fn | Span | Covers |
//! |---|---|---|
//! | [`nmi_prologue`] | `$C07B-$C0CA` | sound fast-path check, sprite-bank/`$FE`/`$FF` masking, scroll zero, OAM DMA, `$07AE` gate |
//! | [`nmi_ppu_setup`] | `$C0CA-$C12C` | `$0725` macro pointer, [`drain`](crate::bank7_ppu_queue::drain_update_queue) call, palette-latch writes, scroll restore, macro-advance bookkeeping |
//! | — | `$C12C-$C137` | sound dispatch + input latch (delegates to `bank7_mmc1`/`bank7_input` when wired) |
//! | — | `$C137-$C1B5` | pause/dialog/timer/RNG/sprite tail (see `bank7_mode::route_nmi_tail`, `bank7_timers`) |
//!
//! Bounds handed to the next stage: prologue returns [`NmiEntry`] telling
//! the caller which path the ASM takes (`Sound`, `Bank5`, `Full`).

use crate::bank7_common::set_nz;
use crate::cpu::{bus_read, bus_write};
use crate::game::Game;

/// NMI gate latch (`$0100`): bit 7 clear = sound-only, bit 6 clear =
/// bank-5 path, both set = full vblank path.
pub const NMI_STATE: u16 = 0x0100;
/// Sprite-bank mirror (`$FF`) and PPU-mask mirror (`$FE`).
pub const SPRITE_BANK: u16 = 0x00FF;
/// PPU-mask mirror.
pub const PPU_MASK_MIRROR: u16 = 0x00FE;
/// Horizontal scroll mirror. The ROM writes `$FD` to `$2005` first.
pub const SCROLL_X: u16 = 0x00FD;
/// Vertical scroll mirror. The ROM writes `$FC` to `$2005` second.
pub const SCROLL_Y: u16 = 0x00FC;
/// Which entry path the NMI prologue selects (`$C07C-$C083`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NmiEntry {
    /// Bit 7 clear (`BPL $C060`): bank-6 sound engine (`bank7_code1`).
    Sound,
    /// Bit 6 clear (`BVC $C077`): bank-5 handler (`LC077` → `bank5_A610`).
    Bank5,
    /// Both set: full vblank handler (continues at `$C083`).
    Full,
}

/// Prologue (`prg7.asm $C07B-$C0CA`).
///
/// Replicates `PHP : BIT $0100` (the `PHP` push is owned by the
/// interpreter's NMI sequence — `service_nmi` already pushed `P` — so the
/// port starts at the `BIT`), the `BPL`/`BVC` routing decision, and, on the
/// `Full` path: the sprite-bank merge, mask merge, scroll zero, OAM_ADDR
/// and OAM DMA (`A = $02`), plus the `$07AE` gate value (returned, not
/// branched; the caller decides the `JSR bank7_code52`).
///
/// Exit on `Full`: `A = $02`, `X` = last `$2002` read (`$80` on the stub:
/// `N = 1`), `Y = 0`. `$FF`/`$FE` updated per the masks. Returns
/// `(entry, side_palette_pending)` where the flag is `$07AE != 0`.
pub fn nmi_prologue(game: &mut Game) -> (NmiEntry, bool) {
    // BIT $0100: N = bit 7, V = bit 6, Z = (A & $0100 == 0).
    let state = bus_read(game, NMI_STATE);
    let a = game.cpu.a;
    if state & 0x80 != 0 {
        game.cpu.p |= crate::cpu::FLAG_N;
    } else {
        game.cpu.p &= !crate::cpu::FLAG_N;
    }
    if state & 0x40 != 0 {
        game.cpu.p |= crate::cpu::FLAG_V;
    } else {
        game.cpu.p &= !crate::cpu::FLAG_V;
    }
    if a & state == 0 {
        game.cpu.p |= crate::cpu::FLAG_Z;
    } else {
        game.cpu.p &= !crate::cpu::FLAG_Z;
    }
    // BPL bank7_code1 ($C060): bit 7 clear.
    if state & 0x80 == 0 {
        return (NmiEntry::Sound, false);
    }
    // BVC LC077 ($C077): bit 6 clear.
    if state & 0x40 == 0 {
        return (NmiEntry::Bank5, false);
    }
    // PLP (drops the PHP copy) : PHA.
    // (No net register effect: pull then push A. Stack traffic is the
    // interpreter's business on the trapped path; the untrapped ASM pushes
    // A here and the epilogue PLAs it. Model A as preserved.)
    // LDA $FF : AND #$7C : ORA $0747 : STA $FF : STA $2000.
    let ff = bus_read(game, SPRITE_BANK);
    game.cpu.a = ff;
    set_nz(&mut game.cpu.p, ff);
    let masked = ff & 0x7C;
    game.cpu.a = masked;
    set_nz(&mut game.cpu.p, masked);
    let m747 = bus_read(game, 0x0747);
    let merged = masked | m747;
    game.cpu.a = merged;
    set_nz(&mut game.cpu.p, merged);
    bus_write(game, SPRITE_BANK, merged);
    bus_write(game, 0x2000, merged);
    // LDA $0727 : BEQ :+ (useless branch — falls through either way).
    let v727 = bus_read(game, 0x0727);
    game.cpu.a = v727;
    set_nz(&mut game.cpu.p, v727);
    // LDY #$00 : LDA $FE : AND #$E0 : LDY $0726.
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    let fe = bus_read(game, PPU_MASK_MIRROR);
    game.cpu.a = fe;
    set_nz(&mut game.cpu.p, fe);
    let e0 = fe & 0xE0;
    game.cpu.a = e0;
    set_nz(&mut game.cpu.p, e0);
    let v726 = bus_read(game, 0x0726);
    game.cpu.y = v726;
    set_nz(&mut game.cpu.p, v726);
    // BNE :+ (dialog-box flag): else LDA $FE : ORA #$18 : ORA $0768.
    if v726 == 0 {
        let fe = bus_read(game, PPU_MASK_MIRROR);
        game.cpu.a = fe;
        set_nz(&mut game.cpu.p, fe);
        let o = fe | 0x18;
        game.cpu.a = o;
        set_nz(&mut game.cpu.p, o);
        let v768 = bus_read(game, 0x0768);
        let o2 = o | v768;
        game.cpu.a = o2;
        set_nz(&mut game.cpu.p, o2);
    }
    // STA $FE : AND #$E1 : STA $2001.
    let cur = game.cpu.a;
    bus_write(game, PPU_MASK_MIRROR, cur);
    let e1 = cur & 0xE1;
    game.cpu.a = e1;
    set_nz(&mut game.cpu.p, e1);
    bus_write(game, 0x2001, e1);
    // LDX $2002 : LDA #$00 : STA $2005 : STA $2005 : STA $2003.
    let status = bus_read(game, 0x2002);
    game.cpu.x = status;
    set_nz(&mut game.cpu.p, status);
    game.cpu.a = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x2005, 0);
    bus_write(game, 0x2005, 0);
    bus_write(game, 0x2003, 0);
    // LDA #$02 : STA $4014 (OAM DMA: 256-byte page copy in the bus).
    game.cpu.a = 0x02;
    set_nz(&mut game.cpu.p, 0x02);
    bus_write(game, 0x4014, 0x02);
    // LDA $07AE : BEQ :+ (caller runs code52 when nonzero).
    let v7ae = bus_read(game, 0x07AE);
    game.cpu.a = v7ae;
    set_nz(&mut game.cpu.p, v7ae);
    (NmiEntry::Full, v7ae != 0)
}

/// PPU-macro setup (`prg7.asm $C0CA-$C12C`).
///
/// Sets `$00/$01` from the `$0725`-indexed table at `$C03D`
/// (`bank7_PPU_Adresses_according_to_725_as_index`), calls
/// [`drain`](crate::bank7_ppu_queue::drain_update_queue), writes the
/// `$3F00` palette latch quad, then (when `$0768 == 0`) restores
/// `$2000`/`$2002`/`$2005` from `$FF`/`$FD`/`$FC`, and finishes with the
/// `$FE → $2001` write plus the `$0725` advance bookkeeping
/// (`$C108-$C12C`, incl. the `bank7_table0`/`$0301` terminator writes).
pub fn nmi_ppu_setup(game: &mut Game) {
    use crate::bank7_ppu_queue::drain_update_queue;
    // LDA $0725 : ASL : TAX.
    let sel = bus_read(game, 0x0725);
    game.cpu.a = sel;
    set_nz(&mut game.cpu.p, sel);
    let doubled = sel.wrapping_mul(2);
    game.cpu.a = doubled;
    // ASL flags: C = old bit 7, N/Z from result.
    if sel & 0x80 != 0 {
        game.cpu.p |= crate::cpu::FLAG_C;
    } else {
        game.cpu.p &= !crate::cpu::FLAG_C;
    }
    set_nz(&mut game.cpu.p, doubled);
    game.cpu.x = doubled;
    set_nz(&mut game.cpu.p, doubled);
    // LDA table,x : STA $00 / LDA table+1,x : STA $01.
    let base = 0xC03D + u16::from(game.cpu.x);
    let lo = bus_read(game, base);
    game.cpu.a = lo;
    set_nz(&mut game.cpu.p, lo);
    game.ram[0x000] = lo;
    let hi = bus_read(game, base.wrapping_add(1));
    game.cpu.a = hi;
    set_nz(&mut game.cpu.p, hi);
    game.ram[0x001] = hi;
    // JSR bank7_LD2EC.
    drain_update_queue(game);
    // LDA #$3F : STA $2006 : LDY #$00 : STY $2006 x3.
    game.cpu.a = 0x3F;
    set_nz(&mut game.cpu.p, 0x3F);
    bus_write(game, 0x2006, 0x3F);
    game.cpu.y = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x2006, 0);
    bus_write(game, 0x2006, 0);
    bus_write(game, 0x2006, 0);
    // LDA $0768 : BNE :+ (skip scroll restore when nonzero).
    let v768 = bus_read(game, 0x0768);
    game.cpu.a = v768;
    set_nz(&mut game.cpu.p, v768);
    if v768 == 0 {
        // LDA $FF : STA $2000 : LDX $2002 : LDA $FD : STA $2005
        // : LDA $FC : STA $2005.
        let ff = bus_read(game, SPRITE_BANK);
        game.cpu.a = ff;
        set_nz(&mut game.cpu.p, ff);
        bus_write(game, 0x2000, ff);
        let status = bus_read(game, 0x2002);
        game.cpu.x = status;
        set_nz(&mut game.cpu.p, status);
        let x = bus_read(game, SCROLL_X);
        game.cpu.a = x;
        set_nz(&mut game.cpu.p, x);
        bus_write(game, 0x2005, x);
        let y = bus_read(game, SCROLL_Y);
        game.cpu.a = y;
        set_nz(&mut game.cpu.p, y);
        bus_write(game, 0x2005, y);
    }
    // : LDA $FE : STA $2001.
    let fe = bus_read(game, PPU_MASK_MIRROR);
    game.cpu.a = fe;
    set_nz(&mut game.cpu.p, fe);
    bus_write(game, 0x2001, fe);
    // LDX $0725 : BEQ :+++ (no macro active: skip the advance).
    let sel = bus_read(game, 0x0725);
    game.cpu.x = sel;
    set_nz(&mut game.cpu.p, sel);
    if sel == 0 {
        // STA $0725 (stores back the same 0 — ASM falls into the store).
        bus_write(game, 0x0725, sel);
        return;
    }
    // INY : CPX #$01 : BEQ :+ (Y was 0 from the palette latch).
    game.cpu.y = game.cpu.y.wrapping_add(1);
    set_nz(&mut game.cpu.p, game.cpu.y);
    crate::bank7_common::cmp_val(&mut game.cpu.p, sel, 0x01);
    if sel == 0x01 {
        // DEC $0725.
        let v = bus_read(game, 0x0725).wrapping_sub(1);
        bus_write(game, 0x0725, v);
        crate::bank7_common::set_nz(&mut game.cpu.p, v);
    } else {
        // INY : LDA #$00 : CPX #$02 : BNE :+++ ...
        game.cpu.y = game.cpu.y.wrapping_add(1);
        set_nz(&mut game.cpu.p, game.cpu.y);
        game.cpu.a = 0;
        set_nz(&mut game.cpu.p, 0);
        crate::bank7_common::cmp_val(&mut game.cpu.p, sel, 0x02);
        if sel != 0x02 {
            bus_write(game, 0x0725, sel);
            return;
        }
        let v = bus_read(game, 0x0725).wrapping_sub(1);
        bus_write(game, 0x0725, v);
        crate::bank7_common::set_nz(&mut game.cpu.p, v);
    }
    // LDX bank7_table0,y : LDA #$00 : STA $0301,x
    // : LDA #$FF : STA $0302,x : LDA $0725 ... (terminator bookkeeping).
    let t0 = bus_read(game, 0xC05D + u16::from(game.cpu.y));
    game.cpu.x = t0;
    set_nz(&mut game.cpu.p, t0);
    game.cpu.a = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x0301 + u16::from(game.cpu.x), 0);
    game.cpu.a = 0xFF;
    set_nz(&mut game.cpu.p, 0xFF);
    bus_write(game, 0x0302 + u16::from(game.cpu.x), 0xFF);
    let sel = bus_read(game, 0x0725);
    game.cpu.a = sel;
    set_nz(&mut game.cpu.p, sel);
    bus_write(game, 0x0725, sel);
}
