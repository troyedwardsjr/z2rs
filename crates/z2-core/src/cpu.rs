//! Embedded 2A03 (6502-without-decimal) interpreter.
//!
//! The interpreter executes the original Zelda II PRG bytes directly on the
//! shared [`crate::game::Game`] RAM mirror (`ram` = NES `$0000-$07FF`,
//! `wram` = battery WRAM `$6000-$7FFF`). Ported Rust routines take over one
//! address at a time via the [`crate::traps`] trap table; everything else
//! keeps interpreting, so the game stays playable at every intermediate
//! stage of the reconstruction.
//!
//! # CPU memory map (as seen by [`step_instruction`])
//!
//! | Range         | Backing                                                      |
//! |---------------|--------------------------------------------------------------|
//! | `$0000-$1FFF` | `Game::ram` (2 KiB mirrored every `$0800`)                   |
//! | `$2000-$3FFF` | PPU registers `$2000-$2007` (mirrored every 8) via `PpuBus` |
//! | `$4000-$4013`, `$4015` | APU registers via `ApuBus` |
//! | `$4014`       | OAM DMA (256-byte page copy into `Game::oam`)                |
//! | `$4016`       | Controller 1 latch/shift (driven by `Game::pad1`)            |
//! | `$4017`       | Writes → `ApuBus` frame counter; reads → controller 2       |
//! | `$4018-$401F` | Open bus (reads return `0x00`)                               |
//! | `$4020-$5FFF` | Expansion / open bus (reads return `0x00`)                   |
//! | `$6000-$7FFF` | `Game::wram` (8 KiB battery WRAM; SLROM keeps it mapped)     |
//! | `$8000-$FFFF` | PRG ROM through [`Mmc1`]; writes commit MMC1 registers       |
//!
//! # Opcode coverage (all 151 legal NMOS 6502 opcodes)
//!
//! | Mnemonic | Modes implemented |
//! |----------|-------------------|
//! | LDA/STA | imm, zp, zpx, abs, absx, absy, indx, indy (STA: no imm) |
//! | LDX/STX | imm, zp, zpy, abs, absy (LDX) / zp, zpy, abs (STX) |
//! | LDY/STY | imm, zp, zpx, abs, absx (LDY) / zp, zpx, abs (STY) |
//! | ADC/SBC/CMP/AND/ORA/EOR | imm, zp, zpx, abs, absx, absy, indx, indy |
//! | CPX/CPY | imm, zp, abs |
//! | BIT | zp, abs |
//! | ASL/LSR/ROL/ROR | acc, zp, zpx, abs, absx |
//! | INC/DEC | zp, zpx, abs, absx |
//! | INX/INY/DEX/DEY/TAX/TAY/TXA/TYA/TSX/TXS | implied |
//! | BCC/BCS/BEQ/BMI/BNE/BPL/BVC/BVS | relative (prevailing + page-cross cost) |
//! | JMP | absolute, indirect (with the NMOS page-wrap bug) |
//! | JSR/RTS/RTI/BRK | — (trap-table interception points, see below) |
//! | PHP/PLP/PHA/PLA | implied |
//! | CLC/SEC/CLI/SEI/CLV/CLD/SED/NOP | implied |
//!
//! ## Deliberately omitted: illegal ("undocumented") opcodes
//!
//! The game was assembled from documented 6502 only, so none of these are
//! implemented; encountering one returns [`ExecError::IllegalOpcode`]:
//! `LAX` (`$A3/$A7/$AF/$B3/$B7/$BF`), `SAX` (`$83/$87/$8F/$97`),
//! `DCP` (`$C3/$C7/$CF/$D3/$D7/$DB/$DF`), `ISC` (`$E3/$E7/$EF/$F3/$F7/$FB/$FF`),
//! `SLO` (`$03/$07/$0F/$13/$17/$1B/$1F`), `RLA` (`$23/$27/$2F/$33/$37/$3B/$3F`),
//! `SRE` (`$43/$47/$4F/$53/$57/$5B/$5F`), `RRA` (`$63/$67/$6F/$73/$77/$7B/$7F`),
//! `SKB`/`NOP-imm` (`$80/$82/$89/$C2/$E2`), `NOP-zpx`/`absx` variants
//! (`$04/$14/$1A/$1C/$34/$3A/$3C/$44/$54/$5A/$5C/$64/$74/$7A/$7C/$D4/$DA/$DC/$F4/$FA/$FC`),
//! `JAM` (`$02/$12/$22/$32/$42/$52/$62/$72/$92/$B2/$D2/$F2`),
//! `ANC` (`$0B/$2B`), `ALR` (`$4B`), `ARR` (`$6B`), `XAA` (`$8B`),
//! `AHX` (`$93/$9F`), `SHX` (`$9E`), `SHY` (`$9C`), `TAS` (`$9B`), `LAS` (`$BB`).
//!
//! ## No decimal mode
//!
//! The 2A03 drops BCD arithmetic: `ADC`/`SBC` always behave in binary, even
//! with the `D` flag set. `CLD`/`SED` still set/clear `D` (some init code
//! executes them) but it has no arithmetic effect.
//!
//! # Trap interception
//!
//! [`step_instruction`] checks the trap table before transferring control:
//! `JSR abs`, `JMP abs/ind` and the `NMI`/`RESET`/`BRK` vectors whose target
//! is trapped call the Rust function and emulate the matching return
//! (`RTS` for `JSR`/`JMP`, `RTI` for vectors), so the 6502 stack stays
//! balanced. See [`crate::traps`] for the table.
//!
//! A body that ends in a tail `JMP` (the `$D382`/`$D385` trampolines'
//! `JMP ($0E)`, a `JMP LDE40` display tail, a branch out of a partially
//! ported routine) asks for a PC redirect instead
//! ([`Game::trap_jump`](crate::game::Game::trap_jump)). When the body
//! returns, [`Game::fire_trap`](crate::game::Game::fire_trap) chains into
//! the target's own trap if it has one and otherwise reports
//! [`TrapExit::Jump`]; the dispatch site then continues interpreting there
//! with the stack untouched — exactly what the original `JMP` leaves (the
//! caller's frame stays underneath for the target's `RTS`; nothing is
//! pushed, so dead stack bytes match the ROM). A plain return reports
//! [`TrapExit::Return`] and the site emulates the `RTS`/`RTI`.
//!
//! # Timing
//!
//! Cycle counts follow the NMOS datasheet (base + page-cross + taken-branch
//! penalties); the PPU/APU stubs do not consume extra cycles except OAM DMA
//! (+513/+514). Interrupts are serviced at instruction boundaries. This is
//! exact enough for lockstep replay; sub-instruction APU/PPU timing belongs
//! to the PPU and APU models.
//!
//! ## Trap cycle accounting (time-collapse fix)
//!
//! Trapped routines are state-exact but were cycle-free: e.g.
//! `Reset_Memory_Ranges`/`Erase` (~15k ASM cycles) executed in ~10 host
//! cycles, pulling frame-4's bank-5 `$00`-zeroing into frame 3. Each trap
//! now carries [`crate::traps::TrapInfo::cycles`] (body + final `RTS`/`RTI`,
//! caller entry excluded) and [`crate::game::Game::fire_trap`] adds it to
//! `cpu.cycles` on every fire. Dispatch entry costs stay here: `JSR` 6,
//! `JMP abs` 3, `JMP ind` 5, `BRK`/vector service 7. The old extra `+6` for
//! the emulated `RTS` on the `JMP` paths is removed — the trap cost already
//! contains the return — so `JMP`-entered and `JSR`-entered traps account
//! identically. `run_budget` (`game.rs`) and `Cpu::run_cycles` loop on
//! absolute `cpu.cycles`, so charged traps shrink the interpreter steps per
//! frame back to untrapped pacing and the absolute-cycle vblank cadence
//! (`FIRST_VBLANK_AT + k*VBLANK_PERIOD`) stays aligned.

