//! PPU update-queue producer/consumer API.
//!
//! The queue is the vblank work list the NMI handler drains: `$00/$01`
//! points at a byte stream of PPU macros, terminated by `$FF`
//! (`prg7.asm $D2EC`, `bank7_LD2EC`). Producers (title, area loaders,
//! status bar, later routines) append entries; the consumer runs during
//! vblank. This module is the API later routines will use.
//!
//! | Rust fn | Label | Addr |
//! |---|---|---|
//! | [`drain_update_queue`] | `bank7_LD2EC` | `$D2EC` |
//! | [`load_sideview_palette`] | `bank7_code52` | `$FD82` |
//!
//! ## Stream format (`$D2EC-$D343`)
//!
//! ```text
//! $FF             -> end of stream (see quirk below)
//! $4C lo hi       -> redirect pointer to hi:lo, continue there
//! hi lo ctrl data -> PPU write: $2006=hi,lo then `ctrl & $3F` data bytes
//!                    to $2007 (bit 7 of ctrl picks the $2000 bit-2 set/
//!                    clear branch; bit 6 skips one stream byte first)
//! ```
//!
//! Exit quirk (`$D2F0-$D2F2`): `BEQ bank7_LD2EC-1` branches onto `$D2EB`,
//! the `RTS` byte closing the *previous* routine, and executes it as `RTS`.
//! The Rust port simply returns.
//!
//! ## Producer API
//!
//! [`queue_begin`] points `$00/$01` at a buffer, [`queue_push`] appends,
//! [`queue_finish`] writes the `$FF` terminator. The NMI path instead
//! points `$00/$01` via the `$0725` table (`$C0CF-$D2EC`); both spellings
//! feed [`drain_update_queue`].

use crate::bank7_common::{adc_val, cmp_val, inc_val, sbc_val, set_nz};
use crate::cpu::{bus_read, bus_write, FLAG_C, FLAG_N, FLAG_Z};
use crate::game::Game;

/// Queue pointer low (`$00`) — set by NMI from the `$0725` table.
pub const QUEUE_PTR_LO: u16 = 0x0000;
/// Queue pointer high (`$01`).
pub const QUEUE_PTR_HI: u16 = 0x0001;
/// Stream terminator (`$FF`).
pub const QUEUE_END: u8 = 0xFF;
/// Stream redirect opcode (`$4C`, 6502 `JMP abs`).
pub const QUEUE_REDIRECT: u8 = 0x4C;
/// Sprite-bank mirror consulted by the drain (`$FF`).
pub const SPRITE_BANK: u16 = 0x00FF;

/// Point `$00/$01` at `addr` (queue buffer start).
pub fn queue_begin(game: &mut Game, addr: u16) {
    game.ram[QUEUE_PTR_LO as usize] = addr as u8;
    game.ram[QUEUE_PTR_HI as usize] = (addr >> 8) as u8;
}

/// Append one stream byte at `cursor` (a `$0302`-style offset from the
/// `$00/$01` base); returns the advanced cursor.
pub fn queue_push(game: &mut Game, cursor: u16, byte: u8) -> u16 {
    let base = u16::from(game.ram[QUEUE_PTR_LO as usize])
        | (u16::from(game.ram[QUEUE_PTR_HI as usize]) << 8);
    bus_write(game, base.wrapping_add(cursor), byte);
    cursor.wrapping_add(1)
}

/// Write the `$FF` terminator at `cursor`.
pub fn queue_finish(game: &mut Game, cursor: u16) {
    let base = u16::from(game.ram[QUEUE_PTR_LO as usize])
        | (u16::from(game.ram[QUEUE_PTR_HI as usize]) << 8);
    bus_write(game, base.wrapping_add(cursor), QUEUE_END);
}

/// Read one stream byte at `($00)+y` (16-bit pointer, `Y` offset).
fn stream_byte(game: &mut Game, y: u8) -> u8 {
    let base = u16::from(game.ram[QUEUE_PTR_LO as usize])
        | (u16::from(game.ram[QUEUE_PTR_HI as usize]) << 8);
    bus_read(game, base.wrapping_add(u16::from(y)))
}

/// `LDA ($00),Y` page-cross penalty (1 cycle when `lo($00) + Y` carries).
fn stream_cross(game: &Game, y: u8) -> u64 {
    u64::from(u16::from(game.ram[QUEUE_PTR_LO as usize]) + u16::from(y) > 0xFF)
}

