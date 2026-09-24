//! Hybrid-execution `Game`.
//!
//! [`Game`] is the shared-RAM-mirror owner the interpreter, the ported
//! routines, the verifier and (later) the PPU/APU models all meet at:
//!
//! * `ram: [u8; 0x800]` — NES internal RAM `$0000-$07FF` (the interpreter
//!   reads/writes it with mirroring; the typed [`crate::ram::Ram`]
//!   views the same bytes — convert with
//!   [`crate::ram::Ram::from_slice`]`(game.ram())` without sharing types).
//! * `wram: [u8; 0x2000]` — battery-backed WRAM `$6000-$7FFF`.
//!
//! Do not confuse this with [`crate::facts::Game`]: that is a tiny
//! snapshot/export handle owning a [`crate::ram::Ram`]. This `Game` owns the
//! live emulation state (CPU, mapper, trap table, frame counter).
//!
//! # Shared contract (verifier, interpreter, PPU, APU)
//!
//! Input bits LSB-first: A,B,Select,Start,Up,Down,Left,Right = 0..7
//! ([`INPUT_A`]..[`INPUT_RIGHT`], masks [`BTN_A`]..[`BTN_RIGHT`]). One logic
//! frame is [`Game::step`] (`input: u8`); state is inspected with
//! [`Game::ram`]/[`Game::wram`]/[`Game::oam`]/[`Game::palette`]/
//! [`Game::frame_indexed`]. The `Port` trait in `z2-verify`
//! mirrors exactly these signatures — this crate does not depend on
//! `z2-verify`, so keep them in sync by hand.
//!
//! # PPU/APU feeds
//!
//! `oam`/`palette`/`frame` are fed per frame from the real PPU model
//! ([`crate::ppu_bind::PpuBind`]): OAM DMA at `$4014`
//! writes `oam` and forwards into the model, palette writes via `$2007`
//! land in PPU palette RAM, and [`Game::step`] latches the scanline-timed
//! framebuffer at the frame end plus both mirrors for lockstep. The APU façade
//! ([`NullApu`]) sinks register writes into a per-frame log; frontends
//! drain it into the `z2-apu` synth after each step (see
//! `z2-native::app::step_frames`).
//!
//! # Feature flag
//!
//! With `interp` disabled, the CPU/mapper/trap fields and [`Game::step`]'s
//! execution half compile out: `step` latches input and advances the frame
//! counter only, and [`Game::call_asm`] panics. The state struct, ROM
//! loader and accessors stay available so ported engine code keeps building.

#[cfg(feature = "interp")]
use crate::cpu::{Cpu, ExecError, Mmc1, NullApu, FLAG_C, FLAG_N, FLAG_Z};
#[cfg(feature = "interp")]
use crate::ppu_bind::PpuBind;
#[cfg(feature = "interp")]
use crate::traps::{Divergence, TrapExit, TrapFn, TrapLog, TrapTable};

// ------------------------------------------------- shared constants

/// Input bit: A.
pub const INPUT_A: u8 = 0;
/// Input bit: B.
pub const INPUT_B: u8 = 1;
/// Input bit: Select.
pub const INPUT_SELECT: u8 = 2;
/// Input bit: Start.
pub const INPUT_START: u8 = 3;
/// Input bit: Up.
pub const INPUT_UP: u8 = 4;
/// Input bit: Down.
pub const INPUT_DOWN: u8 = 5;
/// Input bit: Left.
pub const INPUT_LEFT: u8 = 6;
/// Input bit: Right.
pub const INPUT_RIGHT: u8 = 7;

/// Button mask: A (bit 0).
pub const BTN_A: u8 = 1 << INPUT_A;
/// Button mask: B.
pub const BTN_B: u8 = 1 << INPUT_B;
/// Button mask: Select.
pub const BTN_SELECT: u8 = 1 << INPUT_SELECT;
/// Button mask: Start.
pub const BTN_START: u8 = 1 << INPUT_START;
/// Button mask: Up.
pub const BTN_UP: u8 = 1 << INPUT_UP;
/// Button mask: Down.
pub const BTN_DOWN: u8 = 1 << INPUT_DOWN;
/// Button mask: Left.
pub const BTN_LEFT: u8 = 1 << INPUT_LEFT;
/// Button mask: Right.
pub const BTN_RIGHT: u8 = 1 << INPUT_RIGHT;

/// Framebuffer width (NTSC NES).
pub const FRAME_W: usize = 256;
/// Framebuffer height (NTSC NES).
pub const FRAME_H: usize = 240;
/// Indexed framebuffer bytes.
pub const FRAME_LEN: usize = FRAME_W * FRAME_H;

/// Nominal CPU cycles per NTSC frame (`89342 / 3`, rounded down). The
/// real cadence is fractional and alternates with the odd-frame dot skip;
/// [`Game::step`] runs a dot-exact frame clock ([`FRAME_DOTS`]) and only
/// falls back to this constant for synthetic tests that pre-step the
/// clock past the frame boundary.
#[cfg(feature = "interp")]
pub const FRAME_CPU_CYCLES: u64 = 29_780;

/// PPU dots per scanline.
#[cfg(feature = "interp")]
pub const DOTS_PER_LINE: u64 = 341;

/// PPU dots per NTSC frame (`262 * 341`); one dot shorter on odd frames
/// with rendering enabled (the pre-render line skips dot 340).
#[cfg(feature = "interp")]
pub const FRAME_DOTS: u64 = 262 * DOTS_PER_LINE;

/// Dots from the end of visible line 239 to the vblank flag (line 241
/// dot 1): the post-render line plus one dot.
#[cfg(feature = "interp")]
pub const VBLANK_START_DOTS: u64 = DOTS_PER_LINE + 1;

/// Dots from the end of visible line 239 to the start of the pre-render
/// line (line 240 + 20 vblank lines).
#[cfg(feature = "interp")]
pub const PRERENDER_START_DOTS: u64 = 21 * DOTS_PER_LINE;

/// Pre-render dot at which hardware decides the odd-frame skip (rendering
/// enabled at dot 339 shortens the line to 340 dots).
#[cfg(feature = "interp")]
pub const ODD_SKIP_DOT: u64 = 339;

/// End of the power-on frame (end of visible line 239 of frame 0), in
/// absolute CPU cycles: the lockstep oracle (tetanes `clock_frame`) closes
/// its first frame here, so the first [`Game::step`] ends here too and
/// every later compare boundary stays aligned. In dots: [`FIRST_FRAME_END_DOTS`].
#[cfg(feature = "interp")]
pub const FIRST_FRAME_END: u64 = 27_281;

/// CPU-to-PPU phase calibration, in dots (`dot = 3 * cycle - origin`).
/// Zero: `xtask probe` puts the port's register accesses within one
/// instruction (a few cycles) of the oracle's, and that residue is the
/// NMI's instruction-boundary jitter, not a fixed phase.
#[cfg(feature = "interp")]
pub const DOT_PHASE: u64 = 0;

