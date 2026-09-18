//! `cpal` audio plumbing over `z2-apu`.
//!
//! Headlines come from `z2_apu::audio` (read-only reuse):
//! [`z2_apu::PcmFifo`], [`z2_apu::nominal_frame_samples`],
//! [`z2_apu::two_frame_buffer_samples`].
//!
//! ## 10-minute no-underrun design (acceptance: buffer-sizing math)
//!
//! * Nominal production is `rate/60` samples per video frame: 735 @ 44.1 kHz,
//!   800 @ 48 kHz (±1 from resampler phase — see `z2_apu::apu`).
//! * The emulator thread pushes exactly one `Apu::audio` frame per vblank
//!   into the shared [`SharedAudio`] ring; the `cpal` callback consumes at
//!   the device rate.
//! * Frontend capacity is [`frontend_capacity`] =
//!   `2 * nominal + SLACK(16)` samples (the `z2-apu` 2-frame headline).
//!   Two frames absorb resampler jitter (`2*(735+1) <= 1486` @ 44.1 kHz) and
//!   one callback-period of scheduling slop; the steady-state depth hovers
//!   around one frame.
//! * Drift over 10 min: `10*60*60.0988 ≈ 36059` frames; worst-case ±1 jitter
//!   per frame is absorbed frame-by-frame (never accumulated) because the
//!   exact-rational resampler (`cycles_total * rate / CPU_HZ`) has zero
//!   long-term drift — total samples over 600 frames match nominal within
//!   ±600 (see `z2-apu` `long_run_sample_total_matches_wall_clock`). The
//!   callback pads with the *last* sample (never zeros — zero-padding
//!   clicks) and counts every padded sample in
//!   [`SharedAudio::underruns`], surfaced in the window title.
//! * A healthy 60 fps run keeps `underruns == 0`; a rising counter means the
//!   emulator thread is starving the callback (machine too slow, or the
//!   frame budget overran — check the title-bar FPS first).
//!
//! ## Device-rate negotiation
//!
//! The old code kept the device default rate unconditionally, so a default
//! 44.1 kHz game on a 48 kHz device drained 3900 samples/s faster than the
//! emulator produced — an underrun flood (~234k/min) that is clearly audible
//! (8% zero-order-hold duty ≈ buzzy distortion). [`open_output_stream`] now
//! requests the game rate when the device offers it at the default channel
//! count (same sample format preferred), and only then falls back to the
//! device default with a loud stderr warning. The returned `u32` is the
//! **actual device rate** (compare against [`SharedAudio::rate`] to detect a
//! fallback; suggest `audio_rate = <device rate>` to the user).
//!
//! [`select_device_rate`] models the rate dimension of that choice without
//! hardware (unit-tested); [`drain_minus_production_per_sec`] quantifies the
//! drift of any game/device pair.
//!
//! ## Startup prime
//!
//! The stream used to start playing against an empty ring, so the first
//! callbacks (fired before the emulator's first push) padded + counted a few
//! hundred underruns at `t=0`. [`open_output_stream`] now calls
//! [`SharedAudio::prime_silence`] (one nominal frame of zeros) before
//! `play()`; startup silence is intentional and uncounted.
//!
//! ## Pause semantics (tradeoff, documented)
//!
//! When paused, no frames are pushed but the `cpal` callback keeps firing —
//! ~48k underruns/s, i.e. **millions per minute**, which is the expected
//! signature of pause-starvation, not a synth bug. [`SharedAudio::set_paused`]
//! makes [`SharedAudio::consume`] emit zeros *without touching the ring or
//! the underrun counter* (depth frozen, resume plays the ≤2 buffered frames
//! with no burst; the app may [`SharedAudio::clear`] on resume for a clean
//! cut). Cost: the stream keeps running (continued callbacks) instead of
//! being OS-paused — no re-open glitches, at the price of idle wakeups. The
//! app must call `set_paused`.
//!
//! ## Overrun trim (windowed loop only — never headless)
//!
//! [`SharedAudio::push_frame`] is intentionally unbounded: `--headless`
//! accumulates whole 600-frame dumps for signal-energy gates. When the game
//! outruns the device (48 kHz game on a 44.1 kHz fallback, or 4×
//! fast-forward), the windowed loop — and only it — should call
//! [`SharedAudio::trim_oldest`] with [`suggested_max_depth_samples`] (8
//! frames ≈ 133 ms). Trimmed samples are real (never counted as underruns)
//! and are tallied in [`SharedAudio::overruns`].
//!
//! Nothing here opens an audio device except [`open_output_stream`], which
//! the windowed loop alone calls. Unit tests only touch [`SharedAudio`]
//! and the pure helpers — never a device (CI/headless has none).

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

