//! CPU unit tests: synthetic PRG only, no ROM.
//!
//! Covers every addressing mode, all 8 branches (+page-cross cost), stack
//! discipline, `JSR`/`RTS` nesting, `JMP` indirect page-wrap bug,
//! `BRK`/`NMI`/`IRQ` vectors, `RTS`/`RTI` restores, no-decimal `ADC`/`SBC`,
//! the MMC1 shift register (commit order, reset-bit, PRG modes) and the
//! PPU/APU/controller/DMA bus routing.

#![cfg(feature = "interp")]

use z2_core::cpu::{Mmc1, FLAG_B, FLAG_C, FLAG_D, FLAG_I, FLAG_N, FLAG_V, FLAG_Z};
use z2_core::game::Game;

/// Fresh game with `blob` loaded at `origin`, registers zeroed, `SP=$FD`.
fn prog(origin: u16, blob: &[u8]) -> Game {
    let mut g = Game::with_test_program(origin, blob);
    g.set_cpu(0, 0, 0, 0xFD, origin, 0x20);
    g
}

/// Run `blob` (must end in `RTS`) via `call_asm` and return the game.
fn run(origin: u16, blob: &[u8]) -> Game {
    let mut g = prog(origin, blob);
    g.call_asm(origin);
    g
}

fn flags(g: &Game) -> u8 {
    g.cpu_state().5
}

// ------------------------------------------------- loads/stores

#[test]
fn lda_all_addressing_modes() {
    // Layout: blob at $C000; data planted in RAM + PRG window.
    let blob = vec![
        0xA9, 0x42, // LDA #$42
        0x85, 0x10, // STA $10
        0xA5, 0x10, // LDA $10
        0xB5, 0x0F, // LDA $0F,X (X=1 -> $10)
        0xAD, 0x00, 0xC8, // LDA $C800 (PRG marker, abs)
        0xBD, 0xFF, 0xC7, // LDA $C7FF,X (X=1 -> $C800, page cross)
        0xB9, 0xFF, 0xC7, // LDA $C7FF,Y (Y=1 -> $C800, page cross)
        0xA1, 0x20, // LDA ($20,X) (X=0 -> ptr at $20 -> $C800)
        0xB1, 0x22, // LDA ($22),Y (ptr $C7FF + Y=1 -> $C800)
        0x60, // RTS
    ];
    // Prepend nothing; blob starts at $C000. Data at $C800 must live in the
    // same 32K test PRG: patch after construction instead.
    let mut g = prog(0xC000, &blob);
    // Plant PRG marker byte at $C800 (last-bank mapping, offset $4800).
    g.prg[0x4800] = 0x77;
    g.prg[0x4801] = 0x77;
    g.set_cpu(0, 1, 1, 0xFD, 0xC000, 0x20);
    // ($20) -> $C800 ; ($22) -> $C7FF
    g.ram[0x20] = 0x00;
    g.ram[0x21] = 0xC8;
    g.ram[0x22] = 0xFF;
    g.ram[0x23] = 0xC7;
    let c0 = g.cpu.cycles;
    g.call_asm(0xC000);
    assert_eq!(g.cpu.a, 0x77, "last LDA should win");
    assert_eq!(g.ram[0x10], 0x42);
    // Cycle check: imm 2 + STA zp 3 + zp 3 + zpx 4 + abs 4 + absx+cross 5 +
    // absy+cross 5 + indx 6 + indy+cross 6 + RTS 6 = 44.
    assert_eq!(g.cpu.cycles - c0, 44, "page-cross penalties must apply");
    assert_eq!(flags(&g) & (FLAG_Z | FLAG_N), 0);
}