/// [`FIRST_FRAME_END`] on the dot clock (`3` dots per CPU cycle) shifted
/// by the measured [`DOT_PHASE`].
#[cfg(feature = "interp")]
pub const FIRST_FRAME_END_DOTS: u64 = FIRST_FRAME_END * 3 - DOT_PHASE;

/// First vblank point, in absolute CPU cycles from power-on (line 241
/// dot 1 after frame 0: `(FIRST_FRAME_END_DOTS + VBLANK_START_DOTS) / 3`;
/// the oracle sets its first vblank at cycle `27396`, within one spin
/// iteration). Frame `k`'s vblank point is derived the same way from that
/// frame's dot-exact end (see [`Game::step`]).
#[cfg(feature = "interp")]
pub const FIRST_VBLANK_AT: u64 = (FIRST_FRAME_END_DOTS + VBLANK_START_DOTS) / 3;

/// CPU cycles between the vblank flag rising and the NMI being taken: the
/// 6502 samples the NMI line late in each instruction, so the interrupt
/// sequence starts about two cycles after the edge (the oracle's NMI entry
/// lands 2-3 cycles after its vblank point; `xtask probe` shows the port's
/// mid-frame writes tracking the oracle's to within an instruction once this
/// latency is applied).
#[cfg(feature = "interp")]
pub const NMI_LATENCY_CYCLES: u64 = 2;

/// Nominal steady vblank cadence in CPU cycles (`29780.67` on NTSC,
/// `29780.5` averaged over the odd-frame skip). Informational: the step
/// clock counts dots, it does not add this constant.
#[cfg(feature = "interp")]
pub const VBLANK_PERIOD: u64 = 29_781;

/// Vblank window length in CPU cycles (NTSC scanlines 241-260 to the
/// pre-render line: `(PRERENDER_START_DOTS - VBLANK_START_DOTS) / 3`).
///
/// The flag sets at the vblank hook and clears `VBLANK_LEN` later at the
/// pre-render equivalent ([`Ppu::begin_frame`](z2_ppu::Ppu::begin_frame),
/// which also clears sprite-0/overflow exactly like hardware's
/// `stop_vblank`). The window matters: arming NMI (`$2000` bit 7) while it
/// is set fires immediately (retrigger rule in `crate::cpu::bus_write`),
/// and a stale set flag would fire a whole frame early.
#[cfg(feature = "interp")]
pub const VBLANK_LEN: u64 = (PRERENDER_START_DOTS - VBLANK_START_DOTS) / 3;

/// Default [`Game::try_call_asm`] budget (runaway guard).
#[cfg(feature = "interp")]
pub const CALL_ASM_BUDGET: u64 = 10_000_000;

// ------------------------------------------------- ROM loading

/// Error from [`Game::from_ines`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Fewer than 16 header bytes.
    TooShort,
    /// Missing `NES\x1A` magic.
    BadMagic,
    /// Mapper is not MMC1 (1).
    WrongMapper(u8),
    /// File ends before the declared PRG/CHR payload.
    Truncated,
    /// Zero PRG banks declared.
    NoPrg,
    /// Interpreter compiled out (`--no-default-features`).
    NoInterp,
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadError::TooShort => write!(f, "iNES file shorter than 16-byte header"),
            LoadError::BadMagic => write!(f, "missing NES\\x1A magic"),
            LoadError::WrongMapper(m) => write!(f, "mapper {m} is not MMC1 (1)"),
            LoadError::Truncated => write!(f, "file ends before declared PRG/CHR payload"),
            LoadError::NoPrg => write!(f, "iNES header declares zero PRG banks"),
            LoadError::NoInterp => write!(f, "PRG banking needs the `interp` feature"),
        }
    }
}

impl std::error::Error for LoadError {}

// ------------------------------------------------- Game

/// Live hybrid-execution state: shared RAM mirror + interpreter + traps.
///
/// See the module docs for the ownership/contract notes.
#[derive(Debug)]
pub struct Game {
    /// NES internal RAM `$0000-$07FF` (mirrored through `$1FFF` on the bus).
    pub ram: [u8; 0x800],
    /// Battery-backed WRAM `$6000-$7FFF`.
    pub wram: [u8; 0x2000],
    /// Sprite OAM (256 bytes; `$4014` DMA writes here; PPU feeds later).
    pub oam: [u8; 256],
    /// Palette RAM mirror (32 bytes; zeroed until the PPU model feeds it).
    pub palette: [u8; 32],
    /// Indexed framebuffer, 256×240 (zeroed until the PPU model feeds it).
    pub frame: [u8; FRAME_LEN],
    /// 2A03 registers (interp only).
    #[cfg(feature = "interp")]
    pub cpu: Cpu,
    /// MMC1 mapper (interp only).
    #[cfg(feature = "interp")]
    pub mmc1: Mmc1,
    /// Real-PPU binding: the interpreter bus routes
    /// `$2000-$2007` here and `$4014` DMA forwards here; [`Game::step`]
    /// renders one frame per step into it (see [`crate::ppu_bind`]).
    #[cfg(feature = "interp")]
    pub ppu: PpuBind,
    /// APU register façade (logs register writes; frontends replay them into `z2-apu`).
    #[cfg(feature = "interp")]
    pub apu: NullApu,
    /// Raw PRG-ROM image (16 KiB multiples; mapped via [`Mmc1`]).
    #[cfg(feature = "interp")]
    pub prg: Vec<u8>,
    /// Raw CHR image (8 KiB units; `0`-length file CHR-RAM becomes 8 KiB zeroed).
    #[cfg(feature = "interp")]
    pub chr: Vec<u8>,
    /// Routine trap table (starts empty: pure interpretation).
    #[cfg(feature = "interp")]
    pub traps: TrapTable,
    /// Ring of recently fired traps (feeds [`Divergence::last_writer`]).
    #[cfg(feature = "interp")]
    pub traplog: TrapLog,
    /// Lifetime interpreter faults (illegal opcode / runaway budget).
    ///
    /// A fault aborts the current frame slice (`run_budget` returns
    /// `false`: no render, no mirrors that slice) and the next frame
    /// retries from the stuck PC — so a faulting game wedges *silently*
    /// unless the frontend watches this counter (see `last_exec_error`
    /// for where, and `z2-native` which auto-pauses on first fault).
    /// Zero on a healthy run; nonzero means the port diverged into
    /// unmapped territory (data executed as code).
    #[cfg(feature = "interp")]
    pub exec_errors: u64,
    /// Most recent interpreter fault, if any.
    #[cfg(feature = "interp")]
    pub last_exec_error: Option<ExecError>,
    /// Two-player co-op state (off by default; see [`crate::coop`]).
    #[cfg(feature = "interp")]
    pub coop: crate::coop::CoopState,
    /// Widescreen margin sprites (display only; off by default; see
    /// [`crate::wide_sprites`]). Not part of any save state.
    #[cfg(feature = "interp")]
    pub margin_sprites: crate::wide_sprites::MarginSpriteState,
    /// Wide-gameplay state (off by default; see [`crate::wide_gameplay`]).
    #[cfg(feature = "interp")]
    pub wide_game: crate::wide_gameplay::WideGameplayState,
    pub(crate) pad1: u8,
    /// Pad-2 latch + controller shift state (interp only; the bus is gone
    /// without it, so these fields compile out too).
    #[cfg(feature = "interp")]
    pub(crate) pad2: u8,
    #[cfg(feature = "interp")]
    pub(crate) strobe: bool,
    #[cfg(feature = "interp")]
    pub(crate) shift1: u8,
    #[cfg(feature = "interp")]
    pub(crate) shift2: u8,
    /// Dot-clock position of the end of the last completed visible frame
    /// (end of line 239); the next step's vblank/pre-render/frame-end points
    /// derive from it (see [`Game::step`]).
    #[cfg(feature = "interp")]
    pub(crate) frame_end_dots: u64,
    pub(crate) frame_count: u64,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    /// Zeroed state with no ROM (synthetic tests populate PRG with
    /// [`Game::with_test_program`]).
    pub fn new() -> Self {
        Self {
            ram: [0; 0x800],
            wram: [0; 0x2000],
            oam: [0; 256],
            palette: [0; 32],
            frame: [0; FRAME_LEN],
            #[cfg(feature = "interp")]
            cpu: Cpu::new(),
            #[cfg(feature = "interp")]
            mmc1: Mmc1::new(),
            #[cfg(feature = "interp")]
            ppu: PpuBind::new(),
            #[cfg(feature = "interp")]
            apu: NullApu::default(),
            #[cfg(feature = "interp")]
            prg: vec![0; 0x8000],
            #[cfg(feature = "interp")]
            chr: vec![0; 0x2000],
            #[cfg(feature = "interp")]
            traps: TrapTable::new(),
            #[cfg(feature = "interp")]
            traplog: TrapLog::new(),
            #[cfg(feature = "interp")]
            exec_errors: 0,
            #[cfg(feature = "interp")]
            last_exec_error: None,
            #[cfg(feature = "interp")]
            coop: crate::coop::CoopState::default(),
            #[cfg(feature = "interp")]
            margin_sprites: crate::wide_sprites::MarginSpriteState::default(),
            wide_game: crate::wide_gameplay::WideGameplayState::default(),
            pad1: 0,
            #[cfg(feature = "interp")]
            pad2: 0,
            #[cfg(feature = "interp")]
            strobe: false,
            #[cfg(feature = "interp")]
            shift1: 0,
            #[cfg(feature = "interp")]
            shift2: 0,
            #[cfg(feature = "interp")]
            frame_end_dots: 0,
            frame_count: 0,
        }
    }

