//! NES pulse channel (x2): envelope, sweep, duty, length counter.
//!
//! Modelled per nesdev `APU Pulse`: an 8-step duty sequencer clocked by an
//! 11-bit timer (every 2 CPU cycles), a 4-bit volume/envelope unit, a sweep
//! unit, and an 8-bit length counter shared with the halt flag (bit 5 of
//! `$4000/$4004`, which doubles as the envelope loop flag).

use crate::tables::{LENGTH_TABLE, PULSE_DUTY};

/// One pulse voice. `is_first` selects pulse-1 sweep-negate behaviour
/// (one's-complement subtracts an extra 1; see [`Pulse::sweep_target`]).
#[derive(Debug, Clone)]
pub struct Pulse {
    is_first: bool,
    /// Timer period (11 bits from `$4002/$4003`).
    period: u16,
    /// Timer divider countdown.
    timer: u16,
    /// Duty mode 0..3 (`$400x` bits 7..6).
    duty: u8,
    /// Sequencer position 0..7.
    step: u8,
    /// Length counter (0 = silenced).
    length: u8,
    /// Length-counter halt / envelope loop (`$400x` bit 5).
    halt: bool,
    /// Constant-volume flag (`$400x` bit 4).
    constant: bool,
    /// Volume / envelope period (`$400x` bits 3..0).
    volume: u8,
    /// Envelope state.
    env_start: bool,
    env_divider: u8,
    env_decay: u8,
    /// Sweep state (`$4001/$4005`).
    sweep_enabled: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_divider: u8,
    sweep_reload: bool,
    /// Channel enabled via `$4015` bits 0/1.
    enabled: bool,
}

impl Pulse {
    /// Create a pulse voice; `is_first` must be true for pulse 1.
    #[must_use]
    pub fn new(is_first: bool) -> Self {
        Self {
            is_first,
            period: 0,
            timer: 0,
            duty: 0,
            step: 0,
            length: 0,
            halt: false,
            constant: false,
            volume: 0,
            env_start: false,
            env_divider: 0,
            env_decay: 0,
            sweep_enabled: false,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            sweep_divider: 0,
            sweep_reload: false,
            enabled: false,
        }
    }

    /// `$4000/$4004`: `DDLCVVVV`.
    pub fn write_control(&mut self, v: u8) {
        self.duty = (v >> 6) & 3;
        self.halt = v & 0x20 != 0;
        self.constant = v & 0x10 != 0;
        self.volume = v & 0x0F;
    }

    /// `$4001/$4005`: `EPPPNSSS` (enable, period, negate, shift).
    pub fn write_sweep(&mut self, v: u8) {
        self.sweep_enabled = v & 0x80 != 0;
        self.sweep_period = (v >> 4) & 7;
        self.sweep_negate = v & 0x08 != 0;
        self.sweep_shift = v & 0x07;
        self.sweep_reload = true;
    }

    /// `$4002/$4006`: timer low 8 bits.
    pub fn write_timer_low(&mut self, v: u8) {
        self.period = (self.period & 0x700) | u16::from(v);
    }

    /// `$4003/$4007`: length index (bits 7..3) + timer high (bits 2..0).
    /// Reloads length (when enabled) and restarts the envelope.
    pub fn write_timer_high(&mut self, v: u8) {
        self.period = (self.period & 0x0FF) | (u16::from(v & 0x07) << 8);
        if self.enabled {
            self.length = LENGTH_TABLE[usize::from(v >> 3)];
        }
        self.step = 0;
        self.env_start = true;
    }