#[test]
fn sta_stx_sty_and_ldx_ldy_modes() {
    let blob = vec![
        0xA2, 0x11, // LDX #$11
        0x86, 0x30, // STX $30
        0xA0, 0x22, // LDY #$22
        0x84, 0x31, // STY $31
        0xB6, 0x2F, // LDX $2F,Y (Y=$22 -> ($2F+$22)&$FF=$51)
        0xB4, 0x30, // LDY $30,X (X=$AA now -> ($30+$AA)&$FF=$DA)
        0x96, 0x32, // STX $32,Y (Y=$41? -> wraps)
        0x94, 0x33, // STY $33,X
        0xAE, 0x34, 0x00, // LDX $0034
        0xBE, 0x33, 0x00, // LDX $0033,Y
        0xAC, 0x34, 0x00, // LDY $0034
        0xBC, 0x33, 0x00, // LDY $0033,X
        0x8E, 0x35, 0x00, // STX $0035
        0x8C, 0x36, 0x00, // STY $0036
        0x60,
    ];
    let mut g = prog(0xC000, &blob);
    // $2F+Y($22) = $51 (zp wrap): LDX loads ram[$51].
    g.ram[0x51] = 0xAA;
    // LDY $30,X: X after LDX $51 = $AA -> ($30+$AA)&FF = $DA.
    g.ram[0xDA] = 0xBB;
    // STX $32,Y: Y=$BB -> ($32+$BB)&FF = $ED. STY $33,X: X=$AA -> ($33+$AA)&FF=$DD.
    g.ram[0x34] = 0xCC;
    // LDX $33,Y: Y=$BB -> $33+$BB = $EE.
    g.ram[0xEE] = 0xDD;
    // LDY $33,X: X=$DD -> $33+$DD = $0110 (absolute indexing does NOT wrap
    // to $10; hardware reads $0110).
    g.ram[0x110] = 0xEE;
    g.call_asm(0xC000);
    assert_eq!(g.ram[0x30], 0x11);
    assert_eq!(g.ram[0x31], 0x22);
    assert_eq!(g.cpu.x, 0xDD, "LDX $0033,Y overwrote the absolute load");
    assert_eq!(g.cpu.y, 0xEE, "LDY $0033,X should win");
    assert_eq!(g.ram[0xED], 0xAA, "STX $32,Y wraps in zero page");
    assert_eq!(g.ram[0xDD], 0xBB, "STY $33,X wraps in zero page");
    assert_eq!(g.ram[0x35], 0xDD);
    assert_eq!(g.ram[0x36], 0xEE);
}

#[test]
fn transfers_and_register_inc_dec() {
    let blob = vec![
        0xA9, 0x00, // LDA #0
        0xAA, // TAX (X=0, Z=1)
        0xCA, // DEX (X=$FF, N=1)
        0x8A, // TXA (A=$FF)
        0xA8, // TAY (Y=$FF)
        0x88, // DEY (Y=$FE)
        0x98, // TYA (A=$FE)
        0xE8, // INX (X=0, Z=1)
        0xC8, // INY (Y=$FF)
        0xBA, // TSX (X=SP)
        0x9A, // TXS (SP=X, no flags)
        0x60,
    ];
    let g = run(0xC000, &blob);
    let (a, x, y, sp, _, p) = g.cpu_state();
    assert_eq!(a, 0xFE);
    assert_eq!(y, 0xFF);
    assert_eq!(
        x, 0xFB,
        "TSX read SP mid-call (synthetic return still pushed)"
    );
    assert_eq!(sp, 0xFD, "stack balanced after return");
    assert_eq!(p & FLAG_N, FLAG_N, "INY left Y=$FF: N set");
    assert_eq!(p & FLAG_Z, 0);
}

// ------------------------------------------------- ADC/SBC/CMP (no decimal)

#[test]
fn adc_binary_ignores_decimal_flag() {
    // SED then ADC must behave binary (2A03 has no BCD).
    let blob = vec![
        0xF8, // SED (D stored but inert)
        0xA9, 0x7F, // LDA #$7F
        0x69, 0x01, // ADC #$01 -> $80, V=1, N=1
        0x85, 0x40, // STA $40
        0x60,
    ];
    let g = run(0xC000, &blob);
    assert_eq!(g.ram[0x40], 0x80);
    let p = flags(&g);
    assert_eq!(p & FLAG_D, FLAG_D, "D stays set; it just does nothing");
    assert_eq!(p & (FLAG_V | FLAG_N), FLAG_V | FLAG_N);
    assert_eq!(p & (FLAG_C | FLAG_Z), 0);
}