/// `bank7_LD2EC` (`prg7.asm $D2EC`): drain the PPU update queue.
///
/// Faithful port of `$D2EC-$D343` (see format docs above). All PPU traffic
/// goes through the bus (`$2000`/`$2006`/`$2007` writes, one `$2002` read
/// per entry via `LDX $2002`). The stream must be `$FF`-terminated;
/// unterminated streams loop like the original.
///
/// Exit (on `$FF`): `A = $FF`, `Y = 0`, `Z = 1`, `C = 1`, `N = 0` (from the
/// final `CMP #$FF`); `X` = last `LDX $2002` value (entry `X` preserved
/// when the first byte is `$FF`).
pub fn drain_update_queue(game: &mut Game) {
    // Cycle accounting (the trap table charges the 18-cycle empty-queue
    // exit; everything else is added here per entry, counted from the
    // listing: entry 16 (44 via a `$4C` redirect), header 52 + the two
    // control branches, tile loop 17/18 per byte, pointer advance 15 or
    // 22 on a page carry, plus the `(zp),Y` page-cross penalty of every
    // stream read).
    let mut extra: u64 = 0;
    loop {
        // LDY #$00 : LDA ($00),y : CMP #$FF : BEQ end.
        game.cpu.y = 0;
        set_nz(&mut game.cpu.p, 0);
        let mut y: u8 = 0;
        let first = stream_byte(game, y);
        game.cpu.a = first;
        set_nz(&mut game.cpu.p, first);
        cmp_val(&mut game.cpu.p, first, QUEUE_END);
        if first == QUEUE_END {
            game.cpu.cycles += extra;
            return; // BEQ $D2EB: executes the previous routine's RTS.
        }
        extra += 16;
        // CMP #$4C : BNE entry.
        cmp_val(&mut game.cpu.p, first, QUEUE_REDIRECT);
        if first == QUEUE_REDIRECT {
            extra += 28;
            // INY : LDA ($00),y : TAX : INY : LDA ($00),y : STA $01 : STX $00.
            y = y.wrapping_add(1);
            set_nz(&mut game.cpu.p, y);
            extra += stream_cross(game, y);
            let lo = stream_byte(game, y);
            game.cpu.a = lo;
            set_nz(&mut game.cpu.p, lo);
            game.cpu.x = lo;
            set_nz(&mut game.cpu.p, lo);
            y = y.wrapping_add(1);
            set_nz(&mut game.cpu.p, y);
            extra += stream_cross(game, y);
            let hi = stream_byte(game, y);
            game.cpu.a = hi;
            set_nz(&mut game.cpu.p, hi);
            game.ram[QUEUE_PTR_HI as usize] = hi;
            game.ram[QUEUE_PTR_LO as usize] = lo;
            // LDY #$00 : LDA ($00),y (fall into entry with the new pointer).
            y = 0;
            game.cpu.y = 0;
            set_nz(&mut game.cpu.p, 0);
            let b = stream_byte(game, y);
            game.cpu.a = b;
            set_nz(&mut game.cpu.p, b);
        }
        // LD307: LDX $2002 : STA $2006.
        extra += 52;
        let status = bus_read(game, 0x2002);
        game.cpu.x = status;
        set_nz(&mut game.cpu.p, status); // LDX sets N/Z only (V untouched).
        let hi_byte = game.cpu.a;
        bus_write(game, 0x2006, hi_byte);
        // INY : LDA ($00),y : STA $2006.
        y = y.wrapping_add(1);
        set_nz(&mut game.cpu.p, y);
        game.cpu.y = y;
        extra += stream_cross(game, y);
        let lo_byte = stream_byte(game, y);
        game.cpu.a = lo_byte;
        set_nz(&mut game.cpu.p, lo_byte);
        bus_write(game, 0x2006, lo_byte);
        // INY : LDA ($00),y : ASL : PHA.
        y = y.wrapping_add(1);
        set_nz(&mut game.cpu.p, y);
        game.cpu.y = y;
        extra += stream_cross(game, y);
        let ctrl = stream_byte(game, y);
        game.cpu.a = ctrl;
        set_nz(&mut game.cpu.p, ctrl);
        let a = game.cpu.a;
        let carry = a & 0x80 != 0;
        let shifted = a << 1;
        set_nz(&mut game.cpu.p, shifted);
        if carry {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        game.cpu.a = shifted;
        // PHA (stack push; balance with the PLA below).
        game.ram[0x0100 + usize::from(game.cpu.sp)] = game.cpu.a;
        game.cpu.sp = game.cpu.sp.wrapping_sub(1);
        // LDA $FF : ORA #$04 : BCS skip : AND #$FB / skip: STA $2000.
        let mut ctrl2 = game.ram[SPRITE_BANK as usize];
        game.cpu.a = ctrl2;
        set_nz(&mut game.cpu.p, ctrl2);
        ctrl2 |= 0x04;
        game.cpu.a = ctrl2;
        set_nz(&mut game.cpu.p, ctrl2);
        let bit7 = game.cpu.p & FLAG_C != 0;
        extra += if bit7 { 3 } else { 4 };
        if !bit7 {
            ctrl2 &= 0xFB;
            game.cpu.a = ctrl2;
            set_nz(&mut game.cpu.p, ctrl2);
        }
        bus_write(game, 0x2000, ctrl2);
        // PLA : ASL : BCC skip : ORA #$02 : INY / skip: LSR : LSR : TAX.
        game.cpu.sp = game.cpu.sp.wrapping_add(1);
        let pulled = game.ram[0x0100 + usize::from(game.cpu.sp)];
        game.cpu.a = pulled;
        set_nz(&mut game.cpu.p, pulled);
        let a = game.cpu.a;
        let carry2 = a & 0x80 != 0;
        let shifted2 = a << 1;
        set_nz(&mut game.cpu.p, shifted2);
        if carry2 {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        game.cpu.a = shifted2;
        extra += if carry2 { 6 } else { 3 };
        if carry2 {
            let o = game.cpu.a | 0x02;
            game.cpu.a = o;
            set_nz(&mut game.cpu.p, o);
            y = y.wrapping_add(1);
            set_nz(&mut game.cpu.p, y);
            game.cpu.y = y;
        }
        // LSR : LSR : TAX (count = low 6 bits of the shifted control).
        let l1 = game.cpu.a >> 1;
        let c1 = game.cpu.a & 1 != 0;
        if c1 {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        game.cpu.a = l1;
        set_nz(&mut game.cpu.p, l1);
        let l2 = l1 >> 1;
        let c2 = l1 & 1 != 0;
        if c2 {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        game.cpu.a = l2;
        set_nz(&mut game.cpu.p, l2);
        game.cpu.x = l2;
        set_nz(&mut game.cpu.p, l2);
        // Tile loop: BCS skip-INY : LDA ($00),y : STA $2007 : DEX : BNE.
        // No instruction in the loop touches C, so the LSR carry decides
        // every iteration identically (set = re-read one byte X times).
        loop {
            extra += if c2 { 17 } else { 18 };
            if !c2 {
                y = y.wrapping_add(1);
                set_nz(&mut game.cpu.p, y);
                game.cpu.y = y;
            }
            extra += stream_cross(game, y);
            let tile = stream_byte(game, y);
            game.cpu.a = tile;
            set_nz(&mut game.cpu.p, tile);
            bus_write(game, 0x2007, tile);
            game.cpu.x = game.cpu.x.wrapping_sub(1);
            set_nz(&mut game.cpu.p, game.cpu.x);
            if game.cpu.x == 0 {
                extra -= 1; // final BNE not taken
                break;
            }
        }
        // INY : TYA : CLC : ADC $00 : STA $00 : BCC top : INC $01 : JMP top.
        y = y.wrapping_add(1);
        set_nz(&mut game.cpu.p, y);
        game.cpu.y = y;
        game.cpu.a = y;
        set_nz(&mut game.cpu.p, y);
        game.cpu.p &= !FLAG_C; // CLC.
        let lo_ptr = game.ram[QUEUE_PTR_LO as usize];
        let adv = adc_val(&mut game.cpu.p, y, lo_ptr);
        game.cpu.a = adv;
        game.ram[QUEUE_PTR_LO as usize] = adv;
        if game.cpu.p & FLAG_C == 0 {
            extra += 15; // INY TYA CLC ADC STA + BCC taken
            continue; // BCC top.
        }
        extra += 22; // ... BCC not taken + INC zp + JMP
        let hi_ptr = game.ram[QUEUE_PTR_HI as usize];
        let r = inc_val(&mut game.cpu.p, hi_ptr);
        game.ram[QUEUE_PTR_HI as usize] = r;
        // JMP top.
    }
}

/// `$FD80` 2-byte divide table (`LFD80: $03,$01`) used by [`load_sideview_palette`].
pub const PALETTE_DIV_TABLE: u16 = 0xFD80;

/// `bank7_code52` (`prg7.asm $FD82`): side-view palette-row loader.
///
/// Runs when `$07AE != 0` (called from NMI, `$C0C2`). Derives a PPU address
/// from `$071E/$071D/$0720`, streams 7 bytes from `$0471+X` to `$2007`
/// (zeroing each after upload), then `STY $03A3` (entry `Y`) and clears
/// `$07AE`. Entry `Y` is preserved throughout and stored at the end.
pub fn load_sideview_palette(game: &mut Game) {
    // LDA $0720 : LSR : TAX.
    let v20 = bus_read(game, 0x0720);
    game.cpu.a = v20;
    set_nz(&mut game.cpu.p, v20);
    let c = v20 & 1 != 0;
    let s = v20 >> 1;
    if c {
        game.cpu.p |= FLAG_C;
    } else {
        game.cpu.p &= !FLAG_C;
    }
    game.cpu.a = s;
    game.cpu.p &= !FLAG_N;
    if s == 0 {
        game.cpu.p |= FLAG_Z;
    } else {
        game.cpu.p &= !FLAG_Z;
    }
    game.cpu.x = s;
    set_nz(&mut game.cpu.p, s);
    // LDA $071E : AND #$1F : SEC : SBC LFD80,x.
    let v1e = bus_read(game, 0x071E);
    game.cpu.a = v1e;
    set_nz(&mut game.cpu.p, v1e);
    let masked = v1e & 0x1F;
    game.cpu.a = masked;
    set_nz(&mut game.cpu.p, masked);
    game.cpu.p |= FLAG_C; // SEC.
    let div = bus_read(game, PALETTE_DIV_TABLE.wrapping_add(u16::from(game.cpu.x)));
    // SEC : SBC LFD80,x.
    let res = sbc_val(&mut game.cpu.p, masked, div);
    game.cpu.a = res;
    // LSR : LSR : CLC : ADC #$C0 : STA $01.
    for _ in 0..2u8 {
        let a = game.cpu.a;
        let c = a & 1 != 0;
        let r = a >> 1;
        if c {
            game.cpu.p |= FLAG_C;
        } else {
            game.cpu.p &= !FLAG_C;
        }
        game.cpu.a = r;
        game.cpu.p &= !FLAG_N;
        if r == 0 {
            game.cpu.p |= FLAG_Z;
        } else {
            game.cpu.p &= !FLAG_Z;
        }
    }
    game.cpu.p &= !FLAG_C; // CLC.
    let a = adc_val(&mut game.cpu.p, game.cpu.a, 0xC0);
    game.cpu.a = a;
    game.ram[QUEUE_PTR_HI as usize] = a;
    // LDA $071D : ORA #$03 : STA $00.
    let v1d = bus_read(game, 0x071D);
    game.cpu.a = v1d;
    set_nz(&mut game.cpu.p, v1d);
    let o = v1d | 0x03;
    game.cpu.a = o;
    set_nz(&mut game.cpu.p, o);
    game.ram[QUEUE_PTR_LO as usize] = o;
    // LDX #$00 : STX $07AE.
    game.cpu.x = 0;
    set_nz(&mut game.cpu.p, 0);
    bus_write(game, 0x07AE, 0);
    // LFDA3 loop: 7 rows from $0471+X.
    loop {
        // LDA $2002 (single read; LDA sets N/Z, value discarded).
        let status = bus_read(game, 0x2002);
        game.cpu.a = status;
        set_nz(&mut game.cpu.p, status);
        // LDA $00 : STA $2006.
        let lo = game.ram[QUEUE_PTR_LO as usize];
        game.cpu.a = lo;
        set_nz(&mut game.cpu.p, lo);
        bus_write(game, 0x2006, lo);
        // LDA $01 : CLC : ADC #$08 : STA $2006 : STA $01.
        let hi = game.ram[QUEUE_PTR_HI as usize];
        game.cpu.a = hi;
        set_nz(&mut game.cpu.p, hi);
        game.cpu.p &= !FLAG_C;
        let a = adc_val(&mut game.cpu.p, hi, 0x08);
        game.cpu.a = a;
        bus_write(game, 0x2006, a);
        game.ram[QUEUE_PTR_HI as usize] = a;
        // LDA $0471,x : STA $2007 : LDA #$00 : STA $0471,x.
        let row = bus_read(game, 0x0471 + u16::from(game.cpu.x));
        game.cpu.a = row;
        set_nz(&mut game.cpu.p, row);
        bus_write(game, 0x2007, row);
        game.cpu.a = 0;
        set_nz(&mut game.cpu.p, 0);
        bus_write(game, 0x0471 + u16::from(game.cpu.x), 0);
        // INX : CPX #$07 : BCC LFDA3.
        game.cpu.x = game.cpu.x.wrapping_add(1);
        set_nz(&mut game.cpu.p, game.cpu.x);
        cmp_val(&mut game.cpu.p, game.cpu.x, 0x07);
        if game.cpu.x >= 0x07 {
            break;
        }
    }
    // LDA #$FF : STY $03A3 (entry Y) : RTS.
    game.cpu.a = 0xFF;
    set_nz(&mut game.cpu.p, 0xFF);
    let y = game.cpu.y;
    bus_write(game, 0x03A3, y);
}