    /// `$4015` enable bit.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.length = 0;
        }
    }

    /// Advance the timer; call once every **2** CPU cycles.
    pub fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.period;
            self.step = (self.step + 1) & 7;
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

    /// Advance the sweep divider; call on frame-counter half frames.
    pub fn clock_sweep(&mut self) {
        if self.sweep_divider == 0 && self.sweep_enabled && self.sweep_shift > 0 {
            if let Some(target) = self.sweep_target() {
                if !self.muted(Some(target)) {
                    self.period = target;
                }
            }
        }
        if self.sweep_divider == 0 || self.sweep_reload {
            self.sweep_divider = self.sweep_period;
            self.sweep_reload = false;
        } else {
            self.sweep_divider -= 1;
        }
    }

    /// Sweep target period, or `None` when the computation underflows.
    /// Pulse 1 (one's complement) subtracts an extra 1 when negating.
    fn sweep_target(&self) -> Option<u16> {
        let change = self.period >> self.sweep_shift;
        if self.sweep_negate {
            let extra = u16::from(self.is_first);
            self.period.checked_sub(change)?.checked_sub(extra)
        } else {
            self.period.checked_add(change)
        }
    }

    /// True when the sweep unit silences the channel: period < 8
    /// (unconditional), or the sweep target overflows 11 bits (or
    /// underflowed). The overflow mute only applies when the shifter is
    /// active (`shift > 0`): with shift 0 the target would trivially read
    /// `2 * period`, and hardware plays those notes fine (games routinely
    /// park `$4001 = $00`).
    fn muted(&self, target: Option<u16>) -> bool {
        if self.period < 8 {
            return true;
        }
        if self.sweep_shift == 0 {
            return false;
        }
        match target {
            None => true,
            Some(t) => t > 0x7FF,
        }
    }

    /// Current 4-bit output (0..15). Zero when the length counter expired,
    /// the duty step is low, or the sweep unit mutes the channel.
    #[must_use]
    pub fn output(&self) -> u8 {
        if self.length == 0 {
            return 0;
        }
        if self.muted(self.sweep_target()) {
            return 0;
        }
        if PULSE_DUTY[usize::from(self.duty)][usize::from(self.step)] == 0 {
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

    /// Current timer period (for sweep-mute / test inspection).
    #[must_use]
    pub fn period(&self) -> u16 {
        self.period
    }

    /// Current envelope decay level (for tests).
    #[must_use]
    pub fn envelope_level(&self) -> u8 {
        self.env_decay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice_const_vol() -> Pulse {
        let mut p = Pulse::new(true);
        p.set_enabled(true);
        p.write_control(0x1F); // duty 0, halt, constant vol 15
        p.write_timer_low(0xFF);
        p.write_timer_high(0x07); // length idx 0 -> 10
        p
    }

    #[test]
    fn duty_patterns_emit_expected_high_steps() {
        // Duty 0 (12.5%): exactly one high step per 8 timer clocks.
        for duty in 0..4u8 {
            let mut p = voice_const_vol();
            p.write_control(0x10 | (duty << 6) | 0x0F);
            // Fastest non-muted period: one sequencer step per 9 clocks.
            p.write_timer_low(8);
            p.write_timer_high(0x08); // high 0, keeps length live
            let mut high = 0;
            for _ in 0..8 {
                for _ in 0..9 {
                    p.clock_timer();
                }
                if p.output() == 15 {
                    high += 1;
                }
            }
            assert_eq!(high, [1, 2, 4, 6][duty as usize], "duty {duty}");
        }
    }

    #[test]
    fn envelope_decays_and_holds_at_zero_without_loop() {
        let mut p = Pulse::new(true);
        p.set_enabled(true);
        p.write_control(0x00); // duty 0, no halt/loop, env period 0
        p.write_timer_high(0x08); // length idx 1 -> 254
        assert_eq!(p.length(), 254);
        p.clock_envelope(); // start: decay = 15
        assert_eq!(p.envelope_level(), 15);
        for _ in 0..15 {
            p.clock_envelope();
        }
        assert_eq!(p.envelope_level(), 0);
        p.clock_envelope();
        assert_eq!(p.envelope_level(), 0, "holds at zero without loop");
    }

    #[test]
    fn envelope_loops_when_halt_set() {
        let mut p = Pulse::new(true);
        p.set_enabled(true);
        p.write_control(0x20); // halt = loop, env period 0
        p.write_timer_high(0xF8);
        p.clock_envelope();
        for _ in 0..16 {
            p.clock_envelope();
        }
        assert_eq!(p.envelope_level(), 15, "loops back to 15 with halt set");
    }

    #[test]
    fn sweep_mutes_when_period_below_8() {
        let mut p = voice_const_vol();
        p.write_timer_low(0x05); // period 0x705? no: set small period
        p.write_control(0x1F);
        // Force period < 8 via low/high writes.
        p.write_timer_low(0x05);
        p.write_timer_high(0x08); // high 0, length idx 1
        assert!(p.period() < 8);
        // Even with constant volume and live length, output is muted.
        assert_eq!(p.output(), 0);
    }

    #[test]
    fn sweep_negate_differs_between_pulse1_and_pulse2() {
        // period 16, shift 1, negate: pulse1 -> 16-8-1 = 7 (mute: <8),
        // pulse2 -> 16-8 = 8 (audible).
        let mut p1 = Pulse::new(true);
        let mut p2 = Pulse::new(false);
        for p in [&mut p1, &mut p2] {
            p.set_enabled(true);
            p.write_control(0x1F);
            p.write_timer_low(16);
            p.write_timer_high(0xF8);
            p.write_sweep(0x89); // enabled, period 0, negate, shift 1
        }
        p1.clock_sweep();
        p2.clock_sweep();
        assert_eq!(p1.period(), 7, "pulse1 negate subtracts extra 1");
        assert_eq!(p2.period(), 8, "pulse2 negate has no extra subtract");
        // Duty 0 idles on step 0: advance both to step 1 before sampling.
        p1.clock_timer();
        p2.clock_timer();
        assert_eq!(p1.output(), 0, "pulse1 muted after sweep to 7");
        assert_eq!(p2.output(), 15, "pulse2 still audible at period 8");
    }

    #[test]
    fn length_counter_halt_freezes_countdown() {
        let mut p = Pulse::new(true);
        p.set_enabled(true);
        p.write_control(0x3F); // halt set, constant vol
        p.write_timer_high(0x08); // length idx 1 -> 254
        assert_eq!(p.length(), 254);
        for _ in 0..10 {
            p.clock_length();
        }
        assert_eq!(p.length(), 254, "halted length never counts down");
        p.write_control(0x1F); // clear halt
        p.clock_length();
        assert_eq!(p.length(), 253);
    }

    #[test]
    fn disable_clears_length() {
        let mut p = voice_const_vol();
        assert!(p.length() > 0);
        p.set_enabled(false);
        assert_eq!(p.length(), 0);
        assert_eq!(p.output(), 0);
    }
}