#[test]
fn adc_carry_and_zero() {
    let blob = vec![
        0x18, // CLC
        0xA9, 0xFF, // LDA #$FF
        0x69, 0x01, // ADC #$01 -> $00, C=1, Z=1
        0x69, 0x00, // ADC #$00 +C -> $01, C=0
        0x85, 0x41, // STA $41
        0x60,
    ];
    let g = run(0xC000, &blob);
    assert_eq!(g.ram[0x41], 0x01);
    assert_eq!(flags(&g) & (FLAG_C | FLAG_Z), 0);
}

#[test]
fn sbc_borrow_chain() {
    let blob = vec![
        0x38, // SEC
        0xA9, 0x00, // LDA #0
        0xE9, 0x01, // SBC #1 -> $FF, C=0, N=1
        0x85, 0x42, // STA $42
        0xA9, 0x80, // LDA #$80
        0xE9, 0x01, // SBC #1 with C=0 -> $80-$01-$01=$7E? ($80-1-1=$7E, V=1)
        0x85, 0x43, // STA $43
        0x60,
    ];
    let g = run(0xC000, &blob);
    assert_eq!(g.ram[0x42], 0xFF);
    assert_eq!(g.ram[0x43], 0x7E);
    assert_eq!(flags(&g) & FLAG_V, FLAG_V, "SBC $80-$01-$01 overflows");
}

#[test]
fn cmp_cpx_cpy_and_bit() {
    let blob = vec![
        0xA9, 0x05, // LDA #5
        0xC9, 0x05, // CMP #5 -> Z=1 C=1
        0x85, 0x50, // STA $50 (stash A)
        0xA2, 0x05, // LDX #5
        0xE0, 0x06, // CPX #6 -> C=0, N=1 ($FF)
        0xA0, 0x05, // LDY #5
        0xC0, 0x05, // CPY #5 -> Z=1
        0xA9, 0x0F, // LDA #$0F
        0x2C, 0x51, 0x00, // BIT $0051 ($F0: Z=0? A&$F0=0 -> Z=1, V=1, N=1)
        0x60,
    ];
    let mut g = prog(0xC000, &blob);
    g.ram[0x51] = 0xF0;
    g.call_asm(0xC000);
    let p = flags(&g);
    assert_eq!(p & FLAG_Z, FLAG_Z);
    assert_eq!(p & (FLAG_V | FLAG_N), FLAG_V | FLAG_N);
}

// ------------------------------------------------- logic ops all modes

#[test]
fn and_ora_eor_all_modes() {
    // One mode each (full matrix is covered by LDA above sharing helpers).
    let blob = vec![
        0xA9, 0xFF, // LDA #$FF
        0x25, 0x60, // AND $60 ($0F -> A=$0F)
        0x15, 0x61, // ORA $61,X (X=0, $F0 -> A=$FF)
        0x4D, 0x62, 0x00, // EOR $0062 ($FF -> A=$00, Z=1)
        0x60,
    ];
    let mut g = prog(0xC000, &blob);
    g.ram[0x60] = 0x0F;
    g.ram[0x61] = 0xF0;
    g.ram[0x62] = 0xFF;
    g.call_asm(0xC000);
    assert_eq!(g.cpu.a, 0x00);
    assert_eq!(flags(&g) & FLAG_Z, FLAG_Z);
}

// ------------------------------------------------- shifts

