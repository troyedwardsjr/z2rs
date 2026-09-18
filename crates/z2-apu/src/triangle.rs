//! NES triangle channel: linear counter + length counter.
//!
//! Modelled per nesdev `APU Triangle`: a 32-step waveform sequencer clocked
//! by an 11-bit timer (every CPU cycle), gated by the length counter and a
//! linear counter reloaded from `$4008`. Bit 7 of `$4008` (control) doubles
//! as the length-counter halt flag.

use crate::tables::{LENGTH_TABLE, TRIANGLE_SEQ};

/// Triangle voice.
#[derive(Debug, Clone)]
pub struct Triangle {
    /// Control / length-halt flag (`$4008` bit 7).
    control: bool,
    /// Linear-counter reload value (`$4008` bits 6..0).
    linear_reload: u8,
    /// Current linear counter.
    linear: u8,
    /// Linear reload flag (set by `$400B` writes).
    reload_flag: bool,
    /// Length counter (0 = silenced).
    length: u8,
    /// Timer period (11 bits).
    period: u16,
    /// Timer divider.
    timer: u16,
    /// Waveform position 0..31.
    step: u8,
    /// Channel enabled via `$4015` bit 2.
    enabled: bool,
}

impl Triangle {
    /// Create a silenced triangle voice.
    #[must_use]
    pub fn new() -> Self {
        Self {
            control: false,
            linear_reload: 0,
            linear: 0,
            reload_flag: false,
            length: 0,
            period: 0,
            timer: 0,
            step: 0,
            enabled: false,
        }
    }

    /// `$4008`: `CRRRRRRR` (control, linear reload value).
    pub fn write_control(&mut self, v: u8) {
        self.control = v & 0x80 != 0;
        self.linear_reload = v & 0x7F;
    }

    /// `$400A`: timer low 8 bits.
    pub fn write_timer_low(&mut self, v: u8) {
        self.period = (self.period & 0x700) | u16::from(v);
    }

    /// `$400B`: length index (bits 7..3) + timer high (bits 2..0).
    /// Reloads length (when enabled) and sets the linear reload flag.
    pub fn write_timer_high(&mut self, v: u8) {
        self.period = (self.period & 0x0FF) | (u16::from(v & 0x07) << 8);
        if self.enabled {
            self.length = LENGTH_TABLE[usize::from(v >> 3)];
        }
        self.reload_flag = true;
    }

    /// `$4015` enable bit.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.length = 0;
        }
    }

    /// Advance the linear counter; call on frame-counter quarter frames.
    pub fn clock_linear(&mut self) {
        if self.reload_flag {
            self.linear = self.linear_reload;
        } else if self.linear > 0 {
            self.linear -= 1;
        }
        if !self.control {
            self.reload_flag = false;
        }
    }

    /// Advance the length counter; call on frame-counter half frames.
    pub fn clock_length(&mut self) {
        if !self.control && self.length > 0 {
            self.length -= 1;
        }
    }

    /// Advance the waveform timer; call once per CPU cycle.
    pub fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            self.step = (self.step + 1) & 31;
        } else {
            self.timer -= 1;
        }
    }

    /// Current 4-bit output (0..15). Zero when length or linear counter is 0.
    #[must_use]
    pub fn output(&self) -> u8 {
        if self.length == 0 || self.linear == 0 {
            0
        } else {
            TRIANGLE_SEQ[usize::from(self.step)]
        }
    }

    /// Length counter value (for `$4015` reads / tests).
    #[must_use]
    pub fn length(&self) -> u8 {
        self.length
    }

    /// Linear counter value (for tests).
    #[must_use]
    pub fn linear(&self) -> u8 {
        self.linear
    }

    /// Waveform position (for tests).
    #[must_use]
    pub fn step(&self) -> u8 {
        self.step
    }
}

impl Default for Triangle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_triangle() -> Triangle {
        let mut t = Triangle::new();
        t.set_enabled(true);
        t.write_control(0x8F); // control set, linear reload 15
        t.write_timer_low(0xFF);
        t.write_timer_high(0xF8); // max length, reload flag set
        t.clock_linear(); // linear = 15
        t
    }

    #[test]
    fn linear_counter_reloads_then_counts_down() {
        let mut t = live_triangle();
        assert_eq!(t.linear(), 15);
        // Control set: reload flag stays, so every clock reloads.
        t.clock_linear();
        assert_eq!(t.linear(), 15);
        // Clear control: flag clears after this clock, then counts down.
        t.write_control(0x0F);
        t.clock_linear(); // reload (flag was set), flag now clears
        assert_eq!(t.linear(), 15);
        t.clock_linear();
        assert_eq!(t.linear(), 14);
    }

    #[test]
    fn control_flag_halts_length_counter() {
        let mut t = live_triangle();
        let start = t.length();
        assert!(start > 0);
        for _ in 0..5 {
            t.clock_length();
        }
        assert_eq!(t.length(), start, "control set: length frozen");
        t.write_control(0x0F); // clear control
        t.clock_length();
        assert_eq!(t.length(), start - 1);
    }

    #[test]
    fn output_follows_32_step_sequence() {
        let mut t = live_triangle();
        t.write_timer_low(0x00);
        t.write_timer_high(0xF8); // reloads length, sets reload flag
        t.set_enabled(true);
        t.write_timer_low(0x02); // fast period (2) for stepping the test
                                 // Step the timer until it wraps and check the waveform advances.
        let first = t.output();
        // period 2 -> timer wraps every 3 clocks; run 3 clocks -> step +1.
        for _ in 0..3 {
            t.clock_timer();
        }
        assert_eq!(t.step(), 1);
        assert_eq!(t.output(), 14);
        assert_eq!(first, 15);
    }

    #[test]
    fn silenced_when_linear_or_length_zero() {
        let mut t = Triangle::new();
        t.set_enabled(true);
        t.write_control(0x0F); // no control, linear reload 15
        t.write_timer_low(0x20);
        t.write_timer_high(0xF8); // length live, reload flag set
        assert_eq!(t.output(), 0, "linear not yet clocked");
        t.clock_linear();
        assert_ne!(t.output(), 0);
        t.set_enabled(false);
        assert_eq!(t.output(), 0, "disable clears length");
    }
}