use crate::game::Game;
use crate::traps::TrapExit;

// ---------------------------------------------------------------- flags

/// Carry.
pub const FLAG_C: u8 = 0x01;
/// Zero.
pub const FLAG_Z: u8 = 0x02;
/// Interrupt disable.
pub const FLAG_I: u8 = 0x04;
/// Decimal (stored; ignored by `ADC`/`SBC` on the 2A03).
pub const FLAG_D: u8 = 0x08;
/// Break (not a real flag; set in the byte pushed by `PHP`/`BRK`).
pub const FLAG_B: u8 = 0x10;
/// Unused; always reads back set.
pub const FLAG_U: u8 = 0x20;
/// Overflow.
pub const FLAG_V: u8 = 0x40;
/// Negative.
pub const FLAG_N: u8 = 0x80;

/// NMI vector (`$FFFA/B`).
pub const VEC_NMI: u16 = 0xFFFA;
/// RESET vector (`$FFFC/D`).
pub const VEC_RESET: u16 = 0xFFFC;
/// IRQ/`BRK` vector (`$FFFE/F`).
pub const VEC_IRQ: u16 = 0xFFFE;

/// Error from [`step_instruction`] / [`Cpu::run_cycles`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// Opcode byte is not a legal NMOS 6502 opcode (see module docs).
    IllegalOpcode {
        /// The offending opcode byte.
        opcode: u8,
        /// Address it was fetched from.
        at: u16,
    },
    /// Instruction budget exhausted (runaway code / missing `RTS`).
    MaxInstructions {
        /// Program counter where the budget ran out.
        at: u16,
    },
}

impl core::fmt::Display for ExecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            ExecError::IllegalOpcode { opcode, at } => {
                write!(f, "illegal 6502 opcode ${opcode:02X} at ${at:04X}")
            }
            ExecError::MaxInstructions { at } => {
                write!(f, "instruction budget exhausted at ${at:04X}")
            }
        }
    }
}

impl std::error::Error for ExecError {}

// ---------------------------------------------------------------- CPU

/// 2A03 register file + interrupt lines. Memory lives on [`Game`]; this
/// struct holds only registers, so it stays `Copy`-cheap for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cpu {
    /// Accumulator.
    pub a: u8,
    /// X index.
    pub x: u8,
    /// Y index.
    pub y: u8,
    /// Stack pointer (hardware stack is `ram[$0100+sp]`).
    pub sp: u8,
    /// Program counter.
    pub pc: u16,
    /// Status register (`NV-BDIZC`; `B`/`U` fixed on push/pull).
    pub p: u8,
    /// Executed cycles (datasheet counts + DMA).
    pub cycles: u64,
    /// NMI edge latch; serviced at the next instruction boundary.
    pub nmi_pending: bool,
    /// IRQ level latch; serviced when `I` is clear.
    pub irq_pending: bool,
    /// Tail jump requested by the running trap body ([`Game::trap_jump`]);
    /// consumed by [`Game::fire_trap`] once that body returns.
    pub(crate) trap_jump: Option<u16>,
}

impl Cpu {
    /// Power-on state (`SP=$FD`, `I` set, vectors not yet fetched).
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            p: FLAG_U | FLAG_I,
            cycles: 0,
            nmi_pending: false,
            irq_pending: false,
            trap_jump: None,
        }
    }

    /// Fetch the RESET vector and initialise (`SP=$FD`, `I` set).
    /// A trapped RESET vector runs the Rust handler, then execution
    /// continues at the vector (reset has no caller to return to) — or
    /// wherever the handler tail-jumped ([`Game::trap_jump`]).
    pub fn reset(game: &mut Game) {
        game.cpu.sp = 0xFD;
        game.cpu.p |= FLAG_I;
        // Keep D as-is (2A03: no decimal mode either way).
        let target = bus_read16(game, VEC_RESET);
        game.cpu.cycles += 7;
        game.cpu.pc = target;
        if game.traps.is_trapped(target) {
            if let TrapExit::Jump(pc) = game.fire_trap(target) {
                game.cpu.pc = pc;
            }
        }
    }

    /// Latch an NMI edge (called by the PPU façade / [`Game::step`]).
    pub fn request_nmi(&mut self) {
        self.nmi_pending = true;
    }

    /// Run until `cycles` advances by `budget` (NMI/IRQ serviced at
    /// instruction boundaries, including one pending service first).
    ///
    /// Split-aware: if the next `JSR`/`JMP` targets a trapped
    /// routine whose atomic cost would cross the budget end, that one call
    /// interprets instead (temporarily untrapped), so long routines split
    /// across the boundary like hardware/oracle instead of overshooting a
    /// whole frame (e.g. `$D281` 15k crossing a 29k frame end).
    pub fn run_cycles(game: &mut Game, budget: u64) -> Result<(), ExecError> {
        let end = game.cpu.cycles + budget;
        loop {
            Self::service_pending(game);
            if game.cpu.cycles >= end {
                return Ok(());
            }
            if let Some(target) = split_trap_target(game, end) {
                game.traps.set_untrapped(target, true);
                let r = step_instruction(game);
                game.traps.set_untrapped(target, false);
                r?;
            } else {
                step_instruction(game)?;
            }
        }
    }

    /// Service latched NMI/IRQ edges (NMI wins; IRQ needs `I` clear).
    pub(crate) fn service_pending(game: &mut Game) {
        if game.cpu.nmi_pending {
            game.cpu.nmi_pending = false;
            service_nmi(game);
        } else if game.cpu.irq_pending && game.cpu.p & FLAG_I == 0 {
            game.cpu.irq_pending = false;
            service_irq(game);
        }
    }
}