    /// Load an iNES file image (16-byte header + PRG + CHR).
    ///
    /// Requires MMC1 (mapper 1); anything else is [`LoadError::WrongMapper`].
    /// Read-only over the caller's bytes: never copies the ROM file itself,
    /// only the parsed banks. Mirroring bit seeds [`Mmc1`] control.
    #[cfg(feature = "interp")]
    pub fn from_ines(bytes: &[u8]) -> Result<Self, LoadError> {
        if bytes.len() < 16 {
            return Err(LoadError::TooShort);
        }
        if bytes[0..4] != *b"NES\x1A" {
            return Err(LoadError::BadMagic);
        }
        let prg_units = bytes[4] as usize;
        let chr_units = bytes[5] as usize;
        let flags6 = bytes[6];
        let flags7 = bytes[7];
        let mapper = (flags7 & 0xF0) | (flags6 >> 4);
        if mapper != 1 {
            return Err(LoadError::WrongMapper(mapper));
        }
        if prg_units == 0 {
            return Err(LoadError::NoPrg);
        }
        let mut off = 16usize;
        if flags6 & 0x04 != 0 {
            off += 512; // Trainer.
        }
        let prg_len = prg_units * 0x4000;
        let chr_len = chr_units * 0x2000;
        if bytes.len() < off + prg_len + chr_len {
            return Err(LoadError::Truncated);
        }
        let mut game = Self::new();
        game.prg = bytes[off..off + prg_len].to_vec();
        game.chr = if chr_len == 0 {
            // CHR-RAM cartridge: model as 8 KiB zeroed (CHR-RAM writes go
            // through the PPU bus).
            vec![0; 0x2000]
        } else {
            bytes[off + prg_len..off + prg_len + chr_len].to_vec()
        };
        // Bit 0: 0 = horizontal, 1 = vertical (keep power-on PRG mode 3).
        let mirr = if flags6 & 0x01 != 0 { 2 } else { 3 };
        game.mmc1.ctrl = (game.mmc1.ctrl & !3) | mirr;
        // Seed the PPU from the ROM image: CHR pattern pages selected by
        // the power-on MMC1 (8 KiB mode, banks 0/1) plus the header
        // mirroring bit. Later bank switches re-sync lazily at every
        // vblank hook (see `vblank_hook`).
        game.ppu.sync_from_mapper(&game.chr, &game.mmc1);
        Ok(game)
    }

    /// Synthetic-test constructor: 32 KiB zeroed PRG with `blob` copied at
    /// `origin` (`$8000-$FFFF`), RESET vector pointed at `origin`, MMC1 in
    /// power-on mode 3 (fix-last) with PRG bank 0 — so `origin` reads back
    /// `blob` on either side of `$C000`.
    ///
    /// Panics when `origin < $8000` or the blob overflows the 32 KiB window.
    #[cfg(feature = "interp")]
    pub fn with_test_program(origin: u16, blob: &[u8]) -> Self {
        assert!(origin >= 0x8000, "test program origin must be $8000+");
        let off = (origin - 0x8000) as usize;
        assert!(
            off + blob.len() <= 0x8000,
            "test program overflows 32 KiB PRG window"
        );
        let mut game = Self::new();
        game.prg[off..off + blob.len()].copy_from_slice(blob);
        game.set_reset_vector(origin);
        game
    }

    /// Point the RESET vector (`$FFFC`, fixed last bank) at `target`.
    #[cfg(feature = "interp")]
    pub fn set_reset_vector(&mut self, target: u16) {
        let n = self.prg.len();
        self.prg[n - 4] = target as u8;
        self.prg[n - 3] = (target >> 8) as u8;
    }

    /// Point the NMI vector (`$FFFA`, fixed last bank) at `target`.
    #[cfg(feature = "interp")]
    pub fn set_nmi_vector(&mut self, target: u16) {
        let n = self.prg.len();
        self.prg[n - 6] = target as u8;
        self.prg[n - 5] = (target >> 8) as u8;
    }

