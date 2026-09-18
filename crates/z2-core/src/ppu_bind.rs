//! Real-PPU binding for the interpreter bus.
//!
//! [`PpuBind`] owns a [`z2_ppu::Ppu`] and implements the [`crate::cpu::PpuBus`]
//! façade (`ppu_read` / `ppu_write` over `$2000-$2007`, already de-mirrored
//! by the bus in [`crate::cpu`]), so the interpreter routes every PPU
//! register access to the software model with no CPU-side logic changes.
//! [`crate::game::Game::step`] drives the frame hooks around it: the vblank
//! hook ([`crate::game::Game::vblank_hook`]: MMC1→PPU re-sync, HUD-split
//! derivation, full [`z2_ppu::Ppu::render_frame`], NMI edge) fires at the
//! absolute vblank point mid-step, and OAM DMA (`$4014`) still lives in the
//! bus ([`crate::cpu::bus_write`]), forwarding the copied page via
//! [`PpuBind::oam_dma`].
//!
//! # `$2002` sequencing (the two boot blockers this fixes)
//!
//! * **PPU-warmup phase shift** (`$FF78` reset spin waits 2 vblanks):
//!   [`z2_ppu::Ppu::new`] powers up with vblank **clear** (like hardware),
//!   so the untrapped ASM spin runs until the first [`PpuBind::end_frame`]
//!   sets vblank — the game stays in reset through frame 0 exactly like the
//!   oracle, instead of exiting after 2 stub reads. (The old [`crate::cpu::NullPpu`]
//!   reported bit 7 stuck set.)
//! * **Sprite-0 wedge** (bank-5 `$A740` `BIT $2002 / BVC` loop): bit 6 comes
//!   from the renderer's overlap test ([`z2_ppu::render::render_scanline`]),
//!   cleared at the start of every [`z2_ppu::Ppu::render_frame`] and set
//!   during it when an opaque sprite-0 pixel covers an opaque background
//!   pixel with both layers enabled. `$2002` reads report it via
//!   [`z2_ppu::Ppu::read_status`] (which also clears vblank and resets the
//!   `$2005`/`$2006` latch, as on hardware), unless the [`PpuBind::force_sprite0_hit`]
//!   test override is armed.
//!
//! # Beam clock
//!
//! The model is scanline-timed (see `z2_ppu::state`): before every register
//! access the bus calls [`PpuBind::sync_access`] with the CPU cycle count,
//! which is turned into a `(scanline, dot)` beam position on the dot clock
//! relative to the frame origin ([`PpuBind::set_frame_origin_dots`], the
//! dot at which the pre-render line begins — `Game::step` sets it at every
//! pre-render point). Lines the beam has passed render lazily
//! with the register state that was live, so the sprite-0 HUD split, the
//! intro's `$2006` scroll and mid-frame CHR switches come out right, and
//! `$2002` bit 6 rises when the beam reaches sprite 0 (not a frame late).
//!
//! # CHR wiring
//!
//! iNES layout is `16-byte header (+512 trainer) + PRG + CHR`, so CHR page `N`
//! (4 KiB) lives at `header + PRG_len + N*4096` in the file. [`crate::game::Game::from_ines`]
//! already splits the image into `prg`/`chr`; [`PpuBind::sync_from_mapper`]
//! slices 4 KiB pages out of the `chr` image per [`crate::cpu::Mmc1::map_chr4`]
//! and loads them with [`z2_ppu::Ppu::load_chr_4k`] (plus
//! [`z2_ppu::Ppu::set_mirroring_mmc1`]). It is called once from `from_ines`
//! (power-on banks) and lazily at every [`PpuBind::end_frame`] — the lazy
//! re-sync covers *all* MMC1 mutation paths (bus writes **and** the bank-7
//! `SwapCHR`/MMC1 traps, which mutate `Mmc1` without touching this file),
//! at frame granularity: a mid-frame bank switch renders the whole frame
//! with the end-of-frame banks. That matches the model's documented
//! no-dot-level scope.
//!
//! # Render cadence
//!
//! Every frame renders fully (no every-Nth-frame skipping): lockstep hashes
//! `Game::frame_indexed` after *every* step, so skipped frames would report
//! false framebuffer divergences. A 256×240 indexed render is cheap next to
//! the ~29 780 CPU cycles of [`crate::game::Game::step`]'s budget.
//!
//! # Known gaps (honest, not hidden)
//!
//! * CHR-RAM cartridges: [`z2_ppu::Ppu::write_data`] ignores `$0000-$1FFF`
//!   writes (Zelda II is CHR-ROM); a CHR-RAM game would render blank tiles.
//! * Out-of-range CHR bank selects (bank beyond the end of the `chr` image,
//!   e.g. synthetic 8 KiB test images with a large select) leave the pattern
//!   slot unchanged rather than loading garbage.
//! * `$2000`/`$2001`/`$2003`/`$2005`/`$2006` are write-only on hardware;
//!   reads return open bus here (modelled as `0x00`).
//! * `$2004` (OAMDATA) reads are served from a shadow OAM address kept in
//!   lock-step with the model's (every `$2003`/`$2004` write routes through
//!   this binding); reads do not advance the address.
//! * Mid-frame register effects apply at tile-fetch / pixel granularity on
//!   the current scanline (see `z2_ppu::AccessKind`), not to the dot; the
//!   current line's sprites are composited from the state at line commit
//!   rather than from a previous-line snapshot.
//! * `force_sprite0_hit` is a test-only override for bringing up code that
//!   spins on bit 6 before CHR/scroll/OAM contents can produce a natural
//!   hit; it defaults to off (renderer-driven).