/// If the next instruction is a `JSR`/`JMP` to a trapped routine whose
/// atomic cost (entry + [`TrapInfo::cycles`](crate::traps::TrapInfo))
/// would cross the budget `end`, return its target so the budget loop can
/// interpret that one call instead (split across the boundary).
///
/// Peeks `PC`/`PC+1`/`PC+2` (always PRG for code; `JMP (ind)` pointer reads
/// may hit RAM but are side-effect-free) and consults the trap table; no
/// state is mutated. `None` means step normally (untrapped or fits).
pub(crate) fn split_trap_target(game: &mut Game, end: u64) -> Option<u16> {
    let pc = game.cpu.pc;
    let op = bus_read(game, pc);
    let (target, entry) = match op {
        0x20 | 0x4C => {
            let lo = bus_read(game, pc.wrapping_add(1)) as u16;
            let hi = bus_read(game, pc.wrapping_add(2)) as u16;
            (lo | (hi << 8), if op == 0x20 { 6 } else { 3 })
        }
        0x6C => {
            let lo = bus_read(game, pc.wrapping_add(1)) as u16;
            let hi = bus_read(game, pc.wrapping_add(2)) as u16;
            let ptr = lo | (hi << 8);
            let tlo = bus_read(game, ptr) as u16;
            let thi = bus_read(game, (ptr & 0xFF00) | ((ptr + 1) & 0x00FF)) as u16;
            (tlo | (thi << 8), 5)
        }
        _ => return None,
    };
    if !game.traps.is_trapped(target) {
        return None;
    }
    let cost = game.traps.get(target).map(|i| i.cycles).unwrap_or(0);
    if game.cpu.cycles + entry as u64 + cost > end {
        Some(target)
    } else {
        None
    }
}

// ---------------------------------------------------------------- MMC1

/// MMC1 mapper model (Zelda II SLROM): 5-bit serial shift register plus the
/// four bank registers. Zelda II uses 128 KiB PRG (8x16 KiB) + 128 KiB CHR.
///
/// Serial protocol (mirrors hardware): each write to `$8000-$FFFF` with bit
/// 7 set resets the shift register and forces PRG mode 3 (`ctrl |= $0C`);
/// otherwise bit 0 shifts in LSB-first and every 5th write commits to the
/// register selected by bits 14-13 of the address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mmc1 {
    /// In-progress serial bits (LSB-first).
    pub shift: u8,
    /// Bits accumulated (0-4; commit on 5).
    pub count: u8,
    /// `$8000-$9FFF`: mirroring(1-0), PRG mode(3-2), CHR mode(4).
    pub ctrl: u8,
    /// `$A000-$BFFF`: CHR bank 0 (4 KiB mode) / CHR bank (8 KiB mode).
    pub chr0: u8,
    /// `$C000-$DFFF`: CHR bank 1 (4 KiB mode only).
    pub chr1: u8,
    /// `$E000-$FFFF`: PRG bank select.
    pub prg: u8,
}

impl Default for Mmc1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mmc1 {
    /// Power-on state (`ctrl=$0C`: PRG mode 3, fix-last).
    pub fn new() -> Self {
        Self {
            shift: 0,
            count: 0,
            ctrl: 0x0C,
            chr0: 0,
            chr1: 0,
            prg: 0,
        }
    }

    /// PRG banking mode: `(ctrl >> 2) & 3`.
    /// 0/1 = 32 KiB switch at `$8000`; 2 = fix first, switch `$C000`;
    /// 3 = switch `$8000`, fix last at `$C000`.
    pub fn prg_mode(&self) -> u8 {
        (self.ctrl >> 2) & 3
    }

    /// CHR banking mode: bit 4 (0 = 8 KiB, 1 = two 4 KiB banks).
    pub fn chr_mode(&self) -> u8 {
        (self.ctrl >> 4) & 1
    }

    /// Mirroring select: bits 1-0 (0 = 1-screen A, 1 = 1-screen B,
    /// 2 = vertical, 3 = horizontal).
    pub fn mirroring(&self) -> u8 {
        self.ctrl & 3
    }

    /// Mapper write: serial load or shift-register reset on bit 7.
    pub fn write(&mut self, addr: u16, val: u8) {
        if val & 0x80 != 0 {
            self.shift = 0;
            self.count = 0;
            self.ctrl |= 0x0C;
            return;
        }
        self.shift = (self.shift >> 1) | ((val & 1) << 4);
        self.count += 1;
        if self.count == 5 {
            let reg = self.shift & 0x1F;
            match addr >> 13 {
                4 => self.ctrl = reg,
                5 => self.chr0 = reg,
                6 => self.chr1 = reg,
                _ => self.prg = reg,
            }
            self.shift = 0;
            self.count = 0;
        }
    }

    /// Translate a CPU `$8000-$FFFF` address to a byte index into the raw
    /// PRG image (`prg_len` must be a nonzero multiple of 16 KiB).
    pub fn map_prg(&self, addr: u16, prg_len: usize) -> usize {
        debug_assert!(prg_len >= 0x4000 && prg_len.is_multiple_of(0x4000));
        let banks16 = (prg_len / 0x4000) as u16;
        let bank_of = |b: u16| (b % banks16) as usize;
        let off = (addr - 0x8000) as usize;
        match self.prg_mode() {
            0 | 1 => {
                // 32 KiB switch; low bit ignored.
                let base = bank_of(self.prg as u16 & !1) * 0x4000;
                (base + off) % prg_len
            }
            2 => {
                // Fix first bank at $8000, switch $C000.
                if addr < 0xC000 {
                    off % prg_len
                } else {
                    bank_of(self.prg as u16) * 0x4000 + (off - 0x4000)
                }
            }
            _ => {
                // Fix last bank at $C000, switch $8000.
                if addr < 0xC000 {
                    bank_of(self.prg as u16) * 0x4000 + off
                } else {
                    (banks16 as usize - 1) * 0x4000 + (off - 0x4000)
                }
            }
        }
    }

    /// Translate a 4 KiB CHR page (`page` 0/1) to a bank index for the PPU.
    /// Returns the 4 KiB bank number within the CHR image.
    pub fn map_chr4(&self, page: u8) -> usize {
        if self.chr_mode() == 0 {
            // 8 KiB mode: chr0 selects two consecutive 4 KiB banks.
            ((self.chr0 & !1) as usize) + (page & 1) as usize
        } else if page == 0 {
            self.chr0 as usize
        } else {
            self.chr1 as usize
        }
    }
}

// ------------------------------------------------- PPU/APU façades

/// Minimal PPU register façade for `z2-ppu`.
///
/// Covers only what the CPU bus needs: register reads/writes at
/// `$2000-$2007` (called mirrored). `z2-ppu` implements this trait on the
/// real PPU model (scroll/vram/sprites/NMI edge) and swaps it in for
/// [`NullPpu`]; the interpreter code does not change.
pub trait PpuBus {
    /// Read a PPU register (`$2000-$2007`, already de-mirrored).
    fn ppu_read(&mut self, addr: u16) -> u8;
    /// Write a PPU register (`$2000-$2007`, already de-mirrored).
    fn ppu_write(&mut self, addr: u16, val: u8);
}

/// Minimal APU register façade for `z2-apu`.
///
/// Covers `$4000-$4013`, `$4015` and `$4017` *writes* (`$4017` reads return
/// controller 2 — see [`Game::poll_pad2`]). `z2-apu` implements this on the
/// real synth and swaps it in for [`NullApu`]; controller ports (`$4016`)
/// and OAM DMA (`$4014`) stay owned by [`Game`] (input + memory move).
pub trait ApuBus {
    /// Read an APU register.
    fn apu_read(&mut self, addr: u16) -> u8;
    /// Write an APU register.
    fn apu_write(&mut self, addr: u16, val: u8);
}

