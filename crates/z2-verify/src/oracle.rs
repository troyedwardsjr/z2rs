//! Lockstep oracle backend.
//!
//! # Shared contract
//!
//! [`Port`] and [`Oracle`] are the **shared-contract traits**: the interpreter,
//! PPU and APU crates mirror these exact names and method shapes. Errors are
//! `Result<_, String>` rather than `anyhow::Result`, because `anyhow` is not
//! a workspace dependency and string errors keep the dependency list short.
//!
//! Input bits (FM2 order, LSB first): A, B, Select, Start, Up, Down, Left,
//! Right = bits 0..7. See [`BTN_A`]…[`BTN_RIGHT`].
//!
//! # Power-on RAM pattern
//!
//! Both backends power on to **all zeros** (`$00` in every byte of CPU RAM,
//! WRAM, OAM, palette RAM, and the framebuffer):
//!
//! * [`TetanesOracle`] builds its [`ControlDeck`](tetanes_core::control_deck::ControlDeck)
//!   with `ram_state: RamState::AllZeros`, which fills console WRAM
//!   (`$0000-$07FF`) and cart PRG-RAM (`$6000-$7FFF`) on the hard reset that
//!   `load_rom` performs. PPU OAM / palette / framebuffer are zero-initialised
//!   by tetanes itself. `sram_dir` is set to `None` so no battery file is
//!   ever read or written — every load starts from the same zeros.
//! * [`StubPort`] powers on to zeros for the same reason.
//!
//! Why zeros and not random? Uninitialised reads are a known divergence
//! source: tetanes' default `RamState::Random` differs per instance, so two
//! oracles would diverge at frame 0 before either emulated a single
//! instruction. `AllZeros` is deterministic across instances, matches
//! tetanes' own `RamState::default()` (*"what most emulators do"*), and keeps
//! the oracle-vs-oracle acceptance test meaningful. It is a documented
//! approximation of real hardware (closer to random); the Mesen 2 probe
//! (`tools/mesen_probe.lua`) exists to cross-check boot state against a
//! second emulator.
//!
//! # Backends
//!
//! * [`TetanesOracle`] — the real oracle: wraps `tetanes-core` v0.15
//!   `ControlDeck` (MIT OR Apache-2.0). ROMs load only through the
//!   `z2-assets` hash gate ([`z2_assets::rom`]), so a wrong ROM fails here,
//!   never silently.
//! * [`StubPort`] — deterministic test double (also implements [`Oracle`];
//!   its `load_rom` still runs the hash gate, then returns a zeroed stub, so
//!   ROM-gated tests prove the pinned ROM exists without needing emulation).

use std::path::Path;

use tetanes_core::common::NesRegion;
use tetanes_core::control_deck::{Config, ControlDeck, HeadlessMode};
use tetanes_core::input::{JoypadBtn, Player};
use tetanes_core::memory::RamState;

// ---------------------------------------------------------------------------
// Shared-contract traits (the interpreter, PPU and APU crates mirror these).
// ---------------------------------------------------------------------------

/// One emulated NES port: advance one frame, then expose observable state.
///
/// All accessors return references to buffers owned by the implementor; they
/// are refreshed by every [`step`](Port::step) (and by `load_rom` /
/// `load_state` on oracles), so holding a reference across a `step` call
/// observes the *previous* frame.
pub trait Port {
    /// Latch `input` on controller 1 and clock exactly one NTSC frame.
    fn step(&mut self, input: u8);
    /// CPU stack pointer after the last step, when the side exposes it.
    /// `None` (default) makes the comparator treat the whole stack page as
    /// live (see [`crate::lockstep::Lockstep`]).
    fn sp(&self) -> Option<u8> {
        None
    }
    /// CPU RAM `$0000-$07FF` (2 KiB).
    fn ram(&self) -> &[u8; 0x800];
    /// Cart WRAM `$6000-$7FFF` (8 KiB, MMC1 PRG-Ram on Zelda II).
    fn wram(&self) -> &[u8; 0x2000];
    /// Primary OAM, 64 sprites × 4 bytes.
    fn oam(&self) -> &[u8; 256];
    /// Palette RAM `$3F00-$3F1F` (32 bytes, mirrors folded).
    fn palette(&self) -> &[u8; 32];
    /// Indexed framebuffer, 256×240 palette indices truncated to `u8`.
    fn frame_indexed(&self) -> &[u8; 256 * 240];
}

