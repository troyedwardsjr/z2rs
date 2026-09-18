//! NES APU frame counter: 4-step / 5-step sequencer.
//!
//! Modelled per nesdev `APU Frame Counter`. The sequencer runs off CPU
//! cycles (NTSC): quarter-frame ticks (envelopes, triangle linear counter)
//! and half-frame ticks (length counters, sweeps). Mode 0 (4-step) raises
//! the frame IRQ on the last step unless inhibited; mode 1 (5-step) never
//! interrupts.

/// CPU cycles per NTSC video frame (`1789773 / 60`, rounded).
pub const FRAME_CPU_CYCLES: u64 = 29_830;

/// Step boundaries (cumulative CPU cycles) for 4-step mode.
/// Steps land at ~7457-cycle intervals; the frame wraps at 29830.
const MODE0_STEPS: [u64; 4] = [7457, 14913, 22371, 29829];

/// Step boundaries for 5-step mode (wraps at 37282, ~1.2 frames).
const MODE1_STEPS: [u64; 5] = [7457, 14913, 22371, 29829, 37281];

/// What a single CPU-cycle step of the sequencer produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStep {
    /// Quarter-frame tick: clock envelopes + triangle linear counter.
    pub quarter: bool,
    /// Half-frame tick: additionally clock lengths + sweeps.
    pub half: bool,
    /// Frame IRQ edge (mode 0 last step, interrupts not inhibited).
    pub irq: bool,
}

/// Frame counter (`$4017` + `$4015` IRQ flag).
#[derive(Debug, Clone)]
pub struct FrameCounter {
    /// True = 5-step mode (`$4017` bit 7).
    five_step: bool,
    /// IRQ inhibit (`$4017` bit 6).
    inhibit: bool,
    /// Cycles since the last reset.
    cycle: u64,
    /// Step index already fired.
    step_idx: usize,
    /// Latched frame-IRQ flag (cleared by `$4015` reads / inhibit writes).
    irq_flag: bool,
}

impl FrameCounter {
    /// Create a power-on frame counter (4-step mode, no inhibit).
    #[must_use]
    pub fn new() -> Self {
        Self {
            five_step: false,
            inhibit: false,
            cycle: 0,
            step_idx: 0,
            irq_flag: false,
        }
    }

    /// `$4017` write: `MI------` (mode, IRQ inhibit).
    ///
    /// Resets the divider. Returns true when the caller must immediately
    /// clock quarter- **and** half-frame units: hardware clocks them at
    /// once when switching into 5-step mode (nesdev). All other writes
    /// just reset; the 2-cycle write delay is intentionally not modelled
    /// (documented approximation for the oracle log comparison).
    pub fn write(&mut self, v: u8) -> bool {
        self.five_step = v & 0x80 != 0;
        self.inhibit = v & 0x40 != 0;
        if self.inhibit {
            self.irq_flag = false;
        }
        self.cycle = 0;
        self.step_idx = 0;
        self.five_step
    }

    /// Advance one CPU cycle.
    pub fn step(&mut self) -> FrameStep {
        self.cycle += 1;
        let (bounds, last): (&[u64], usize) = if self.five_step {
            (&MODE1_STEPS, 4)
        } else {
            (&MODE0_STEPS, 3)
        };
        let mut out = FrameStep::default();
        if self.step_idx < bounds.len() && self.cycle >= bounds[self.step_idx] {
            let idx = self.step_idx;
            self.step_idx += 1;
            out.quarter = true;
            // Mode 0: length/sweep on steps 2 and 4; mode 1: steps 2 and 5.
            out.half = idx == 1 || idx == last;
            if !self.five_step && idx == last && !self.inhibit {
                self.irq_flag = true;
                out.irq = true;
            }
            if self.step_idx >= bounds.len() {
                self.cycle = 0;
                self.step_idx = 0;
            }
        }
        out
    }

    /// Latched frame-IRQ flag (drives `$4015` bit 6 when not inhibited).
    #[must_use]
    pub fn irq_flag(&self) -> bool {
        self.irq_flag && !self.inhibit
    }

    /// Clear the latched IRQ flag (on `$4015` reads).
    pub fn clear_irq(&mut self) {
        self.irq_flag = false;
    }

    /// 5-step mode flag (for tests).
    #[must_use]
    pub fn five_step(&self) -> bool {
        self.five_step
    }
}

impl Default for FrameCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: &mut FrameCounter, n: u64) -> (u32, u32, u32) {
        let (mut q, mut h, mut i) = (0, 0, 0);
        for _ in 0..n {
            let s = f.step();
            q += u32::from(s.quarter);
            h += u32::from(s.half);
            i += u32::from(s.irq);
        }
        (q, h, i)
    }

    #[test]
    fn four_step_timing_matches_nesdev() {
        let mut f = FrameCounter::new();
        assert!(!f.five_step());
        // Step edges: quarter ticks at 7457/14913/22371/29829.
        let (q, h, i) = run(&mut f, 7456);
        assert_eq!((q, h, i), (0, 0, 0));
        let (q, h, i) = run(&mut f, 1);
        assert_eq!((q, h, i), (1, 0, 0), "step 1: envelope only");
        let (q, h, i) = run(&mut f, 7456);
        assert_eq!((q, h, i), (1, 1, 0), "step 2: envelope + length");
        let (q, h, i) = run(&mut f, 7458);
        assert_eq!((q, h, i), (1, 0, 0), "step 3: envelope only");
        let (q, h, i) = run(&mut f, 7458);
        assert_eq!((q, h, i), (1, 1, 1), "step 4: envelope + length + IRQ");
        assert!(f.irq_flag());
        // Next frame restarts cleanly.
        let (q, _, _) = run(&mut f, 7457);
        assert_eq!(q, 1);
    }

    #[test]
    fn five_step_timing_has_no_irq_and_extra_step() {
        let mut f = FrameCounter::new();
        assert!(f.write(0x80), "entering 5-step requests immediate clocks");
        let (q, h, i) = run(&mut f, 37281);
        assert_eq!((q, h, i), (5, 2, 0), "5 quarters, 2 halves, no IRQ");
        assert!(!f.irq_flag());
    }

    #[test]
    fn inhibit_blocks_irq_and_clears_flag() {
        let mut f = FrameCounter::new();
        run(&mut f, 29829);
        assert!(f.irq_flag());
        f.write(0x40); // inhibit set: clears flag
        assert!(!f.irq_flag());
        run(&mut f, 29830);
        assert!(!f.irq_flag(), "inhibited: no IRQ latched");
    }

    #[test]
    fn mode_switch_resets_divider() {
        let mut f = FrameCounter::new();
        run(&mut f, 7000);
        f.write(0x00); // rewrite 4-step: divider resets
        let (q, _, _) = run(&mut f, 7456);
        assert_eq!(q, 0, "counter restarted by the write");
        let (q, _, _) = run(&mut f, 1);
        assert_eq!(q, 1);
    }

    #[test]
    fn frame_length_is_one_ntsc_frame() {
        assert_eq!(FRAME_CPU_CYCLES, 29_830);
        let mut f = FrameCounter::new();
        let (q, h, i) = run(&mut f, FRAME_CPU_CYCLES);
        assert_eq!((q, h), (4, 2), "one frame = 4 quarters + 2 halves");
        assert_eq!(i, 1);
    }
}