    /// Point the IRQ/`BRK` vector (`$FFFE`, fixed last bank) at `target`.
    #[cfg(feature = "interp")]
    pub fn set_irq_vector(&mut self, target: u16) {
        let n = self.prg.len();
        self.prg[n - 2] = target as u8;
        self.prg[n - 1] = (target >> 8) as u8;
    }

    // ------------------------------------------------ shared contract

    /// Run one logic frame on `input` (bits A,B,Select,Start,Up,Down,Left,
    /// Right = 0..7).
    ///
    /// Latches the pad state for `$4016`/`$4017` polling, then runs the CPU
    /// on a dot-exact NTSC frame clock (interp only). A step covers exactly
    /// one PPU frame as the lockstep oracle counts them: from the end of
    /// visible line 239 to the next end of line 239. Inside it, in order:
    ///
    /// 1. to the vblank point (line 241 dot 1 of the previous frame,
    ///    [`VBLANK_START_DOTS`] after the step start): the vblank hook
    ///    ([`Game::vblank_hook`]) sets the flag and services the NMI edge
    ///    at once, so the handler preempts main code like hardware;
    /// 2. to the pre-render line ([`PRERENDER_START_DOTS`]): the flag and
    ///    sprite flags clear ([`z2_ppu::Ppu::begin_frame`]); the odd-frame
    ///    dot skip (odd frame number with rendering enabled — the oracle's
    ///    rule) fixes this frame's length, and the PPU binding gets the
    ///    frame origin for its beam clock;
    /// 3. to the frame end: visible lines render lazily as the CPU touches
    ///    the PPU (scanline-timed, see [`crate::ppu_bind`]); at the end the
    ///    frame is completed and latched into `Game::frame`, and the
    ///    OAM/palette mirrors feed lockstep.
    ///
    /// The power-on step ends at [`FIRST_FRAME_END`] with no vblank inside.
    pub fn step(&mut self, input: u8) {
        self.pad1 = input;
        #[cfg(feature = "interp")]
        {
            Cpu::service_pending(self);
            let frame_start = self.cpu.cycles;
            let k = self.frame_count;
            let mut live;
            let end: u64;
            if k == 0 && frame_start < FIRST_FRAME_END {
                // Power-on frame: no vblank point inside.
                self.frame_end_dots = FIRST_FRAME_END_DOTS;
                end = FIRST_FRAME_END;
                live = self.run_budget(end);
            } else if frame_start * 3 >= self.frame_end_dots + FRAME_DOTS {
                // Pre-stepped clock (synthetic tests using run_cycles /
                // call_asm first): fall back to a full nominal budget and
                // re-anchor the dot clock on it.
                end = frame_start + FRAME_CPU_CYCLES;
                self.frame_end_dots = end * 3;
                live = self.run_budget(end);
            } else {
                let base = self.frame_end_dots;
                let t = (base + VBLANK_START_DOTS) / 3;
                let origin = (base + PRERENDER_START_DOTS) / 3;
                // Phase 1: to the vblank point; the flag rises there (k==1
                // is the cold-boot warmup vblank that never sets it), the
                // NMI edge is taken NMI_LATENCY_CYCLES later.
                live = self.run_budget(t);
                let mut edge = false;
                let mut flag_set = false;
                if live {
                    flag_set = self.vblank_flag(k - 1);
                    // The edge latches with the flag: a `$2002` read in the
                    // latency window clears the flag but not the pending
                    // interrupt (hardware only suppresses an NMI when the
                    // read lands on the very dot the flag rises).
                    edge = flag_set && self.ppu.nmi_edge();
                    live = self.run_budget(t + NMI_LATENCY_CYCLES);
                }
                if live {
                    self.nmi_edge_hook(edge);
                    // Phase 2: vblank tail; the flag clears at the
                    // pre-render equivalent.
                    live = self.run_budget(origin);
                }
                if live && flag_set {
                    self.ppu.model_mut().begin_frame();
                }
                // Odd-frame dot skip (tetanes: frame counter odd at the
                // pre-render line with rendering enabled *at dot 339*).
                // Visible frame k follows k completed frames. Run to that
                // dot before deciding: the NMI's own `$2001` disable /
                // re-enable pair can straddle the pre-render line on long
                // frames.
                let decide_at = (base + PRERENDER_START_DOTS + ODD_SKIP_DOT) / 3;
                self.ppu
                    .set_frame_origin_dots(base + PRERENDER_START_DOTS, DOTS_PER_LINE as u16);
                if live {
                    live = self.run_budget(decide_at);
                }
                let skip = if k % 2 == 1 && self.ppu.model().rendering_enabled() {
                    1
                } else {
                    0
                };
                let prerender_dots = DOTS_PER_LINE - skip;
                self.ppu
                    .set_frame_origin_dots(base + PRERENDER_START_DOTS, prerender_dots as u16);
                self.frame_end_dots = base + FRAME_DOTS - skip;
                end = self.frame_end_dots / 3;
                if live {
                    // Phase 3: the visible frame.
                    live = self.run_budget(end);
                }
            }
            if live {
                // Frame complete (end of line 239): finish whatever the
                // beam has not reached, latch it, and mirror OAM/palette
                // (`$4014`/`$2004`, `$2007`) for lockstep.
                self.capture_frame(end);
                self.oam = *self.ppu.oam_bytes();
                self.palette = *self.ppu.palette_bytes();
            }
            if self.coop.enabled {
                crate::coop::end_of_frame(self);
            }
            if self.wide_game.enabled {
                crate::wide_gameplay::end_of_frame(self);
            }
        }
        self.frame_count += 1;
    }

    /// Run the CPU until `cycles` reaches `end` (NMI/IRQ serviced at
    /// instruction boundaries). Returns `false` on illegal opcode
    /// (runaway guard: the frame freezes, like the old whole-frame loop).
    ///
    /// Frame-budget interaction: trapped routines advance
    /// `cpu.cycles` by their [`TrapInfo::cycles`](crate::traps::TrapInfo)
    /// cost inside [`Game::fire_trap`], so each `run_budget` slice executes
    /// fewer interpreter steps when traps fire — the same pacing as
    /// untrapped interpretation. Without charging, trapped boots ran ~33k
    /// cycles of clears/erases for free and reached later code a frame
    /// early (f=3 collapse); with charging the absolute-cycle vblank points
    /// (`FIRST_VBLANK_AT + k*VBLANK_PERIOD`) and the NMI cadence stay
    /// aligned with the oracle.
    ///
    /// Split rule: a `JSR`/`JMP` to a trapped routine whose atomic cost
    /// would cross `end` interprets that one call instead (via
    /// [`crate::cpu::split_trap_target`]), so 15k routines like
    /// `Reset_Memory_Ranges` split across the frame end like hardware
    /// instead of overshooting it atomically (which would complete zeropage
    /// clears a frame early: f=3 `$0000` 02-vs-00).
    #[cfg(feature = "interp")]
    fn run_budget(&mut self, end: u64) -> bool {
        while self.cpu.cycles < end {
            Cpu::service_pending(self);
            if let Some(target) = crate::cpu::split_trap_target(self, end) {
                self.traps.set_untrapped(target, true);
                let r = crate::cpu::step_instruction(self);
                self.traps.set_untrapped(target, false);
                if let Err(e) = r {
                    self.record_exec_error(e);
                    return false;
                }
            } else if let Err(e) = crate::cpu::step_instruction(self) {
                self.record_exec_error(e);
                return false;
            }
            if self.cpu.cycles >= end {
                break;
            }
        }
        true
    }