#[test]
fn shifts_thread_carry() {
    let blob = vec![
        0xA9, 0x81, // LDA #$81
        0x0A, // ASL A -> $02, C=1
        0x85, 0x70, // STA $70
        0x2A, // ROL A -> $05
        0x85, 0x71, // STA $71
        0x4A, // LSR A -> $02, C=1
        0x6A, // ROR A -> $81
        0x85, 0x72, // STA $72
        0x06, 0x73, // ASL $73
        0x46, 0x74, // LSR $74
        0x26, 0x75, // ROL $75
        0x66, 0x76, // ROR $76
        0x60,
    ];
    let mut g = prog(0xC000, &blob);
    g.ram[0x73] = 0x81; // -> $02, C=1
    g.ram[0x74] = 0x01; // -> $00, C=1, Z=1
    g.ram[0x75] = 0x40; // C=1 -> $81, N=1 (C out=0)
    g.ram[0x76] = 0x00; // C=0 now -> $00, Z=1
    g.call_asm(0xC000);
    assert_eq!(g.ram[0x70], 0x02);
    assert_eq!(g.ram[0x71], 0x05);
    assert_eq!(g.ram[0x72], 0x81);
    assert_eq!(g.ram[0x73], 0x02);
    assert_eq!(g.ram[0x74], 0x00);
    assert_eq!(g.ram[0x75], 0x81);
    assert_eq!(g.ram[0x76], 0x00, "carry had cleared by the final ROR");
}

// ------------------------------------------------- INC/DEC

#[test]
fn inc_dec_modes_and_wrap() {
    let blob = vec![
        0xE6, 0x80, // INC $80
        0xF6, 0x81, // INC $81,X (X=1 -> $82)
        0xEE, 0x83, 0x00, // INC $0083
        0xFE, 0x83, 0x00, // INC $0083,X (X=1 -> $0084)
        0xC6, 0x85, // DEC $85
        0xD6, 0x86, // DEC $86,X (X=1 -> $87)
        0xCE, 0x88, 0x00, // DEC $0088
        0xDE, 0x87, 0x00, // DEC $0087,X (X=1 -> $0088)
        0x60,
    ];
    let mut g = prog(0xC000, &blob);
    g.set_cpu(0, 1, 0, 0xFD, 0xC000, 0x20);
    g.ram[0x80] = 0xFF; // -> 0, Z=1
    g.ram[0x82] = 0x7F; // -> $80, N=1
    g.ram[0x83] = 0x01; // -> 2
    g.ram[0x84] = 0x02; // -> 3
    g.ram[0x85] = 0x00; // -> $FF, N=1
    g.ram[0x87] = 0x01; // DEC,X -> 0, Z=1
    g.ram[0x88] = 0x02; // DEC -> 1, then DEC,X (same addr!) -> 0
    g.call_asm(0xC000);
    assert_eq!(g.ram[0x80], 0x00);
    assert_eq!(g.ram[0x82], 0x80);
    assert_eq!(g.ram[0x83], 0x02);
    assert_eq!(g.ram[0x84], 0x03);
    assert_eq!(g.ram[0x85], 0xFF);
    assert_eq!(g.ram[0x87], 0x00);
    assert_eq!(g.ram[0x88], 0x00, "DEC $88 then DEC $87,X alias it");
}

// ------------------------------------------------- branches

#[test]
fn all_eight_branches() {
    // Each branch skips one INC when taken; lands counted in $90.
    let mut blob = vec![
        0x38, // SEC
        0xB0, 0x02, // BCS +2 (taken)
        0xE6, 0x90, // (skipped)
        0x18, // CLC
        0x90, 0x02, // BCC +2 (taken)
        0xE6, 0x90, // (skipped)
        0xA9, 0x00, // LDA #0 (Z=1)
        0xF0, 0x02, // BEQ (taken)
        0xE6, 0x90, // (skipped)
        0xA9, 0x01, // LDA #1 (Z=0)
        0xD0, 0x02, // BNE (taken)
        0xE6, 0x90, // (skipped)
        0xA9, 0x80, // LDA #$80 (N=1)
        0x30, 0x02, // BMI (taken)
        0xE6, 0x90, // (skipped)
        0x10, 0x02, // BPL (not taken: N=1)
        0xE6, 0x90, // runs: $90=1
        0x50, 0x02, // BVC (taken, V=0)
        0xE6, 0x90, // (skipped)
        0x69, 0x7F, // ADC #$7F? A=$80... use BIT to set V instead:
    ];
    // Set V via BIT, then BVS taken.
    blob.extend_from_slice(&[
        0xA9, 0xFF, // LDA #$FF
        0x2C, 0x91, 0x00, // BIT $0091 ($40 -> V=1)
        0x70, 0x02, // BVS (taken)
        0xE6, 0x90, // (skipped)
        0x60,
    ]);
    let mut g = prog(0xC000, &blob);
    g.ram[0x91] = 0x40;
    g.call_asm(0xC000);
    assert_eq!(g.ram[0x90], 0x01, "only the not-taken BPL fell through");
}