/// Null PPU: ignores writes, reads `0` — except `$2002` (PPUSTATUS), which
/// reports bit 7 (vblank) stuck set. That is a deliberate boot hack: real
/// init/main-loop code spins on vblank (`LDA $2002 : BPL wait`), and with a
/// zeroed stub the game would wait forever. Bit 6 (sprite-0 hit) stays
/// clear, so sprite-0 waits still hang — those need the real PPU.
/// Records traffic counters so tests (and later bring-up) can prove the bus
/// routes correctly. NMI edges are driven by [`Game::step`], not by this stub.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullPpu {
    /// Number of register reads served.
    pub reads: u64,
    /// Number of register writes sunk.
    pub writes: u64,
    /// Last write `(register, value)`.
    pub last_write: Option<(u16, u8)>,
}

impl PpuBus for NullPpu {
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.reads += 1;
        if addr == 0x2002 {
            0x80 // vblank stuck set (boot hack, see above)
        } else {
            0
        }
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        self.writes += 1;
        self.last_write = Some((addr, val));
    }
}

/// Null APU: sinks writes, reads `0`, records traffic like [`NullPpu`].
///
/// The per-frame write log is the bridge to the real synth: the
/// interpreter-run sound engine writes here; frontends drain it into
/// `z2-apu` after each frame. `Copy` was dropped for the log `Vec`; field
/// reads (`reads`/`writes`/`last_write`) behave exactly as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NullApu {
    /// Number of register reads served.
    pub reads: u64,
    /// Number of register writes sunk.
    pub writes: u64,
    /// Last write `(register, value)`.
    pub last_write: Option<(u16, u8)>,
    /// Every write this frame, in order. Drained by
    /// [`NullApu::drain_log`] (frontends call it once per stepped frame).
    pub log: Vec<(u16, u8)>,
}

impl NullApu {
    /// Take the frame's write log, leaving it empty.
    pub fn drain_log(&mut self) -> Vec<(u16, u8)> {
        std::mem::take(&mut self.log)
    }
}

impl ApuBus for NullApu {
    fn apu_read(&mut self, _addr: u16) -> u8 {
        self.reads += 1;
        0
    }

    fn apu_write(&mut self, addr: u16, val: u8) {
        self.writes += 1;
        self.last_write = Some((addr, val));
        self.log.push((addr, val));
    }
}

// ---------------------------------------------------------------- bus

/// Read one byte from the CPU address space (PPU/APU reads may have side
/// effects, hence `&mut`).
pub(crate) fn bus_read(game: &mut Game, addr: u16) -> u8 {
    match addr {
        0x0000..=0x1FFF => game.ram[(addr & 0x07FF) as usize],
        0x2000..=0x3FFF => {
            // Beam sync: the register access happens on the instruction's
            // last cycle (~3 after the fetch we are attributed to).
            let reg = 0x2000 + (addr & 7);
            let kind = crate::ppu_bind::PpuBind::access_kind(reg, false);
            game.ppu
                .sync_access(game.cpu.cycles + BUS_ACCESS_OFFSET, kind);
            game.ppu.ppu_read(reg)
        }
        0x4000..=0x4013 | 0x4015 => game.apu.apu_read(addr),
        0x4014 => 0, // OAM DMA is write-only.
        0x4016 => game.poll_pad1(),
        // $4017 read = controller 2 (the APU frame counter is write-only).
        0x4017 => game.poll_pad2(),
        0x4018..=0x401F => 0, // APU test-mode / open bus stub.
        0x4020..=0x5FFF => 0, // Expansion / open bus stub.
        0x6000..=0x7FFF => game.wram[(addr - 0x6000) as usize],
        _ => {
            // $8000-$FFFF: PRG ROM through MMC1.
            let idx = game.mmc1.map_prg(addr, game.prg.len());
            game.prg[idx]
        }
    }
}

/// Write one byte to the CPU address space.
pub(crate) fn bus_write(game: &mut Game, addr: u16, val: u8) {
    match addr {
        0x0000..=0x1FFF => game.ram[(addr & 0x07FF) as usize] = val,
        0x2000..=0x3FFF => {
            let reg = 0x2000 + (addr & 7);
            // NMI retrigger (hardware): a 0->1 transition on `$2000` bit 7
            // while vblank is set fires the edge immediately — even
            // mid-frame, before the next vblank hook. Snapshot the enable
            // bit first: re-writing 1->1 (every NMI prologue's `STA $2000`
            // does) must NOT retrigger. Serviced at the next instruction
            // boundary by the budget loops' `service_pending`.
            let armed_before = reg == 0x2000 && game.ppu.model().nmi_enabled();
            let kind = crate::ppu_bind::PpuBind::access_kind(reg, true);
            game.ppu
                .sync_access(game.cpu.cycles + BUS_ACCESS_OFFSET, kind);
            game.ppu.ppu_write(reg, val);
            if reg == 0x2000 && val & 0x80 != 0 && !armed_before && game.ppu.vblank() {
                game.cpu.nmi_pending = true;
            }
        }
        0x4000..=0x4013 | 0x4015 | 0x4017 => game.apu.apu_write(addr, val),
        0x4014 => {
            // OAM DMA: copy one page into OAM and forward it into the PPU
            // model (PpuBind::oam_dma); the OAM address latch is untouched.
            let base = (val as u16) << 8;
            for i in 0..256u16 {
                game.oam[i as usize] = bus_read(game, base | i);
            }
            let page = game.oam;
            game.ppu.sync_access(
                game.cpu.cycles + BUS_ACCESS_OFFSET,
                z2_ppu::AccessKind::Line,
            );
            game.ppu.oam_dma(&page);
            game.cpu.cycles += if game.cpu.cycles & 1 == 1 { 514 } else { 513 };
        }
        0x4016 => game.set_strobe(val),
        0x4018..=0x401F => {} // APU test-mode stub: ignore.
        0x4020..=0x5FFF => {} // Expansion stub: ignore.
        0x6000..=0x7FFF => game.wram[(addr - 0x6000) as usize] = val,
        _ => {
            game.mmc1.write(addr, val);
            // CHR/mirroring changes take effect from the beam's current
            // scanline (cheap no-op when the banks did not change).
            let cycles = game.cpu.cycles + BUS_ACCESS_OFFSET;
            game.ppu.mapper_sync(&game.chr, &game.mmc1, cycles);
        }
    }
}

/// CPU cycles between an instruction's attributed start (`cpu.cycles` when
/// the bus access runs) and the cycle the access actually happens on: the
/// common `LDA/STA/BIT abs` forms touch the bus on their 4th cycle.
const BUS_ACCESS_OFFSET: u64 = 3;

/// Little-endian 16-bit read.
pub(crate) fn bus_read16(game: &mut Game, addr: u16) -> u16 {
    let lo = bus_read(game, addr) as u16;
    let hi = bus_read(game, addr.wrapping_add(1)) as u16;
    lo | (hi << 8)
}

/// Push one byte on the hardware stack (`ram[$0100+sp]`).
fn push(game: &mut Game, val: u8) {
    game.ram[0x0100 + game.cpu.sp as usize] = val;
    game.cpu.sp = game.cpu.sp.wrapping_sub(1);
}

/// Pop one byte from the hardware stack.
fn pop(game: &mut Game) -> u8 {
    game.cpu.sp = game.cpu.sp.wrapping_add(1);
    game.ram[0x0100 + game.cpu.sp as usize]
}