    /// Record an interpreter fault (see [`Game::exec_errors`]).
    #[cfg(feature = "interp")]
    fn record_exec_error(&mut self, e: ExecError) {
        self.exec_errors += 1;
        self.last_exec_error = Some(e);
    }

    /// Complete the visible frame at `end` (end of line 239) into
    /// `Game::frame`: re-sync CHR banks (cheap no-op unless changed),
    /// advance the beam to the frame end, render whatever it has not
    /// reached with the live state, and latch the framebuffer.
    #[cfg(feature = "interp")]
    fn capture_frame(&mut self, end: u64) {
        self.ppu.sync_from_mapper(&self.chr, &self.mmc1);
        self.ppu.sync_cycles(end);
        let fb = self.ppu.finish_frame();
        self.frame = fb;
    }

    /// Vblank start (interp only): raise the flag, or leave it clear for
    /// `k == 0` (cold-boot warmup — the first vblank flag never sets, so
    /// the `$FF78` reset spin consumes an extra vblank; matches the
    /// oracle's observable 3-vblank boot and real-hardware first-vblank
    /// unreliability). Returns whether the flag was set (the caller clears
    /// it at the pre-render equivalent).
    #[cfg(feature = "interp")]
    fn vblank_flag(&mut self, k: u64) -> bool {
        if k == 0 {
            false
        } else {
            self.ppu.model_mut().end_frame();
            true
        }
    }

    /// NMI edge, [`NMI_LATENCY_CYCLES`] after the flag rose: service the
    /// edge latched when the flag was set (flag set **and** `$2000` bit 7
    /// armed at that moment — never unconditionally, so NMI-disabled spans
    /// stay quiet), preempting main code mid-frame like hardware.
    #[cfg(feature = "interp")]
    fn nmi_edge_hook(&mut self, edge: bool) {
        if edge {
            self.cpu.nmi_pending = true;
        }
        Cpu::service_pending(self);
    }

    /// Borrow the 2 KiB RAM mirror.
    pub fn ram(&self) -> &[u8; 0x800] {
        &self.ram
    }

    /// Borrow the 8 KiB battery WRAM.
    pub fn wram(&self) -> &[u8; 0x2000] {
        &self.wram
    }

    /// Borrow OAM (256 bytes; fed by `$4014` DMA and the PPU model).
    pub fn oam(&self) -> &[u8; 256] {
        &self.oam
    }

    /// Borrow palette RAM (32 bytes; fed by the PPU model).
    pub fn palette(&self) -> &[u8; 32] {
        &self.palette
    }

    /// Borrow the indexed framebuffer (256×240; fed by the PPU model).
    pub fn frame_indexed(&self) -> &[u8; FRAME_LEN] {
        &self.frame
    }

    // ------------------------------------------------ widescreen (record)

    /// Enable/disable the PPU render record (widescreen / HD consumers).
    /// Off by default; never changes `frame` or game logic.
    #[cfg(feature = "interp")]
    pub fn set_record(&mut self, on: bool) {
        self.ppu.model_mut().set_record(on);
    }

    /// Whether the render record is on.
    #[cfg(feature = "interp")]
    #[must_use]
    pub fn record_enabled(&self) -> bool {
        self.ppu.model().record_enabled()
    }

    /// Record of the frame latched by the last [`Game::step`] (`None` when off).
    #[cfg(feature = "interp")]
    #[must_use]
    pub fn frame_record(&self) -> Option<&z2_ppu::FrameRecord> {
        self.ppu.model().frame_record()
    }

    /// Decode widescreen margins (`tiles` per side) for the last frame into
    /// `out`. Returns false, leaving `out` untouched, when the record is off.
    #[cfg(feature = "interp")]
    pub fn wide_margins(&self, tiles: u8, out: &mut z2_ppu::Margins) -> bool {
        let Some(rec) = self.frame_record() else {
            return false;
        };
        crate::wide_margins::build_margins(&self.ram, &self.wram, &self.prg, rec, tiles, out);
        self.margin_sprites_into(&mut out.sprites);
        true
    }

    /// Turn widescreen margin sprites for side-view objects on or off
    /// (default off). On registers the display-only `$EF11` observer
    /// ([`crate::wide_sprites`]); the game runs byte-identically either way.
    /// Call after the default trap groups. Never called by verification.
    #[cfg(feature = "interp")]
    pub fn set_margin_sprites(&mut self, on: bool) {
        crate::wide_sprites::set_enabled(self, on);
    }

    /// Whether the side-view margin-sprite observer is on.
    #[cfg(feature = "interp")]
    #[must_use]
    pub fn margin_sprites_enabled(&self) -> bool {
        self.margin_sprites.enabled
    }

    /// Side-view objects outside the window latched for the last frame
    /// (empty while [`Game::set_margin_sprites`] is off).
    #[cfg(feature = "interp")]
    #[must_use]
    pub fn sideview_margin_sprites(&self) -> &[z2_ppu::MarginSprite] {
        crate::wide_sprites::shown(self)
    }

    /// **Hook for other margin-sprite providers.** A list drawn after (below)
    /// the side-view objects by [`Game::wide_margins`] and
    /// [`Game::compose_wide`]. The provider owns it: clear and refill it once
    /// per frame, after [`Game::step`]. Display only — never read by the game
    /// and not part of any save state.
    #[cfg(feature = "interp")]
    pub fn margin_sprites_mut(&mut self) -> &mut Vec<z2_ppu::MarginSprite> {
        &mut self.margin_sprites.extra
    }

    /// Every margin sprite of the last frame, in priority order: side-view
    /// objects, wide-gameplay overworld blobs (drawn whenever that mode
    /// keeps them alive in a margin, since they are real enemies), then
    /// [`Game::margin_sprites_mut`]'s list. Replaces `out`.
    #[cfg(feature = "interp")]
    pub fn margin_sprites_into(&self, out: &mut Vec<z2_ppu::MarginSprite>) {
        out.clear();
        out.extend_from_slice(crate::wide_sprites::shown(self));
        out.extend_from_slice(self.wide_game.shown());
        out.extend_from_slice(&self.margin_sprites.extra);
    }

    /// Shadow runs of the margin-sprite observer that failed (zero when
    /// healthy; a failure only costs that object's margin sprites).
    #[cfg(feature = "interp")]
    #[must_use]
    pub fn margin_sprite_errors(&self) -> u64 {
        self.margin_sprites.shadow_errors
    }