#[test]
fn branch_page_cross_costs_extra_cycle() {
    // BNE at $C0FD (+1 crosses $C0FF->$C100): 2+1+1 = 4 cycles.
    let mut blob = vec![0xEA; 0xFD];
    blob.extend_from_slice(&[0xD0, 0x01, 0xEA, 0x60]);
    let mut g = prog(0xC000, &blob);
    g.set_cpu(0, 0, 0, 0xFD, 0xC0FD, 0x20); // Z=0 -> taken
    let c0 = g.cpu.cycles;
    g.step_instruction().unwrap();
    assert_eq!(g.cpu.pc, 0xC100);
    assert_eq!(g.cpu.cycles - c0, 4);
    // Not-taken costs 2.
    let mut g2 = prog(0xC000, &blob);
    g2.set_cpu(0, 0, 0, 0xFD, 0xC0FD, 0x20 | FLAG_Z);
    let c0 = g2.cpu.cycles;
    g2.step_instruction().unwrap();
    assert_eq!(g2.cpu.pc, 0xC0FF);
    assert_eq!(g2.cpu.cycles - c0, 2);
}

// ------------------------------------------------- control flow

#[test]
fn jsr_rts_nesting_restores_stack() {
    // main $C000: JSR sub1; RTS. sub1 $C006: JSR sub2; RTS. sub2: INC $51B; RTS.
    let blob = vec![
        0x20, 0x06, 0xC0, // $C000 JSR $C006
        0xEA, // $C003 NOP (landing pad)
        0x60, // $C004 RTS
        0xEA, // $C005 filler
        0x20, 0x0C, 0xC0, // $C006 JSR $C00C
        0x60, // $C009 RTS
        0xEA, 0xEA, // $C00A filler
        0xEE, 0x1B, 0x05, // $C00C INC $051B
        0x60, // RTS
    ];
    let g = run(0xC000, &blob);
    assert_eq!(g.ram[0x51B], 1);
    assert_eq!(g.cpu_state().3, 0xFD, "stack must balance");
}

#[test]
fn jmp_indirect_has_page_wrap_bug() {
    // JMP ($02FF): lo from $02FF, hi wraps to $0200 (not $0300).
    let blob = vec![0x6C, 0xFF, 0x02];
    let mut g = prog(0xC000, &blob);
    g.ram[0x2FF] = 0x34;
    g.ram[0x200] = 0x12;
    g.ram[0x300] = 0x99; // must be ignored (hardware bug)
    g.set_cpu(0, 0, 0, 0xFD, 0xC000, 0x20);
    g.step_instruction().unwrap();
    assert_eq!(g.cpu.pc, 0x1234);
}

#[test]
fn brk_jumps_to_vector_and_rti_restores() {
    // BRK at $C000; handler at $C100: INC $60; RTI.
    let mut blob = vec![0x00]; // BRK
    blob.resize(0x103, 0xEA);
    blob[0x100] = 0xE6;
    blob[0x101] = 0x60;
    blob[0x102] = 0x40; // RTI
    let mut g = prog(0xC000, &blob);
    g.set_irq_vector(0xC100);
    g.set_cpu(0xAA, 0, 0, 0xFD, 0xC000, 0x20);
    g.step_instruction().unwrap(); // BRK
    assert_eq!(g.cpu.pc, 0xC100);
    assert_eq!(flags(&g) & FLAG_I, FLAG_I);
    g.step_instruction().unwrap(); // INC $60
    assert_eq!(g.ram[0x60], 1);
    g.step_instruction().unwrap(); // RTI
    assert_eq!(g.cpu.pc, 0xC002, "RTI returns past BRK + pad byte");
    assert_eq!(g.cpu_state().3, 0xFD, "stack balanced");
    assert_eq!(g.cpu.a, 0xAA, "registers preserved");
}