use z2_ppu::{AccessKind, Ppu, PPUSTATUS_SPRITE0};

/// Re-export for call sites that name the status bits.
pub use z2_ppu::{PPUSTATUS_SPRITE0 as STATUS_SPRITE0, PPUSTATUS_VBLANK as STATUS_VBLANK};

use crate::cpu::{Mmc1, PpuBus};

/// CHR pattern-table slot bytes (one 4 KiB page per slot).
use z2_ppu::CHR_BANK_LEN;

/// Real-PPU binding: the [`PpuBus`] the interpreter bus talks to.
///
/// Keeps the [`crate::cpu::NullPpu`]-shaped traffic counters (`reads`,
/// `writes`, `last_write`) so bus-routing tests keep compiling and the
/// bring-up can prove traffic reaches the model.
#[derive(Debug, Clone)]
pub struct PpuBind {
    /// The software PPU model (nametables, CHR, OAM, palette, registers).
    pub ppu: Ppu,
    /// Number of `$2000-$2007` reads served.
    pub reads: u64,
    /// Number of `$2000-$2007` writes sunk.
    pub writes: u64,
    /// Last write `(register, value)`.
    pub last_write: Option<(u16, u8)>,
    /// Shadow OAM address for `$2004` readback (mirrors the model's
    /// internal `$2003` latch; `$4014` DMA leaves it unchanged on both).
    oam_addr: u8,
    /// Test-only sprite-0-hit override for `$2002` reads: `None` (default)
    /// reports the renderer's flag; `Some(v)` forces bit 6 to `v`. See
    /// [`PpuBind::force_sprite0_hit`].
    force_sprite0: Option<bool>,
    /// Last `(chr0, chr1, ctrl)` served by [`PpuBind::sync_from_mapper`];
    /// `None` forces the first sync (power-on banks must load even though
    /// they read `0`).
    mapper_key: Option<(u8, u8, u8)>,
    /// Sprite-0 hit observed by the last [`PpuBind::render_frame`] (the
    /// beam's answer for that frame). The model flag itself is cleared by
    /// [`Ppu::begin_frame`](z2_ppu::Ppu::begin_frame) mid-tail (pre-render
    /// equivalent), so post-frame reads always see clear; this latch keeps
    /// the frame's answer observable for diagnostics and tests.
    last_hit: bool,
    /// Dot-clock position at which the current frame's pre-render scanline
    /// starts (see module docs, "Beam clock"); `3` dots per CPU cycle.
    origin_dots: u64,
    /// Length of the current frame's pre-render line in dots (341, or 340
    /// on odd frames with rendering enabled — the NTSC dot skip).
    prerender_dots: u16,
}

