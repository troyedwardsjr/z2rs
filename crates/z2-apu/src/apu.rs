//! NES APU top level: register interface, nonlinear mixer, resampler.
//!
//! [`Apu`] owns the five voices ([`Pulse`] x2, [`Triangle`], [`Noise`],
//! [`Dmc`]) plus the [`FrameCounter`], and exposes the hardware register
//! interface (`$4000-$4017` writes, `$4015` status reads) that the bank 6
//! sound engine drives (see [`crate::engine`]).
//!
//! ## Two clocking contracts (pick one, do not mix)
//!
//! * **Standalone / `Game::audio` path** (the simpler one to wire):
//!   the engine calls [`Engine::tick_frame`](crate::engine::Engine::tick_frame)
//!   once per video frame to emit `$4000-$4017` writes, then
//!   [`Apu::audio`] advances all timers by exactly one NTSC frame
//!   ([`FRAME_CPU_CYCLES`]) and appends the resampled PCM.
//! * **Cycle-driven path** (future full CPU integration):
//!   call [`Apu::clock_cpu_cycle`] once per CPU cycle and pull PCM with
//!   [`Apu::drain_samples`]. [`Apu::audio`] must not be used on top.
//!
//! ## Resampler
//!
//! Voices render one mixer sample per CPU cycle (1.789773 MHz). An
//! exact-rational fixed-point accumulator maps CPU cycles to output
//! samples: `due = cycles_total * rate / CPU_HZ` (all integer math, no
//! drift), averaging the raw mixer values in between (box filter). Per
//! frame this yields 735 +/- 1 samples at 44.1 kHz (800 +/- 1 at 48 kHz);
//! the frontend FIFO (see [`crate::audio`]) absorbs the jitter.

use crate::audio::sample_rate_supported;
use crate::dmc::{Dmc, DmcSource, SilentSource};
use crate::frame::{FrameCounter, FRAME_CPU_CYCLES};
use crate::noise::Noise;
use crate::pulse::Pulse;
use crate::reglog::RegLog;
use crate::tables::{pulse_mix_table, tnd_mix_table};
use crate::triangle::Triangle;

/// NES CPU clock (NTSC) in Hz.
pub const CPU_HZ: u64 = 1_789_773;

/// Output gain applied after the DC-blocking high-pass filter.
pub const OUTPUT_GAIN: f32 = 1.0;

/// High-pass cutoff in Hz (hardware-flavoured DC blocker).
pub const HPF_CUTOFF_HZ: f32 = 90.0;

/// Lowest / highest supported PCM output rate in Hz.
pub const MIN_SAMPLE_RATE: u32 = 8_000;
/// Lowest / highest supported PCM output rate in Hz.
pub const MAX_SAMPLE_RATE: u32 = 192_000;

/// NES APU: five voices + frame counter + mixer + resampler.
///
/// Created with [`Apu::new`]; driven through [`Apu::write_reg`] /
/// [`Apu::read_status`] and rendered with [`Apu::audio`].
pub struct Apu {
    pulse1: Pulse,
    pulse2: Pulse,
    tri: Triangle,
    noise: Noise,
    dmc: Dmc,
    frame: FrameCounter,
    pulse_mix: [f32; 31],
    tnd_mix: [f32; 203],
    sample_rate: u32,
    // Exact-rational resampler state.
    cycles_total: u64,
    samples_emitted: u64,
    mix_acc: f64,
    mix_count: u64,
    // DC-blocking high-pass filter state.
    hpf_alpha: f32,
    hpf_prev_in: f32,
    hpf_prev_out: f32,
    // Rendered, unconsumed PCM.
    fifo: Vec<i16>,
    dmc_source: Box<dyn DmcSource>,
    frames_rendered: u64,
    cycle: u64,
}