/// A [`Port`] that can load the pinned ROM and snapshot/restore full state.
pub trait Oracle: Port {
    /// Verify `path` through the `z2-assets` hash gate, then power on.
    /// Returns `Err(String)` when the ROM is missing or is not the pinned
    /// No-Intro USA dump.
    fn load_rom(path: &Path) -> Result<Self, String>
    where
        Self: Sized;
    /// Opaque full-state blob (tetanes save-state bytes / stub encoding).
    fn save_state(&self) -> Vec<u8>;
    /// Restore a blob previously produced by [`save_state`](Oracle::save_state).
    fn load_state(&mut self, s: &[u8]) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Input bit order (FM2 / NES shift-register order).
// ---------------------------------------------------------------------------

/// Bit 0: A.
pub const BTN_A: u8 = 0;
/// Bit 1: B.
pub const BTN_B: u8 = 1;
/// Bit 2: Select.
pub const BTN_SELECT: u8 = 2;
/// Bit 3: Start.
pub const BTN_START: u8 = 3;
/// Bit 4: Up.
pub const BTN_UP: u8 = 4;
/// Bit 5: Down.
pub const BTN_DOWN: u8 = 5;
/// Bit 6: Left.
pub const BTN_LEFT: u8 = 6;
/// Bit 7: Right.
pub const BTN_RIGHT: u8 = 7;

/// Bit index → tetanes button, in FM2 order.
const BIT_BUTTONS: [JoypadBtn; 8] = [
    JoypadBtn::A,
    JoypadBtn::B,
    JoypadBtn::Select,
    JoypadBtn::Start,
    JoypadBtn::Up,
    JoypadBtn::Down,
    JoypadBtn::Left,
    JoypadBtn::Right,
];

// ---------------------------------------------------------------------------
// TetanesOracle: the real lockstep oracle.
// ---------------------------------------------------------------------------

/// NTSC framebuffer size in pixels.
pub const FRAME_PIXELS: usize = 256 * 240;

/// Oracle PPU internal registers (see [`TetanesOracle::ppu_regs`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OraclePpuRegs {
    /// Current VRAM address / scroll register `v`.
    pub v: u16,
    /// Temporary VRAM address `t`.
    pub t: u16,
    /// Fine X scroll (0-7).
    pub fine_x: u8,
    /// Background pattern table (0/1).
    pub bg_table: u8,
    /// 8x8 sprite pattern table (0/1).
    pub spr_table: u8,
    /// Sprite height (8/16).
    pub spr_height: u8,
    /// `PPUCTRL` bit 7.
    pub nmi_enabled: bool,
    /// `PPUCTRL` bit 2.
    pub vram_step32: bool,
    /// `PPUMASK` bg or sprites enabled.
    pub rendering_enabled: bool,
    /// `PPUSTATUS` sprite-0 hit flag.
    pub spr_zero_hit: bool,
    /// Current scanline.
    pub scanline: u16,
    /// Current dot.
    pub cycle: u16,
    /// Mapper nametable mirroring (debug string of the cart's mode).
    pub mirroring: u8,
}

/// tetanes-core oracle: owns a [`ControlDeck`](tetanes_core::control_deck::ControlDeck)
/// plus owned snapshot caches backing the [`Port`] accessors.
pub struct TetanesOracle {
    deck: ControlDeck,
    ram: Box<[u8; 0x800]>,
    wram: Box<[u8; 0x2000]>,
    oam: Box<[u8; 256]>,
    palette: Box<[u8; 32]>,
    frame: Box<[u8; FRAME_PIXELS]>,
}