pub use z2_apu::audio::{nominal_frame_samples, two_frame_buffer_samples, BUFFER_SLACK_SAMPLES};
pub use z2_apu::{RATE_44100, RATE_48000};

/// Headline game rates.
pub const SUPPORTED_RATES: [u32; 2] = [RATE_44100, RATE_48000];

/// True for the two shipped game rates.
#[must_use]
pub fn rate_supported(rate: u32) -> bool {
    z2_apu::audio::sample_rate_supported(rate)
}

/// Clamp to a shipped rate (unknown → 44100).
#[must_use]
pub fn clamp_rate(rate: u32) -> u32 {
    if rate_supported(rate) {
        rate
    } else {
        RATE_44100
    }
}

/// Frontend FIFO capacity for the 2-frame buffer at `rate`.
#[must_use]
pub fn frontend_capacity(rate: u32) -> usize {
    two_frame_buffer_samples(clamp_rate(rate))
}

/// Nominal samples per frame at `rate` (rounded `rate/60`).
#[must_use]
pub fn frame_samples(rate: u32) -> usize {
    nominal_frame_samples(clamp_rate(rate))
}

/// Samples produced over `frames` video frames (nominal, ±`frames` jitter).
#[must_use]
pub fn nominal_samples_for_frames(rate: u32, frames: u64) -> u64 {
    frame_samples(rate) as u64 * frames
}

/// Convert one game sample to the `F32` device range (`-1.0..=1.0`).
///
/// `-32768 → -1.0`, `0 → 0.0`, `32767 → 32767/32768 < 1.0` (clamped, never
/// clipped over full-scale).
#[must_use]
pub fn pcm_i16_to_f32(s: i16) -> f32 {
    (f32::from(s) / 32768.0).clamp(-1.0, 1.0)
}

/// Convert one game sample to `U16` device range (`0..=65535`, silence at
/// 32768).
#[must_use]
pub fn pcm_i16_to_u16(s: i16) -> u16 {
    (i32::from(s) + 32768) as u16
}

/// Usable channel count for a device config (`0` defensively maps to mono;
/// `cpal` never yields `0` in practice).
#[must_use]
pub fn effective_channels(channels: u16) -> usize {
    usize::from(channels).max(1)
}

/// Rate-dimension model of the [`open_output_stream`] negotiation, without
/// hardware: the game rate when any supported `(min, max)` range covers it,
/// else the device default.
///
/// `want_rate` is clamped to a shipped rate first (unknown → 44100).
#[must_use]
pub fn select_device_rate(want_rate: u32, default_rate: u32, supported: &[(u32, u32)]) -> u32 {
    let want = clamp_rate(want_rate);
    if default_rate == want {
        return want;
    }
    if supported.iter().any(|&(lo, hi)| lo <= want && want <= hi) {
        want
    } else {
        default_rate
    }
}

/// Device drain minus game production, in samples/sec. Positive means the
/// callback outruns the emulator (underrun flood); negative means the ring
/// grows (overrun — trim per [`SharedAudio::trim_oldest`]).
///
/// Example: 44.1 kHz game on a 48 kHz device → `+3900`/s (≈234k/min, i.e.
/// "millions" within minutes).
#[must_use]
pub fn drain_minus_production_per_sec(game_rate: u32, device_rate: u32) -> i64 {
    i64::from(device_rate) - i64::from(game_rate)
}

/// Suggested [`SharedAudio::trim_oldest`] bound: 8 nominal frames (~133 ms).
/// Generous next to callback granularity (5–20 ms) and emulator jitter
/// (±1 frame); only pathological drift / fast-forward backlogs hit it.
#[must_use]
pub fn suggested_max_depth_samples(rate: u32) -> usize {
    8 * frame_samples(rate)
}

/// Samples [`SharedAudio::prime_silence`] pushes: one nominal frame.
#[must_use]
pub fn startup_prime_samples(rate: u32) -> usize {
    frame_samples(rate)
}