    /// Scene identity and camera of the last frame when it was sideview
    /// play, for HD pack layers. `None` otherwise or with the record off.
    #[cfg(feature = "interp")]
    #[must_use]
    pub fn sideview_scene(&self) -> Option<crate::wide_margins::SideviewScene> {
        crate::wide_margins::sideview_scene(&self.ram, self.frame_record()?)
    }

    /// Margins plus composition into `wide` (centre = `frame` verbatim).
    /// With the record off, `wide` still gets the centre and backdrop
    /// margins (`margins` cleared to backdrop) and the result is false.
    #[cfg(feature = "interp")]
    pub fn compose_wide(
        &self,
        tiles: u8,
        margins: &mut z2_ppu::Margins,
        wide: &mut z2_ppu::WideFrame,
    ) -> bool {
        if let Some(rec) = self.frame_record() {
            crate::wide_margins::build_margins(
                &self.ram, &self.wram, &self.prg, rec, tiles, margins,
            );
            self.margin_sprites_into(&mut margins.sprites);
            z2_ppu::render_wide_indexed(&self.frame, rec, margins, &self.chr, wide);
            true
        } else {
            margins.set_tiles(tiles);
            margins.clear_backdrop();
            margins.sprites.clear();
            let empty = z2_ppu::FrameRecord::new();
            z2_ppu::render_wide_indexed(&self.frame, &empty, margins, &self.chr, wide);
            false
        }
    }

    /// Frames executed by [`Game::step`].
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    // ------------------------------------------------ execution (interp)

    /// Fetch RESET vector and initialise (interp only).
    #[cfg(feature = "interp")]
    pub fn reset(&mut self) {
        Cpu::reset(self);
    }

    /// Latch an NMI edge (serviced at the next instruction boundary).
    #[cfg(feature = "interp")]
    pub fn request_nmi(&mut self) {
        self.cpu.request_nmi();
    }

    /// Execute one instruction at the current PC (trap-aware).
    #[cfg(feature = "interp")]
    pub fn step_instruction(&mut self) -> Result<(), ExecError> {
        Cpu::service_pending(self);
        crate::cpu::step_instruction(self)
    }

    /// Run `budget` CPU cycles, servicing interrupts at boundaries.
    #[cfg(feature = "interp")]
    pub fn run_cycles(&mut self, budget: u64) -> Result<(), ExecError> {
        Cpu::run_cycles(self, budget)
    }

    /// Interpret the unported routine at `target` until its matching `RTS`.
    ///
    /// Pushes a synthetic return address, so the final `RTS` restores the
    /// entry stack level and halts (rather than jumping into the wild).
    /// Restores the entry PC afterwards; only the routine's memory /
    /// register effects remain. Requires stack-balanced code (net-zero
    /// pushes/pops apart from the final `RTS`) and panics past
    /// [`CALL_ASM_BUDGET`] instructions.
    #[cfg(feature = "interp")]
    pub fn call_asm(&mut self, target: u16) {
        if let Err(e) = self.try_call_asm(target, CALL_ASM_BUDGET) {
            panic!("call_asm(${target:04X}): {e}");
        }
    }

    /// [`Game::call_asm`] with an explicit budget; returns instructions run.
    #[cfg(feature = "interp")]
    pub fn try_call_asm(&mut self, target: u16, budget: u64) -> Result<u64, ExecError> {
        let sp0 = self.cpu.sp;
        // Synthetic return address $FFFF: the matching RTS pops it and the
        // stack-level check in `run_until_sp` halts before fetching at
        // $0000. (JSR pushes ret = PC-1, RTS lands at ret+1; pushing $FFFF
        // as the stored value emulates a JSR whose last byte sat at $FFFF.)
        self.ram[0x0100 + self.cpu.sp as usize] = 0xFF;
        self.cpu.sp = self.cpu.sp.wrapping_sub(1);
        self.ram[0x0100 + self.cpu.sp as usize] = 0xFF;
        self.cpu.sp = self.cpu.sp.wrapping_sub(1);
        self.run_until_sp(target, sp0, budget)
    }

    /// Interpret from `pc` until the stack pointer is back at `sp` — the
    /// `RTS`/`RTI` that pops the frame sitting there — then restore the
    /// entry `PC`; returns instructions run. Unlike [`Game::try_call_asm`]
    /// nothing synthetic is pushed: ports use it to run unported code inside
    /// a `JSR` frame they pushed themselves (`bank7_common::jsr_sub`, a
    /// tail jump out of an inner body), so dead stack bytes match the
    /// ROM's. `Err` past `budget` instructions or on an illegal opcode.
    #[cfg(feature = "interp")]
    pub fn run_until_sp(&mut self, pc: u16, sp: u8, budget: u64) -> Result<u64, ExecError> {
        let pc0 = self.cpu.pc;
        self.cpu.pc = pc;
        let mut n: u64 = 0;
        loop {
            Cpu::service_pending(self);
            if self.cpu.sp == sp {
                break;
            }
            if n >= budget {
                self.cpu.pc = pc0;
                return Err(ExecError::MaxInstructions { at: self.cpu.pc });
            }
            crate::cpu::step_instruction(self)?;
            n += 1;
        }
        self.cpu.pc = pc0;
        Ok(n)
    }

    // ------------------------------------------------ traps (interp)

    /// Register a ported routine (`name`, ledger `bank`, entry `addr`).
    ///
    /// Backward-compat wrapper: the stored cycle cost comes from
    /// [`crate::traps::default_trap_cycles`] (`0` for unknown addresses),
    /// so existing call sites keep compiling while known boot traps pick
    /// up real costs. Use [`Game::trap_register_cycles`] to override.
    #[cfg(feature = "interp")]
    pub fn trap_register(&mut self, name: &'static str, bank: Option<u8>, addr: u16, func: TrapFn) {
        self.traps.register(name, bank, addr, func);
    }

    /// Register a ported routine with an explicit cycle cost (see
    /// [`crate::traps::TrapInfo::cycles`]: body + final `RTS`/`RTI`,
    /// caller entry excluded). Replaces any previous entry at `addr`.
    #[cfg(feature = "interp")]
    pub fn trap_register_cycles(
        &mut self,
        name: &'static str,
        bank: Option<u8>,
        addr: u16,
        func: TrapFn,
        cycles: u64,
    ) {
        self.traps
            .register_with_cycles(name, bank, addr, func, cycles);
    }

    /// A/B toggle: run `addr` untrapped (`true`) or trapped (`false`).
    #[cfg(feature = "interp")]
    pub fn set_untrapped(&mut self, addr: u16, untrapped: bool) {
        self.traps.set_untrapped(addr, untrapped);
    }

    /// Borrow the trap log (last [`TRAP_LOG_CAP`](crate::traps::TRAP_LOG_CAP) routines).
    #[cfg(feature = "interp")]
    pub fn trap_log(&self) -> &TrapLog {
        &self.traplog
    }