impl TetanesOracle {
    /// Deck configuration pinning every determinism-relevant knob.
    ///
    /// * `region: Ntsc` — the pinned ROM is the USA (NTSC) dump.
    /// * `ram_state: AllZeros` — the power-on pattern (see module docs).
    /// * `sram_dir: None` — hermetic: no battery file I/O, so reloads are
    ///   bit-identical without touching the filesystem.
    /// * `headless_mode: NO_AUDIO` — skip APU mixing for speed. Video
    ///   rendering stays **on**: [`Port::frame_indexed`] needs the pixels.
    /// * `concurrent_dpad: true` — report opposing D-pad bits exactly as the
    ///   track holds them, like the hardware controller port does. The
    ///   default (`false`) drops Left when Right is held (and Up under
    ///   Down), which silently breaks the Left+Right glitch the
    ///   `warp-glitch` TAS is built on and makes the oracle disagree with
    ///   a hardware-faithful port from the first glitch frame.
    fn deck_config() -> Config {
        Config {
            region: NesRegion::Ntsc,
            ram_state: RamState::AllZeros,
            sram_dir: None,
            headless_mode: HeadlessMode::NO_AUDIO,
            concurrent_dpad: true,
            ..Default::default()
        }
    }

    /// Latch `input` onto player-1 pad.
    ///
    /// Bits map 1:1 onto the pad in FM2 order (see [`BIT_BUTTONS`]).
    /// Opposing D-Pad pairs (Up+Down, Left+Right) are passed through as
    /// held (`concurrent_dpad = true` in [`Self::deck_config`]), matching
    /// the hardware port and the game side's `$4016` shift register.
    fn latch_input(&mut self, input: u8) {
        let pad = self.deck.joypad_mut(Player::One);
        for (bit, btn) in BIT_BUTTONS.iter().enumerate() {
            pad.set_button(*btn, input & (1 << bit) != 0);
        }
    }

    /// Re-copy observable state out of the deck into the [`Port`] caches.
    fn refresh(&mut self) {
        self.ram.copy_from_slice(self.deck.wram());
        for (i, slot) in self.wram.iter_mut().enumerate() {
            *slot = self.deck.memory().prg_peek(0x6000 + i as u16);
        }
        self.oam[..].copy_from_slice(self.deck.ppu().oamdata.as_ref());
        for (i, slot) in self.palette.iter_mut().enumerate() {
            *slot = self.deck.bus().ppu_bus_peek(0x3F00 + i as u16);
        }
        let raw = self.deck.frame_buffer_raw();
        for (i, slot) in self.frame.iter_mut().enumerate() {
            // Raw pixels are palette indices (0..63, plus PPU emphasis bits);
            // truncation to u8 is a deterministic projection and equality on
            // the projection is what lockstep compares on both sides.
            *slot = (raw[i] & 0xFF) as u8;
        }
    }

    /// Peek one PPU-bus byte (`$0000-$3FFF`: pattern tables through the
    /// mapper, nametables through mirroring, palette RAM). Diagnostics only.
    #[must_use]
    pub fn ppu_peek(&self, addr: u16) -> u8 {
        self.deck.bus().ppu_bus_peek(addr & 0x3FFF)
    }

    /// Snapshot of the oracle PPU's internal registers (diagnostics only).
    #[must_use]
    pub fn ppu_regs(&self) -> OraclePpuRegs {
        let p = self.deck.ppu();
        OraclePpuRegs {
            v: p.scroll.v,
            t: p.scroll.t,
            fine_x: p.scroll.fine_x as u8,
            bg_table: u8::from(p.ctrl_bg_select != 0),
            spr_table: u8::from(p.ctrl_spr_select != 0),
            spr_height: p.ctrl_spr_height as u8,
            nmi_enabled: p.ctrl_nmi_enabled,
            vram_step32: p.ctrl_vram_increment,
            rendering_enabled: p.mask_rendering_enabled,
            spr_zero_hit: p.status_spr_zero_hit,
            scanline: p.scanline,
            cycle: p.cycle,
            mirroring: self.deck.mapper().mirroring() as u8,
        }
    }