/// Thread-shared PCM ring: emulator pushes, `cpal` callback consumes.
///
/// Extra counters beyond the `z2-apu` FIFO headline:
/// * paused fills (silence, ring untouched) never count anywhere;
/// * a poisoned-mutex fill counts in `underruns` (honest metering);
/// * [`trim_oldest`](Self::trim_oldest) drops count in [`overruns`](Self::overruns);
/// * runtime `cpal` stream errors count in [`stream_errors`](Self::stream_errors).
#[derive(Debug, Clone, Default)]
pub struct SharedAudio {
    inner: Arc<Mutex<z2_apu::PcmFifo>>,
    rate: u32,
    paused: Arc<AtomicBool>,
    /// Poison-path pads + [`trim_oldest`](Self::trim_oldest)-adjacent extras
    /// folded into [`underruns`](Self::underruns).
    extra_underruns: Arc<AtomicU64>,
    overruns: Arc<AtomicU64>,
    stream_errors: Arc<AtomicU64>,
}

impl SharedAudio {
    /// Empty ring at `rate` (clamped to a shipped rate).
    pub fn new(rate: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(z2_apu::PcmFifo::new())),
            rate: clamp_rate(rate),
            paused: Arc::new(AtomicBool::new(false)),
            extra_underruns: Arc::new(AtomicU64::new(0)),
            overruns: Arc::new(AtomicU64::new(0)),
            stream_errors: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Output rate in Hz.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Push one rendered `Apu::audio` frame (called once per vblank).
    ///
    /// Unbounded by design (headless accumulates whole dumps); the windowed
    /// loop bounds live latency via [`trim_oldest`](Self::trim_oldest).
    pub fn push_frame(&self, samples: &[i16]) {
        if let Ok(mut f) = self.inner.lock() {
            f.push_frame(samples);
        }
    }

    /// Callback-side consume: exactly `n` samples, padded + counted on
    /// starvation (see `PcmFifo::consume`).
    ///
    /// While [`set_paused`](Self::set_paused), emits `n` zeros without
    /// touching the ring or any counter (see the module pause docs).
    pub fn consume(&self, n: usize, out: &mut Vec<i16>) {
        if self.paused.load(Ordering::Relaxed) {
            out.extend(std::iter::repeat_n(0, n));
            return;
        }
        if let Ok(mut f) = self.inner.lock() {
            f.consume(n, out);
        } else {
            self.extra_underruns.fetch_add(n as u64, Ordering::Relaxed);
            out.extend(std::iter::repeat_n(0, n));
        }
    }

    /// Buffered (unconsumed) sample count.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.inner.lock().map(|f| f.depth()).unwrap_or(0)
    }

    /// Lifetime underrun-sample count (title-bar meter; healthy run = 0,
    /// excluding intentional [`prime_silence`](Self::prime_silence) and
    /// [`set_paused`](Self::set_paused) fills).
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.inner
            .lock()
            .map(|f| f.underruns())
            .unwrap_or(0)
            .saturating_add(self.extra_underruns.load(Ordering::Relaxed))
    }

    /// `(pushed, popped)` totals. `popped` freezes while paused (pause fills
    /// bypass the ring) and advances on [`trim_oldest`](Self::trim_oldest).
    #[must_use]
    pub fn totals(&self) -> (u64, u64) {
        self.inner.lock().map(|f| f.totals()).unwrap_or((0, 0))
    }

    /// Drop buffered samples (on seek / reset / rate change).
    pub fn clear(&self) {
        if let Ok(mut f) = self.inner.lock() {
            f.clear();
        }
    }

    /// Push one nominal frame of zeros when the ring is empty; returns samples
    /// pushed (0 when already primed). Called by [`open_output_stream`]
    /// before `play()` so the first callbacks read silence instead of
    /// counting startup underruns.
    pub fn prime_silence(&self) -> usize {
        if self.depth() != 0 {
            return 0;
        }
        let n = startup_prime_samples(self.rate);
        if let Ok(mut f) = self.inner.lock() {
            if f.depth() == 0 {
                f.push_frame(&vec![0i16; n]);
                return n;
            }
        }
        0
    }

    /// Set the pause-mute (the app calls this on `P` / focus loss / resume).
    /// While set, [`consume`](Self::consume) emits uncounted zeros and the
    /// ring is frozen; [`underruns`](Self::underruns) does not move.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Pause-mute state.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Drop the oldest samples down to `max_depth`; returns samples dropped.
    /// Only pops buffered (real) samples, so [`underruns`](Self::underruns)
    /// never moves; drops tally in [`overruns`](Self::overruns).
    ///
    /// Windowed loop only — never headless (which accumulates dumps).
    pub fn trim_oldest(&self, max_depth: usize) -> usize {
        let mut scratch = Vec::new();
        if let Ok(mut f) = self.inner.lock() {
            let drop = f.depth().saturating_sub(max_depth);
            if drop > 0 {
                f.consume(drop, &mut scratch);
                self.overruns.fetch_add(drop as u64, Ordering::Relaxed);
                return drop;
            }
        }
        0
    }

    /// Suggested [`trim_oldest`](Self::trim_oldest) bound for this ring's rate.
    #[must_use]
    pub fn suggested_max_depth(&self) -> usize {
        suggested_max_depth_samples(self.rate)
    }

    /// Lifetime overrun-sample count (trimmed real samples; healthy run = 0).
    #[must_use]
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Record one runtime `cpal` stream error (the `err_cb` calls this, then
    /// logs to stderr — errors are always visible *and* counted for the
    /// title bar).
    pub fn note_stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Lifetime runtime `cpal` stream-error count (title-bar meter).
    #[must_use]
    pub fn stream_errors(&self) -> u64 {
        self.stream_errors.load(Ordering::Relaxed)
    }
}