    /// `"name@$ADDR"` of the most recently fired trap, if any — feeds
    /// [`Divergence::last_writer`].
    #[cfg(feature = "interp")]
    pub fn last_writer(&self) -> Option<String> {
        self.traplog.last().map(|r| r.attribution())
    }

    /// Record and run the trap at `addr`, then follow its tail jumps.
    ///
    /// Called by the CPU on `JSR`/`JMP`/vector transfer to a trapped
    /// address, and by ports whose body `JSR`s into a trapped routine
    /// (`bank7_common::jsr_sub`). After the body, a pending
    /// [`Game::trap_jump`] request is resolved here: a trapped target fires
    /// in turn (the chain runs until a body returns or jumps into unported
    /// code), so the caller only ever sees [`TrapExit::Return`] — emulate
    /// the routine's `RTS`/`RTI` — or [`TrapExit::Jump`] — continue at an
    /// unported address with the stack as the original `JMP` left it.
    /// Nothing is pushed for a jump, so dead stack bytes match the ROM's.
    /// An unregistered `addr` is a no-op (`Return`).
    ///
    /// Cycle accounting: advances `cpu.cycles` by each fired
    /// trap's [`TrapInfo::cycles`](crate::traps::TrapInfo) cost (body +
    /// final `RTS`/`RTI`, entry `JSR`/`JMP`/service already counted at the
    /// dispatch site in `cpu.rs`) before following its jump, so the next
    /// body in a chain sees the clock the ROM would. The charge lands
    /// before the dispatcher's emulated `RTS`/`RTI`, so `run_budget`/
    /// `Cpu::run_cycles` frame budgets and the absolute-cycle NMI cadence
    /// observe trapped time.
    #[cfg(feature = "interp")]
    pub fn fire_trap(&mut self, addr: u16) -> TrapExit {
        debug_assert!(
            self.cpu.trap_jump.is_none(),
            "stale trap_jump request before firing ${addr:04X}"
        );
        let mut addr = addr;
        loop {
            let (func, name, cycles) = match self.traps.get(addr) {
                Some(info) => (info.func, info.name, info.cycles),
                None => return TrapExit::Return,
            };
            self.traplog.push(addr, name, self.frame_count);
            func(self);
            self.cpu.cycles += cycles;
            match self.cpu.trap_jump.take() {
                None => return TrapExit::Return,
                Some(next) if self.traps.is_trapped(next) => addr = next,
                Some(next) => return TrapExit::Jump(next),
            }
        }
    }

    /// Tail `JMP target` out of the running trap body.
    ///
    /// Once the body returns, [`Game::fire_trap`] continues at `target`
    /// instead of emulating the routine's `RTS`: `target`'s own trap fires
    /// when it has one (chaining through further tail jumps), otherwise the
    /// dispatch site interprets from there. The 6502 stack is left exactly
    /// as the original `JMP` would have — the caller's return frame stays
    /// underneath for `target`'s `RTS`, nothing is pushed. Also the right
    /// call for a branch into code that stays with the interpreter (a
    /// partially ported routine) and for a `PLA : PLA : JMP` non-local exit
    /// once the port has popped the frame itself. Charge the `JMP` (or
    /// branch) in the port; `target` charges itself. A body running inside
    /// an inner `JSR` frame (`bank7_common::inner_jsr_frame`/`jsr_sub`) may
    /// use it too: the jump then runs inside that frame.
    #[cfg(feature = "interp")]
    pub fn trap_jump(&mut self, target: u16) {
        self.cpu.trap_jump = Some(target);
    }

    /// Take the pending [`Game::trap_jump`] request, if any. The dispatch
    /// path consumes it itself; tests that call a port body directly use
    /// this to observe and clear it.
    #[cfg(feature = "interp")]
    pub fn take_trap_jump(&mut self) -> Option<u16> {
        self.cpu.trap_jump.take()
    }

    /// Compare against oracle bytes; first mismatch wins, attributed with
    /// [`Game::last_writer`]. `None` = identical (no divergence).
    #[cfg(feature = "interp")]
    pub fn divergence_vs(
        &self,
        oracle_ram: &[u8; 0x800],
        oracle_wram: &[u8; 0x2000],
    ) -> Option<Divergence> {
        for (i, (&e, &a)) in oracle_ram.iter().zip(self.ram.iter()).enumerate() {
            if e != a {
                return Some(Divergence {
                    frame: self.frame_count,
                    addr: i as u16,
                    expected: e,
                    actual: a,
                    last_writer: self.last_writer(),
                });
            }
        }
        for (i, (&e, &a)) in oracle_wram.iter().zip(self.wram.iter()).enumerate() {
            if e != a {
                return Some(Divergence {
                    frame: self.frame_count,
                    addr: (0x6000 + i) as u16,
                    expected: e,
                    actual: a,
                    last_writer: self.last_writer(),
                });
            }
        }
        None
    }

    /// Replay-harness helper: run one [`Game::step`] per input byte.
    /// Deterministic; the ignored `interp_replay` test drives warp-glitch
    /// movies through this.
    pub fn run_inputs(&mut self, inputs: &[u8]) {
        for &b in inputs {
            self.step(b);
        }
    }

    /// Set both pad latches directly (synthetic-test setup; [`Game::step`]
    /// is the normal per-frame path).
    #[cfg(feature = "interp")]
    pub fn set_pad(&mut self, pad1: u8, pad2: u8) {
        self.pad1 = pad1;
        self.pad2 = pad2;
    }

    /// Latch both pads and clock one frame (co-op / netplay path).
    ///
    /// Exactly `set pad 2; step(pad1)`: with `pad2 == 0` on a game whose pad
    /// 2 is idle it is identical to [`Game::step`] (which never touches pad
    /// 2, so the verification path is unaffected). While co-op is enabled
    /// the pad-2 manual-save chord is filtered ([`crate::coop::mask_pad2`]).
    /// Pad 2 stays latched until the next `step2`/`set_pad`, so a frontend
    /// that drops back to `step` after a session should call `step2(p1, 0)`
    /// once.
    #[cfg(feature = "interp")]
    pub fn step2(&mut self, pad1: u8, pad2: u8) {
        self.pad2 = crate::coop::mask_pad2(self.coop.enabled, pad2);
        self.step(pad1);
    }

    /// Enable/disable two-player co-op at runtime (default off).
    ///
    /// Enabling registers the co-op trap group ([`crate::coop::register_coop_traps`],
    /// idempotent) — call it **after** the default trap groups — and runs
    /// the PPU with an unlimited sprite count while enabled. Never called by
    /// verification.
    #[cfg(feature = "interp")]
    pub fn set_coop(&mut self, on: bool) {
        crate::coop::set_enabled(self, on);
    }

    /// Co-op tunables (defaults are what frontends and netplay use).
    #[cfg(feature = "interp")]
    pub fn set_coop_options(&mut self, opts: crate::coop::CoopOptions) {
        self.coop.opts = opts;
    }

