//! NES noise channel: 15/6-bit LFSR, envelope, length counter.
//!
//! Modelled per nesdev `APU Noise`: a timer clocked every CPU cycle drives a
//! 15-bit linear-feedback shift register. Mode flag (`$400E` bit 7) selects
//! the feedback tap (bit 1 = long/15-bit, bit 6 = short/93-step loop).
//! Output is the envelope volume when LFSR bit 0 is clear, else 0.

use crate::tables::{LENGTH_TABLE, NOISE_PERIOD};

/// Noise voice.
#[derive(Debug, Clone)]
pub struct Noise {
    /// Length-counter halt / envelope loop (`$400C` bit 5).
    halt: bool,
    /// Constant-volume flag (`$400C` bit 4).
    constant: bool,
    /// Volume / envelope period (`$400C` bits 3..0).
    volume: u8,
    /// Envelope state.
    env_start: bool,
    env_divider: u8,
    env_decay: u8,
    /// Short-mode flag (`$400E` bit 7): feedback tap bit 6 instead of bit 1.
    short_mode: bool,
    /// Timer period index (`$400E` bits 3..0).
    period_idx: u8,
    /// Timer divider.
    timer: u16,
    /// 15-bit shift register (never all-zero in hardware; init 1).
    lsr: u16,
    /// Length counter (0 = silenced).
    length: u8,
    /// Channel enabled via `$4015` bit 3.
    enabled: bool,
}

impl Noise {
    /// Create a silenced noise voice (LSR = 1, per hardware power-on).
    #[must_use]
    pub fn new() -> Self {
        Self {
            halt: false,
            constant: false,
            volume: 0,
            env_start: false,
            env_divider: 0,
            env_decay: 0,
            short_mode: false,
            period_idx: 0,
            timer: 0,
            lsr: 1,
            length: 0,
            enabled: false,
        }
    }

    /// `$400C`: `--LCVVVV` (loop/halt, constant, volume).
    pub fn write_control(&mut self, v: u8) {
        self.halt = v & 0x20 != 0;
        self.constant = v & 0x10 != 0;
        self.volume = v & 0x0F;
    }

    /// `$400E`: `M---PPPP` (mode, period index). Low 2 bits are unused.
    pub fn write_mode_period(&mut self, v: u8) {
        self.short_mode = v & 0x80 != 0;
        self.period_idx = v & 0x0F;
    }

    /// `$400F`: length index (bits 7..3). Reloads length (when enabled)
    /// and restarts the envelope.
    pub fn write_length(&mut self, v: u8) {
        if self.enabled {
            self.length = LENGTH_TABLE[usize::from(v >> 3)];
        }
        self.env_start = true;
    }

    /// `$4015` enable bit.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.length = 0;
        }
    }

    /// Current timer period in CPU cycles.
    #[must_use]
    pub fn timer_period(&self) -> u16 {
        NOISE_PERIOD[usize::from(self.period_idx)]
    }

    /// Advance the timer + LFSR; call once per CPU cycle.
    pub fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.timer_period();
            let tap = if self.short_mode { 6 } else { 1 };
            let feedback = (self.lsr & 1) ^ ((self.lsr >> tap) & 1);
            self.lsr = (self.lsr >> 1) | (feedback << 14);
        } else {
            self.timer -= 1;
        }
    }

    /// Advance the envelope; call on frame-counter quarter frames.
    pub fn clock_envelope(&mut self) {
        if self.env_start {
            self.env_start = false;
            self.env_decay = 15;
            self.env_divider = self.volume;
        } else if self.env_divider == 0 {
            self.env_divider = self.volume;
            if self.env_decay > 0 {
                self.env_decay -= 1;
            } else if self.halt {
                self.env_decay = 15;
            }
        } else {
            self.env_divider -= 1;
        }
    }

    /// Advance the length counter; call on frame-counter half frames.
    pub fn clock_length(&mut self) {
        if !self.halt && self.length > 0 {
            self.length -= 1;
        }
    }

    /// Current 4-bit output (0..15). Zero when length expired or LFSR
    /// bit 0 is set.
    #[must_use]
    pub fn output(&self) -> u8 {
        if self.length == 0 || (self.lsr & 1) != 0 {
            return 0;
        }
        if self.constant {
            self.volume
        } else {
            self.env_decay
        }
    }

    /// Length counter value (for `$4015` reads / tests).
    #[must_use]
    pub fn length(&self) -> u8 {
        self.length
    }

    /// Raw shift-register value (for tests).
    #[must_use]
    pub fn shift_register(&self) -> u16 {
        self.lsr
    }

    /// Short (93-step) mode flag (for tests).
    #[must_use]
    pub fn short_mode(&self) -> bool {
        self.short_mode
    }
}

impl Default for Noise {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_noise() -> Noise {
        let mut n = Noise::new();
        n.set_enabled(true);
        n.write_control(0x3F); // halt + constant vol 15
        n.write_mode_period(0x00); // long mode, fastest
        n.write_length(0xF8); // length idx 31 -> 30
        n
    }

    #[test]
    fn period_table_spot_checks_through_channel() {
        let mut n = Noise::new();
        n.write_mode_period(0x00);
        assert_eq!(n.timer_period(), 4);
        n.write_mode_period(0x0F);
        assert_eq!(n.timer_period(), 4068);
        n.write_mode_period(0x05);
        assert_eq!(n.timer_period(), 96);
    }

    #[test]
    fn long_mode_cycles_32767_steps_before_repeat() {
        // 15-bit maximal LFSR: visits every non-zero state exactly once.
        let mut seen = std::collections::HashSet::new();
        let mut n = live_noise();
        for _ in 0..32767 {
            // One LFSR step per (period + 1) clocks; period 4 here.
            for _ in 0..5 {
                n.clock_timer();
            }
            assert!(seen.insert(n.shift_register()), "LFSR repeated early");
        }
        assert_eq!(n.shift_register(), 1, "maximal cycle returns to seed");
    }

    #[test]
    fn short_mode_taps_bit6() {
        let mut a = live_noise(); // long
        let mut b = live_noise();
        b.write_mode_period(0x80); // short, same rate
        assert!(b.short_mode());
        // First step from seed 1: long feedback = 1^0 = 1 -> 0x4001;
        // short feedback = 1^0 = 1 -> 0x4001 (same here) — diverge later.
        let mut diverged = false;
        for _ in 0..200 {
            for _ in 0..5 {
                a.clock_timer();
                b.clock_timer();
            }
            if a.shift_register() != b.shift_register() {
                diverged = true;
                break;
            }
        }
        assert!(diverged, "short mode must diverge from long mode");
    }

    #[test]
    fn output_gated_by_lsr_bit0_and_length() {
        let mut n = live_noise();
        // Seed 1: bit 0 set -> silent even with live length.
        assert_eq!(n.output(), 0);
        for _ in 0..5 {
            n.clock_timer();
        }
        // After one step LSR = 0x4000, bit 0 clear -> audible at vol 15.
        assert_eq!(n.output(), 15);
        n.set_enabled(false);
        assert_eq!(n.output(), 0);
    }

    #[test]
    fn length_halt_freezes_countdown() {
        let mut n = live_noise(); // halt set via 0x1F
        let start = n.length();
        for _ in 0..5 {
            n.clock_length();
        }
        assert_eq!(n.length(), start);
        n.write_control(0x1F);
        n.clock_length();
        assert_eq!(n.length(), start - 1);
    }
}