    /// Latch pad-1 input for instruction-level stepping (diagnostics).
    pub fn set_input(&mut self, input: u8) {
        self.latch_input(input);
    }

    /// Clock one CPU instruction (diagnostics: sub-frame probing). The
    /// [`Port`] caches are *not* refreshed; call [`Port::step`] for
    /// frame-granular use.
    pub fn clock_instr(&mut self) -> Result<(), String> {
        self.deck
            .clock_instr()
            .map_err(|e| format!("oracle clock_instr: {e}"))
    }

    /// CPU program counter (diagnostics).
    #[must_use]
    pub fn cpu_pc(&self) -> u16 {
        self.deck.cpu().pc
    }

    /// CPU cycle counter (diagnostics).
    #[must_use]
    pub fn cpu_cycle(&self) -> u32 {
        self.deck.cpu().cycle
    }

    /// PPU frame counter (diagnostics).
    #[must_use]
    pub fn ppu_frame_number(&self) -> u32 {
        self.deck.ppu().frame_number()
    }

    /// Refresh the [`Port`] caches from the deck (after instruction-level
    /// stepping).
    pub fn refresh_caches(&mut self) {
        self.refresh();
    }

    /// Number of bytes [`save_state`](Oracle::save_state) will produce.
    /// Useful for corpus budgeting; `None` before a ROM is loaded.
    pub fn state_len(&self) -> Option<usize> {
        self.deck.serialized_state_len().ok()
    }
}

impl Port for TetanesOracle {
    fn step(&mut self, input: u8) {
        self.latch_input(input);
        // A clock failure (unloaded ROM, corrupt state) is a hard harness
        // bug, not a divergence: fail fast like Mesen `--testrunner` does.
        let _ = self.deck.clock_frame().expect("oracle clock_frame failed");
        self.refresh();
    }

    fn ram(&self) -> &[u8; 0x800] {
        &self.ram
    }

    fn wram(&self) -> &[u8; 0x2000] {
        &self.wram
    }

    fn oam(&self) -> &[u8; 256] {
        &self.oam
    }

    fn palette(&self) -> &[u8; 32] {
        &self.palette
    }

    fn frame_indexed(&self) -> &[u8; 256 * 240] {
        &self.frame
    }

    fn sp(&self) -> Option<u8> {
        Some(self.deck.cpu().sp)
    }
}

impl Oracle for TetanesOracle {
    fn load_rom(path: &Path) -> Result<Self, String> {
        // Hash gate first: reject anything that is not the pinned dump
        // before tetanes ever parses it. `open_at` accepts both headered
        // (262160 B) and headerless (262144 B) copies.
        z2_assets::rom::open_at(path).map_err(|e| format!("rom gate: {e}"))?;
        let mut deck = ControlDeck::with_config(Self::deck_config());
        // Belt and braces: the config already carries AllZeros, but pin the
        // pattern at the call site too so a future config refactor cannot
        // silently reintroduce per-instance randomness.
        deck.set_ram_state(RamState::AllZeros);
        deck.load_rom_path(path)
            .map_err(|e| format!("tetanes load_rom {}: {e}", path.display()))?;
        let mut oracle = TetanesOracle {
            deck,
            ram: Box::new([0; 0x800]),
            wram: Box::new([0; 0x2000]),
            oam: Box::new([0; 256]),
            palette: Box::new([0; 32]),
            frame: Box::new([0; FRAME_PIXELS]),
        };
        oracle.refresh();
        Ok(oracle)
    }