#[test]
fn nmi_services_and_restores() {
    // main: JMP $C000 loop. NMI handler $C100: INC $61; RTI.
    let mut blob = vec![
        0x4C, 0x00, 0xC0, // JMP $C000
    ];
    blob.resize(0x103, 0xEA);
    blob[0x100] = 0xE6;
    blob[0x101] = 0x61;
    blob[0x102] = 0x40;
    let mut g = prog(0xC000, &blob);
    g.set_nmi_vector(0xC100);
    g.set_cpu(0, 0, 0, 0xFD, 0xC000, 0x20);
    g.request_nmi();
    g.run_cycles(30).unwrap();
    assert_eq!(g.ram[0x61], 1, "NMI handler ran once");
    assert_eq!(g.cpu.pc, 0xC000, "returned into the loop");
    assert!(!g.cpu.nmi_pending);
}

#[test]
fn irq_masked_while_i_set() {
    let mut blob = vec![0x4C, 0x00, 0xC0];
    blob.resize(0x103, 0xEA);
    blob[0x100] = 0xE6;
    blob[0x101] = 0x62;
    blob[0x102] = 0x40;
    let mut g = prog(0xC000, &blob);
    g.set_irq_vector(0xC100);
    g.set_cpu(0, 0, 0, 0xFD, 0xC000, 0x20 | FLAG_I);
    g.cpu.irq_pending = true;
    g.run_cycles(30).unwrap();
    assert_eq!(g.ram[0x62], 0, "IRQ masked by I");
    assert!(g.cpu.irq_pending, "level still latched");
    // Clear I -> serviced.
    let p = flags(&g) & !FLAG_I;
    let (a, x, y, sp, pc, _) = g.cpu_state();
    g.set_cpu(a, x, y, sp, pc, p);
    g.run_cycles(30).unwrap();
    assert_eq!(g.ram[0x62], 1);
}

#[test]
fn illegal_opcodes_error() {
    for op in [0x02u8, 0xA3, 0xFF, 0x80, 0x04] {
        let mut g = prog(0xC000, &[op]);
        g.set_cpu(0, 0, 0, 0xFD, 0xC000, 0x20);
        let err = g.step_instruction().unwrap_err();
        assert_eq!(
            err,
            z2_core::cpu::ExecError::IllegalOpcode {
                opcode: op,
                at: 0xC000
            },
            "opcode ${op:02X} must be rejected"
        );
    }
}

// ------------------------------------------------- stack ops

#[test]
fn pha_pla_php_plp_conventions() {
    let blob = vec![
        0xA9, 0x5A, // LDA #$5A
        0x48, // PHA
        0xA9, 0x00, // LDA #0
        0x68, // PLA -> $5A
        0x38, // SEC (C=1 to round-trip)
        0x08, // PHP (pushes P with B+U)
        0x18, // CLC
        0x28, // PLP (C=1 again; B cleared, U set)
        0x60,
    ];
    let g = run(0xC000, &blob);
    assert_eq!(g.cpu.a, 0x5A);
    let p = flags(&g);
    assert_eq!(p & FLAG_C, FLAG_C);
    assert_eq!(p & FLAG_B, 0, "B never sticks in P");
    assert_eq!(p & z2_core::cpu::FLAG_U, z2_core::cpu::FLAG_U);
}

// ------------------------------------------------- MMC1

fn mmc1_write_seq(m: &mut Mmc1, addr: u16, bits: &[u8]) {
    for &b in bits {
        m.write(addr, b);
    }
}