impl Apu {
    /// Create an APU rendering PCM at `sample_rate` Hz (44100 / 48000 are
    /// the supported game rates; anything in
    /// `MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE` works).
    ///
    /// Unknown rates fall back to 44100 Hz.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let rate = if (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            sample_rate
        } else {
            44_100
        };
        Self {
            pulse1: Pulse::new(true),
            pulse2: Pulse::new(false),
            tri: Triangle::new(),
            noise: Noise::new(),
            dmc: Dmc::new(),
            frame: FrameCounter::new(),
            pulse_mix: pulse_mix_table(),
            tnd_mix: tnd_mix_table(),
            sample_rate: rate,
            cycles_total: 0,
            samples_emitted: 0,
            mix_acc: 0.0,
            mix_count: 0,
            hpf_alpha: hpf_alpha(rate),
            hpf_prev_in: 0.0,
            hpf_prev_out: 0.0,
            fifo: Vec::new(),
            dmc_source: Box::new(SilentSource),
            frames_rendered: 0,
            cycle: 0,
        }
    }

    /// Change the PCM output rate (same range rules as [`Apu::new`]).
    /// Returns the rate actually selected. Fractional resampler phase is
    /// preserved, so switching mid-stream does not click beyond the
    /// unavoidable filter-state transient.
    pub fn set_sample_rate(&mut self, rate: u32) -> u32 {
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate) || !sample_rate_supported(rate) {
            return self.sample_rate;
        }
        self.sample_rate = rate;
        self.hpf_alpha = hpf_alpha(rate);
        rate
    }

    /// Current PCM output rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Install the DMC sample-byte source (assets-backed at runtime).
    /// Until installed, the silent source is used (DMC plays zeros).
    pub fn install_dmc_source(&mut self, src: Box<dyn DmcSource>) {
        self.dmc_source = src;
    }

    /// Write an APU register (`$4000-$4017`). Writes outside that range
    /// are ignored (including `$4014` OAMDMA, which the PPU/DMA owns).
    pub fn write_reg(&mut self, addr: u16, value: u8) {
        match addr {
            0x4000 => self.pulse1.write_control(value),
            0x4001 => self.pulse1.write_sweep(value),
            0x4002 => self.pulse1.write_timer_low(value),
            0x4003 => self.pulse1.write_timer_high(value),
            0x4004 => self.pulse2.write_control(value),
            0x4005 => self.pulse2.write_sweep(value),
            0x4006 => self.pulse2.write_timer_low(value),
            0x4007 => self.pulse2.write_timer_high(value),
            0x4008 => self.tri.write_control(value),
            // $4009 is unused.
            0x400A => self.tri.write_timer_low(value),
            0x400B => self.tri.write_timer_high(value),
            0x400C => self.noise.write_control(value),
            // $400D is unused.
            0x400E => self.noise.write_mode_period(value),
            0x400F => self.noise.write_length(value),
            0x4010 => self.dmc.write_control(value),
            0x4011 => self.dmc.write_output(value),
            0x4012 => self.dmc.write_address(value),
            0x4013 => self.dmc.write_length(value),
            // $4014: OAMDMA — not ours, ignore.
            0x4015 => self.write_status(value),
            // Entering 5-step mode clocks quarter + half units at once.
            0x4017 if self.frame.write(value) => {
                self.clock_quarter();
                self.clock_half();
            }
            0x4017 => {}
            _ => {}
        }
    }

    /// [`Apu::write_reg`] plus a [`RegLog`] record (oracle verification).
    pub fn write_reg_logged(
        &mut self,
        addr: u16,
        value: u8,
        frame_no: u32,
        cycle: u64,
        log: &mut RegLog,
    ) {
        log.record(frame_no, cycle, addr, value);
        self.write_reg(addr, value);
    }

    /// `$4015` status read. Bit 6 (frame IRQ) is cleared by the read;
    /// bit 7 (DMC IRQ) is not.
    #[must_use]
    pub fn read_status(&mut self) -> u8 {
        let mut s = 0u8;
        if self.pulse1.length() > 0 {
            s |= 0x01;
        }
        if self.pulse2.length() > 0 {
            s |= 0x02;
        }
        if self.tri.length() > 0 {
            s |= 0x04;
        }
        if self.noise.length() > 0 {
            s |= 0x08;
        }
        if self.dmc.active() {
            s |= 0x10;
        }
        if self.frame.irq_flag() {
            s |= 0x40;
        }
        if self.dmc.irq() {
            s |= 0x80;
        }
        self.frame.clear_irq();
        s
    }

    /// True when either interrupt source is asserting (for the CPU's IRQ
    /// line; wired by the CPU core).
    #[must_use]
    pub fn irq_pending(&self) -> bool {
        self.frame.irq_flag() || self.dmc.irq()
    }

    /// `$4015` write: channel enables. Disabling a length-counter voice
    /// clears its length; disabling DMC stops playback and clears its IRQ.
    fn write_status(&mut self, v: u8) {
        self.pulse1.set_enabled(v & 0x01 != 0);
        self.pulse2.set_enabled(v & 0x02 != 0);
        self.tri.set_enabled(v & 0x04 != 0);
        self.noise.set_enabled(v & 0x08 != 0);
        self.dmc.set_enabled(v & 0x10 != 0);
    }

    /// Advance the APU by one CPU cycle, feeding the resampler.
    /// Cycle-driven integration path (see module docs).
    pub fn clock_cpu_cycle(&mut self) {
        self.cycle += 1;
        // Voice timers.
        if self.cycle & 1 == 0 {
            self.pulse1.clock_timer();
            self.pulse2.clock_timer();
        }
        self.tri.clock_timer();
        self.noise.clock_timer();
        let Self {
            dmc, dmc_source, ..
        } = self;
        dmc.clock(&mut **dmc_source);
        // Frame sequencer.
        let step = self.frame.step();
        if step.quarter {
            self.clock_quarter();
        }
        if step.half {
            self.clock_half();
        }
        self.push_mix_sample();
    }

    /// Render one NTSC frame of CPU cycles into the sample FIFO.
    pub fn render_frame_cycles(&mut self) {
        for _ in 0..FRAME_CPU_CYCLES {
            self.clock_cpu_cycle();
        }
        self.frames_rendered += 1;
    }

    /// Move rendered PCM out of the FIFO (cycle-driven path).
    pub fn drain_samples(&mut self, out: &mut Vec<i16>) {
        out.extend_from_slice(&self.fifo);
        self.fifo.clear();
    }

    /// **Game audio API.** Advance the APU by exactly one video frame and
    /// append that frame's PCM (nominally `rate/60` samples: 735 at
    /// 44100 Hz, 800 at 48000 Hz, +/- 1 from resampler phase) to `out`.
    ///
    /// Call once per emulated video frame, after the engine's register
    /// writes for that frame. `out` is appended to, never cleared, so the
    /// frontend can accumulate a 2-frame buffer (see [`crate::audio`]).
    pub fn audio(&mut self, out: &mut Vec<i16>) {
        self.render_frame_cycles();
        self.drain_samples(out);
    }

    /// Frames rendered via [`Apu::render_frame_cycles`] / [`Apu::audio`].
    #[must_use]
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// Total PCM samples emitted since construction.
    #[must_use]
    pub fn samples_rendered(&self) -> u64 {
        self.samples_emitted
    }

    /// Unconsumed samples sitting in the FIFO.
    #[must_use]
    pub fn fifo_depth(&self) -> usize {
        self.fifo.len()
    }

    /// Current pre-filter mixer voltage in `[0, ~1]` (for tests / meters).
    #[must_use]
    pub fn mix_voltage(&self) -> f32 {
        let p = self.pulse1.output() + self.pulse2.output();
        let t = 3 * self.tri.output() + 2 * self.noise.output() + self.dmc.output();
        self.pulse_mix[usize::from(p)] + self.tnd_mix[usize::from(t)]
    }

    fn clock_quarter(&mut self) {
        self.pulse1.clock_envelope();
        self.pulse2.clock_envelope();
        self.tri.clock_linear();
        self.noise.clock_envelope();
    }

    fn clock_half(&mut self) {
        self.pulse1.clock_length();
        self.pulse2.clock_length();
        self.tri.clock_length();
        self.noise.clock_length();
        self.pulse1.clock_sweep();
        self.pulse2.clock_sweep();
    }

    /// Mix the voices, feed the exact-rational resampler, high-pass and
    /// emit `i16` samples as they come due.
    fn push_mix_sample(&mut self) {
        self.cycles_total += 1;
        self.mix_acc += f64::from(self.mix_voltage());
        self.mix_count += 1;
        let rate = u64::from(self.sample_rate);
        let due = (self.cycles_total * rate) / CPU_HZ;
        while self.samples_emitted < due {
            let avg = (self.mix_acc / self.mix_count as f64) as f32;
            self.mix_acc = 0.0;
            self.mix_count = 0;
            self.samples_emitted += 1;
            // DC-blocking high-pass (hardware-flavoured), then gain + clamp.
            let y = self.hpf_alpha * (self.hpf_prev_out + avg - self.hpf_prev_in);
            self.hpf_prev_in = avg;
            self.hpf_prev_out = y;
            let v = (y * OUTPUT_GAIN * 32767.0).round();
            self.fifo.push(v.clamp(-32768.0, 32767.0) as i16);
        }
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new(44_100)
    }
}