    fn save_state(&self) -> Vec<u8> {
        let len = self
            .deck
            .serialized_state_len()
            .expect("oracle serialized_state_len failed");
        let mut buf = vec![0u8; len];
        let n = self
            .deck
            .serialize_state_into(&mut buf)
            .expect("oracle serialize_state_into failed");
        buf.truncate(n);
        buf
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), String> {
        // The deck must already hold the same ROM: a save state carries
        // only the mutable tail, and tetanes restores the ROM half from the
        // loaded cart.
        self.deck
            .deserialize_state(s)
            .map_err(|e| format!("oracle load_state: {e}"))?;
        self.refresh();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// StubPort: deterministic test double.
// ---------------------------------------------------------------------------

/// Magic opening a [`StubPort`] state blob.
const STUB_MAGIC: &[u8; 8] = b"Z2STUB01";

/// Deterministic [`Oracle`] test double: no emulation, but the same
/// observable shapes, the same all-zeros power-on pattern, and a
/// difference-preserving step function (every byte advances by the same
/// wrapping add on both sides, so an injected one-byte corruption survives
/// indefinitely and lockstep reports it at the exact frame + address).
///
/// `load_rom` runs the real hash gate and then returns a zeroed stub, so
/// ROM-gated stub tests still prove the pinned ROM exists.
pub struct StubPort {
    ram: Box<[u8; 0x800]>,
    wram: Box<[u8; 0x2000]>,
    oam: Box<[u8; 256]>,
    palette: Box<[u8; 32]>,
    frame: Box<[u8; FRAME_PIXELS]>,
    frame_no: u64,
}

impl StubPort {
    /// Zeroed stub (the shared all-zeros power-on pattern).
    pub fn new() -> Self {
        StubPort {
            ram: Box::new([0; 0x800]),
            wram: Box::new([0; 0x2000]),
            oam: Box::new([0; 256]),
            palette: Box::new([0; 32]),
            frame: Box::new([0; FRAME_PIXELS]),
            frame_no: 0,
        }
    }

    /// Frames stepped so far.
    pub fn frame_no(&self) -> u64 {
        self.frame_no
    }

    /// Test-only fault injection: overwrite one CPU-RAM byte.
    pub fn corrupt_ram(&mut self, addr: usize, value: u8) {
        self.ram[addr & 0x7FF] = value;
    }

    /// Test-only fault injection: overwrite one WRAM byte.
    pub fn corrupt_wram(&mut self, addr: usize, value: u8) {
        self.wram[addr & 0x1FFF] = value;
    }

    /// Test-only fault injection: overwrite one OAM byte.
    pub fn corrupt_oam(&mut self, idx: usize, value: u8) {
        self.oam[idx & 0xFF] = value;
    }

    /// Test-only fault injection: overwrite one palette byte.
    pub fn corrupt_palette(&mut self, idx: usize, value: u8) {
        self.palette[idx & 0x1F] = value;
    }

    /// Test-only fault injection: overwrite one framebuffer pixel.
    pub fn corrupt_frame(&mut self, idx: usize, value: u8) {
        self.frame[idx % FRAME_PIXELS] = value;
    }

    fn mix(&mut self, input: u8) {
        self.frame_no = self.frame_no.wrapping_add(1);
        let m = input
            .wrapping_add((self.frame_no & 0xFF) as u8)
            .wrapping_add(1);
        for b in self
            .ram
            .iter_mut()
            .chain(self.wram.iter_mut())
            .chain(self.oam.iter_mut())
            .chain(self.palette.iter_mut())
            .chain(self.frame.iter_mut())
        {
            *b = b.wrapping_add(m);
        }
    }
}

impl Default for StubPort {
    fn default() -> Self {
        Self::new()
    }
}

impl Port for StubPort {
    fn step(&mut self, input: u8) {
        self.mix(input);
    }

    fn ram(&self) -> &[u8; 0x800] {
        &self.ram
    }

    fn wram(&self) -> &[u8; 0x2000] {
        &self.wram
    }

    fn oam(&self) -> &[u8; 256] {
        &self.oam
    }

    fn palette(&self) -> &[u8; 32] {
        &self.palette
    }

    fn frame_indexed(&self) -> &[u8; 256 * 240] {
        &self.frame
    }
}

impl Oracle for StubPort {
    fn load_rom(path: &Path) -> Result<Self, String> {
        // Same gate as the real oracle; the bytes are then discarded — the
        // stub is pure deterministic state, which is what makes it a useful
        // double for the lockstep comparator itself.
        z2_assets::rom::open_at(path).map_err(|e| format!("rom gate: {e}"))?;
        Ok(StubPort::new())
    }

    fn save_state(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 8 + 0x800 + 0x2000 + 256 + 32 + FRAME_PIXELS);
        out.extend_from_slice(STUB_MAGIC);
        out.extend_from_slice(&self.frame_no.to_le_bytes());
        out.extend_from_slice(&self.ram[..]);
        out.extend_from_slice(&self.wram[..]);
        out.extend_from_slice(&self.oam[..]);
        out.extend_from_slice(&self.palette[..]);
        out.extend_from_slice(&self.frame[..]);
        out
    }

    fn load_state(&mut self, s: &[u8]) -> Result<(), String> {
        let want = 8 + 8 + 0x800 + 0x2000 + 256 + 32 + FRAME_PIXELS;
        if s.len() != want {
            return Err(format!("stub load_state: {} bytes, want {want}", s.len()));
        }
        if &s[..8] != STUB_MAGIC {
            return Err("stub load_state: bad magic".to_string());
        }
        let mut off = 16;
        self.frame_no = u64::from_le_bytes(s[8..16].try_into().unwrap());
        let mut take = |n: usize| -> Vec<u8> {
            let v = s[off..off + n].to_vec();
            off += n;
            v
        };
        self.ram.copy_from_slice(&take(0x800));
        self.wram.copy_from_slice(&take(0x2000));
        self.oam.copy_from_slice(&take(256));
        self.palette.copy_from_slice(&take(32));
        self.frame.copy_from_slice(&take(FRAME_PIXELS));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_powers_on_to_zeros() {
        let s = StubPort::new();
        assert!(s.ram().iter().all(|&b| b == 0));
        assert!(s.wram().iter().all(|&b| b == 0));
        assert!(s.oam().iter().all(|&b| b == 0));
        assert!(s.palette().iter().all(|&b| b == 0));
        assert!(s.frame_indexed().iter().all(|&b| b == 0));
    }

    #[test]
    fn stub_state_roundtrips() {
        let mut s = StubPort::new();
        for i in 0..17 {
            s.step((i * 37) as u8);
        }
        let blob = s.save_state();
        let mut t = StubPort::new();
        t.load_state(&blob).unwrap();
        assert_eq!(t.frame_no(), s.frame_no());
        assert_eq!(t.ram(), s.ram());
        assert_eq!(t.wram(), s.wram());
        assert_eq!(t.oam(), s.oam());
        assert_eq!(t.palette(), s.palette());
        assert_eq!(t.frame_indexed(), s.frame_indexed());
        assert!(t.load_state(&blob[..blob.len() - 1]).is_err());
        assert!(t.load_state(&[0u8; 64]).is_err());
    }

    #[test]
    fn stub_step_is_deterministic() {
        let mut a = StubPort::new();
        let mut b = StubPort::new();
        for i in 0..300 {
            let input = (i as u8).wrapping_mul(53).wrapping_add(11);
            a.step(input);
            b.step(input);
        }
        assert_eq!(a.ram(), b.ram());
        assert_eq!(a.frame_indexed(), b.frame_indexed());
    }

    #[test]
    fn input_bit_order_documents_fm2() {
        // A,B,Select,Start,Up,Down,Left,Right = bits 0..7.
        assert_eq!([BTN_A, BTN_B, BTN_SELECT, BTN_START], [0, 1, 2, 3]);
        assert_eq!([BTN_UP, BTN_DOWN, BTN_LEFT, BTN_RIGHT], [4, 5, 6, 7]);
        assert_eq!(BIT_BUTTONS.len(), 8);
    }
}