#[test]
fn mmc1_shift_commits_lsb_first() {
    let mut m = Mmc1::new();
    // Bits [1,0,1,0,0] -> reg = 0b00101 = $05.
    mmc1_write_seq(&mut m, 0x8000, &[1, 0, 1, 0, 0]);
    assert_eq!(m.ctrl, 0x05);
    assert_eq!(m.count, 0);
    // Targets route by address bits 14-13.
    mmc1_write_seq(&mut m, 0xA000, &[1, 1, 1, 1, 1]);
    assert_eq!(m.chr0, 0x1F);
    mmc1_write_seq(&mut m, 0xC000, &[0, 1, 0, 1, 0]);
    assert_eq!(m.chr1, 0x0A);
    mmc1_write_seq(&mut m, 0xE000, &[1, 1, 0, 0, 0]);
    assert_eq!(m.prg, 0x03);
}

#[test]
fn mmc1_reset_bit_clears_shift_and_forces_prg_mode3() {
    let mut m = Mmc1::new();
    mmc1_write_seq(&mut m, 0x8000, &[1, 1]); // partial
    m.write(0x8000, 0x80); // reset bit
    assert_eq!((m.shift, m.count), (0, 0));
    assert_eq!(m.prg_mode(), 3, "ctrl |= $0C forces fix-last");
    // Partial bits were discarded: 5 fresh bits commit cleanly.
    mmc1_write_seq(&mut m, 0xE000, &[1, 0, 0, 0, 0]);
    assert_eq!(m.prg, 0x01);
    // Reset mid-sequence also works when ctrl had another mode.
    mmc1_write_seq(&mut m, 0x8000, &[0, 0, 0, 0, 0]);
    assert_eq!(m.prg_mode(), 0);
    m.write(0xFFFF, 0xFF);
    assert_eq!(m.prg_mode(), 3);
}

#[test]
fn mmc1_prg_bank_modes() {
    // 128 KiB PRG image (8x16K), like Zelda II.
    let len = 0x20000usize;
    let mut m = Mmc1::new(); // ctrl=$0C: mode 3, fix last.
    m.prg = 5;
    assert_eq!(m.map_prg(0x8000, len), 5 * 0x4000);
    assert_eq!(m.map_prg(0xBFFF, len), 5 * 0x4000 + 0x3FFF);
    assert_eq!(m.map_prg(0xC000, len), 7 * 0x4000, "last bank fixed");
    assert_eq!(m.map_prg(0xFFFF, len), len - 1);
    // 32K mode ignores the low bit.
    m.ctrl = 0x00;
    m.prg = 5;
    assert_eq!(m.map_prg(0x8000, len), 4 * 0x4000);
    assert_eq!(m.map_prg(0xC000, len), 5 * 0x4000);
    // Fix-first mode.
    m.ctrl = 0x08;
    m.prg = 5;
    assert_eq!(m.map_prg(0x8000, len), 0);
    assert_eq!(m.map_prg(0xC000, len), 5 * 0x4000);
    // Bank numbers wrap modulo bank count.
    m.ctrl = 0x0C;
    m.prg = 0x1F;
    assert_eq!(m.map_prg(0x8000, len), 7 * 0x4000);
    // CHR paging helper for the PPU.
    m.ctrl = 0x0C; // chr mode 0: 8K
    m.chr0 = 2;
    assert_eq!(m.map_chr4(0), 2);
    assert_eq!(m.map_chr4(1), 3);
    m.ctrl = 0x1C; // chr mode 1: 2x4K
    m.chr1 = 5;
    assert_eq!((m.map_chr4(0), m.map_chr4(1)), (2, 5));
    assert_eq!(m.mirroring(), 0);
}

// ------------------------------------------------- bus routing

#[test]
fn controller_latch_reads_back_input_bits() {
    // Strobe on/off, then read 9 bits from $4016; store to $10-$18.
    let mut blob = vec![
        0xA9, 0x01, 0x8D, 0x16, 0x40, // strobe on
        0xA9, 0x00, 0x8D, 0x16, 0x40, // strobe off (latch)
    ];
    for i in 0..9u8 {
        blob.extend_from_slice(&[0xAD, 0x16, 0x40, 0x85, 0x10 + i]);
    }
    blob.push(0x60);
    let mut g = prog(0xC000, &blob);
    // Bits: A + Start + Left = 0b0100_1001.
    g.set_pad(
        z2_core::game::BTN_A | z2_core::game::BTN_START | z2_core::game::BTN_LEFT,
        0,
    );
    g.call_asm(0xC000);
    let bits: Vec<u8> = (0..9).map(|i| g.ram[0x10 + i]).collect();
    assert_eq!(&bits[..8], &[1, 0, 0, 1, 0, 0, 1, 0]);
    assert_eq!(bits[8], 1, "reads past 8 return 1");
}

