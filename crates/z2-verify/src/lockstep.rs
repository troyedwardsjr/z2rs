//! First-divergence lockstep comparator.
//!
//! [`Lockstep::run`] steps an [`Oracle`] and a [`Port`] side by side on the
//! same input track and reports the first byte that disagrees. Compare order
//! per frame is fixed (cheapest, most-diagnostic regions first):
//!
//! 1. zero page + stack (`$0000-$01FF`),
//! 2. main RAM (`$0200-$07FF`),
//! 3. WRAM (`$6000-$7FFF`),
//! 4. OAM (256 bytes),
//! 5. palette RAM (32 bytes),
//! 6. framebuffer (61 440 indexed pixels; FNV-1a fast path, then a linear
//!    scan for the first differing pixel so the report still carries an
//!    exact address).
//!
//! [`Divergence::last_writer`] is `None` until the interpreter's trap log feeds it; the
//! field exists now so the struct shape is already final.
//!
//! # Stack page and the oracle's column 255
//!
//! Two deliberate, documented relaxations (both switchable):
//!
//! * **Dead stack bytes.** Bytes *below* the stack pointer are stale
//!   return addresses and register pushes; they carry no game state and
//!   differ whenever the NMI lands one instruction earlier or later than on
//!   the oracle (the port is frame/scanline-timed, not cycle-exact). By
//!   default the zero page compares fully and the stack page only from
//!   `$0100 + SP + 1` up (both sides' SP must agree, and the live bytes
//!   must match). [`CompareOpts::strict_stack`] compares the whole page.
//! * **Overscan edge pixels the oracle mis-renders.** tetanes-core 0.15
//!   never draws a sprite's pixel in the rightmost column (its sprite
//!   cover table is filled for dots `x+1..min(x+9, 256)`, so dot 256 —
//!   pixel 255 — is never covered, and its background pixel there comes
//!   out wrong too), and it draws sprites on scanline 0 (a sprite with OAM
//!   Y `$00` shows on line 0, where hardware — whose sprite evaluation
//!   skips the pre-render line — shows nothing). This model follows the
//!   hardware rules, so framebuffer mismatches confined to column 255 and
//!   row 0 are not reported by default ([`CompareOpts::strict_edges`]
//!   restores exact comparison). Both edges sit in the overscan a CRT
//!   never shows.

use std::fmt;

use crate::oracle::{Oracle, Port, FRAME_PIXELS};

// ---------------------------------------------------------------------------
// Regions.
// ---------------------------------------------------------------------------

/// Which observable region diverged first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// CPU RAM `$0000-$01FF` (zero page + stack).
    ZeroPageStack,
    /// CPU RAM `$0200-$07FF`.
    MainRam,
    /// Cart WRAM `$6000-$7FFF`.
    Wram,
    /// Primary OAM.
    Oam,
    /// Palette RAM `$3F00-$3F1F`.
    Palette,
    /// Indexed framebuffer.
    Frame,
}