fn push16(game: &mut Game, val: u16) {
    push(game, (val >> 8) as u8);
    push(game, val as u8);
}

fn pop16(game: &mut Game) -> u16 {
    let lo = pop(game) as u16;
    let hi = pop(game) as u16;
    lo | (hi << 8)
}

/// Leave a fired `JSR`/`JMP` trap: emulate its `RTS` (pop a return address
/// and jump past it, so the 6502 stack stays balanced) or, when the body
/// tail-jumped into unported code ([`TrapExit::Jump`]), continue there with
/// the stack untouched — exactly what the original `JMP` leaves.
fn leave_trap(game: &mut Game, exit: TrapExit) {
    game.cpu.pc = match exit {
        TrapExit::Return => pop16(game).wrapping_add(1),
        TrapExit::Jump(pc) => pc,
    };
}

/// Leave a fired vector trap: emulate its `RTI` (restore status + PC) or
/// continue at the tail-jump target with the interrupt frame still pushed
/// (the target's own `RTI` pops it).
fn leave_vector_trap(game: &mut Game, exit: TrapExit) {
    match exit {
        TrapExit::Return => {
            let p = pop(game);
            game.cpu.p = (p & !FLAG_B) | FLAG_U;
            game.cpu.pc = pop16(game);
        }
        TrapExit::Jump(pc) => game.cpu.pc = pc,
    }
}

// ------------------------------------------------- vectors/traps

/// Fetch a vector through the trap table: `Some(exit)` when a Rust handler
/// ran (the caller leaves it via [`leave_vector_trap`]), `None` to jump to
/// the vector's ROM bytes.
fn vector_trap(game: &mut Game, vec: u16) -> Option<TrapExit> {
    let target = bus_read16(game, vec);
    if game.traps.is_trapped(target) {
        Some(game.fire_trap(target))
    } else {
        None
    }
}

fn service_nmi(game: &mut Game) {
    // Hardware pushes first, then jumps — preserve that order so a trapped
    // vector sees the same stack a real NMI would leave behind.
    let pc = game.cpu.pc;
    let p = game.cpu.p;
    push16(game, pc);
    push(game, (p & !FLAG_B) | FLAG_U);
    game.cpu.p |= FLAG_I;
    game.cpu.cycles += 7;
    match vector_trap(game, VEC_NMI) {
        Some(exit) => leave_vector_trap(game, exit),
        None => game.cpu.pc = bus_read16(game, VEC_NMI),
    }
}

fn service_irq(game: &mut Game) {
    let pc = game.cpu.pc;
    let p = game.cpu.p;
    push16(game, pc);
    push(game, (p & !FLAG_B) | FLAG_U);
    game.cpu.p |= FLAG_I;
    game.cpu.cycles += 7;
    match vector_trap(game, VEC_IRQ) {
        Some(exit) => leave_vector_trap(game, exit),
        None => game.cpu.pc = bus_read16(game, VEC_IRQ),
    }
}

// ------------------------------------------------- ALU helpers

fn set_nz(game: &mut Game, v: u8) {
    game.cpu.p = (game.cpu.p & !(FLAG_N | FLAG_Z)) | if v == 0 { FLAG_Z } else { 0 } | (v & FLAG_N);
}

/// Binary `ADC` (the 2A03 has no decimal mode: `D` is ignored).
fn alu_adc(game: &mut Game, rhs: u8) {
    let a = game.cpu.a;
    let c = (game.cpu.p & FLAG_C) as u16;
    let sum = a as u16 + rhs as u16 + c;
    let res = sum as u8;
    game.cpu.p = (game.cpu.p & !(FLAG_C | FLAG_Z | FLAG_V | FLAG_N))
        | if sum > 0xFF { FLAG_C } else { 0 }
        | if res == 0 { FLAG_Z } else { 0 }
        | if ((a ^ res) & (rhs ^ res) & 0x80) != 0 {
            FLAG_V
        } else {
            0
        }
        | (res & FLAG_N);
    game.cpu.a = res;
}

/// Binary `SBC` (`A - rhs - (1-C)`; `D` ignored like `ADC`).
fn alu_sbc(game: &mut Game, rhs: u8) {
    alu_adc(game, !rhs);
}

fn alu_cmp(game: &mut Game, reg: u8, rhs: u8) {
    let res = reg.wrapping_sub(rhs);
    game.cpu.p = (game.cpu.p & !(FLAG_C | FLAG_Z | FLAG_N))
        | if reg >= rhs { FLAG_C } else { 0 }
        | if res == 0 { FLAG_Z } else { 0 }
        | (res & FLAG_N);
}

fn alu_bit(game: &mut Game, rhs: u8) {
    game.cpu.p = (game.cpu.p & !(FLAG_Z | FLAG_V | FLAG_N))
        | if game.cpu.a & rhs == 0 { FLAG_Z } else { 0 }
        | (rhs & (FLAG_V | FLAG_N));
}

// ------------------------------------------------- effective addresses

/// Zero page.
fn ea_zp(game: &mut Game) -> u16 {
    let z = bus_read(game, game.cpu.pc) as u16;
    game.cpu.pc = game.cpu.pc.wrapping_add(1);
    z
}

/// Zero page + X (wraps within the page).
fn ea_zpx(game: &mut Game) -> u16 {
    ea_zp(game).wrapping_add(game.cpu.x as u16) & 0xFF
}

/// Zero page + Y (wraps within the page; only `LDX`/`STX` use it).
fn ea_zpy(game: &mut Game) -> u16 {
    ea_zp(game).wrapping_add(game.cpu.y as u16) & 0xFF
}

/// Absolute; returns `(address, _)`.
fn ea_abs(game: &mut Game) -> (u16, bool) {
    let lo = bus_read(game, game.cpu.pc) as u16;
    let hi = bus_read(game, game.cpu.pc.wrapping_add(1)) as u16;
    game.cpu.pc = game.cpu.pc.wrapping_add(2);
    (lo | (hi << 8), false)
}

/// Absolute + X; reports a page cross (extra cycle on reads).
fn ea_absx(game: &mut Game) -> (u16, bool) {
    let (base, _) = ea_abs(game);
    let ea = base.wrapping_add(game.cpu.x as u16);
    (ea, (base & 0xFF00) != (ea & 0xFF00))
}

/// Absolute + Y; reports a page cross.
fn ea_absy(game: &mut Game) -> (u16, bool) {
    let (base, _) = ea_abs(game);
    let ea = base.wrapping_add(game.cpu.y as u16);
    (ea, (base & 0xFF00) != (ea & 0xFF00))
}

/// Indexed indirect `(zp,X)`.
fn ea_indx(game: &mut Game) -> u16 {
    let z = bus_read(game, game.cpu.pc).wrapping_add(game.cpu.x) as u16;
    game.cpu.pc = game.cpu.pc.wrapping_add(1);
    let lo = game.ram[z as usize & 0xFF] as u16;
    let hi = game.ram[(z as usize + 1) & 0xFF] as u16;
    lo | (hi << 8)
}