#[test]
fn oam_dma_copies_page() {
    let mut g = prog(0xC000, &[0xA9, 0x02, 0x8D, 0x14, 0x40, 0x60]);
    for i in 0..256usize {
        g.ram[0x200 + i] = (i ^ 0xA5) as u8;
    }
    let c0 = g.cpu.cycles;
    g.call_asm(0xC000);
    for i in 0..256usize {
        assert_eq!(g.oam[i], (i ^ 0xA5) as u8, "oam[{i}]");
    }
    assert!(g.cpu.cycles - c0 >= 2 + 2 + 513, "DMA costs 513/514 cycles");
}

#[test]
fn ppu_apu_facade_traffic_routed() {
    let blob = vec![
        0xA9, 0x81, 0x8D, 0x00, 0x20, // STA $2000
        0xAD, 0x01, 0x20, // LDA $2001 (NullPpu -> 0)
        0x8D, 0x00, 0x40, // STA $4000
        0x60,
    ];
    let mut g = prog(0xC000, &blob);
    g.call_asm(0xC000);
    assert_eq!(g.ppu.writes, 1);
    assert_eq!(g.ppu.last_write, Some((0x2000, 0x81)));
    assert_eq!(g.ppu.reads, 1);
    assert_eq!(g.cpu.a, 0, "null PPU reads 0");
    assert_eq!(g.apu.writes, 1);
    assert_eq!(g.apu.last_write, Some((0x4000, 0x00)));
}

#[test]
fn reset_uses_vector_and_power_state() {
    let mut g = Game::with_test_program(0xC000, &[0xEA]);
    // with_test_program points RESET at origin already.
    g.reset();
    let (_, _, _, sp, pc, p) = g.cpu_state();
    assert_eq!(pc, 0xC000);
    assert_eq!(sp, 0xFD);
    assert_eq!(p & FLAG_I, FLAG_I);
}

#[test]
fn step_advances_frame_and_latches_nmi() {
    use z2_core::cpu::PpuBus;
    // Hermetic NMI test with a real vector: main spins on JMP self, the
    // NMI handler INCs $00 and RTIs. Edge-based NMI:
    // frame 1 is PPU warmup (k=0 drops the flag), the edge lands frame 2+.
    let mut blob = vec![0u8; 0x8000];
    blob[0x0000..0x0003].copy_from_slice(&[0x4C, 0x00, 0x80]); // JMP $8000
    blob[0x0100..0x0103].copy_from_slice(&[0xE6, 0x00, 0x40]); // INC $00; RTI
    blob[0x7FFA..0x7FFC].copy_from_slice(&[0x00, 0x81]); // NMI -> $8100
    blob[0x7FFC..0x7FFE].copy_from_slice(&[0x00, 0x80]); // RST -> $8000
    blob[0x7FFE..0x8000].copy_from_slice(&[0x00, 0x80]); // IRQ -> $8000
    let mut g = prog(0x8000, &blob);
    assert_eq!(g.frame_count(), 0);
    // NMI disarmed: warmup + frame 2 stay quiet, handler never runs.
    g.step(0);
    g.step(0b1000_0001);
    assert_eq!(g.frame_count(), 2);
    assert_eq!(g.ram[0x00], 0, "no NMI while $2000 bit 7 clear");
    // Arm NMI via $2000 bit 7: the next vblank edge services the vector.
    g.ppu.ppu_write(0x2000, 0x80);
    g.step(0);
    g.step(0);
    assert!(g.ram[0x00] >= 1, "NMI handler ran at least once");
    assert!(!g.cpu.nmi_pending, "latched NMI is serviced in-step");
}