impl Default for PpuBind {
    fn default() -> Self {
        Self::new()
    }
}

impl PpuBind {
    /// Power-on binding: fresh [`Ppu`] (vblank clear, like hardware),
    /// zeroed counters, no sprite-0 override, mapper cache empty so the
    /// first [`PpuBind::sync_from_mapper`] always loads.
    ///
    /// OAM powers on zeroed (not the model's `$FF` park pattern): the
    /// lockstep oracle (tetanes) powers OAM to `$00`, and the game
    /// explicitly initialises OAM (`Remove_All_Sprites`) before relying on
    /// it, so the power-on value is unobservable to correct code — matching
    /// the oracle keeps frame-0 lockstep exact.
    #[must_use]
    pub fn new() -> Self {
        let mut ppu = Ppu::new();
        ppu.oam_dma(&[0u8; 256]);
        Self {
            ppu,
            reads: 0,
            writes: 0,
            last_write: None,
            oam_addr: 0,
            force_sprite0: None,
            mapper_key: None,
            last_hit: false,
            origin_dots: 0,
            prerender_dots: z2_ppu::DOTS_PER_LINE,
        }
    }

    /// Set the dot-clock position at which the frame's pre-render scanline
    /// begins and that line's length (`341`, or `340` with the odd-frame
    /// skip). `Game::step` calls this once per frame at the pre-render
    /// point.
    pub fn set_frame_origin_dots(&mut self, origin_dots: u64, prerender_dots: u16) {
        self.origin_dots = origin_dots;
        self.prerender_dots = prerender_dots;
    }

    /// Frame origin (dot-clock position of the pre-render line start).
    #[must_use]
    pub fn frame_origin(&self) -> u64 {
        self.origin_dots
    }

    /// Beam position for a CPU cycle count: `(line, dot)` with `-1` =
    /// pre-render, `0..=239` visible, `240..=260` post-render/vblank.
    /// Cycles before the origin are still the previous frame's vblank.
    #[must_use]
    pub fn beam_for_cycles(&self, cycles: u64) -> (i16, u16) {
        let dots = cycles * 3;
        if dots < self.origin_dots {
            return (z2_ppu::VBLANK_LINE, 0);
        }
        let rel = dots - self.origin_dots;
        let pre = u64::from(self.prerender_dots);
        if rel < pre {
            return (z2_ppu::PRERENDER_LINE, rel as u16);
        }
        let r = rel - pre;
        let per = u64::from(z2_ppu::DOTS_PER_LINE);
        let line = (r / per).min(u64::from(z2_ppu::LINES_PER_FRAME as u16) - 2) as i16;
        (line, (r % per) as u16)
    }

    /// Advance the beam to `cycles` (renders every scanline passed).
    pub fn sync_cycles(&mut self, cycles: u64) {
        let (line, dot) = self.beam_for_cycles(cycles);
        self.ppu.set_beam(line, dot);
    }

    /// Advance the beam to `cycles` ahead of a register access of `kind`
    /// (see [`z2_ppu::AccessKind`]: how much of the current line commits
    /// with the pre-access state).
    pub fn sync_access(&mut self, cycles: u64, kind: AccessKind) {
        let (line, dot) = self.beam_for_cycles(cycles);
        self.ppu.set_beam_access(line, dot, kind);
    }

    /// Access kind for a `$2000-$2007` register access: `$2000` changes
    /// pattern-table selects (fetch pipeline), `$2001` the pixel mux,
    /// `$2003`/`$2004` OAM (next line's sprites), `$2005` the fine-x mux
    /// (immediate) plus `t` (no effect until the dot-257 reload),
    /// `$2006`/`$2007` `v` (fetch pipeline).
    #[must_use]
    pub fn access_kind(reg: u16, write: bool) -> AccessKind {
        match (reg, write) {
            (0x2000, true) => AccessKind::Fetch,
            (0x2001 | 0x2005, true) => AccessKind::Pixel,
            (0x2003 | 0x2004, true) => AccessKind::Line,
            (0x2006 | 0x2007, _) => AccessKind::Fetch,
            _ => AccessKind::Read,
        }
    }