/// One-pole high-pass coefficient for `cutoff` at `rate`.
fn hpf_alpha(rate: u32) -> f32 {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * HPF_CUTOFF_HZ);
    let dt = 1.0 / rate as f32;
    rc / (rc + dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All voices at full-scale: exercises the mixer top end.
    fn maxed_apu() -> Apu {
        let mut a = Apu::new(44_100);
        // Pulse 1: const vol 15, period 0x7FF (no sweep mute), max length.
        a.write_reg(0x4015, 0x0F);
        a.write_reg(0x4000, 0xDF); // duty 3, const 15
        a.write_reg(0x4001, 0x00); // sweep disabled
        a.write_reg(0x4002, 0xFF);
        a.write_reg(0x4003, 0xFF);
        a.write_reg(0x4004, 0xDF);
        a.write_reg(0x4005, 0x00);
        a.write_reg(0x4006, 0xFF);
        a.write_reg(0x4007, 0xFF);
        // Triangle: control + max linear, max length.
        a.write_reg(0x4008, 0xFF);
        a.write_reg(0x400A, 0xFF);
        a.write_reg(0x400B, 0xFF);
        a.tri.clock_linear();
        // Noise: const 15, fastest, max length; step LFSR off bit-0-set.
        a.write_reg(0x400C, 0x1F);
        a.write_reg(0x400E, 0x00);
        a.write_reg(0x400F, 0xFF);
        for _ in 0..5 {
            a.noise.clock_timer();
        }
        // DMC: direct max level.
        a.write_reg(0x4011, 0x7F);
        a
    }

    #[test]
    fn mixer_full_scale_stays_in_bounds() {
        let a = maxed_apu();
        assert_eq!(a.pulse1.output(), 15);
        assert_eq!(a.pulse2.output(), 15);
        assert_eq!(a.tri.output(), 15);
        assert_eq!(a.noise.output(), 15);
        assert_eq!(a.dmc.output(), 127);
        let v = a.mix_voltage();
        assert!(v > 0.99 && v <= 1.0, "full-scale mix = {v}");
    }

    #[test]
    fn pcm_output_never_clips() {
        let mut a = maxed_apu();
        let mut out = Vec::new();
        a.audio(&mut out); // HPF transient is the hottest moment
        assert!(!out.is_empty());
        assert!(out
            .iter()
            .all(|&s| i32::from(s) <= 32767 && i32::from(s) >= -32768));
        assert!(out.iter().any(|&s| s > 10_000), "full-scale must be loud");
    }

    #[test]
    fn silence_renders_near_zero_after_filter_settles() {
        let mut a = Apu::new(44_100); // power-on = all silent
        let mut out = Vec::new();
        a.audio(&mut out);
        a.audio(&mut out);
        assert!(!out.is_empty());
        let tail = &out[out.len() - 100..];
        assert!(tail.iter().all(|&s| s.abs() < 50), "settled silence ≈ 0");
    }

    #[test]
    fn frame_sample_counts_match_nominal_rates() {
        for (rate, nominal) in [(44_100, 735), (48_000, 800)] {
            let mut a = Apu::new(rate);
            let mut out = Vec::new();
            a.audio(&mut out);
            let n = out.len() as i64;
            assert!(
                (n - nominal).abs() <= 1,
                "rate {rate}: got {n}, want {nominal}±1"
            );
        }
    }

    #[test]
    fn long_run_sample_total_matches_wall_clock() {
        for (rate, nominal) in [(44_100u32, 735u64), (48_000, 800)] {
            let mut a = Apu::new(rate);
            let mut out = Vec::new();
            for _ in 0..600 {
                a.audio(&mut out);
            }
            let want = 600 * nominal;
            let got = out.len() as u64;
            assert!(
                got.abs_diff(want) <= 600,
                "rate {rate}: {got} vs nominal {want} over 600 frames"
            );
        }
    }

    #[test]
    fn register_interface_read_write_roundtrip() {
        let mut a = Apu::new(44_100);
        assert_eq!(a.read_status() & 0x0F, 0, "power-on lengths are 0");
        a.write_reg(0x4015, 0x0F);
        a.write_reg(0x4000, 0x1F);
        a.write_reg(0x4002, 0xFF);
        a.write_reg(0x4003, 0xF8); // length idx 31 -> 30
        assert_ne!(a.read_status() & 0x01, 0, "pulse1 length live");
        a.write_reg(0x4015, 0x00);
        assert_eq!(a.read_status() & 0x0F, 0, "disable clears lengths");
        // Unmapped / DMA writes are harmless no-ops.
        a.write_reg(0x4009, 0xFF);
        a.write_reg(0x4014, 0x02);
        a.write_reg(0x5000, 0xFF);
    }

    #[test]
    fn status_read_clears_frame_irq_but_not_dmc_irq() {
        let mut a = Apu::new(44_100);
        a.write_reg(0x4017, 0x00); // 4-step, IRQ allowed
        let mut out = Vec::new();
        a.audio(&mut out); // one frame -> frame IRQ latched
        assert_ne!(a.read_status() & 0x40, 0, "frame IRQ set after a frame");
        assert_eq!(a.read_status() & 0x40, 0, "read clears the flag");
        assert!(!a.irq_pending());
    }

    #[test]
    fn five_step_mode_never_raises_frame_irq() {
        let mut a = Apu::new(44_100);
        a.write_reg(0x4017, 0x80);
        let mut out = Vec::new();
        for _ in 0..3 {
            a.audio(&mut out);
        }
        assert_eq!(a.read_status() & 0x40, 0);
    }

    #[test]
    fn sample_rate_selection_and_rejection() {
        let mut a = Apu::new(123); // out of range -> 44100 fallback
        assert_eq!(a.sample_rate(), 44_100);
        assert_eq!(a.set_sample_rate(48_000), 48_000);
        // Non-game rates are rejected (the synth only ships 44.1/48 kHz
        // paths); the current rate is kept.
        assert_eq!(a.set_sample_rate(22_050), 48_000);
        assert_eq!(a.sample_rate(), 48_000);
    }
}