impl Region {
    /// Stable short name for logs and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Region::ZeroPageStack => "zp+stack",
            Region::MainRam => "ram",
            Region::Wram => "wram",
            Region::Oam => "oam",
            Region::Palette => "palette",
            Region::Frame => "frame",
        }
    }

    /// How to read [`Divergence::addr`] for this region: CPU/PPU address for
    /// memory regions, byte index for OAM and the framebuffer.
    pub fn addr_space(self) -> &'static str {
        match self {
            Region::ZeroPageStack | Region::MainRam => "cpu $0000-$07FF",
            Region::Wram => "cpu $6000-$7FFF",
            Region::Oam => "oam byte index",
            Region::Palette => "ppu $3F00-$3F1F",
            Region::Frame => "framebuffer pixel index",
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Divergence.
// ---------------------------------------------------------------------------

/// First byte at which the two sides disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    /// Frame index (0-based) at which the divergence was observed, i.e. the
    /// `step` count at which the post-step states first differed.
    pub frame: u64,
    /// Which region diverged (compare order is fixed; see module docs).
    pub region: Region,
    /// Byte address: CPU address for RAM/WRAM, PPU address for palette,
    /// byte index for OAM and the framebuffer (see [`Region::addr_space`]).
    pub addr: u32,
    /// Oracle (reference) byte.
    pub expected: u8,
    /// DUT (device-under-test) byte.
    pub actual: u8,
    /// Producing write, once the interpreter's trap log feeds it. `None` for now.
    pub last_writer: Option<String>,
}

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "frame {}: {} {:#06X} ({}) expected {:#04X} actual {:#04X}",
            self.frame,
            self.region,
            self.addr,
            self.region.addr_space(),
            self.expected,
            self.actual,
        )?;
        if let Some(w) = &self.last_writer {
            write!(f, " last_writer={w}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Divergence {}

// ---------------------------------------------------------------------------
// Frame hashing.
// ---------------------------------------------------------------------------

/// FNV-1a (64-bit) over an indexed framebuffer.
///
/// Used as a fast path: hash both sides, and only linear-scan for the exact
/// pixel when the hashes differ. Pure function of the bytes, so both sides
/// must agree whenever their pixels agree.
pub fn frame_hash(fb: &[u8; FRAME_PIXELS]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut h = OFFSET;
    for &b in fb.iter() {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

// ---------------------------------------------------------------------------
// Lockstep runner.
// ---------------------------------------------------------------------------

/// Comparator options (see the module docs for the two relaxations);
/// the default is both relaxations on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompareOpts {
    /// Compare the whole stack page, not just the live part.
    pub strict_stack: bool,
    /// Report framebuffer mismatches confined to column 255 / row 0.
    pub strict_edges: bool,
}

/// Lockstep verification runner.
pub struct Lockstep;

impl Lockstep {
    /// Step `oracle` (reference) and `dut` (device under test) through
    /// `frames` frames on `inputs`, comparing after every frame.
    ///
    /// * `inputs[f]` is the pad byte for frame `f`; when the track is
    ///   shorter than `frames`, missing frames run with input `0x00`
    ///   (no buttons). A longer track's tail is ignored.
    /// * Returns `Ok(())` when all `frames` frames agree, else the first
    ///   [`Divergence`] in compare order (zero page + stack, `$0200-$07FF`,
    ///   WRAM, OAM, palette, framebuffer).
    /// * `last_writer` is always `None` until the interpreter's trap log feeds it.
    pub fn run(
        oracle: &mut impl Oracle,
        dut: &mut impl Port,
        inputs: &[u8],
        frames: usize,
    ) -> Result<(), Divergence> {
        Self::run_with(oracle, dut, inputs, frames, CompareOpts::default())
    }

    /// [`Lockstep::run`] with explicit comparator options.
    pub fn run_with(
        oracle: &mut impl Oracle,
        dut: &mut impl Port,
        inputs: &[u8],
        frames: usize,
        opts: CompareOpts,
    ) -> Result<(), Divergence> {
        for f in 0..frames {
            let input = inputs.get(f).copied().unwrap_or(0x00);
            oracle.step(input);
            dut.step(input);
            if let Some(d) = Self::compare_with(oracle, dut, f as u64, opts) {
                return Err(d);
            }
        }
        Ok(())
    }

    /// First differing byte of the zero page + stack region under `opts`:
    /// zero page fully, then the stack page from the live stack top (or the
    /// whole page when strict or when either side hides its SP). A stack
    /// pointer disagreement reports at `$0100 + min(sp) + 1`.
    fn first_diff_zp_stack(
        oracle: &impl Port,
        dut: &impl Port,
        opts: CompareOpts,
    ) -> Option<(usize, u8, u8)> {
        if let Some((i, e, a)) = first_diff(&oracle.ram()[..0x100], &dut.ram()[..0x100]) {
            return Some((i, e, a));
        }
        let (lo, hi) = (&oracle.ram()[0x100..0x200], &dut.ram()[0x100..0x200]);
        let live_from = match (opts.strict_stack, oracle.sp(), dut.sp()) {
            (false, Some(o), Some(d)) => {
                if o != d {
                    let at = usize::from(o.min(d)) + 1;
                    let at = at.min(0xFF);
                    return Some((0x100 + at, lo[at], hi[at]));
                }
                usize::from(o) + 1
            }
            _ => 0,
        };
        let live_from = live_from.min(0x100);
        first_diff(&lo[live_from..], &hi[live_from..])
            .map(|(i, e, a)| (0x100 + live_from + i, e, a))
    }

    /// First differing framebuffer pixel under `opts` (column 255 and row
    /// 0 skipped unless strict — see module docs).
    fn first_diff_frame(
        oracle: &impl Port,
        dut: &impl Port,
        opts: CompareOpts,
    ) -> Option<(usize, u8, u8)> {
        let (a, b) = (oracle.frame_indexed(), dut.frame_indexed());
        if frame_hash(a) == frame_hash(b) {
            return None;
        }
        if opts.strict_edges {
            return first_diff(a, b);
        }
        a.iter()
            .zip(b.iter())
            .enumerate()
            .find(|(i, (x, y))| x != y && i % 256 != 255 && *i >= 256)
            .map(|(i, (&x, &y))| (i, x, y))
    }

    /// Compare every region independently (no first-mismatch short-circuit):
    /// one [`Divergence`] per disagreeing region, in compare order. Used by
    /// the `--continue` diagnostic driver; the verification gate stays
    /// [`Lockstep::run`].
    pub fn compare_all(oracle: &impl Port, dut: &impl Port, frame: u64) -> Vec<Divergence> {
        Self::compare_all_with(oracle, dut, frame, CompareOpts::default())
    }

    /// [`Lockstep::compare_all`] with explicit comparator options.
    pub fn compare_all_with(
        oracle: &impl Port,
        dut: &impl Port,
        frame: u64,
        opts: CompareOpts,
    ) -> Vec<Divergence> {
        let mut out = Vec::new();
        let mk = |region, addr, e, a| Divergence {
            frame,
            region,
            addr,
            expected: e,
            actual: a,
            last_writer: None,
        };
        if let Some((i, e, a)) = Self::first_diff_zp_stack(oracle, dut, opts) {
            out.push(mk(Region::ZeroPageStack, i as u32, e, a));
        }
        if let Some((i, e, a)) = first_diff(&oracle.ram()[0x200..], &dut.ram()[0x200..]) {
            out.push(mk(Region::MainRam, 0x200 + i as u32, e, a));
        }
        if let Some((i, e, a)) = first_diff(oracle.wram(), dut.wram()) {
            out.push(mk(Region::Wram, 0x6000 + i as u32, e, a));
        }
        if let Some((i, e, a)) = first_diff(oracle.oam(), dut.oam()) {
            out.push(mk(Region::Oam, i as u32, e, a));
        }
        if let Some((i, e, a)) = first_diff(oracle.palette(), dut.palette()) {
            out.push(mk(Region::Palette, 0x3F00 + i as u32, e, a));
        }
        if let Some((i, e, a)) = Self::first_diff_frame(oracle, dut, opts) {
            out.push(mk(Region::Frame, i as u32, e, a));
        }
        out
    }

    /// Number of differing bytes between two equal-length slices.
    pub fn count_diff(a: &[u8], b: &[u8]) -> usize {
        a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
    }

    /// Compare post-step state; `None` when the sides agree.
    fn compare_with(
        oracle: &impl Port,
        dut: &impl Port,
        frame: u64,
        opts: CompareOpts,
    ) -> Option<Divergence> {
        // 1. Zero page + stack (live part by default; see module docs).
        if let Some((i, e, a)) = Self::first_diff_zp_stack(oracle, dut, opts) {
            return Some(Divergence {
                frame,
                region: Region::ZeroPageStack,
                addr: i as u32,
                expected: e,
                actual: a,
                last_writer: None,
            });
        }
        // 2. Rest of CPU RAM.
        if let Some((i, e, a)) = first_diff(&oracle.ram()[0x200..], &dut.ram()[0x200..]) {
            return Some(Divergence {
                frame,
                region: Region::MainRam,
                addr: 0x200 + i as u32,
                expected: e,
                actual: a,
                last_writer: None,
            });
        }
        // 3. WRAM.
        if let Some((i, e, a)) = first_diff(oracle.wram(), dut.wram()) {
            return Some(Divergence {
                frame,
                region: Region::Wram,
                addr: 0x6000 + i as u32,
                expected: e,
                actual: a,
                last_writer: None,
            });
        }
        // 4. OAM.
        if let Some((i, e, a)) = first_diff(oracle.oam(), dut.oam()) {
            return Some(Divergence {
                frame,
                region: Region::Oam,
                addr: i as u32,
                expected: e,
                actual: a,
                last_writer: None,
            });
        }
        // 5. Palette.
        if let Some((i, e, a)) = first_diff(oracle.palette(), dut.palette()) {
            return Some(Divergence {
                frame,
                region: Region::Palette,
                addr: 0x3F00 + i as u32,
                expected: e,
                actual: a,
                last_writer: None,
            });
        }
        // 6. Framebuffer: hash fast path, then exact scan (column 255 and
        // row 0 skipped unless strict — see module docs).
        if let Some((i, e, a)) = Self::first_diff_frame(oracle, dut, opts) {
            return Some(Divergence {
                frame,
                region: Region::Frame,
                addr: i as u32,
                expected: e,
                actual: a,
                last_writer: None,
            });
        }
        None
    }
}

/// First index at which `a` and `b` differ.
fn first_diff(a: &[u8], b: &[u8]) -> Option<(usize, u8, u8)> {
    debug_assert_eq!(a.len(), b.len());
    for (i, (&x, &y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            return Some((i, x, y));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::StubPort;

    fn pair() -> (StubPort, StubPort) {
        (StubPort::new(), StubPort::new())
    }

    #[test]
    fn identical_stubs_agree() {
        let (mut o, mut d) = pair();
        let inputs: Vec<u8> = (0..500).map(|i| (i as u8).wrapping_mul(37)).collect();
        assert!(Lockstep::run(&mut o, &mut d, &inputs, 500).is_ok());
    }

    #[test]
    fn short_track_pads_with_zero() {
        let (mut o, mut d) = pair();
        assert!(Lockstep::run(&mut o, &mut d, &[], 64).is_ok());
        assert!(Lockstep::run(&mut o, &mut d, &[0xFF; 4], 64).is_ok());
    }

    #[test]
    fn zero_frames_is_vacuous() {
        let (mut o, mut d) = pair();
        d.corrupt_ram(0x456, 0x99);
        assert!(Lockstep::run(&mut o, &mut d, &[], 0).is_ok());
    }

    #[test]
    fn ram_corruption_reported_at_exact_addr() {
        let (mut o, mut d) = pair();
        d.corrupt_ram(0x456, 0xA5);
        let err = Lockstep::run(&mut o, &mut d, &[0x11; 8], 8).unwrap_err();
        assert_eq!(err.frame, 0);
        assert_eq!(err.region, Region::MainRam);
        assert_eq!(err.addr, 0x456);
        assert_eq!(err.expected, o.ram()[0x456]);
        assert_eq!(err.actual, d.ram()[0x456]);
        // The stub step adds the same delta to both sides, so the injected
        // fault survives verbatim as the difference.
        assert_eq!(err.actual.wrapping_sub(err.expected), 0xA5);
        assert_eq!(err.last_writer, None);
    }

    #[test]
    fn zp_stack_region_split() {
        let (mut o, mut d) = pair();
        d.corrupt_ram(0x0042, 0x7E);
        let err = Lockstep::run(&mut o, &mut d, &[], 4).unwrap_err();
        assert_eq!(err.region, Region::ZeroPageStack);
        assert_eq!(err.addr, 0x42);
    }

    #[test]
    fn compare_order_prefers_earlier_region() {
        let (mut o, mut d) = pair();
        d.corrupt_palette(3, 0x1C);
        d.corrupt_ram(0x700, 0x01);
        let err = Lockstep::run(&mut o, &mut d, &[], 2).unwrap_err();
        assert_eq!(err.region, Region::MainRam);
    }

    #[test]
    fn wram_oam_palette_frame_regions() {
        let (mut o, mut d) = pair();
        d.corrupt_wram(0x1234, 0x5A);
        let err = Lockstep::run(&mut o, &mut d, &[], 1).unwrap_err();
        assert_eq!((err.region, err.addr), (Region::Wram, 0x6000 + 0x1234));

        let (mut o, mut d) = pair();
        d.corrupt_oam(200, 0x09);
        let err = Lockstep::run(&mut o, &mut d, &[], 1).unwrap_err();
        assert_eq!((err.region, err.addr), (Region::Oam, 200));

        let (mut o, mut d) = pair();
        d.corrupt_palette(17, 0x2D);
        let err = Lockstep::run(&mut o, &mut d, &[], 1).unwrap_err();
        assert_eq!((err.region, err.addr), (Region::Palette, 0x3F00 + 17));

        let (mut o, mut d) = pair();
        d.corrupt_frame(12345, 0x07);
        let err = Lockstep::run(&mut o, &mut d, &[], 1).unwrap_err();
        assert_eq!((err.region, err.addr), (Region::Frame, 12345));
    }

    #[test]
    fn divergence_display_is_greppable() {
        let d = Divergence {
            frame: 12,
            region: Region::Wram,
            addr: 0x6123,
            expected: 0x01,
            actual: 0x02,
            last_writer: None,
        };
        let s = d.to_string();
        assert!(s.contains("frame 12") && s.contains("wram") && s.contains("0x6123"));
    }

    #[test]
    fn frame_hash_stable_and_sensitive() {
        let (mut o, mut d) = pair();
        o.step(0x83);
        d.step(0x83);
        assert_eq!(frame_hash(o.frame_indexed()), frame_hash(d.frame_indexed()));
        d.corrupt_frame(0, 0xFF);
        assert_ne!(frame_hash(o.frame_indexed()), frame_hash(d.frame_indexed()));
    }
}