    /// MMC1 register changed at `cycles`: advance the beam, then re-select
    /// CHR pages / mirroring so later scanlines use the new banks.
    pub fn mapper_sync(&mut self, chr_rom: &[u8], mmc1: &Mmc1, cycles: u64) {
        let key = (mmc1.chr0, mmc1.chr1, mmc1.ctrl);
        if self.mapper_key == Some(key) {
            return;
        }
        self.sync_access(cycles, AccessKind::Fetch);
        self.sync_from_mapper(chr_rom, mmc1);
        let (p0, p1) = (self.ppu.chr_page_no(0), self.ppu.chr_page_no(1));
        self.ppu.trace_mapper(mmc1.ctrl, p0, p1);
    }

    /// `$4014` OAM DMA forward: copy a full 256-byte page into PPU OAM.
    /// Called by the bus after it copies the same page into `Game::oam`;
    /// the OAM address latch is untouched on both sides.
    pub fn oam_dma(&mut self, page: &[u8; 256]) {
        self.ppu.oam_dma(page);
    }

    /// Re-select CHR pattern pages + mirroring from the MMC1, loading only
    /// on change (cheap per-frame poll; the 8 KiB memcpy runs solely on a
    /// bank-switch frame). Out-of-range bank selects leave the slot
    /// unchanged (see module docs).
    pub fn sync_from_mapper(&mut self, chr_rom: &[u8], mmc1: &Mmc1) {
        let key = (mmc1.chr0, mmc1.chr1, mmc1.ctrl);
        if self.mapper_key == Some(key) {
            return;
        }
        self.mapper_key = Some(key);
        for slot in 0u8..2 {
            let bank = mmc1.map_chr4(slot);
            let off = bank.saturating_mul(CHR_BANK_LEN);
            // Exact-length range: `None` when the bank runs past the image
            // end (synthetic small-CHR fixtures); the slot keeps its old
            // contents in that case (documented gap).
            if let Some(data) = chr_rom.get(off..off.saturating_add(CHR_BANK_LEN)) {
                let _ = self
                    .ppu
                    .load_chr_4k(usize::from(slot), (bank & 0xFF) as u8, data);
            }
        }
        self.ppu.set_mirroring_mmc1(mmc1.ctrl);
    }

    /// Finish the frame at its end (line 239 done): renders every scanline
    /// the beam has not reached yet with the current state and returns the
    /// owned framebuffer for `Game::frame`. Latches the beam's sprite-0
    /// answer into [`PpuBind::last_render_hit`] (the model flag is cleared
    /// at the pre-render equivalent by `begin_frame`, so this is the
    /// post-frame witness of what the beam saw). Does not touch vblank
    /// (`Game::step` raises it at line 241).
    pub fn finish_frame(&mut self) -> z2_ppu::render::IndexedFrame {
        let frame = self.ppu.finish_frame();
        self.last_hit = self.ppu.sprite0_hit();
        frame
    }

    /// Whole-frame convenience for tests/wiring: [`PpuBind::finish_frame`]
    /// plus the vblank flag.
    pub fn render_frame(&mut self) -> z2_ppu::render::IndexedFrame {
        let frame = self.finish_frame();
        self.ppu.end_frame();
        frame
    }

    /// Sprite-0 hit observed by the last [`PpuBind::render_frame`].
    #[must_use]
    pub fn last_render_hit(&self) -> bool {
        self.last_hit
    }

    /// Hardware NMI edge at vblank start: the last render set vblank **and**
    /// the game armed NMI via `$2000` bit 7. `Game::step` latches
    /// `cpu.nmi_pending` only on this edge — never unconditionally, so
    /// NMI-disabled spans (boot warm-up, sound-only paths) stay quiet.
    #[must_use]
    pub fn nmi_edge(&self) -> bool {
        self.ppu.vblank() && self.ppu.nmi_enabled()
    }