/// Render one synthetic `Apu` frame at `rate` (silence after the filter
/// settles — used by the loop self-test without needing a ROM).
///
/// Returns the PCM appended for that frame (nominal ±1 samples).
pub fn synthetic_apu_frame(rate: u32) -> Vec<i16> {
    let mut apu = z2_apu::Apu::new(clamp_rate(rate));
    let mut out = Vec::new();
    // Two warm-up frames settle the DC-blocking HPF; the third is the probe.
    apu.audio(&mut Vec::new());
    apu.audio(&mut Vec::new());
    apu.audio(&mut out);
    out
}

/// Open the default `cpal` output stream feeding from `shared`.
///
/// The callback duplicates mono game PCM to all device channels (see
/// [`effective_channels`]). The device default sample format is honoured
/// (`F32`/`I16`/`U16`, converted via [`pcm_i16_to_f32`]/[`pcm_i16_to_u16`]);
/// the game rate is requested when the device offers it at the default
/// channel count, else the device default is kept with a stderr warning
/// (see [`drain_minus_production_per_sec`]). The ring is primed with one
/// frame of silence before `play()` so startup costs zero underruns.
/// Never called in tests — the windowed loop owns the returned stream
/// handle (dropping it stops audio). Never panics on missing devices:
/// every failure is `eprintln!`-ed **and** returned as `Err`.
///
/// Returns the stream plus the **actual device rate** (compare with
/// [`SharedAudio::rate`]: a mismatch means fallback — set `audio_rate` to
/// the device rate to silence the drift).
///
/// Errors are human-readable (no `anyhow` dependency).
pub fn open_output_stream(shared: &SharedAudio) -> Result<(cpal::Stream, u32), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host.default_output_device().ok_or_else(|| {
        let m = "no default audio output device".to_string();
        eprintln!("z2 audio: {m} (running silent; check the OS output device)");
        m
    })?;
    let default_supported = device.default_output_config().map_err(|e| {
        let m = format!("default output config: {e}");
        eprintln!("z2 audio: {m}");
        m
    })?;
    let want_rate = shared.rate();
    let default_config = default_supported.config();
    let default_rate = default_config.sample_rate.0;

    // Prefer the game rate when offered at the default channel count (same
    // sample format first, then any format); otherwise keep the default.
    let chosen: cpal::SupportedStreamConfig = if default_rate == want_rate {
        default_supported
    } else {
        match device.supported_output_configs() {
            Ok(ranges) => {
                let want = cpal::SampleRate(want_rate);
                let mut same_format: Option<cpal::SupportedStreamConfig> = None;
                let mut any_format: Option<cpal::SupportedStreamConfig> = None;
                for r in ranges {
                    if r.channels() != default_config.channels {
                        continue;
                    }
                    if r.min_sample_rate() > want || r.max_sample_rate() < want {
                        continue;
                    }
                    // Re-checked against the range; `None` is unreachable
                    // here but handled without panicking.
                    if r.sample_format() == default_supported.sample_format() {
                        if let Some(cfg) = r.try_with_sample_rate(want) {
                            same_format = Some(cfg);
                            break;
                        }
                    } else if any_format.is_none() {
                        any_format = r.try_with_sample_rate(want);
                    }
                }
                same_format.or(any_format).unwrap_or(default_supported)
            }
            Err(e) => {
                eprintln!(
                    "z2 audio: cannot enumerate rates ({e}); keeping device default {default_rate} Hz"
                );
                default_supported
            }
        }
    };

    let picked_format = chosen.sample_format();
    let config = chosen.config();
    let device_rate = config.sample_rate.0;
    let channels = effective_channels(config.channels);
    if device_rate != want_rate {
        eprintln!(
            "z2 audio: rate fallback — game {want_rate} Hz vs device {device_rate} Hz \
             (drift {:+} samples/s; underrun/overrun meters will climb — \
             set audio_rate to {device_rate} to silence)",
            drain_minus_production_per_sec(want_rate, device_rate),
        );
    }
    let make_err_cb = || {
        let err_shared = shared.clone();
        move |e| {
            err_shared.note_stream_error();
            eprintln!("z2 audio stream error: {e}");
        }
    };
    let stream = match picked_format {
        cpal::SampleFormat::F32 => {
            let shared_cb = shared.clone();
            device
                .build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        let n = data.len() / channels;
                        let mut mono = Vec::with_capacity(n);
                        shared_cb.consume(n, &mut mono);
                        for (i, slot) in data.chunks_mut(channels).enumerate() {
                            let s = mono.get(i).copied().unwrap_or(0);
                            let v = pcm_i16_to_f32(s);
                            for ch in slot.iter_mut() {
                                *ch = v;
                            }
                        }
                    },
                    make_err_cb(),
                    None,
                )
                .map_err(|e| {
                    let m = format!("build output stream: {e}");
                    eprintln!("z2 audio: {m}");
                    m
                })?
        }
        cpal::SampleFormat::I16 => {
            let shared_cb = shared.clone();
            device
                .build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        let n = data.len() / channels;
                        let mut mono = Vec::with_capacity(n);
                        shared_cb.consume(n, &mut mono);
                        for (i, slot) in data.chunks_mut(channels).enumerate() {
                            let s = mono.get(i).copied().unwrap_or(0);
                            for ch in slot.iter_mut() {
                                *ch = s;
                            }
                        }
                    },
                    make_err_cb(),
                    None,
                )
                .map_err(|e| {
                    let m = format!("build output stream: {e}");
                    eprintln!("z2 audio: {m}");
                    m
                })?
        }
        cpal::SampleFormat::U16 => {
            let shared_cb = shared.clone();
            device
                .build_output_stream(
                    &config,
                    move |data: &mut [u16], _| {
                        let n = data.len() / channels;
                        let mut mono = Vec::with_capacity(n);
                        shared_cb.consume(n, &mut mono);
                        for (i, slot) in data.chunks_mut(channels).enumerate() {
                            let s = mono.get(i).copied().unwrap_or(0);
                            let v = pcm_i16_to_u16(s);
                            for ch in slot.iter_mut() {
                                *ch = v;
                            }
                        }
                    },
                    make_err_cb(),
                    None,
                )
                .map_err(|e| {
                    let m = format!("build output stream: {e}");
                    eprintln!("z2 audio: {m}");
                    m
                })?
        }
        fmt => {
            let m = format!("unsupported sample format {fmt:?}");
            eprintln!("z2 audio: {m}");
            return Err(m);
        }
    };
    shared.prime_silence();
    stream.play().map_err(|e| {
        let m = format!("start output stream: {e}");
        eprintln!("z2 audio: {m}");
        m
    })?;
    Ok((stream, device_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headlines_match_expected_rates() {
        assert_eq!(frame_samples(44100), 735);
        assert_eq!(frame_samples(48000), 800);
        assert_eq!(frontend_capacity(44100), 2 * 735 + 16);
        assert_eq!(frontend_capacity(48000), 2 * 800 + 16);
        assert!(!rate_supported(22050));
        assert_eq!(clamp_rate(22050), 44100);
    }

    #[test]
    fn two_frame_buffer_absorbs_jitter() {
        assert!(frontend_capacity(44100) >= 2 * (735 + 1));
        assert!(frontend_capacity(48000) >= 2 * (800 + 1));
    }

    #[test]
    fn healthy_push_consume_has_no_underruns() {
        let ring = SharedAudio::new(44100);
        let frame = vec![100i16; 735];
        let mut out = Vec::new();
        for _ in 0..120 {
            ring.push_frame(&frame);
            ring.consume(735, &mut out);
        }
        assert_eq!(ring.underruns(), 0);
        assert_eq!(ring.depth(), 0);
        assert_eq!(ring.totals(), (120 * 735, 120 * 735));
    }

    #[test]
    fn starved_consume_pads_and_counts() {
        let ring = SharedAudio::new(44100);
        ring.push_frame(&[7i16, 8, 9]);
        let mut out = Vec::new();
        ring.consume(5, &mut out);
        assert_eq!(out, vec![7, 8, 9, 9, 9]);
        assert_eq!(ring.underruns(), 2);
    }

    #[test]
    fn ten_minute_nominal_math() {
        // 10 min at 60.0988 Hz ≈ 36059 frames; nominal totals scale linearly
        // and the exact-rational resampler never accumulates drift beyond
        // ±1/frame (see z2-apu long-run test).
        let frames = (10.0 * 60.0 * 60.0988) as u64;
        assert!((36000..36100).contains(&frames), "frames={frames}");
        let total = nominal_samples_for_frames(44100, frames);
        assert_eq!(total, frames * 735);
    }

    #[test]
    fn synthetic_frame_self_test_produces_nominal_audio() {
        for rate in [44100u32, 48000] {
            let pcm = synthetic_apu_frame(rate);
            let nominal = frame_samples(rate) as i64;
            assert!(
                ((pcm.len() as i64) - nominal).abs() <= 1,
                "rate {rate}: got {}, want {nominal}±1",
                pcm.len()
            );
        }
    }

    #[test]
    fn format_conversion_tables() {
        // F32: full-scale maps without clipping over 1.0.
        assert_eq!(pcm_i16_to_f32(-32768), -1.0);
        assert_eq!(pcm_i16_to_f32(0), 0.0);
        assert_eq!(pcm_i16_to_f32(-16384), -0.5);
        let top = pcm_i16_to_f32(32767);
        assert!(top < 1.0 && (f64::from(top) - 32767.0 / 32768.0).abs() < 1e-6);
        // U16: silence sits at mid-rail.
        assert_eq!(pcm_i16_to_u16(-32768), 0);
        assert_eq!(pcm_i16_to_u16(-1), 32767);
        assert_eq!(pcm_i16_to_u16(0), 32768);
        assert_eq!(pcm_i16_to_u16(32767), 65535);
        // I16 path is the identity (callback copies samples through).
    }

    #[test]
    fn channel_mapping_duplicates_mono() {
        // Models the `build_output_stream` callback bodies: `n` mono samples
        // fan out to every channel of each frame.
        let mono = [10i16, 20];
        for channels in [1usize, 2, 6] {
            let eff = effective_channels(channels as u16);
            let mut interleaved = vec![0i16; mono.len() * eff];
            for (i, slot) in interleaved.chunks_mut(eff).enumerate() {
                let s = mono.get(i).copied().unwrap_or(0);
                for ch in slot.iter_mut() {
                    *ch = s;
                }
            }
            assert_eq!(interleaved.len(), mono.len() * eff);
            for (i, slot) in interleaved.chunks(eff).enumerate() {
                assert!(slot.iter().all(|&v| v == mono[i]), "ch{channels}: {slot:?}");
            }
        }
        assert_eq!(effective_channels(0), 1, "zero-channel guard is mono");
        assert_eq!(effective_channels(2), 2);
    }

    #[test]
    fn rate_selection_table() {
        // Default already matches: no enumeration needed.
        assert_eq!(select_device_rate(44100, 44100, &[]), 44100);
        // Game rate offered anywhere in a supported range wins.
        assert_eq!(
            select_device_rate(44100, 48000, &[(44100, 44100), (48000, 48000)]),
            44100
        );
        assert_eq!(select_device_rate(48000, 44100, &[(8000, 96000)]), 48000);
        // Not offered: honest fallback to the device default.
        assert_eq!(select_device_rate(44100, 48000, &[(48000, 96000)]), 48000);
        assert_eq!(select_device_rate(44100, 48000, &[]), 48000);
        // Unknown want clamps to 44100 first.
        assert_eq!(select_device_rate(22050, 48000, &[(44100, 48000)]), 44100);
        assert_eq!(select_device_rate(22050, 16000, &[(16000, 16000)]), 16000);
    }

    #[test]
    fn drift_math_quantifies_audibility() {
        // The reported user setup: 44.1 kHz game on a 48 kHz device.
        assert_eq!(drain_minus_production_per_sec(44100, 48000), 3900);
        assert_eq!(drain_minus_production_per_sec(48000, 44100), -3900);
        assert_eq!(drain_minus_production_per_sec(48000, 48000), 0);
        // +3900/s floods to millions within minutes (unpaused, unmatched).
        assert_eq!(3900i64 * 600, 2_340_000, "10 min of 44.1-on-48k drift");
        // Paused starvation is the bigger millions-source: the callback keeps
        // draining at the device rate with no production at all.
        assert_eq!(48000u64 * 60, 2_880_000, "1 min paused @48k, unmuted");
    }

    #[test]
    fn startup_prime_covers_first_callback() {
        // Old behaviour, documented: an empty ring pads + counts.
        let bare = SharedAudio::new(44100);
        let mut out = Vec::new();
        bare.consume(512, &mut out);
        assert_eq!(bare.underruns(), 512);

        // New behaviour: one primed frame absorbs the first callbacks.
        let ring = SharedAudio::new(44100);
        assert_eq!(ring.prime_silence(), 735);
        assert_eq!(ring.underruns(), 0, "prime is uncounted silence");
        let mut first = Vec::new();
        ring.consume(512, &mut first);
        assert_eq!(ring.underruns(), 0);
        assert_eq!(ring.depth(), 735 - 512);
        assert!(first.iter().all(|&s| s == 0));
        // Already primed: second call is a no-op.
        assert_eq!(ring.prime_silence(), 0);
        assert_eq!(startup_prime_samples(48000), 800);
    }

    #[test]
    fn paused_consume_freezes_meter_and_ring() {
        let ring = SharedAudio::new(44100);
        ring.push_frame(&[42i16; 735]);
        ring.set_paused(true);
        assert!(ring.is_paused());
        let mut out = Vec::new();
        // A minute of paused callbacks at 48 kHz: 2.88M samples of silence.
        for _ in 0..60 {
            out.clear();
            ring.consume(48000, &mut out);
            assert!(out.iter().all(|&s| s == 0));
        }
        assert_eq!(ring.underruns(), 0, "paused fills are not underruns");
        assert_eq!(ring.depth(), 735, "ring frozen while paused");
        assert_eq!(ring.totals(), (735, 0), "popped frozen while paused");
        // Resume: buffered frame still there, no burst.
        ring.set_paused(false);
        let mut back = Vec::new();
        ring.consume(735, &mut back);
        assert_eq!(back, vec![42i16; 735]);
        assert_eq!(ring.underruns(), 0);
    }

    #[test]
    fn trim_bounds_pathology_without_underruns() {
        // Simulate a fast-forward backlog on a live-sized ring.
        let ring = SharedAudio::new(44100);
        let frame = vec![5i16; 735];
        for _ in 0..40 {
            ring.push_frame(&frame);
        }
        assert_eq!(ring.depth(), 40 * 735);
        let cap = ring.suggested_max_depth();
        assert_eq!(cap, suggested_max_depth_samples(44100));
        assert_eq!(cap, 8 * 735);
        let dropped = ring.trim_oldest(cap);
        assert_eq!(dropped, 40 * 735 - cap);
        assert_eq!(ring.depth(), cap);
        assert_eq!(ring.overruns(), dropped as u64);
        assert_eq!(
            ring.underruns(),
            0,
            "trimming real samples is not starvation"
        );
        assert_eq!(ring.trim_oldest(cap), 0, "at cap: no-op");
    }

    #[test]
    fn stream_errors_start_at_zero_and_count() {
        let ring = SharedAudio::new(48000);
        assert_eq!(ring.stream_errors(), 0);
        ring.note_stream_error();
        ring.note_stream_error();
        assert_eq!(ring.stream_errors(), 2);
    }

    #[test]
    fn pause_and_counters_survive_clone() {
        // The `cpal` callback holds a clone; flags/counters are shared.
        let ring = SharedAudio::new(44100);
        let cb = ring.clone();
        ring.set_paused(true);
        assert!(cb.is_paused());
        cb.note_stream_error();
        assert_eq!(ring.stream_errors(), 1);
    }
}