    /// Read-only co-op view for frontends; `None` while co-op is off.
    #[cfg(feature = "interp")]
    pub fn coop_status(&self) -> Option<crate::coop::CoopStatus> {
        crate::coop::status(self)
    }

    /// FNV-1a of the co-op state that influences future frames (mix into a
    /// netplay desync hash alongside RAM/WRAM).
    #[cfg(feature = "interp")]
    pub fn coop_hash(&self) -> u64 {
        crate::coop::hash(&self.coop)
    }

    /// Drop P2 from the current area so it re-anchors next to P1 on the
    /// next sideview frame. Call after replacing RAM (snapshot restore).
    #[cfg(feature = "interp")]
    pub fn coop_reset_area(&mut self) {
        crate::coop::leave_area(&mut self.coop);
    }

    /// Turn wide gameplay on with a margin of `Some(tiles)` per side
    /// (clamped to 16; `8 * tiles` pixels), or off with `None` (the
    /// default). On registers the two bank-0 overworld traps and patches the
    /// townsfolk spawn table in the in-memory PRG copy; off unregisters and
    /// restores both. Never called by verification. It changes gameplay, so
    /// netplay peers must agree on it (it is part of the trap-set identity
    /// the frontends exchange, see [`Game::wide_gameplay_tiles`]).
    #[cfg(feature = "interp")]
    pub fn set_wide_gameplay(&mut self, margin_tiles: Option<u8>) {
        crate::wide_gameplay::set(self, margin_tiles);
    }

    /// Wide-gameplay margin in tiles per side, `None` while off.
    #[cfg(feature = "interp")]
    pub fn wide_gameplay_tiles(&self) -> Option<u8> {
        self.wide_game
            .enabled
            .then_some(self.wide_game.margin_px / 8)
    }

    /// Overworld encounter blobs that live in the widescreen margins, as
    /// [`z2_ppu::MarginSprite`]s in window coordinates and OAM priority order,
    /// for the picture the last [`Game::step`] rendered. Empty unless wide
    /// gameplay is on and blobs are out in a margin.
    #[cfg(feature = "interp")]
    pub fn overworld_margin_sprites(&self) -> &[z2_ppu::MarginSprite] {
        self.wide_game.shown()
    }

    /// FNV-1a of the wide-gameplay state that influences future frames (mix
    /// into a netplay desync hash alongside RAM/WRAM).
    #[cfg(feature = "interp")]
    pub fn wide_gameplay_hash(&self) -> u64 {
        crate::wide_gameplay::hash(&self.wide_game)
    }

    // ------------------------------------------------ cpu state (tests)

    /// `(A, X, Y, SP, PC, P)` for test assertions.
    #[cfg(feature = "interp")]
    pub fn cpu_state(&self) -> (u8, u8, u8, u8, u16, u8) {
        (
            self.cpu.a,
            self.cpu.x,
            self.cpu.y,
            self.cpu.sp,
            self.cpu.pc,
            self.cpu.p,
        )
    }

    /// Set all registers (synthetic-test setup).
    #[cfg(feature = "interp")]
    pub fn set_cpu(&mut self, a: u8, x: u8, y: u8, sp: u8, pc: u16, p: u8) {
        self.cpu.a = a;
        self.cpu.x = x;
        self.cpu.y = y;
        self.cpu.sp = sp;
        self.cpu.pc = pc;
        self.cpu.p = p;
    }

    // ------------------------------------------------ controller (bus)

    /// `$4016` write: bit 0 = strobe (latch while high).
    #[cfg(feature = "interp")]
    pub(crate) fn set_strobe(&mut self, val: u8) {
        let new = val & 1 != 0;
        if self.strobe && !new {
            self.shift1 = self.pad1;
            self.shift2 = self.pad2;
        }
        self.strobe = new;
    }

    /// `$4016` read: pad-1 serial bit (1s after the 8 latched bits).
    #[cfg(feature = "interp")]
    pub(crate) fn poll_pad1(&mut self) -> u8 {
        if self.strobe {
            return self.pad1 & 1;
        }
        let b = self.shift1 & 1;
        self.shift1 = (self.shift1 >> 1) | 0x80;
        b
    }

    /// `$4017` read: pad-2 serial bit (the APU façade owns `$4017` writes;
    /// `$4017` reads are controller 2 on hardware, routed here by the bus).
    #[cfg(feature = "interp")]
    pub(crate) fn poll_pad2(&mut self) -> u8 {
        if self.strobe {
            return self.pad2 & 1;
        }
        let b = self.shift2 & 1;
        self.shift2 = (self.shift2 >> 1) | 0x80;
        b
    }
}

// ------------------------------------------------- hand-ported routines

/// Hand-ported NMI RNG advance (ledger RAM `$051A-$0522`, code `prg7.asm`
/// `$C189-$C1B0`).
///
/// The original shifts a 9-bit-carry LFSR across `$051A..$0522` (entry
/// `X=0`, `Y=9`; `ROR $051A,x` × 9 with carry seeded from bit 1 of
/// `$051A` vs `$051B`). This replicates every observable effect so
/// trapped/untrapped A/B runs are bit-identical: the 9 RAM bytes, scratch
/// `$00`, `A` (`(b0 ^ b1)`), `X=9`, `Y=0`, and the flags the loop epilogue
/// leaves behind — `C` from the final `ROR`'s shifted-out bit 0 (INX/DEY
/// preserve `C`), `Z=1`/`N=0` from the final `DEY` (`Y` 1→0), which runs
/// after the last `ROR` and overwrites its `N`/`Z`.
///
/// Original bytes (for the untrapped side of the A/B test):
/// `AD 1A 05 29 02 85 00 AD 1B 05 29 02 45 00 18 F0 01 38 7E 1A 05 E8 88 D0 F9`.
#[cfg(feature = "interp")]
pub fn rng_advance(game: &mut Game) {
    let b0 = game.ram[0x51A] & 0x02;
    let b1 = game.ram[0x51B] & 0x02;
    game.ram[0x000] = b0;
    game.cpu.a = b0 ^ b1;
    let mut carry = (b0 ^ b1) != 0;
    for i in 0..9usize {
        let addr = 0x51A + i;
        let v = game.ram[addr];
        let out = (v >> 1) | if carry { 0x80 } else { 0 };
        carry = v & 1 != 0;
        game.ram[addr] = out;
    }
    game.cpu.x = 9;
    game.cpu.y = 0;
    // Flags as the loop epilogue leaves them: the final `ROR $0522` sets
    // `C` from the shifted-out bit 0, then `INX`/`DEY` overwrite `N`/`Z`
    // (`Y` 1→0 gives `Z=1`, `N=0`); `C` survives (neither touches it).
    game.cpu.p =
        (game.cpu.p & !(FLAG_C | FLAG_Z | FLAG_N)) | if carry { FLAG_C } else { 0 } | FLAG_Z;
}