    /// Raw vblank flag (for tests/wiring).
    #[must_use]
    pub fn vblank(&self) -> bool {
        self.ppu.vblank()
    }

    /// Effective sprite-0-hit flag: the [`PpuBind::force_sprite0_hit`]
    /// override when armed, else the renderer's flag.
    #[must_use]
    pub fn sprite0_hit(&self) -> bool {
        self.force_sprite0.unwrap_or_else(|| self.ppu.sprite0_hit())
    }

    /// Test hook: force `$2002` bit 6 to `on` regardless of what the
    /// renderer computed (unblocks boot bring-up when pattern/scroll/OAM
    /// contents cannot yet produce a natural hit). Off by default;
    /// [`PpuBind::clear_sprite0_override`] restores renderer-driven reads.
    pub fn force_sprite0_hit(&mut self, on: bool) {
        self.force_sprite0 = Some(on);
    }

    /// Restore renderer-driven sprite-0-hit reads (default state).
    pub fn clear_sprite0_override(&mut self) {
        self.force_sprite0 = None;
    }

    /// Current sprite-0 override (`None` = renderer-driven).
    #[must_use]
    pub fn sprite0_override(&self) -> Option<bool> {
        self.force_sprite0
    }

    /// Borrow PPU OAM (256 bytes) for the lockstep mirror feed.
    #[must_use]
    pub fn oam_bytes(&self) -> &[u8; 256] {
        self.ppu.oam()
    }

    /// Borrow PPU palette RAM (32 bytes) for the lockstep mirror feed.
    #[must_use]
    pub fn palette_bytes(&self) -> &[u8; 32] {
        self.ppu.palette()
    }

    /// Borrow the model (read-only wiring/tests).
    #[must_use]
    pub fn model(&self) -> &Ppu {
        &self.ppu
    }

    /// Borrow the model mutably (test setup: CHR fixtures, forced flags).
    pub fn model_mut(&mut self) -> &mut Ppu {
        &mut self.ppu
    }

    /// Allocation-free copy of every binding field plus the model's
    /// emulation state ([`Ppu::copy_state_from`]; trace and render record
    /// excluded) from `src` (save states, see [`crate::state`]).
    pub fn copy_state_from(&mut self, src: &PpuBind) {
        self.ppu.copy_state_from(&src.ppu);
        self.reads = src.reads;
        self.writes = src.writes;
        self.last_write = src.last_write;
        self.oam_addr = src.oam_addr;
        self.force_sprite0 = src.force_sprite0;
        self.mapper_key = src.mapper_key;
        self.last_hit = src.last_hit;
        self.origin_dots = src.origin_dots;
        self.prerender_dots = src.prerender_dots;
    }

    /// Serialize the binding fields followed by [`Ppu::write_state`].
    pub(crate) fn write_state(&self, w: &mut crate::state::Writer) {
        w.u64(self.reads);
        w.u64(self.writes);
        match self.last_write {
            Some((a, v)) => {
                w.u8(1);
                w.u16(a);
                w.u8(v);
            }
            None => w.bytes(&[0, 0, 0, 0]),
        }
        w.u8(self.oam_addr);
        w.u8(match self.force_sprite0 {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        });
        match self.mapper_key {
            Some((a, b, c)) => w.bytes(&[1, a, b, c]),
            None => w.bytes(&[0, 0, 0, 0]),
        }
        w.bool(self.last_hit);
        w.u64(self.origin_dots);
        w.u16(self.prerender_dots);
        self.ppu.write_state(w.vec());
    }