/// Indirect indexed `(zp),Y`; reports a page cross.
fn ea_indy(game: &mut Game) -> (u16, bool) {
    let z = bus_read(game, game.cpu.pc) as usize;
    game.cpu.pc = game.cpu.pc.wrapping_add(1);
    let base = game.ram[z] as u16 | ((game.ram[(z + 1) & 0xFF]) as u16) << 8;
    let ea = base.wrapping_add(game.cpu.y as u16);
    (ea, (base & 0xFF00) != (ea & 0xFF00))
}

/// Immediate operand byte.
fn imm(game: &mut Game) -> u8 {
    let v = bus_read(game, game.cpu.pc);
    game.cpu.pc = game.cpu.pc.wrapping_add(1);
    v
}

// ------------------------------------------------- shifts/rotates

fn op_asl(game: &mut Game, v: u8) -> u8 {
    game.cpu.p = (game.cpu.p & !FLAG_C) | if v & 0x80 != 0 { FLAG_C } else { 0 };
    let r = v << 1;
    set_nz(game, r);
    r
}

fn op_lsr(game: &mut Game, v: u8) -> u8 {
    game.cpu.p = (game.cpu.p & !FLAG_C) | if v & 1 != 0 { FLAG_C } else { 0 };
    let r = v >> 1;
    set_nz(game, r);
    r
}

fn op_rol(game: &mut Game, v: u8) -> u8 {
    let c = game.cpu.p & FLAG_C;
    game.cpu.p = (game.cpu.p & !FLAG_C) | if v & 0x80 != 0 { FLAG_C } else { 0 };
    let r = (v << 1) | c;
    set_nz(game, r);
    r
}

fn op_ror(game: &mut Game, v: u8) -> u8 {
    let c = if game.cpu.p & FLAG_C != 0 { 0x80 } else { 0 };
    game.cpu.p = (game.cpu.p & !FLAG_C) | if v & 1 != 0 { FLAG_C } else { 0 };
    let r = (v >> 1) | c;
    set_nz(game, r);
    r
}

// ------------------------------------------------- branches

/// Relative branch helper; returns extra cycles (1 if taken + 1 on page cross).
fn branch(game: &mut Game, cond: bool) -> u64 {
    let disp = bus_read(game, game.cpu.pc) as i8;
    game.cpu.pc = game.cpu.pc.wrapping_add(1);
    if !cond {
        return 0;
    }
    let from = game.cpu.pc;
    let to = from.wrapping_add(disp as i16 as u16);
    game.cpu.pc = to;
    1 + u64::from((from & 0xFF00) != (to & 0xFF00))
}

// ------------------------------------------------- single step

