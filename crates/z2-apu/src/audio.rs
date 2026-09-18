//! Frontend PCM buffer: sizing notes + underrun counter.
//!
//! ## Buffer-sizing note (acceptance: no crackle at 60 fps, 2-frame buffer)
//!
//! At 60 fps the APU produces a nominal `rate/60` samples per video frame
//! (735 at 44100 Hz, 800 at 48000 Hz, +/- 1 from resampler phase — see
//! [`crate::apu`]). The frontend should hold **two frames** of PCM:
//!
//! * capacity = `2 * nominal_frame_samples(rate) + SLACK` where
//!   [`BUFFER_SLACK_SAMPLES`] (16) covers resampler jitter;
//! * the emulator thread pushes one [`Apu::audio`](crate::apu::Apu::audio)
//!   frame per vblank; the audio callback consumes at the device rate;
//! * any callback read that arrives before its samples do is an
//!   **underrun**: [`PcmFifo::consume`] pads with the last sample (not
//!   zeros — zero-padding clicks) and counts the padded samples in
//!   [`PcmFifo::underruns`]. A healthy 60 fps run keeps this at 0; a
//!   rising counter means the emulator thread is starving the callback.
//!
//! The native frontend wires: `Apu` -> `PcmFifo` -> audio callback.
//! Web does the same into an `AudioWorklet` ring; the counts and
//! capacities here apply unchanged.

use std::collections::VecDeque;

/// Game PCM rates shipped by the synth.
pub const RATE_44100: u32 = 44_100;
/// Game PCM rates shipped by the synth.
pub const RATE_48000: u32 = 48_000;

/// True for the two shipped game rates.
#[must_use]
pub const fn sample_rate_supported(rate: u32) -> bool {
    rate == RATE_44100 || rate == RATE_48000
}

/// Nominal PCM samples per NTSC video frame at `rate` (rounded).
#[must_use]
pub const fn nominal_frame_samples(rate: u32) -> usize {
    ((rate + 30) / 60) as usize
}

/// Extra capacity (samples) covering resampler +/-1 jitter over 2 frames.
pub const BUFFER_SLACK_SAMPLES: usize = 16;

/// Recommended frontend FIFO capacity for the 2-frame buffer at `rate`.
#[must_use]
pub const fn two_frame_buffer_samples(rate: u32) -> usize {
    2 * nominal_frame_samples(rate) + BUFFER_SLACK_SAMPLES
}

/// Frontend PCM FIFO with an underrun counter.
///
/// Single-threaded by design: the emulator thread pushes whole frames,
/// the audio callback consumes device-sized chunks. (Native will wrap
/// this in a mutex / lock-free ring at the threading boundary.)
#[derive(Debug, Clone, Default)]
pub struct PcmFifo {
    buf: VecDeque<i16>,
    /// Last sample seen (for padding across fully-drained calls).
    last: i16,
    /// Samples the callback asked for that were not available (padded).
    underruns: u64,
    /// Total samples ever pushed.
    pushed: u64,
    /// Total samples ever returned (including padded).
    popped: u64,
}

impl PcmFifo {
    /// Create an empty FIFO.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one rendered frame (or any chunk) into the FIFO.
    pub fn push_frame(&mut self, samples: &[i16]) {
        if let Some(&tail) = samples.last() {
            self.last = tail;
        }
        self.buf.extend(samples.iter().copied());
        self.pushed += samples.len() as u64;
    }

    /// Consume exactly `n` samples into `out`.
    ///
    /// When fewer than `n` are available, the remainder is padded with
    /// the last sample heard (0 before the first push — not mid-stream
    /// zeros, which click) and each padded sample increments
    /// [`PcmFifo::underruns`].
    pub fn consume(&mut self, n: usize, out: &mut Vec<i16>) {
        for _ in 0..n {
            match self.buf.pop_front() {
                Some(s) => {
                    self.last = s;
                    out.push(s);
                }
                None => {
                    out.push(self.last);
                    self.underruns += 1;
                }
            }
        }
        self.popped += n as u64;
    }

    /// Buffered (unconsumed) sample count.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.buf.len()
    }

    /// Lifetime underrun-sample count. Must stay 0 in a healthy 60 fps run.
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Total samples pushed / popped (popped includes padded).
    #[must_use]
    pub fn totals(&self) -> (u64, u64) {
        (self.pushed, self.popped)
    }

    /// Drop all buffered samples (on seek / reset, not on underrun).
    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal_frame_sizes() {
        assert_eq!(nominal_frame_samples(RATE_44100), 735);
        assert_eq!(nominal_frame_samples(RATE_48000), 800);
        assert_eq!(two_frame_buffer_samples(RATE_44100), 2 * 735 + 16);
        assert_eq!(two_frame_buffer_samples(RATE_48000), 2 * 800 + 16);
        assert!(sample_rate_supported(44_100) && sample_rate_supported(48_000));
        assert!(!sample_rate_supported(22_050));
    }

    #[test]
    fn two_frame_buffer_absorbs_resampler_jitter() {
        // Worst case: two consecutive +1-jitter frames still fit.
        let cap = two_frame_buffer_samples(RATE_44100);
        assert!(cap >= 2 * (735 + 1));
        let cap48 = two_frame_buffer_samples(RATE_48000);
        assert!(cap48 >= 2 * (800 + 1));
    }

    #[test]
    fn healthy_run_has_no_underruns() {
        let mut f = PcmFifo::new();
        let frame = vec![100i16; 735];
        let mut out = Vec::new();
        for _ in 0..120 {
            f.push_frame(&frame); // emulator: 1 frame per vblank
            f.consume(735, &mut out); // callback: 1 frame per tick
        }
        assert_eq!(f.underruns(), 0);
        assert_eq!(f.depth(), 0);
        assert_eq!(f.totals(), (120 * 735, 120 * 735));
    }

    #[test]
    fn starved_callback_pads_and_counts_underruns() {
        let mut f = PcmFifo::new();
        f.push_frame(&[7i16, 8, 9]);
        let mut out = Vec::new();
        f.consume(5, &mut out);
        assert_eq!(out, vec![7, 8, 9, 9, 9], "pads with last sample");
        assert_eq!(f.underruns(), 2);
        // Empty FIFO pads with 0.
        let mut out2 = Vec::new();
        f.consume(2, &mut out2);
        assert_eq!(out2, vec![9, 9], "remembers last sample across calls");
        assert_eq!(f.underruns(), 4);
    }

    #[test]
    fn clear_drops_buffer_without_counting_underruns() {
        let mut f = PcmFifo::new();
        f.push_frame(&[1i16; 100]);
        f.clear();
        assert_eq!(f.depth(), 0);
        assert_eq!(f.underruns(), 0);
    }
}