    /// Inverse of [`PpuBind::write_state`].
    pub(crate) fn read_state(
        &mut self,
        r: &mut crate::state::Reader,
    ) -> Result<(), crate::state::StateError> {
        use crate::state::StateError;
        let reads = r.u64()?;
        let writes = r.u64()?;
        let lw = (r.bool()?, r.u16()?, r.u8()?);
        let oam_addr = r.u8()?;
        let force = match r.u8()? {
            0 => None,
            1 => Some(false),
            2 => Some(true),
            _ => return Err(StateError::Invalid("sprite-0 override")),
        };
        let mk = (r.bool()?, r.u8()?, r.u8()?, r.u8()?);
        let last_hit = r.bool()?;
        let origin_dots = r.u64()?;
        let prerender_dots = r.u16()?;
        self.ppu
            .read_state(r.take(Ppu::STATE_BYTES)?)
            .map_err(StateError::Ppu)?;
        self.reads = reads;
        self.writes = writes;
        self.last_write = lw.0.then_some((lw.1, lw.2));
        self.oam_addr = oam_addr;
        self.force_sprite0 = force;
        self.mapper_key = mk.0.then_some((mk.1, mk.2, mk.3));
        self.last_hit = last_hit;
        self.origin_dots = origin_dots;
        self.prerender_dots = prerender_dots;
        Ok(())
    }
}

impl PpuBus for PpuBind {
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.reads += 1;
        match addr {
            0x2002 => {
                let mut s = self.ppu.read_status();
                // Test override wins over the renderer flag; vblank bit and
                // the read side effects (vblank clear, latch reset) are
                // always the model's.
                if let Some(force) = self.force_sprite0 {
                    if force {
                        s |= PPUSTATUS_SPRITE0;
                    } else {
                        s &= !PPUSTATUS_SPRITE0;
                    }
                }
                s
            }
            // OAMDATA readback via the shadow address (model has no OAM
            // read port); reads do not advance the address.
            0x2004 => self.ppu.oam()[self.oam_addr as usize],
            // Buffered VRAM / palette read.
            0x2007 => self.ppu.read_data(),
            // Write-only registers ($2000/$2001/$2003/$2005/$2006): open bus.
            _ => 0,
        }
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        self.writes += 1;
        self.last_write = Some((addr, val));
        match addr {
            0x2000 => self.ppu.write_ctrl(val),
            0x2001 => self.ppu.write_mask(val),
            // $2002 is read-only on hardware: count the traffic, drop it.
            0x2002 => {}
            0x2003 => {
                self.oam_addr = val;
                self.ppu.write_oam_addr(val);
            }
            0x2004 => {
                self.ppu.write_oam_data(val);
                self.oam_addr = self.oam_addr.wrapping_add(1);
            }
            0x2005 => self.ppu.write_scroll(val),
            0x2006 => self.ppu.write_addr(val),
            0x2007 => self.ppu.write_data(val),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z2_ppu::PPUSTATUS_VBLANK;

    #[test]
    fn fresh_binding_reports_no_vblank() {
        let mut b = PpuBind::new();
        // Hardware-like power-on: vblank clear until the first frame ends.
        assert_eq!(b.ppu_read(0x2002) & PPUSTATUS_VBLANK, 0);
        assert!(!b.vblank());
    }

    #[test]
    fn status_read_sets_then_clears_vblank() {
        let mut b = PpuBind::new();
        b.model_mut().end_frame();
        assert!(b.vblank());
        assert_ne!(b.ppu_read(0x2002) & PPUSTATUS_VBLANK, 0);
        assert!(!b.vblank(), "$2002 read clears vblank");
        assert_eq!(b.ppu_read(0x2002) & PPUSTATUS_VBLANK, 0);
    }

    #[test]
    fn sprite0_override_wins_over_renderer() {
        let mut b = PpuBind::new();
        assert_eq!(b.ppu_read(0x2002) & PPUSTATUS_SPRITE0, 0);
        b.force_sprite0_hit(true);
        assert_ne!(b.ppu_read(0x2002) & PPUSTATUS_SPRITE0, 0);
        assert!(b.sprite0_hit());
        b.force_sprite0_hit(false);
        b.model_mut().end_frame();
        // Forced clear holds even with vblank set; vblank bit unaffected.
        let s = b.ppu_read(0x2002);
        assert_eq!(s & PPUSTATUS_SPRITE0, 0);
        assert_ne!(s & PPUSTATUS_VBLANK, 0);
        b.clear_sprite0_override();
        assert_eq!(b.sprite0_override(), None);
    }
}