/// Execute the single instruction at `cpu.pc` (after servicing nothing —
/// callers service pending interrupts at frame/instruction boundaries).
///
/// `JSR`/`JMP`/vectors to trapped addresses run the Rust handler and
/// emulate the matching return; see the module docs.
pub fn step_instruction(game: &mut Game) -> Result<(), ExecError> {
    let at = game.cpu.pc;
    let op = bus_read(game, at);
    game.cpu.pc = at.wrapping_add(1);
    // Base cost first; page-cross / taken-branch penalties added below.
    let mut cost: u64 = 2;
    match op {
        // ---- LDA
        0xA9 => {
            game.cpu.a = imm(game);
            set_nz(game, game.cpu.a);
        }
        0xA5 => {
            let ea = ea_zp(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 3;
        }
        0xB5 => {
            let ea = ea_zpx(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 4;
        }
        0xAD => {
            let (ea, _) = ea_abs(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 4;
        }
        0xBD => {
            let (ea, cross) = ea_absx(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 4 + u64::from(cross);
        }
        0xB9 => {
            let (ea, cross) = ea_absy(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 4 + u64::from(cross);
        }
        0xA1 => {
            let ea = ea_indx(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 6;
        }
        0xB1 => {
            let (ea, cross) = ea_indy(game);
            game.cpu.a = bus_read(game, ea);
            set_nz(game, game.cpu.a);
            cost = 5 + u64::from(cross);
        }
        // ---- STA
        0x85 => {
            let ea = ea_zp(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 3;
        }
        0x95 => {
            let ea = ea_zpx(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 4;
        }
        0x8D => {
            let (ea, _) = ea_abs(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 4;
        }
        0x9D => {
            let (ea, _) = ea_absx(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 5;
        }
        0x99 => {
            let (ea, _) = ea_absy(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 5;
        }
        0x81 => {
            let ea = ea_indx(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 6;
        }
        0x91 => {
            let (ea, _) = ea_indy(game);
            let a = game.cpu.a;
            bus_write(game, ea, a);
            cost = 6;
        }
        // ---- LDX
        0xA2 => {
            game.cpu.x = imm(game);
            set_nz(game, game.cpu.x);
        }
        0xA6 => {
            let ea = ea_zp(game);
            game.cpu.x = bus_read(game, ea);
            set_nz(game, game.cpu.x);
            cost = 3;
        }
        0xB6 => {
            let ea = ea_zpy(game);
            game.cpu.x = bus_read(game, ea);
            set_nz(game, game.cpu.x);
            cost = 4;
        }
        0xAE => {
            let (ea, _) = ea_abs(game);
            game.cpu.x = bus_read(game, ea);
            set_nz(game, game.cpu.x);
            cost = 4;
        }
        0xBE => {
            let (ea, cross) = ea_absy(game);
            game.cpu.x = bus_read(game, ea);
            set_nz(game, game.cpu.x);
            cost = 4 + u64::from(cross);
        }
        // ---- STX
        0x86 => {
            let ea = ea_zp(game);
            let x = game.cpu.x;
            bus_write(game, ea, x);
            cost = 3;
        }
        0x96 => {
            let ea = ea_zpy(game);
            let x = game.cpu.x;
            bus_write(game, ea, x);
            cost = 4;
        }
        0x8E => {
            let (ea, _) = ea_abs(game);
            let x = game.cpu.x;
            bus_write(game, ea, x);
            cost = 4;
        }
        // ---- LDY
        0xA0 => {
            game.cpu.y = imm(game);
            set_nz(game, game.cpu.y);
        }
        0xA4 => {
            let ea = ea_zp(game);
            game.cpu.y = bus_read(game, ea);
            set_nz(game, game.cpu.y);
            cost = 3;
        }
        0xB4 => {
            let ea = ea_zpx(game);
            game.cpu.y = bus_read(game, ea);
            set_nz(game, game.cpu.y);
            cost = 4;
        }
        0xAC => {
            let (ea, _) = ea_abs(game);
            game.cpu.y = bus_read(game, ea);
            set_nz(game, game.cpu.y);
            cost = 4;
        }
        0xBC => {
            let (ea, cross) = ea_absx(game);
            game.cpu.y = bus_read(game, ea);
            set_nz(game, game.cpu.y);
            cost = 4 + u64::from(cross);
        }
        // ---- STY
        0x84 => {
            let ea = ea_zp(game);
            let y = game.cpu.y;
            bus_write(game, ea, y);
            cost = 3;
        }
        0x94 => {
            let ea = ea_zpx(game);
            let y = game.cpu.y;
            bus_write(game, ea, y);
            cost = 4;
        }
        0x8C => {
            let (ea, _) = ea_abs(game);
            let y = game.cpu.y;
            bus_write(game, ea, y);
            cost = 4;
        }
        // ---- transfers / inc-dec registers (implied)
        0xAA => {
            game.cpu.x = game.cpu.a;
            set_nz(game, game.cpu.x);
        }
        0xA8 => {
            game.cpu.y = game.cpu.a;
            set_nz(game, game.cpu.y);
        }
        0x8A => {
            game.cpu.a = game.cpu.x;
            set_nz(game, game.cpu.a);
        }
        0x98 => {
            game.cpu.a = game.cpu.y;
            set_nz(game, game.cpu.a);
        }
        0xBA => {
            game.cpu.x = game.cpu.sp;
            set_nz(game, game.cpu.x);
        }
        0x9A => {
            game.cpu.sp = game.cpu.x;
        }
        0xCA => {
            game.cpu.x = game.cpu.x.wrapping_sub(1);
            let x = game.cpu.x;
            set_nz(game, x);
            cost = 2;
        }
        0x88 => {
            game.cpu.y = game.cpu.y.wrapping_sub(1);
            let y = game.cpu.y;
            set_nz(game, y);
            cost = 2;
        }
        0xE8 => {
            game.cpu.x = game.cpu.x.wrapping_add(1);
            let x = game.cpu.x;
            set_nz(game, x);
            cost = 2;
        }
        0xC8 => {
            game.cpu.y = game.cpu.y.wrapping_add(1);
            let y = game.cpu.y;
            set_nz(game, y);
            cost = 2;
        }
        // ---- ADC
        0x69 => {
            let v = imm(game);
            alu_adc(game, v);
        }
        0x65 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 3;
        }
        0x75 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 4;
        }
        0x6D => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 4;
        }
        0x7D => {
            let (ea, cross) = ea_absx(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 4 + u64::from(cross);
        }
        0x79 => {
            let (ea, cross) = ea_absy(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 4 + u64::from(cross);
        }
        0x61 => {
            let ea = ea_indx(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 6;
        }
        0x71 => {
            let (ea, cross) = ea_indy(game);
            let v = bus_read(game, ea);
            alu_adc(game, v);
            cost = 5 + u64::from(cross);
        }
        // ---- SBC
        0xE9 => {
            let v = imm(game);
            alu_sbc(game, v);
        }
        0xE5 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 3;
        }
        0xF5 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 4;
        }
        0xED => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 4;
        }
        0xFD => {
            let (ea, cross) = ea_absx(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 4 + u64::from(cross);
        }
        0xF9 => {
            let (ea, cross) = ea_absy(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 4 + u64::from(cross);
        }
        0xE1 => {
            let ea = ea_indx(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 6;
        }
        0xF1 => {
            let (ea, cross) = ea_indy(game);
            let v = bus_read(game, ea);
            alu_sbc(game, v);
            cost = 5 + u64::from(cross);
        }
        // ---- CMP
        0xC9 => {
            let v = imm(game);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
        }
        0xC5 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 3;
        }
        0xD5 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 4;
        }
        0xCD => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 4;
        }
        0xDD => {
            let (ea, cross) = ea_absx(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 4 + u64::from(cross);
        }
        0xD9 => {
            let (ea, cross) = ea_absy(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 4 + u64::from(cross);
        }
        0xC1 => {
            let ea = ea_indx(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 6;
        }
        0xD1 => {
            let (ea, cross) = ea_indy(game);
            let v = bus_read(game, ea);
            let a = game.cpu.a;
            alu_cmp(game, a, v);
            cost = 5 + u64::from(cross);
        }
        // ---- CPX / CPY
        0xE0 => {
            let v = imm(game);
            let x = game.cpu.x;
            alu_cmp(game, x, v);
        }
        0xE4 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let x = game.cpu.x;
            alu_cmp(game, x, v);
            cost = 3;
        }
        0xEC => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let x = game.cpu.x;
            alu_cmp(game, x, v);
            cost = 4;
        }
        0xC0 => {
            let v = imm(game);
            let y = game.cpu.y;
            alu_cmp(game, y, v);
        }
        0xC4 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let y = game.cpu.y;
            alu_cmp(game, y, v);
            cost = 3;
        }
        0xCC => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let y = game.cpu.y;
            alu_cmp(game, y, v);
            cost = 4;
        }
        // ---- AND
        0x29 => {
            let v = imm(game);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
        }
        0x25 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 3;
        }
        0x35 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        0x2D => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        0x3D => {
            let (ea, cross) = ea_absx(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4 + u64::from(cross);
        }
        0x39 => {
            let (ea, cross) = ea_absy(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4 + u64::from(cross);
        }
        0x21 => {
            let ea = ea_indx(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 6;
        }
        0x31 => {
            let (ea, cross) = ea_indy(game);
            let v = bus_read(game, ea);
            game.cpu.a &= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 5 + u64::from(cross);
        }
        // ---- ORA
        0x09 => {
            let v = imm(game);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
        }
        0x05 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 3;
        }
        0x15 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        0x0D => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        0x1D => {
            let (ea, cross) = ea_absx(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4 + u64::from(cross);
        }
        0x19 => {
            let (ea, cross) = ea_absy(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4 + u64::from(cross);
        }
        0x01 => {
            let ea = ea_indx(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 6;
        }
        0x11 => {
            let (ea, cross) = ea_indy(game);
            let v = bus_read(game, ea);
            game.cpu.a |= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 5 + u64::from(cross);
        }
        // ---- EOR
        0x49 => {
            let v = imm(game);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
        }
        0x45 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 3;
        }
        0x55 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        0x4D => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        0x5D => {
            let (ea, cross) = ea_absx(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4 + u64::from(cross);
        }
        0x59 => {
            let (ea, cross) = ea_absy(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4 + u64::from(cross);
        }
        0x41 => {
            let ea = ea_indx(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 6;
        }
        0x51 => {
            let (ea, cross) = ea_indy(game);
            let v = bus_read(game, ea);
            game.cpu.a ^= v;
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 5 + u64::from(cross);
        }
        // ---- BIT
        0x24 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            alu_bit(game, v);
            cost = 3;
        }
        0x2C => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            alu_bit(game, v);
            cost = 4;
        }
        // ---- ASL
        0x0A => {
            let a = game.cpu.a;
            game.cpu.a = op_asl(game, a);
        }
        0x06 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let r = op_asl(game, v);
            bus_write(game, ea, r);
            cost = 5;
        }
        0x16 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            let r = op_asl(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x0E => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let r = op_asl(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x1E => {
            let (ea, _) = ea_absx(game);
            let v = bus_read(game, ea);
            let r = op_asl(game, v);
            bus_write(game, ea, r);
            cost = 7;
        }
        // ---- LSR
        0x4A => {
            let a = game.cpu.a;
            game.cpu.a = op_lsr(game, a);
        }
        0x46 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let r = op_lsr(game, v);
            bus_write(game, ea, r);
            cost = 5;
        }
        0x56 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            let r = op_lsr(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x4E => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let r = op_lsr(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x5E => {
            let (ea, _) = ea_absx(game);
            let v = bus_read(game, ea);
            let r = op_lsr(game, v);
            bus_write(game, ea, r);
            cost = 7;
        }
        // ---- ROL
        0x2A => {
            let a = game.cpu.a;
            game.cpu.a = op_rol(game, a);
        }
        0x26 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let r = op_rol(game, v);
            bus_write(game, ea, r);
            cost = 5;
        }
        0x36 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            let r = op_rol(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x2E => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let r = op_rol(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x3E => {
            let (ea, _) = ea_absx(game);
            let v = bus_read(game, ea);
            let r = op_rol(game, v);
            bus_write(game, ea, r);
            cost = 7;
        }
        // ---- ROR
        0x6A => {
            let a = game.cpu.a;
            game.cpu.a = op_ror(game, a);
        }
        0x66 => {
            let ea = ea_zp(game);
            let v = bus_read(game, ea);
            let r = op_ror(game, v);
            bus_write(game, ea, r);
            cost = 5;
        }
        0x76 => {
            let ea = ea_zpx(game);
            let v = bus_read(game, ea);
            let r = op_ror(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x6E => {
            let (ea, _) = ea_abs(game);
            let v = bus_read(game, ea);
            let r = op_ror(game, v);
            bus_write(game, ea, r);
            cost = 6;
        }
        0x7E => {
            let (ea, _) = ea_absx(game);
            let v = bus_read(game, ea);
            let r = op_ror(game, v);
            bus_write(game, ea, r);
            cost = 7;
        }
        // ---- INC
        0xE6 => {
            let ea = ea_zp(game);
            let r = bus_read(game, ea).wrapping_add(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 5;
        }
        0xF6 => {
            let ea = ea_zpx(game);
            let r = bus_read(game, ea).wrapping_add(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 6;
        }
        0xEE => {
            let (ea, _) = ea_abs(game);
            let r = bus_read(game, ea).wrapping_add(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 6;
        }
        0xFE => {
            let (ea, _) = ea_absx(game);
            let r = bus_read(game, ea).wrapping_add(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 7;
        }
        // ---- DEC
        0xC6 => {
            let ea = ea_zp(game);
            let r = bus_read(game, ea).wrapping_sub(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 5;
        }
        0xD6 => {
            let ea = ea_zpx(game);
            let r = bus_read(game, ea).wrapping_sub(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 6;
        }
        0xCE => {
            let (ea, _) = ea_abs(game);
            let r = bus_read(game, ea).wrapping_sub(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 6;
        }
        0xDE => {
            let (ea, _) = ea_absx(game);
            let r = bus_read(game, ea).wrapping_sub(1);
            bus_write(game, ea, r);
            set_nz(game, r);
            cost = 7;
        }
        // ---- branches
        0x90 => {
            let c = game.cpu.p & FLAG_C == 0;
            cost = 2 + branch(game, c);
        }
        0xB0 => {
            let c = game.cpu.p & FLAG_C != 0;
            cost = 2 + branch(game, c);
        }
        0xF0 => {
            let c = game.cpu.p & FLAG_Z != 0;
            cost = 2 + branch(game, c);
        }
        0xD0 => {
            let c = game.cpu.p & FLAG_Z == 0;
            cost = 2 + branch(game, c);
        }
        0x30 => {
            let c = game.cpu.p & FLAG_N != 0;
            cost = 2 + branch(game, c);
        }
        0x10 => {
            let c = game.cpu.p & FLAG_N == 0;
            cost = 2 + branch(game, c);
        }
        0x50 => {
            let c = game.cpu.p & FLAG_V == 0;
            cost = 2 + branch(game, c);
        }
        0x70 => {
            let c = game.cpu.p & FLAG_V != 0;
            cost = 2 + branch(game, c);
        }
        // ---- jumps / calls
        0x4C => {
            let (target, _) = ea_abs(game);
            cost = 3;
            if game.traps.is_trapped(target) {
                // Trap cost (TrapInfo::cycles, body + RTS) lands in
                // fire_trap; no extra +6 — it already contains the return.
                let exit = game.fire_trap(target);
                leave_trap(game, exit);
            } else {
                game.cpu.pc = target;
            }
        }
        0x6C => {
            // NMOS indirect-JMP page-wrap bug: the high byte wraps within
            // the page when the vector sits at $xxFF.
            let (ptr, _) = ea_abs(game);
            let lo = bus_read(game, ptr) as u16;
            let hi = bus_read(game, (ptr & 0xFF00) | ((ptr + 1) & 0x00FF)) as u16;
            let target = lo | (hi << 8);
            cost = 5;
            if game.traps.is_trapped(target) {
                // Same convention as JMP abs: cost holds the return.
                let exit = game.fire_trap(target);
                leave_trap(game, exit);
            } else {
                game.cpu.pc = target;
            }
        }
        0x20 => {
            let (target, _) = ea_abs(game);
            cost = 6;
            // JSR pushes the address of its last byte (PC after the
            // operand fetch, minus one). Trap cost (body + RTS) lands in
            // fire_trap; the emulated RTS itself costs nothing extra.
            let ret = game.cpu.pc.wrapping_sub(1);
            if game.traps.is_trapped(target) {
                push16(game, ret);
                let exit = game.fire_trap(target);
                leave_trap(game, exit);
            } else {
                push16(game, ret);
                game.cpu.pc = target;
            }
        }
        0x60 => {
            game.cpu.pc = pop16(game).wrapping_add(1);
            cost = 6;
        }
        0x40 => {
            let p = pop(game);
            game.cpu.p = (p & !FLAG_B) | FLAG_U;
            game.cpu.pc = pop16(game);
            cost = 6;
        }
        0x00 => {
            // BRK: skip the padding byte, push PC + status (B set), jump.
            game.cpu.pc = game.cpu.pc.wrapping_add(1);
            let pc = game.cpu.pc;
            let p = game.cpu.p;
            push16(game, pc);
            push(game, p | FLAG_B | FLAG_U);
            game.cpu.p |= FLAG_I;
            cost = 7;
            match vector_trap(game, VEC_IRQ) {
                Some(exit) => leave_vector_trap(game, exit),
                None => game.cpu.pc = bus_read16(game, VEC_IRQ),
            }
        }
        // ---- stack
        0x08 => {
            let p = game.cpu.p;
            push(game, p | FLAG_B | FLAG_U);
            cost = 3;
        }
        0x28 => {
            let p = pop(game);
            game.cpu.p = (p & !FLAG_B) | FLAG_U;
            cost = 4;
        }
        0x48 => {
            let a = game.cpu.a;
            push(game, a);
            cost = 3;
        }
        0x68 => {
            game.cpu.a = pop(game);
            let a = game.cpu.a;
            set_nz(game, a);
            cost = 4;
        }
        // ---- flags / NOP
        0x18 => game.cpu.p &= !FLAG_C,
        0x38 => game.cpu.p |= FLAG_C,
        0x58 => game.cpu.p &= !FLAG_I,
        0x78 => game.cpu.p |= FLAG_I,
        0xB8 => game.cpu.p &= !FLAG_V,
        0xD8 => game.cpu.p &= !FLAG_D,
        0xF8 => game.cpu.p |= FLAG_D,
        0xEA => {}
        _ => {
            return Err(ExecError::IllegalOpcode { opcode: op, at });
        }
    }
    game.cpu.cycles += cost;
    Ok(())
}
