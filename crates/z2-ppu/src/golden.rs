//! Golden-frame verification harness.
//!
//! The acceptance test for the software PPU is: *the indexed framebuffer
//! matches the oracle on 100% of sampled frames of all four movies, with the
//! game running through the interpreter so only the PPU model is under
//! test.* Sampling = every corpus snapshot plus every 600th movie frame.
//!
//! The oracle itself lives in `z2-verify::oracle` and the interpreter in
//! `z2-core`; this crate depends on neither, so this module defines the seam
//! both sides program to:
//!
//! * [`OracleFrameSource`]: what the caller implements
//!   around the oracle to serve indexed oracle frames by frame number.
//! * [`verify_against_oracle`]: drives the model's `render` closure over the
//!   sampled frames and diffs each against the oracle with
//!   [`crate::diff_indexed`].
//!
//! Until the oracle lands, `tests/golden.rs` exercises this harness against
//! stub sources and skips the ROM-gated end-to-end test gracefully.

use crate::render::{diff_indexed, FrameDiff, IndexedFrame};

/// Sample every Nth movie frame for golden comparison.
pub const GOLDEN_EVERY_NTH_MOVIE_FRAME: u64 = 600;

/// Frame indices to sample from a movie of `len_frames` raw frames: `0`,
/// `every_nth`, `2*every_nth`, ... below `len_frames`. `every_nth == 0`
/// yields no samples (guard against division by zero).
#[must_use]
pub fn sample_indices(len_frames: u64, every_nth: u64) -> Vec<u64> {
    if every_nth == 0 {
        return Vec::new();
    }
    (0..len_frames).step_by(every_nth as usize).collect()
}

/// Indexed oracle frames by frame number (implemented by the caller;
/// see module docs).
pub trait OracleFrameSource {
    /// Number of raw movie frames available.
    fn len_frames(&self) -> u64;
    /// Oracle's indexed framebuffer at raw frame `frame`, or `None` when the
    /// oracle cannot serve it (counts as skipped, not failed).
    fn frame_indexed(&self, frame: u64) -> Option<IndexedFrame>;
    /// Human name for failure messages (movie file / snapshot label).
    fn source_name(&self) -> &str {
        "oracle"
    }
}

/// One sampled frame that failed to match the oracle.
#[derive(Debug, Clone)]
pub struct GoldenMismatch {
    /// Raw movie frame index.
    pub frame: u64,
    /// Pixel-level diff (model vs oracle).
    pub diff: FrameDiff,
}

/// Outcome of [`verify_against_oracle`].
#[derive(Debug, Clone, Default)]
pub struct GoldenReport {
    /// Frames sampled (every-Nth indices within the oracle length).
    pub sampled: usize,
    /// Frames the oracle could not serve (skipped, not failed).
    pub skipped: usize,
    /// Sampled frames whose pixels differed.
    pub mismatched: Vec<GoldenMismatch>,
}

impl GoldenReport {
    /// True when every served frame matched.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.mismatched.is_empty()
    }
}

/// Render the model at each sampled frame and diff against the oracle.
///
/// `render` is the model under test: given a raw frame index it restores the
/// game state (snapshot / movie replay through the interpreter) and returns
/// this crate's indexed framebuffer. Main wires that closure; the harness
/// only compares pixels.
pub fn verify_against_oracle(
    render: impl Fn(u64) -> IndexedFrame,
    oracle: &impl OracleFrameSource,
    every_nth: u64,
) -> GoldenReport {
    let mut report = GoldenReport::default();
    for frame in sample_indices(oracle.len_frames(), every_nth) {
        report.sampled += 1;
        let Some(want) = oracle.frame_indexed(frame) else {
            report.skipped += 1;
            continue;
        };
        let got = render(frame);
        let diff = diff_indexed(&got, &want);
        if !diff.is_clean() {
            report.mismatched.push(GoldenMismatch { frame, diff });
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubOracle {
        frames: Vec<IndexedFrame>,
    }

    impl OracleFrameSource for StubOracle {
        fn len_frames(&self) -> u64 {
            self.frames.len() as u64
        }
        fn frame_indexed(&self, frame: u64) -> Option<IndexedFrame> {
            self.frames.get(frame as usize).copied()
        }
        fn source_name(&self) -> &str {
            "stub"
        }
    }

    #[test]
    fn sampling_picks_every_nth_frame_from_zero() {
        assert_eq!(sample_indices(0, 600), Vec::<u64>::new());
        assert_eq!(sample_indices(5, 600), vec![0]);
        assert_eq!(sample_indices(1800, 600), vec![0, 600, 1200]);
        assert_eq!(sample_indices(1801, 600), vec![0, 600, 1200, 1800]);
        assert_eq!(sample_indices(100, 0), Vec::<u64>::new());
        assert_eq!(sample_indices(4, 1), vec![0, 1, 2, 3]);
    }

    #[test]
    fn verify_clean_when_model_matches_oracle() {
        let frames = vec![[0x0Fu8; crate::WIDTH * crate::HEIGHT]; 3];
        let oracle = StubOracle { frames };
        let report = verify_against_oracle(|_| [0x0F; crate::WIDTH * crate::HEIGHT], &oracle, 1);
        assert_eq!(report.sampled, 3);
        assert_eq!(report.skipped, 0);
        assert!(report.is_clean());
    }

    #[test]
    fn verify_reports_mismatching_frame_with_diff() {
        let mut bad = [0x0Fu8; crate::WIDTH * crate::HEIGHT];
        bad[3 * crate::WIDTH + 2] = 0x01;
        let oracle = StubOracle {
            frames: vec![[0x0F; crate::WIDTH * crate::HEIGHT], bad],
        };
        let report = verify_against_oracle(|_| [0x0F; crate::WIDTH * crate::HEIGHT], &oracle, 1);
        assert_eq!(report.sampled, 2);
        assert!(!report.is_clean());
        assert_eq!(report.mismatched.len(), 1);
        assert_eq!(report.mismatched[0].frame, 1);
        assert_eq!(report.mismatched[0].diff.count, 1);
        assert_eq!(report.mismatched[0].diff.first[0], (2, 3));
    }
}
