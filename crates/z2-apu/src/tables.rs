//! NES APU hardware tables (console spec, not game data).
//!
//! These constants describe the Nintendo Entertainment System's audio
//! hardware as documented on nesdev (`APU`, `APU Envelope and Length
//! Counter`, `APU Sweep`, `APU Noise`, `APU DMC`, `APU Mixer`). They are
//! public-domain hardware facts, safe to bake into the repo — unlike the
//! Zelda II note/envelope tables in `prg6.asm`, which must be loaded from
//! the ROM-backed assets at runtime (see [`crate::engine`]) and are
//! **never** copied here.

/// Length-counter reload values, indexed by the 5-bit length index in
/// `$4003/$4007/$400B/$400F` (nesdev `APU Length Counter`).
pub const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, //
    12, 16, 24, 18, 48, 20, 96, 22, 192, 24, 72, 26, 16, 28, 32, 30,
];

/// Pulse duty-cycle patterns: `PULSE_DUTY[duty][step]`, 1 = high.
/// Duty 0 = 12.5%, 1 = 25%, 2 = 50%, 3 = 75% (inverted 25%).
pub const PULSE_DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 0, 0, 0],
    [1, 0, 0, 1, 1, 1, 1, 1],
];

/// Triangle 32-step waveform sequence (output 0..15).
pub const TRIANGLE_SEQ: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, //
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// Noise timer periods in CPU cycles, indexed by `$400E` bits 3..0
/// (NTSC values, nesdev `APU Noise`).
pub const NOISE_PERIOD: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

/// DMC sample-rate periods in CPU cycles, indexed by `$4010` bits 3..0
/// (NTSC values, nesdev `APU DMC`).
pub const DMC_RATE: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];

/// Nonlinear pulse mixer table (nesdev `APU Mixer`):
/// `PULSE_MIX[p1 + p2] = 95.52 / (8128 / (p1+p2) + 100)`.
#[must_use]
pub fn pulse_mix_table() -> [f32; 31] {
    let mut t = [0.0; 31];
    for (i, slot) in t.iter_mut().enumerate().skip(1) {
        let n = i as f32;
        *slot = 95.52 / (8128.0 / n + 100.0);
    }
    t
}

/// Nonlinear triangle/noise/DMC mixer table (nesdev `APU Mixer`):
/// `TND_MIX[3*t + 2*n + d] = 163.4 / (24329 / (3t+2n+d) + 100)`.
#[must_use]
pub fn tnd_mix_table() -> [f32; 203] {
    let mut t = [0.0; 203];
    for (i, slot) in t.iter_mut().enumerate().skip(1) {
        let n = i as f32;
        *slot = 163.4 / (24329.0 / n + 100.0);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_table_spot_checks() {
        assert_eq!(LENGTH_TABLE[0x00], 10);
        assert_eq!(LENGTH_TABLE[0x01], 254);
        assert_eq!(LENGTH_TABLE[0x1F], 30);
        assert_eq!(LENGTH_TABLE[0x10], 12);
        assert_eq!(LENGTH_TABLE.len(), 32);
    }

    #[test]
    fn noise_period_table_spot_checks() {
        assert_eq!(NOISE_PERIOD[0], 4);
        assert_eq!(NOISE_PERIOD[5], 96);
        assert_eq!(NOISE_PERIOD[15], 4068);
    }

    #[test]
    fn dmc_rate_table_spot_checks() {
        assert_eq!(DMC_RATE[0], 428);
        assert_eq!(DMC_RATE[15], 54);
        // Strictly decreasing (faster rate = shorter period).
        for w in DMC_RATE.windows(2) {
            assert!(w[0] > w[1]);
        }
    }

    #[test]
    fn duty_patterns_have_expected_high_steps() {
        // 12.5% / 25% / 50% / (inverted) 25%.
        for (duty, want) in [1u32, 2, 4, 6].iter().enumerate() {
            let got: u32 = PULSE_DUTY[duty].iter().map(|&s| u32::from(s)).sum();
            assert_eq!(got, *want, "duty {duty}");
        }
    }

    #[test]
    fn triangle_sequence_shape() {
        assert_eq!(TRIANGLE_SEQ[0], 15);
        assert_eq!(TRIANGLE_SEQ[15], 0);
        assert_eq!(TRIANGLE_SEQ[16], 0);
        assert_eq!(TRIANGLE_SEQ[31], 15);
        assert!(TRIANGLE_SEQ.iter().all(|&v| v <= 15));
    }

    #[test]
    fn mixer_tables_bounds_and_monotonic() {
        let p = pulse_mix_table();
        let t = tnd_mix_table();
        assert_eq!(p[0], 0.0);
        assert_eq!(t[0], 0.0);
        assert!(p[30] > 0.25 && p[30] < 0.27, "pulse[30]={}", p[30]);
        assert!(t[202] > 0.73 && t[202] < 0.75, "tnd[202]={}", t[202]);
        for w in p.windows(2) {
            assert!(w[0] < w[1]);
        }
        for w in t.windows(2) {
            assert!(w[0] < w[1]);
        }
        // Combined full-scale fits in [0, 1].
        assert!(p[30] + t[202] <= 1.0);
    }
}
